use serde::{Deserialize, Serialize};
use std::env;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};
use toml_edit::{DocumentMut, Item, Table, value};
use url::Url;

const SNAPSHOT_FILE: &str = "grok-build.snapshot.json";
const PROFILE: &str = "grillforge";
const DEFAULT_CONTEXT_WINDOW: i64 = 500_000;
const CLI_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrokBuildCliDetection {
    pub path: PathBuf,
    pub version: String,
}

pub fn detect_grok_build_cli() -> Result<Option<GrokBuildCliDetection>, GrokBuildAdapterError> {
    let executable = if cfg!(windows) { "grok.exe" } else { "grok" };
    let mut candidates = env::var_os("PATH")
        .map(|path| {
            env::split_paths(&path)
                .map(|directory| directory.join(executable))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(directory) = env::var_os("GROK_BIN_DIR") {
        candidates.push(PathBuf::from(directory).join(executable));
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".grok/bin").join(executable));
    }
    #[cfg(target_os = "macos")]
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin").join(executable),
        PathBuf::from("/usr/local/bin").join(executable),
    ]);
    if let Some(detection) = detect_grok_build_cli_in(candidates)? {
        return Ok(Some(detection));
    }
    let shell_candidates =
        crate::cli_discovery::login_shell_candidates(executable).map_err(|error| {
            GrokBuildAdapterError::Invalid(format!(
                "discover Grok Build CLI through the login shell: {error}"
            ))
        })?;
    detect_grok_build_cli_in(shell_candidates)
}

pub fn detect_grok_build_cli_in(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<Option<GrokBuildCliDetection>, GrokBuildAdapterError> {
    crate::cli_discovery::first_valid_candidate(candidates, |path| inspect_grok_build_cli(path))
}

pub fn inspect_grok_build_cli(
    path: impl AsRef<Path>,
) -> Result<GrokBuildCliDetection, GrokBuildAdapterError> {
    let path = path.as_ref().to_path_buf();
    let mut command = crate::cli_discovery::version_command(&path).map_err(|source| {
        GrokBuildAdapterError::Io {
            operation: "prepare Grok Build CLI inspection",
            path: path.clone(),
            source,
        }
    })?;
    let mut child = command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| GrokBuildAdapterError::Io {
            operation: "inspect Grok Build CLI",
            path: path.clone(),
            source,
        })?;
    let deadline = Instant::now() + CLI_TIMEOUT;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|source| GrokBuildAdapterError::Io {
                operation: "inspect Grok Build CLI",
                path: path.clone(),
                source,
            })?
        {
            let output = child
                .wait_with_output()
                .map_err(|source| GrokBuildAdapterError::Io {
                    operation: "inspect Grok Build CLI",
                    path: path.clone(),
                    source,
                })?;
            if !status.success() {
                return Err(GrokBuildAdapterError::Invalid(format!(
                    "Grok Build CLI did not return a version: {}",
                    path.display()
                )));
            }
            let version = String::from_utf8(output.stdout)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    GrokBuildAdapterError::Invalid(format!(
                        "Grok Build CLI did not return a version: {}",
                        path.display()
                    ))
                })?;
            return Ok(GrokBuildCliDetection { path, version });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GrokBuildAdapterError::Invalid(format!(
                "Grok Build CLI version check timed out: {}",
                path.display()
            )));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrokBuildPaths {
    pub config_path: PathBuf,
}

impl GrokBuildPaths {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
        }
    }
}

pub fn paths_from_home(home: impl AsRef<Path>) -> GrokBuildPaths {
    GrokBuildPaths::new(home.as_ref().join(".grok/config.toml"))
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrokBuildRequest {
    base_url: String,
    api_key: String,
    model: String,
    name: String,
}

impl GrokBuildRequest {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<Self, GrokBuildAdapterError> {
        let base_url = base_url.into();
        validate_base_url(&base_url)?;
        let api_key = api_key.into();
        validate_secret(&api_key)?;
        let model = model.into();
        validate_text("model", &model)?;
        let name = name.into();
        validate_text("display name", &name)?;
        Ok(Self {
            base_url,
            api_key,
            model,
            name,
        })
    }
}

impl Debug for GrokBuildRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GrokBuildRequest")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("name", &self.name)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrokBuildTakeoverStatus {
    Inactive,
    Active,
    Drifted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrokBuildStatus {
    pub snapshot_present: bool,
    pub takeover: GrokBuildTakeoverStatus,
}

#[derive(Debug)]
pub struct GrokBuildAdapter {
    paths: GrokBuildPaths,
    snapshot_path: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
struct RecoverySnapshot {
    version: u8,
    original: Option<Vec<u8>>,
    expected: Vec<u8>,
}

impl GrokBuildAdapter {
    pub fn new(paths: GrokBuildPaths, grillforge_root: impl Into<PathBuf>) -> Self {
        Self {
            paths,
            snapshot_path: grillforge_root.into().join(SNAPSHOT_FILE),
        }
    }

    pub fn apply(
        &self,
        request: GrokBuildRequest,
    ) -> Result<GrokBuildStatus, GrokBuildAdapterError> {
        let current = read_optional(&self.paths.config_path)?;
        let previous_snapshot = read_optional(&self.snapshot_path)?;
        let original = match &previous_snapshot {
            Some(bytes) => {
                let snapshot = parse_snapshot(bytes)?;
                if current.as_deref() != Some(snapshot.expected.as_slice()) {
                    return Err(GrokBuildAdapterError::Drifted);
                }
                snapshot.original
            }
            None => current.clone(),
        };
        let expected = project(current.as_deref(), &request)?;
        let snapshot = RecoverySnapshot {
            version: 1,
            original,
            expected: expected.clone(),
        };
        let snapshot_bytes = serde_json::to_vec_pretty(&snapshot).map_err(|error| {
            GrokBuildAdapterError::Invalid(format!("could not encode Grok Build snapshot: {error}"))
        })?;

        if let Err(primary) = write_optional(&self.snapshot_path, Some(&snapshot_bytes))
            .and_then(|()| write_optional(&self.paths.config_path, Some(&expected)))
        {
            let rollback = write_optional(&self.paths.config_path, current.as_deref())
                .and_then(|()| write_optional(&self.snapshot_path, previous_snapshot.as_deref()));
            return Err(combine_rollback(primary, rollback));
        }
        if read_optional(&self.paths.config_path)?.as_deref() != Some(expected.as_slice()) {
            return Err(GrokBuildAdapterError::Invalid(
                "Grok Build apply verification failed".into(),
            ));
        }
        self.status()
    }

    pub fn disable(&self) -> Result<GrokBuildStatus, GrokBuildAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(GrokBuildStatus {
                snapshot_present: false,
                takeover: GrokBuildTakeoverStatus::Inactive,
            });
        };
        let snapshot = parse_snapshot(&bytes)?;
        if read_optional(&self.paths.config_path)?.as_deref() != Some(snapshot.expected.as_slice())
        {
            return Err(GrokBuildAdapterError::Drifted);
        }
        write_optional(&self.paths.config_path, snapshot.original.as_deref())?;
        if read_optional(&self.paths.config_path)? != snapshot.original {
            return Err(GrokBuildAdapterError::Invalid(
                "Grok Build restore verification failed; recovery snapshot was retained".into(),
            ));
        }
        fs::remove_file(&self.snapshot_path).map_err(|source| GrokBuildAdapterError::Io {
            operation: "remove Grok Build recovery snapshot",
            path: self.snapshot_path.clone(),
            source,
        })?;
        self.status()
    }

    pub fn status(&self) -> Result<GrokBuildStatus, GrokBuildAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(GrokBuildStatus {
                snapshot_present: false,
                takeover: GrokBuildTakeoverStatus::Inactive,
            });
        };
        let snapshot = parse_snapshot(&bytes)?;
        let takeover = if read_optional(&self.paths.config_path)?.as_deref()
            == Some(snapshot.expected.as_slice())
        {
            GrokBuildTakeoverStatus::Active
        } else {
            GrokBuildTakeoverStatus::Drifted
        };
        Ok(GrokBuildStatus {
            snapshot_present: true,
            takeover,
        })
    }
}

fn project(
    original: Option<&[u8]>,
    request: &GrokBuildRequest,
) -> Result<Vec<u8>, GrokBuildAdapterError> {
    let text = match original {
        Some(bytes) => std::str::from_utf8(bytes).map_err(|_| {
            GrokBuildAdapterError::Invalid("Grok Build config.toml must be UTF-8".into())
        })?,
        None => "",
    };
    let mut document = text.parse::<DocumentMut>().map_err(|error| {
        GrokBuildAdapterError::Invalid(format!("Grok Build config.toml is invalid TOML: {error}"))
    })?;
    let models = document
        .entry("models")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| {
            GrokBuildAdapterError::Invalid("Grok Build models must be a table".into())
        })?;
    models["default"] = value(PROFILE);
    let model_table = document
        .entry("model")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| GrokBuildAdapterError::Invalid("Grok Build model must be a table".into()))?;
    let mut profile = Table::new();
    profile["model"] = value(&request.model);
    profile["base_url"] = value(&request.base_url);
    profile["name"] = value(&request.name);
    profile["api_key"] = value(&request.api_key);
    profile["api_backend"] = value("responses");
    profile["context_window"] = value(DEFAULT_CONTEXT_WINDOW);
    model_table[PROFILE] = Item::Table(profile);
    Ok(document.to_string().into_bytes())
}

fn validate_base_url(value: &str) -> Result<(), GrokBuildAdapterError> {
    let url = Url::parse(value).map_err(|_| {
        GrokBuildAdapterError::Invalid(format!("invalid Grok Build base URL: {value}"))
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(GrokBuildAdapterError::Invalid(format!(
            "invalid Grok Build base URL: {value}"
        )));
    }
    Ok(())
}

fn validate_secret(value: &str) -> Result<(), GrokBuildAdapterError> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(GrokBuildAdapterError::Invalid(
            "Grok Build API key is empty or invalid".into(),
        ));
    }
    Ok(())
}

fn validate_text(field: &str, value: &str) -> Result<(), GrokBuildAdapterError> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(GrokBuildAdapterError::Invalid(format!(
            "Grok Build {field} is empty or invalid"
        )));
    }
    Ok(())
}

fn parse_snapshot(bytes: &[u8]) -> Result<RecoverySnapshot, GrokBuildAdapterError> {
    let snapshot: RecoverySnapshot = serde_json::from_slice(bytes).map_err(|error| {
        GrokBuildAdapterError::Invalid(format!("Grok Build recovery snapshot is invalid: {error}"))
    })?;
    if snapshot.version != 1 {
        return Err(GrokBuildAdapterError::Invalid(format!(
            "unsupported Grok Build recovery snapshot version: {}",
            snapshot.version
        )));
    }
    Ok(snapshot)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, GrokBuildAdapterError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(GrokBuildAdapterError::Io {
            operation: "read Grok Build configuration",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn write_optional(path: &Path, bytes: Option<&[u8]>) -> Result<(), GrokBuildAdapterError> {
    match bytes {
        Some(bytes) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|source| GrokBuildAdapterError::Io {
                    operation: "create Grok Build configuration directory",
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            crate::storage::atomic_replace(path, bytes).map_err(|source| {
                GrokBuildAdapterError::Io {
                    operation: "write Grok Build configuration",
                    path: path.to_path_buf(),
                    source,
                }
            })
        }
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(GrokBuildAdapterError::Io {
                operation: "remove Grok Build configuration",
                path: path.to_path_buf(),
                source,
            }),
        },
    }
}

fn combine_rollback(
    primary: GrokBuildAdapterError,
    rollback: Result<(), GrokBuildAdapterError>,
) -> GrokBuildAdapterError {
    match rollback {
        Ok(()) => primary,
        Err(rollback) => {
            GrokBuildAdapterError::Invalid(format!("{primary}; rollback also failed: {rollback}"))
        }
    }
}

#[derive(Debug)]
pub enum GrokBuildAdapterError {
    Invalid(String),
    Drifted,
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl Display for GrokBuildAdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Drifted => formatter.write_str(
                "Grok Build configuration differs from the last GrillForge apply; resolve the drift before continuing",
            ),
            Self::Io { operation, path, source } => write!(formatter, "{operation} failed for {}: {source}", path.display()),
        }
    }
}

impl Error for GrokBuildAdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
