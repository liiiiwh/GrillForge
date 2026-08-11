use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::env;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

const PROVIDER_ID: &str = "grillforge";
const OFFICIAL_SCHEMA: &str = "https://opencode.ai/config.json";
const SNAPSHOT_FILE: &str = "opencode.snapshot.json";
const CLI_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodePaths {
    pub config_path: PathBuf,
}

impl OpenCodePaths {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
        }
    }
}

pub fn paths_from_home(home: impl AsRef<Path>) -> OpenCodePaths {
    OpenCodePaths::new(home.as_ref().join(".config/opencode/opencode.json"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeCliDetection {
    pub path: PathBuf,
    pub version: String,
}

pub fn detect_opencode_cli() -> Result<Option<OpenCodeCliDetection>, OpenCodeAdapterError> {
    let executable = if cfg!(windows) {
        "opencode.exe"
    } else {
        "opencode"
    };
    let mut candidates = env::var_os("PATH")
        .map(|path| {
            env::split_paths(&path)
                .map(|directory| directory.join(executable))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for variable in ["OPENCODE_INSTALL_DIR", "XDG_BIN_DIR"] {
        if let Some(directory) = env::var_os(variable) {
            candidates.push(PathBuf::from(directory).join(executable));
        }
    }
    if let Some(home) = dirs::home_dir() {
        candidates.extend(crate::cli_discovery::node_cli_candidates_from_home(
            &home, executable,
        ));
        candidates.extend([
            home.join("bin").join(executable),
            home.join(".opencode/bin").join(executable),
        ]);
        #[cfg(target_os = "macos")]
        candidates.push(home.join("Applications/OpenCode.app/Contents/MacOS/opencode-cli"));
    }
    #[cfg(target_os = "macos")]
    candidates.extend([
        PathBuf::from("/Applications/OpenCode.app/Contents/MacOS/opencode-cli"),
        PathBuf::from("/opt/homebrew/bin").join(executable),
        PathBuf::from("/usr/local/bin").join(executable),
    ]);
    crate::cli_discovery::first_valid_candidate_across_sources(
        candidates,
        || {
            crate::cli_discovery::login_shell_candidates(executable).map_err(|error| {
                OpenCodeAdapterError::Invalid(format!(
                    "discover OpenCode CLI through the login shell: {error}"
                ))
            })
        },
        |path| inspect_opencode_cli(path),
    )
}

pub fn detect_opencode_cli_in(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<Option<OpenCodeCliDetection>, OpenCodeAdapterError> {
    crate::cli_discovery::first_valid_candidate(candidates, |path| inspect_opencode_cli(path))
}

pub fn inspect_opencode_cli(
    path: impl AsRef<Path>,
) -> Result<OpenCodeCliDetection, OpenCodeAdapterError> {
    let path = path.as_ref().to_path_buf();
    let mut command = crate::cli_discovery::version_command(&path).map_err(|source| {
        OpenCodeAdapterError::Io {
            operation: "prepare OpenCode CLI inspection",
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
        .map_err(|source| OpenCodeAdapterError::Io {
            operation: "inspect OpenCode CLI",
            path: path.clone(),
            source,
        })?;
    let deadline = Instant::now() + CLI_TIMEOUT;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|source| OpenCodeAdapterError::Io {
                operation: "inspect OpenCode CLI",
                path: path.clone(),
                source,
            })?
        {
            let output = child
                .wait_with_output()
                .map_err(|source| OpenCodeAdapterError::Io {
                    operation: "inspect OpenCode CLI",
                    path: path.clone(),
                    source,
                })?;
            if !status.success() {
                return Err(OpenCodeAdapterError::Invalid(format!(
                    "OpenCode CLI did not return a version: {}",
                    path.display()
                )));
            }
            let version = String::from_utf8(output.stdout)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    OpenCodeAdapterError::Invalid(format!(
                        "OpenCode CLI did not return a version: {}",
                        path.display()
                    ))
                })?;
            return Ok(OpenCodeCliDetection { path, version });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(OpenCodeAdapterError::Invalid(format!(
                "OpenCode CLI version check timed out: {}",
                path.display()
            )));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodeModel {
    id: String,
    name: String,
}

impl OpenCodeModel {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<Self, OpenCodeAdapterError> {
        let id = id.into();
        if !id.starts_with("grillforge/")
            || id.len() == "grillforge/".len()
            || id.trim() != id
            || id.chars().any(char::is_control)
        {
            return Err(OpenCodeAdapterError::Invalid(format!(
                "OpenCode model must use a GrillForge route alias: {id}"
            )));
        }
        let name = name.into();
        if name.trim().is_empty() || name.trim() != name || name.chars().any(char::is_control) {
            return Err(OpenCodeAdapterError::Invalid(format!(
                "OpenCode model name is invalid: {id}"
            )));
        }
        Ok(Self { id, name })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodeRequest {
    gateway_base_url: String,
    gateway_token: String,
    models: Vec<OpenCodeModel>,
    default_model: String,
}

impl OpenCodeRequest {
    pub fn new(
        gateway_base_url: impl Into<String>,
        gateway_token: impl Into<String>,
        mut models: Vec<OpenCodeModel>,
        default_model: impl Into<String>,
    ) -> Result<Self, OpenCodeAdapterError> {
        let gateway_base_url = gateway_base_url.into();
        validate_gateway_url(&gateway_base_url)?;
        let gateway_token = gateway_token.into();
        if gateway_token.trim().is_empty()
            || gateway_token.trim() != gateway_token
            || gateway_token.chars().any(char::is_control)
        {
            return Err(OpenCodeAdapterError::Invalid(
                "OpenCode gateway token must not be empty, padded, or contain control characters"
                    .into(),
            ));
        }
        if models.is_empty() {
            return Err(OpenCodeAdapterError::Invalid(
                "OpenCode requires at least one managed model".into(),
            ));
        }
        models.sort_by(|left, right| left.id.cmp(&right.id));
        if models.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return Err(OpenCodeAdapterError::Invalid(
                "OpenCode managed model ids must be unique".into(),
            ));
        }
        let default_model = default_model.into();
        if !models.iter().any(|model| model.id == default_model) {
            return Err(OpenCodeAdapterError::Invalid(
                "OpenCode default model must be present in managed models".into(),
            ));
        }
        Ok(Self {
            gateway_base_url,
            gateway_token,
            models,
            default_model,
        })
    }
}

impl Debug for OpenCodeRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenCodeRequest")
            .field("gateway_base_url", &self.gateway_base_url)
            .field("gateway_token", &"[REDACTED]")
            .field("models", &self.models)
            .field("default_model", &self.default_model)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeTakeoverStatus {
    Inactive,
    Active,
    Drifted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeStatus {
    pub snapshot_present: bool,
    pub takeover: OpenCodeTakeoverStatus,
}

#[derive(Debug)]
pub struct OpenCodeAdapter {
    paths: OpenCodePaths,
    snapshot_path: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
struct RecoverySnapshot {
    version: u8,
    original: Option<Vec<u8>>,
    expected: Vec<u8>,
}

impl OpenCodeAdapter {
    pub fn new(paths: OpenCodePaths, grillforge_root: impl Into<PathBuf>) -> Self {
        Self {
            paths,
            snapshot_path: grillforge_root.into().join(SNAPSHOT_FILE),
        }
    }

    pub fn apply(&self, request: OpenCodeRequest) -> Result<OpenCodeStatus, OpenCodeAdapterError> {
        let current = read_optional(&self.paths.config_path)?;
        let previous_snapshot = read_optional(&self.snapshot_path)?;
        let original = match &previous_snapshot {
            Some(bytes) => {
                let snapshot = parse_snapshot(bytes)?;
                if current.as_deref() != Some(snapshot.expected.as_slice()) {
                    return Err(OpenCodeAdapterError::Drifted);
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
            OpenCodeAdapterError::Invalid(format!("could not encode OpenCode snapshot: {error}"))
        })?;
        if let Err(error) = write_pair(
            &self.snapshot_path,
            &snapshot_bytes,
            &self.paths.config_path,
            &expected,
        ) {
            let rollback = write_optional(&self.paths.config_path, current.as_deref())
                .and_then(|()| write_optional(&self.snapshot_path, previous_snapshot.as_deref()));
            return Err(combine_rollback(error, rollback));
        }
        if read_optional(&self.paths.config_path)?.as_deref() != Some(expected.as_slice()) {
            return Err(OpenCodeAdapterError::Invalid(
                "OpenCode apply verification failed".into(),
            ));
        }
        self.status()
    }

    pub fn disable(&self) -> Result<OpenCodeStatus, OpenCodeAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(OpenCodeStatus {
                snapshot_present: false,
                takeover: OpenCodeTakeoverStatus::Inactive,
            });
        };
        let snapshot = parse_snapshot(&bytes)?;
        if read_optional(&self.paths.config_path)?.as_deref() != Some(snapshot.expected.as_slice())
        {
            return Err(OpenCodeAdapterError::Drifted);
        }
        write_optional(&self.paths.config_path, snapshot.original.as_deref())?;
        if read_optional(&self.paths.config_path)? != snapshot.original {
            return Err(OpenCodeAdapterError::Invalid(
                "OpenCode restore verification failed; recovery snapshot was retained".into(),
            ));
        }
        fs::remove_file(&self.snapshot_path).map_err(|source| OpenCodeAdapterError::Io {
            operation: "remove OpenCode recovery snapshot",
            path: self.snapshot_path.clone(),
            source,
        })?;
        self.status()
    }

    pub fn status(&self) -> Result<OpenCodeStatus, OpenCodeAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(OpenCodeStatus {
                snapshot_present: false,
                takeover: OpenCodeTakeoverStatus::Inactive,
            });
        };
        let snapshot = parse_snapshot(&bytes)?;
        Ok(OpenCodeStatus {
            snapshot_present: true,
            takeover: if read_optional(&self.paths.config_path)?.as_deref()
                == Some(snapshot.expected.as_slice())
            {
                OpenCodeTakeoverStatus::Active
            } else {
                OpenCodeTakeoverStatus::Drifted
            },
        })
    }
}

fn project(
    original: Option<&[u8]>,
    request: &OpenCodeRequest,
) -> Result<Vec<u8>, OpenCodeAdapterError> {
    let mut config = match original {
        Some(bytes) => {
            let text = std::str::from_utf8(bytes).map_err(|_| {
                OpenCodeAdapterError::Invalid("OpenCode config must be UTF-8".into())
            })?;
            json5::from_str::<Value>(text).map_err(|error| {
                OpenCodeAdapterError::Invalid(format!("OpenCode config is invalid JSON5: {error}"))
            })?
        }
        None => json!({ "$schema": OFFICIAL_SCHEMA }),
    };
    let root = config.as_object_mut().ok_or_else(|| {
        OpenCodeAdapterError::Invalid("OpenCode configuration root must be a JSON object".into())
    })?;
    if !root.contains_key("$schema") {
        root.insert("$schema".into(), Value::String(OFFICIAL_SCHEMA.into()));
    }
    let providers = match root.entry("provider") {
        serde_json::map::Entry::Vacant(entry) => entry
            .insert(Value::Object(Map::new()))
            .as_object_mut()
            .expect("inserted object"),
        serde_json::map::Entry::Occupied(entry) => {
            entry.into_mut().as_object_mut().ok_or_else(|| {
                OpenCodeAdapterError::Invalid("OpenCode provider must be a JSON object".into())
            })?
        }
    };
    let model_map = request
        .models
        .iter()
        .map(|model| (model.id.clone(), json!({ "name": model.name })))
        .collect::<Map<_, _>>();
    providers.insert(
        PROVIDER_ID.into(),
        json!({
            "npm": "@ai-sdk/anthropic",
            "name": "GrillForge",
            "options": {
                "baseURL": request.gateway_base_url,
                "apiKey": request.gateway_token,
            },
            "models": model_map,
        }),
    );
    root.insert(
        "model".into(),
        Value::String(format!("{PROVIDER_ID}/{}", request.default_model)),
    );
    serde_json::to_vec_pretty(&config).map_err(|error| {
        OpenCodeAdapterError::Invalid(format!("could not encode OpenCode configuration: {error}"))
    })
}

fn validate_gateway_url(value: &str) -> Result<(), OpenCodeAdapterError> {
    let url = Url::parse(value).map_err(|_| {
        OpenCodeAdapterError::Invalid(format!("invalid OpenCode gateway URL: {value}"))
    })?;
    let host = url
        .host_str()
        .and_then(|host| host.parse::<std::net::IpAddr>().ok());
    if url.scheme() != "http"
        || !host.is_some_and(|host| host.is_loopback())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(OpenCodeAdapterError::Invalid(format!(
            "OpenCode gateway URL must be an HTTP loopback URL without credentials, query, or fragment: {value}"
        )));
    }
    Ok(())
}

fn parse_snapshot(bytes: &[u8]) -> Result<RecoverySnapshot, OpenCodeAdapterError> {
    let snapshot: RecoverySnapshot = serde_json::from_slice(bytes).map_err(|error| {
        OpenCodeAdapterError::Invalid(format!("OpenCode recovery snapshot is invalid: {error}"))
    })?;
    if snapshot.version != 1 {
        return Err(OpenCodeAdapterError::Invalid(format!(
            "unsupported OpenCode recovery snapshot version: {}",
            snapshot.version
        )));
    }
    Ok(snapshot)
}

fn write_pair(
    first_path: &Path,
    first: &[u8],
    second_path: &Path,
    second: &[u8],
) -> Result<(), OpenCodeAdapterError> {
    write_optional(first_path, Some(first))?;
    write_optional(second_path, Some(second))
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, OpenCodeAdapterError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(OpenCodeAdapterError::Io {
            operation: "read OpenCode configuration",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn write_optional(path: &Path, bytes: Option<&[u8]>) -> Result<(), OpenCodeAdapterError> {
    match bytes {
        Some(bytes) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|source| OpenCodeAdapterError::Io {
                    operation: "create OpenCode configuration directory",
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            crate::storage::atomic_replace(path, bytes).map_err(|source| OpenCodeAdapterError::Io {
                operation: "write OpenCode configuration",
                path: path.to_path_buf(),
                source,
            })
        }
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(OpenCodeAdapterError::Io {
                operation: "remove OpenCode configuration",
                path: path.to_path_buf(),
                source,
            }),
        },
    }
}

fn combine_rollback(
    primary: OpenCodeAdapterError,
    rollback: Result<(), OpenCodeAdapterError>,
) -> OpenCodeAdapterError {
    match rollback {
        Ok(()) => primary,
        Err(rollback) => {
            OpenCodeAdapterError::Invalid(format!("{primary}; rollback also failed: {rollback}"))
        }
    }
}

#[derive(Debug)]
pub enum OpenCodeAdapterError {
    Invalid(String),
    Drifted,
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl Display for OpenCodeAdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Drifted => formatter.write_str(
                "OpenCode configuration differs from the last GrillForge apply; resolve the drift before continuing",
            ),
            Self::Io { operation, path, source } => write!(formatter, "{operation} failed for {}: {source}", path.display()),
        }
    }
}

impl Error for OpenCodeAdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
