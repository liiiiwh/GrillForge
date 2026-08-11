use serde::Serialize;
use serde::de::DeserializeOwned;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct YamlStore {
    path: PathBuf,
}

#[derive(Debug)]
pub enum StoreError {
    Serialize {
        file: String,
    },
    Deserialize {
        file: String,
    },
    Io {
        file: String,
        source: std::io::Error,
    },
}

impl Display for StoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialize { file } => write!(formatter, "could not serialize {file}"),
            Self::Deserialize { file } => write!(formatter, "could not parse {file}"),
            Self::Io { file, .. } => write!(formatter, "could not access {file}"),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl YamlStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn read<T: DeserializeOwned>(&self) -> Result<T, StoreError> {
        let bytes = fs::read(&self.path).map_err(|source| self.io_error(source))?;
        serde_yaml::from_slice(&bytes).map_err(|_| StoreError::Deserialize {
            file: self.file_name(),
        })
    }

    pub fn write<T: Serialize>(&self, value: &T) -> Result<(), StoreError> {
        let bytes = serde_yaml::to_string(value)
            .map(String::into_bytes)
            .map_err(|_| StoreError::Serialize {
                file: self.file_name(),
            })?;

        let parent = self.path.parent().ok_or_else(|| {
            self.io_error(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path has no parent",
            ))
        })?;
        fs::create_dir_all(parent).map_err(|source| self.io_error(source))?;

        if self.path.exists() {
            let previous = fs::read(&self.path).map_err(|source| self.io_error(source))?;
            let backup = self.path.with_extension("yaml.bak");
            atomic_replace(&backup, &previous).map_err(|source| StoreError::Io {
                file: backup
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("backup")
                    .to_string(),
                source,
            })?;
        }

        atomic_replace(&self.path, &bytes).map_err(|source| self.io_error(source))
    }

    fn file_name(&self) -> String {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("configuration")
            .to_string()
    }

    fn io_error(&self, source: std::io::Error) -> StoreError {
        StoreError::Io {
            file: self.file_name(),
            source,
        }
    }
}

pub(crate) fn atomic_replace(path: &Path, data: &[u8]) -> Result<(), std::io::Error> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
    })?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name")
        })?;
    let temporary = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(path)
                .map(|metadata| metadata.permissions().mode())
                .unwrap_or(0o600);
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }

        file.write_all(data)?;
        file.sync_all()?;
        drop(file);

        replace_file(&temporary, path)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, destination: &Path) -> Result<(), std::io::Error> {
    fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> Result<(), std::io::Error> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    if !destination.exists() {
        return fs::rename(temporary, destination);
    }

    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let replacement: Vec<u16> = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: both UTF-16 buffers are NUL-terminated and live for the call.
    let replaced = unsafe {
        ReplaceFileW(
            destination.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
