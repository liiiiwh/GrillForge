use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::env;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::fs;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

const PROVIDER_ID: &str = "grillforge";
const SNAPSHOT_FILE: &str = "pi.snapshot.json";
// Node-based Pi distributions can spend a few seconds loading modules before
// printing `--version`, especially when installed through NVM. Detection runs
// off the UI thread, so allow startup without classifying a valid CLI as absent.
const CLI_VERSION_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiPaths {
    pub models_path: PathBuf,
    pub settings_path: PathBuf,
}

impl PiPaths {
    pub fn new(models_path: impl Into<PathBuf>, settings_path: impl Into<PathBuf>) -> Self {
        Self {
            models_path: models_path.into(),
            settings_path: settings_path.into(),
        }
    }
}

pub fn paths_from_home(home: impl AsRef<Path>) -> PiPaths {
    let root = home.as_ref().join(".pi/agent");
    PiPaths::new(root.join("models.json"), root.join("settings.json"))
}

pub fn current_pi_paths() -> Result<PiPaths, PiAdapterError> {
    dirs::home_dir()
        .map(paths_from_home)
        .ok_or(PiAdapterError::HomeDirectoryMissing)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiCliDetection {
    pub path: PathBuf,
    pub version: String,
}

pub fn detect_pi_cli() -> Result<Option<PiCliDetection>, PiAdapterError> {
    let executable = if cfg!(windows) { "pi.exe" } else { "pi" };
    let mut candidates = env::var_os("PATH")
        .map(|search_path| {
            env::split_paths(&search_path)
                .map(|directory| directory.join(executable))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        candidates.extend(pi_cli_candidates_from_home(home, executable));
    }
    #[cfg(target_os = "macos")]
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin").join(executable),
        PathBuf::from("/usr/local/bin").join(executable),
    ]);

    crate::cli_discovery::first_valid_candidate_across_sources(
        candidates,
        || {
            crate::cli_discovery::login_shell_candidates(executable).map_err(|error| {
                PiAdapterError::Invalid(format!("discover Pi CLI through the login shell: {error}"))
            })
        },
        |path| inspect_pi_cli(path),
    )
}

pub fn pi_cli_candidates_from_home(home: impl AsRef<Path>, executable: &str) -> Vec<PathBuf> {
    crate::cli_discovery::node_cli_candidates_from_home(home, executable)
}

pub fn detect_pi_cli_in(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<Option<PiCliDetection>, PiAdapterError> {
    crate::cli_discovery::first_valid_candidate(candidates, |path| inspect_pi_cli(path))
}

pub fn inspect_pi_cli(path: impl AsRef<Path>) -> Result<PiCliDetection, PiAdapterError> {
    let path = path.as_ref().to_path_buf();
    let mut command =
        crate::cli_discovery::version_command(&path).map_err(|source| PiAdapterError::Io {
            operation: "prepare Pi CLI inspection",
            path: path.clone(),
            source,
        })?;
    let mut child = command
        .arg("--version")
        // Version inspection must never perform update/catalog network work.
        .env("PI_OFFLINE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| PiAdapterError::Io {
            operation: "inspect Pi CLI",
            path: path.clone(),
            source,
        })?;
    let deadline = Instant::now() + CLI_VERSION_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().map_err(|source| PiAdapterError::Io {
            operation: "inspect Pi CLI",
            path: path.clone(),
            source,
        })? {
            let output = child
                .wait_with_output()
                .map_err(|source| PiAdapterError::Io {
                    operation: "inspect Pi CLI",
                    path: path.clone(),
                    source,
                })?;
            if !status.success() {
                return Err(PiAdapterError::Invalid(format!(
                    "Pi CLI did not return a version: {}",
                    path.display()
                )));
            }
            let version = String::from_utf8(output.stdout)
                .ok()
                .map(|version| version.trim().to_string())
                .filter(|version| !version.is_empty())
                .ok_or_else(|| {
                    PiAdapterError::Invalid(format!(
                        "Pi CLI did not return a version: {}",
                        path.display()
                    ))
                })?;
            return Ok(PiCliDetection { path, version });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PiAdapterError::Invalid(format!(
                "Pi CLI version check timed out: {}",
                path.display()
            )));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiModelSpec {
    id: String,
    name: String,
    reasoning: bool,
    input: Vec<String>,
    context_window: u64,
    max_tokens: u64,
    cost: PiModelCost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PiModelCost {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

impl PiModelSpec {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        reasoning: bool,
        input: Vec<String>,
        context_window: u64,
        max_tokens: u64,
    ) -> Result<Self, PiAdapterError> {
        let id = id.into();
        let name = name.into();
        if !valid_route_alias(&id) {
            return Err(PiAdapterError::Invalid(format!(
                "Pi model must use a GrillForge route alias: {id}"
            )));
        }
        if name.trim().is_empty() || name.trim() != name || name.chars().any(char::is_control) {
            return Err(PiAdapterError::Invalid(format!(
                "Pi model name is invalid: {id}"
            )));
        }
        if input.is_empty()
            || input
                .iter()
                .any(|kind| !matches!(kind.as_str(), "text" | "image"))
            || input.iter().collect::<HashSet<_>>().len() != input.len()
        {
            return Err(PiAdapterError::Invalid(format!(
                "Pi model input must contain unique text/image values: {id}"
            )));
        }
        if context_window == 0 || max_tokens == 0 || max_tokens > context_window {
            return Err(PiAdapterError::Invalid(format!(
                "Pi model token limits are invalid: {id}"
            )));
        }
        Ok(Self {
            id,
            name,
            reasoning,
            input,
            context_window,
            max_tokens,
            cost: PiModelCost {
                input: 0,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiRequest {
    gateway_base_url: String,
    gateway_token: String,
    models: Vec<PiModelSpec>,
    default_model: Option<String>,
}

impl PiRequest {
    pub fn new(
        gateway_base_url: impl Into<String>,
        gateway_token: impl Into<String>,
        mut models: Vec<PiModelSpec>,
        default_model: Option<String>,
    ) -> Result<Self, PiAdapterError> {
        let gateway_base_url = gateway_base_url.into();
        validate_loopback_url(&gateway_base_url)?;
        let gateway_token = gateway_token.into();
        if gateway_token.trim().is_empty()
            || gateway_token.trim() != gateway_token
            || gateway_token.chars().any(char::is_control)
        {
            return Err(PiAdapterError::Invalid(
                "Pi gateway token must not be empty, padded, or contain control characters".into(),
            ));
        }
        if models.is_empty() {
            return Err(PiAdapterError::Invalid(
                "Pi requires at least one managed model".into(),
            ));
        }
        models.sort_by(|left, right| left.id.cmp(&right.id));
        if models.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return Err(PiAdapterError::Invalid(
                "Pi managed model ids must be unique".into(),
            ));
        }
        if default_model
            .as_ref()
            .is_some_and(|id| !models.iter().any(|model| &model.id == id))
        {
            return Err(PiAdapterError::Invalid(
                "Pi default model must be present in enabled models".into(),
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

impl Debug for PiRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PiRequest")
            .field("gateway_base_url", &self.gateway_base_url)
            .field("gateway_token", &"[REDACTED]")
            .field("models", &self.models)
            .field("default_model", &self.default_model)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiTakeoverStatus {
    Inactive,
    Active,
    Drifted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiStatus {
    pub snapshot_present: bool,
    pub takeover: PiTakeoverStatus,
}

#[derive(Debug)]
pub struct PiAdapter {
    paths: PiPaths,
    snapshot_path: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
struct RecoverySnapshot {
    version: u8,
    original: ManagedFiles,
    expected: ManagedFiles,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedFiles {
    models: Option<Vec<u8>>,
    settings: Option<Vec<u8>>,
}

impl ManagedFiles {
    fn semantically_matches(&self, expected: &Self) -> bool {
        json_bytes_match(self.models.as_deref(), expected.models.as_deref())
            && json_bytes_match(self.settings.as_deref(), expected.settings.as_deref())
    }
}

impl PiAdapter {
    pub fn new(paths: PiPaths, grillforge_root: impl Into<PathBuf>) -> Self {
        Self {
            paths,
            snapshot_path: grillforge_root.into().join(SNAPSHOT_FILE),
        }
    }

    pub fn snapshot_path(&self) -> &Path {
        &self.snapshot_path
    }

    pub fn apply(&self, request: PiRequest) -> Result<PiStatus, PiAdapterError> {
        let current = self.capture()?;
        let previous_snapshot_bytes = read_optional(&self.snapshot_path)?;
        let original = match &previous_snapshot_bytes {
            Some(bytes) => {
                let snapshot = parse_snapshot(bytes)?;
                if !current.semantically_matches(&snapshot.expected) {
                    return Err(PiAdapterError::Drifted);
                }
                snapshot.original
            }
            None => current.clone(),
        };
        let expected = ManagedFiles {
            models: Some(project_models(current.models.as_deref(), &request)?),
            settings: Some(project_settings(current.settings.as_deref(), &request)?),
        };
        let snapshot = RecoverySnapshot {
            version: 1,
            original,
            expected: expected.clone(),
        };
        let snapshot_bytes = serde_json::to_vec_pretty(&snapshot).map_err(|error| {
            PiAdapterError::Invalid(format!("could not encode Pi snapshot: {error}"))
        })?;

        if let Err(error) = self.write_apply(&snapshot_bytes, &expected) {
            let rollback = self.restore_files(&current).and_then(|()| {
                write_optional(&self.snapshot_path, previous_snapshot_bytes.as_deref())
            });
            return Err(combine_rollback(error, rollback));
        }
        self.status()
    }

    pub fn disable(&self) -> Result<PiStatus, PiAdapterError> {
        let Some(snapshot_bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(PiStatus {
                snapshot_present: false,
                takeover: PiTakeoverStatus::Inactive,
            });
        };
        let snapshot = parse_snapshot(&snapshot_bytes)?;
        if !self.capture()?.semantically_matches(&snapshot.expected) {
            return Err(PiAdapterError::Drifted);
        }
        self.restore_files(&snapshot.original)?;
        if self.capture()? != snapshot.original {
            return Err(PiAdapterError::Invalid(
                "Pi restore verification failed; recovery snapshot was retained".into(),
            ));
        }
        fs::remove_file(&self.snapshot_path).map_err(|source| PiAdapterError::Io {
            operation: "remove Pi recovery snapshot",
            path: self.snapshot_path.clone(),
            source,
        })?;
        self.status()
    }

    pub fn status(&self) -> Result<PiStatus, PiAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(PiStatus {
                snapshot_present: false,
                takeover: PiTakeoverStatus::Inactive,
            });
        };
        let snapshot = parse_snapshot(&bytes)?;
        Ok(PiStatus {
            snapshot_present: true,
            takeover: if self.capture()?.semantically_matches(&snapshot.expected) {
                PiTakeoverStatus::Active
            } else {
                PiTakeoverStatus::Drifted
            },
        })
    }

    fn capture(&self) -> Result<ManagedFiles, PiAdapterError> {
        Ok(ManagedFiles {
            models: read_optional(&self.paths.models_path)?,
            settings: read_optional(&self.paths.settings_path)?,
        })
    }

    fn write_apply(&self, snapshot: &[u8], expected: &ManagedFiles) -> Result<(), PiAdapterError> {
        write_optional(&self.snapshot_path, Some(snapshot))?;
        self.restore_files(expected)?;
        if !self.capture()?.semantically_matches(expected) {
            return Err(PiAdapterError::Invalid(
                "Pi apply verification failed".into(),
            ));
        }
        Ok(())
    }

    fn restore_files(&self, files: &ManagedFiles) -> Result<(), PiAdapterError> {
        write_optional(&self.paths.models_path, files.models.as_deref())?;
        write_optional(&self.paths.settings_path, files.settings.as_deref())
    }
}

fn json_bytes_match(current: Option<&[u8]>, expected: Option<&[u8]>) -> bool {
    match (current, expected) {
        (None, None) => true,
        (Some(current), Some(expected)) => {
            match (
                serde_json::from_slice::<Value>(current),
                serde_json::from_slice::<Value>(expected),
            ) {
                (Ok(current), Ok(expected)) => current == expected,
                _ => current == expected,
            }
        }
        _ => false,
    }
}

fn project_models(original: Option<&[u8]>, request: &PiRequest) -> Result<Vec<u8>, PiAdapterError> {
    let mut root = parse_json_object(original, "Pi models.json")?;
    let providers = object_entry(&mut root, "providers", "Pi models.json providers")?;
    providers.insert(
        PROVIDER_ID.into(),
        json!({
            "baseUrl": request.gateway_base_url,
            "api": "anthropic-messages",
            "apiKey": request.gateway_token,
            "models": request.models,
        }),
    );
    pretty_json(root, "Pi models.json")
}

fn project_settings(
    original: Option<&[u8]>,
    request: &PiRequest,
) -> Result<Vec<u8>, PiAdapterError> {
    let mut root = parse_json_object(original, "Pi settings.json")?;
    root.insert("defaultProvider".into(), Value::String(PROVIDER_ID.into()));
    match &request.default_model {
        Some(model) => {
            root.insert("defaultModel".into(), Value::String(model.clone()));
        }
        None => {
            root.remove("defaultModel");
        }
    }
    root.insert(
        "enabledModels".into(),
        Value::Array(
            request
                .models
                .iter()
                .map(|model| Value::String(model.id.clone()))
                .collect(),
        ),
    );
    pretty_json(root, "Pi settings.json")
}

fn parse_json_object(
    bytes: Option<&[u8]>,
    label: &str,
) -> Result<Map<String, Value>, PiAdapterError> {
    let Some(bytes) = bytes else {
        return Ok(Map::new());
    };
    serde_json::from_slice::<Value>(bytes)
        .map_err(|error| PiAdapterError::Invalid(format!("{label} is invalid JSON: {error}")))?
        .as_object()
        .cloned()
        .ok_or_else(|| PiAdapterError::Invalid(format!("{label} root must be an object")))
}

fn object_entry<'a>(
    root: &'a mut Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<&'a mut Map<String, Value>, PiAdapterError> {
    let value = root.entry(key).or_insert_with(|| Value::Object(Map::new()));
    value
        .as_object_mut()
        .ok_or_else(|| PiAdapterError::Invalid(format!("{label} must be an object")))
}

fn pretty_json(root: Map<String, Value>, label: &str) -> Result<Vec<u8>, PiAdapterError> {
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(root))
        .map_err(|error| PiAdapterError::Invalid(format!("could not encode {label}: {error}")))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn validate_loopback_url(value: &str) -> Result<(), PiAdapterError> {
    let url = Url::parse(value)
        .map_err(|_| PiAdapterError::Invalid(format!("invalid Pi gateway URL: {value}")))?;
    let loopback = url
        .host_str()
        .and_then(|host| host.parse::<IpAddr>().ok())
        .is_some_and(|address| address.is_loopback());
    if url.scheme() != "http"
        || !loopback
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(PiAdapterError::Invalid(
            "Pi gateway must be a plain HTTP loopback URL without credentials, query, or fragment"
                .into(),
        ));
    }
    Ok(())
}

fn valid_route_alias(value: &str) -> bool {
    value.strip_prefix("grillforge/").is_some_and(|id| {
        !id.is_empty()
            && id.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.')
            })
    })
}

fn parse_snapshot(bytes: &[u8]) -> Result<RecoverySnapshot, PiAdapterError> {
    let snapshot: RecoverySnapshot = serde_json::from_slice(bytes).map_err(|error| {
        PiAdapterError::Invalid(format!("Pi recovery snapshot is invalid: {error}"))
    })?;
    if snapshot.version != 1 {
        return Err(PiAdapterError::Invalid(format!(
            "unsupported Pi recovery snapshot version: {}",
            snapshot.version
        )));
    }
    Ok(snapshot)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, PiAdapterError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(PiAdapterError::Io {
            operation: "read Pi configuration",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn write_optional(path: &Path, bytes: Option<&[u8]>) -> Result<(), PiAdapterError> {
    match bytes {
        Some(bytes) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|source| PiAdapterError::Io {
                    operation: "create Pi configuration directory",
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            crate::storage::atomic_replace(path, bytes).map_err(|source| PiAdapterError::Io {
                operation: "write Pi configuration",
                path: path.to_path_buf(),
                source,
            })
        }
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(PiAdapterError::Io {
                operation: "remove Pi configuration",
                path: path.to_path_buf(),
                source,
            }),
        },
    }
}

fn combine_rollback(
    primary: PiAdapterError,
    rollback: Result<(), PiAdapterError>,
) -> PiAdapterError {
    match rollback {
        Ok(()) => primary,
        Err(rollback) => {
            PiAdapterError::Invalid(format!("{primary}; rollback also failed: {rollback}"))
        }
    }
}

#[derive(Debug)]
pub enum PiAdapterError {
    HomeDirectoryMissing,
    Invalid(String),
    Drifted,
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl Display for PiAdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HomeDirectoryMissing => formatter.write_str("could not locate the home directory"),
            Self::Invalid(message) => formatter.write_str(message),
            Self::Drifted => formatter.write_str(
                "Pi configuration differs from the last GrillForge apply; restore or resolve the drift before continuing",
            ),
            Self::Io {
                operation,
                path,
                source,
            } => write!(formatter, "{operation} failed for {}: {source}", path.display()),
        }
    }
}

impl Error for PiAdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
