use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};

const SKILL_NAME: &str = "grillforge-model-selector";
const FILES: [EmbeddedFile; 3] = [
    EmbeddedFile {
        relative_path: "SKILL.md",
        contents: include_bytes!("../../../skills/grillforge-model-selector/SKILL.md"),
        unix_mode: 0o644,
    },
    EmbeddedFile {
        relative_path: "agents/openai.yaml",
        contents: include_bytes!("../../../skills/grillforge-model-selector/agents/openai.yaml"),
        unix_mode: 0o644,
    },
    EmbeddedFile {
        relative_path: "scripts/select_models.py",
        contents: include_bytes!(
            "../../../skills/grillforge-model-selector/scripts/select_models.py"
        ),
        unix_mode: 0o755,
    },
];

struct EmbeddedFile {
    relative_path: &'static str,
    contents: &'static [u8],
    unix_mode: u32,
}

pub struct SkillInstaller;

#[derive(Debug)]
pub struct SkillInstallError {
    operation: &'static str,
    path: PathBuf,
    source: std::io::Error,
}

impl Display for SkillInstallError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "could not {} selector skill path {}",
            self.operation,
            self.path.display()
        )
    }
}

impl Error for SkillInstallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

impl SkillInstaller {
    pub fn install(destination_root: impl AsRef<Path>) -> Result<(), SkillInstallError> {
        let destination_root = destination_root.as_ref();
        let destination = destination_root.join(SKILL_NAME);
        let directories = [
            destination_root.to_path_buf(),
            destination.clone(),
            destination.join("agents"),
            destination.join("scripts"),
        ];
        for directory in &directories {
            preflight_path(directory, PathKind::Directory)?;
        }
        for embedded in &FILES {
            preflight_path(&destination.join(embedded.relative_path), PathKind::File)?;
        }

        for directory in directories {
            fs::create_dir_all(&directory)
                .map_err(|source| install_error("create", directory, source))?;
        }

        for embedded in &FILES {
            let path = destination.join(embedded.relative_path);
            if is_current(&path, embedded)? {
                continue;
            }
            crate::storage::atomic_replace(&path, embedded.contents)
                .map_err(|source| install_error("write", path.clone(), source))?;
            set_mode(&path, embedded.unix_mode)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum PathKind {
    Directory,
    File,
}

fn preflight_path(path: &Path, expected: PathKind) -> Result<(), SkillInstallError> {
    match fs::metadata(path) {
        Ok(metadata)
            if matches!(expected, PathKind::Directory) && metadata.is_dir()
                || matches!(expected, PathKind::File) && metadata.is_file() =>
        {
            Ok(())
        }
        Ok(_) => Err(install_error(
            "validate",
            path.to_path_buf(),
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                match expected {
                    PathKind::Directory => "expected a directory",
                    PathKind::File => "expected a regular file",
                },
            ),
        )),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(install_error("inspect", path.to_path_buf(), source)),
    }
}

fn is_current(path: &Path, embedded: &EmbeddedFile) -> Result<bool, SkillInstallError> {
    match fs::read(path) {
        Ok(contents) if contents == embedded.contents => mode_is(path, embedded.unix_mode),
        Ok(_) => Ok(false),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(install_error("read", path.to_path_buf(), source)),
    }
}

#[cfg(unix)]
fn mode_is(path: &Path, expected: u32) -> Result<bool, SkillInstallError> {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o777 == expected)
        .map_err(|source| install_error("inspect permissions on", path.to_path_buf(), source))
}

#[cfg(not(unix))]
fn mode_is(_path: &Path, _expected: u32) -> Result<bool, SkillInstallError> {
    Ok(true)
}

fn install_error(
    operation: &'static str,
    path: PathBuf,
    source: std::io::Error,
) -> SkillInstallError {
    SkillInstallError {
        operation,
        path,
        source,
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), SkillInstallError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|source| install_error("set permissions on", path.to_path_buf(), source))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), SkillInstallError> {
    Ok(())
}
