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
const SNAPSHOT_FILE: &str = "openclaw.snapshot.json";
const CLI_VERSION_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenClawPaths {
    pub config_path: PathBuf,
}

impl OpenClawPaths {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
        }
    }
}

pub fn paths_from_home(home: impl AsRef<Path>) -> OpenClawPaths {
    OpenClawPaths::new(home.as_ref().join(".openclaw/openclaw.json"))
}

pub fn current_openclaw_paths() -> Result<OpenClawPaths, OpenClawAdapterError> {
    dirs::home_dir()
        .map(paths_from_home)
        .ok_or(OpenClawAdapterError::HomeDirectoryMissing)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenClawCliDetection {
    pub path: PathBuf,
    pub version: String,
}

pub fn detect_openclaw_cli() -> Result<Option<OpenClawCliDetection>, OpenClawAdapterError> {
    let executable = if cfg!(windows) {
        "openclaw.exe"
    } else {
        "openclaw"
    };
    let mut candidates = env::var_os("PATH")
        .map(|search_path| {
            env::split_paths(&search_path)
                .map(|directory| directory.join(executable))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        candidates.extend(crate::cli_discovery::node_cli_candidates_from_home(
            &home, executable,
        ));
        candidates.extend([home.join(".openclaw/bin").join(executable)]);
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
                OpenClawAdapterError::Invalid(format!(
                    "discover OpenClaw CLI through the login shell: {error}"
                ))
            })
        },
        |path| inspect_openclaw_cli(path),
    )
}

pub fn detect_openclaw_cli_in(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<Option<OpenClawCliDetection>, OpenClawAdapterError> {
    crate::cli_discovery::first_valid_candidate(candidates, |path| inspect_openclaw_cli(path))
}

pub fn inspect_openclaw_cli(
    path: impl AsRef<Path>,
) -> Result<OpenClawCliDetection, OpenClawAdapterError> {
    let path = path.as_ref().to_path_buf();
    let mut command = crate::cli_discovery::version_command(&path).map_err(|source| {
        OpenClawAdapterError::Io {
            operation: "prepare OpenClaw CLI inspection",
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
        .map_err(|source| OpenClawAdapterError::Io {
            operation: "inspect OpenClaw CLI",
            path: path.clone(),
            source,
        })?;
    let deadline = Instant::now() + CLI_VERSION_TIMEOUT;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|source| OpenClawAdapterError::Io {
                operation: "inspect OpenClaw CLI",
                path: path.clone(),
                source,
            })?
        {
            let output = child
                .wait_with_output()
                .map_err(|source| OpenClawAdapterError::Io {
                    operation: "inspect OpenClaw CLI",
                    path: path.clone(),
                    source,
                })?;
            if !status.success() {
                return Err(OpenClawAdapterError::Invalid(format!(
                    "OpenClaw CLI did not return a version: {}",
                    path.display()
                )));
            }
            let version = String::from_utf8(output.stdout)
                .ok()
                .map(|version| version.trim().to_string())
                .filter(|version| !version.is_empty())
                .ok_or_else(|| {
                    OpenClawAdapterError::Invalid(format!(
                        "OpenClaw CLI did not return a version: {}",
                        path.display()
                    ))
                })?;
            return Ok(OpenClawCliDetection { path, version });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(OpenClawAdapterError::Invalid(format!(
                "OpenClaw CLI version check timed out: {}",
                path.display()
            )));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenClawModelSpec {
    id: String,
    name: String,
    reasoning: bool,
    input: Vec<String>,
    context_window: u64,
    max_tokens: u64,
}

impl OpenClawModelSpec {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        reasoning: bool,
        input: Vec<String>,
        context_window: u64,
        max_tokens: u64,
    ) -> Result<Self, OpenClawAdapterError> {
        let id = id.into();
        let name = name.into();
        if !valid_route_alias(&id) {
            return Err(OpenClawAdapterError::Invalid(format!(
                "OpenClaw model must use a GrillForge route alias: {id}"
            )));
        }
        if name.trim().is_empty() || name.trim() != name || name.chars().any(char::is_control) {
            return Err(OpenClawAdapterError::Invalid(format!(
                "OpenClaw model name is invalid: {id}"
            )));
        }
        if input.is_empty()
            || input
                .iter()
                .any(|kind| !matches!(kind.as_str(), "text" | "image"))
            || input.iter().collect::<HashSet<_>>().len() != input.len()
        {
            return Err(OpenClawAdapterError::Invalid(format!(
                "OpenClaw model input must contain unique text/image values: {id}"
            )));
        }
        if context_window == 0 || max_tokens == 0 || max_tokens > context_window {
            return Err(OpenClawAdapterError::Invalid(format!(
                "OpenClaw model token limits are invalid: {id}"
            )));
        }
        Ok(Self {
            id,
            name,
            reasoning,
            input,
            context_window,
            max_tokens,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenClawRequest {
    gateway_base_url: String,
    gateway_token: String,
    models: Vec<OpenClawModelSpec>,
    primary: String,
    fallbacks: Vec<String>,
}

impl OpenClawRequest {
    pub fn new(
        gateway_base_url: impl Into<String>,
        gateway_token: impl Into<String>,
        mut models: Vec<OpenClawModelSpec>,
        primary: impl Into<String>,
        fallbacks: Vec<String>,
    ) -> Result<Self, OpenClawAdapterError> {
        let gateway_base_url = gateway_base_url.into();
        validate_loopback_url(&gateway_base_url)?;
        let gateway_token = gateway_token.into();
        if gateway_token.trim().is_empty()
            || gateway_token.trim() != gateway_token
            || gateway_token.chars().any(char::is_control)
        {
            return Err(OpenClawAdapterError::Invalid(
                "OpenClaw gateway token must not be empty, padded, or contain control characters"
                    .into(),
            ));
        }
        if models.is_empty() {
            return Err(OpenClawAdapterError::Invalid(
                "OpenClaw requires at least one managed model".into(),
            ));
        }
        models.sort_by(|left, right| left.id.cmp(&right.id));
        if models.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return Err(OpenClawAdapterError::Invalid(
                "OpenClaw managed model ids must be unique".into(),
            ));
        }
        let primary = primary.into();
        if !models.iter().any(|model| model.id == primary) {
            return Err(OpenClawAdapterError::Invalid(
                "OpenClaw primary model must be present in the managed model pool".into(),
            ));
        }
        if fallbacks.iter().any(|fallback| fallback == &primary)
            || fallbacks
                .iter()
                .any(|fallback| !models.iter().any(|model| &model.id == fallback))
            || fallbacks.iter().collect::<HashSet<_>>().len() != fallbacks.len()
        {
            return Err(OpenClawAdapterError::Invalid(
                "OpenClaw fallback models must be unique non-primary members of the managed model pool"
                    .into(),
            ));
        }
        Ok(Self {
            gateway_base_url,
            gateway_token,
            models,
            primary,
            fallbacks,
        })
    }
}

impl Debug for OpenClawRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenClawRequest")
            .field("gateway_base_url", &self.gateway_base_url)
            .field("gateway_token", &"[REDACTED]")
            .field("models", &self.models)
            .field("primary", &self.primary)
            .field("fallbacks", &self.fallbacks)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenClawTakeoverStatus {
    Inactive,
    Active,
    Drifted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenClawStatus {
    pub snapshot_present: bool,
    pub takeover: OpenClawTakeoverStatus,
}

#[derive(Debug)]
pub struct OpenClawAdapter {
    paths: OpenClawPaths,
    snapshot_path: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
struct RecoverySnapshot {
    version: u8,
    original: Option<Vec<u8>>,
    expected: Vec<u8>,
}

impl OpenClawAdapter {
    pub fn new(paths: OpenClawPaths, grillforge_root: impl Into<PathBuf>) -> Self {
        Self {
            paths,
            snapshot_path: grillforge_root.into().join(SNAPSHOT_FILE),
        }
    }

    pub fn snapshot_path(&self) -> &Path {
        &self.snapshot_path
    }

    pub fn apply(&self, request: OpenClawRequest) -> Result<OpenClawStatus, OpenClawAdapterError> {
        let current = read_optional(&self.paths.config_path)?;
        let previous_snapshot = read_optional(&self.snapshot_path)?;
        let original = match &previous_snapshot {
            Some(bytes) => {
                let snapshot = parse_snapshot(bytes)?;
                if current.as_deref() != Some(snapshot.expected.as_slice()) {
                    return Err(OpenClawAdapterError::Drifted);
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
            OpenClawAdapterError::Invalid(format!(
                "could not encode OpenClaw recovery snapshot: {error}"
            ))
        })?;

        let applied = write_pair(
            &self.snapshot_path,
            &snapshot_bytes,
            &self.paths.config_path,
            &expected,
        )
        .and_then(|()| {
            if read_optional(&self.paths.config_path)?.as_deref() == Some(expected.as_slice()) {
                Ok(())
            } else {
                Err(OpenClawAdapterError::Invalid(
                    "OpenClaw apply verification failed".into(),
                ))
            }
        });
        if let Err(error) = applied {
            let rollback_config = write_optional(&self.paths.config_path, current.as_deref());
            let rollback_snapshot =
                write_optional(&self.snapshot_path, previous_snapshot.as_deref());
            return Err(combine_rollback(
                error,
                rollback_config.and(rollback_snapshot),
            ));
        }
        self.status()
    }

    pub fn disable(&self) -> Result<OpenClawStatus, OpenClawAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(OpenClawStatus {
                snapshot_present: false,
                takeover: OpenClawTakeoverStatus::Inactive,
            });
        };
        let snapshot = parse_snapshot(&bytes)?;
        if read_optional(&self.paths.config_path)?.as_deref() != Some(snapshot.expected.as_slice())
        {
            return Err(OpenClawAdapterError::Drifted);
        }
        write_optional(&self.paths.config_path, snapshot.original.as_deref())?;
        if read_optional(&self.paths.config_path)? != snapshot.original {
            return Err(OpenClawAdapterError::Invalid(
                "OpenClaw restore verification failed; recovery snapshot was retained".into(),
            ));
        }
        fs::remove_file(&self.snapshot_path).map_err(|source| OpenClawAdapterError::Io {
            operation: "remove OpenClaw recovery snapshot",
            path: self.snapshot_path.clone(),
            source,
        })?;
        self.status()
    }

    pub fn status(&self) -> Result<OpenClawStatus, OpenClawAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(OpenClawStatus {
                snapshot_present: false,
                takeover: OpenClawTakeoverStatus::Inactive,
            });
        };
        let snapshot = parse_snapshot(&bytes)?;
        Ok(OpenClawStatus {
            snapshot_present: true,
            takeover: if read_optional(&self.paths.config_path)?.as_deref()
                == Some(snapshot.expected.as_slice())
            {
                OpenClawTakeoverStatus::Active
            } else {
                OpenClawTakeoverStatus::Drifted
            },
        })
    }
}

fn project(
    original: Option<&[u8]>,
    request: &OpenClawRequest,
) -> Result<Vec<u8>, OpenClawAdapterError> {
    let mut root = parse_json5_object(original)?;
    let models = root.entry("models").or_insert_with(|| {
        json!({
            "mode": "merge",
            "providers": {}
        })
    });
    let models = models
        .as_object_mut()
        .ok_or_else(|| OpenClawAdapterError::Invalid("OpenClaw models must be an object".into()))?;
    let providers = object_entry(models, "providers", "OpenClaw models.providers")?;
    providers.insert(
        PROVIDER_ID.into(),
        json!({
            "baseUrl": request.gateway_base_url,
            "apiKey": request.gateway_token,
            "api": "anthropic-messages",
            "models": request.models,
        }),
    );

    let agents = object_entry(&mut root, "agents", "OpenClaw agents")?;
    let defaults = object_entry(agents, "defaults", "OpenClaw agents.defaults")?;
    let model = object_entry(defaults, "model", "OpenClaw agents.defaults.model")?;
    model.insert("primary".into(), Value::String(model_ref(&request.primary)));
    if request.fallbacks.is_empty() {
        model.remove("fallbacks");
    } else {
        model.insert(
            "fallbacks".into(),
            Value::Array(
                request
                    .fallbacks
                    .iter()
                    .map(|id| Value::String(model_ref(id)))
                    .collect(),
            ),
        );
    }
    let catalog = object_entry(defaults, "models", "OpenClaw agents.defaults.models")?;
    catalog.retain(|id, _| !id.starts_with("grillforge/"));
    for model in &request.models {
        catalog.insert(model_ref(&model.id), json!({"alias": model.name}));
    }

    let mut bytes = serde_json::to_vec_pretty(&Value::Object(root)).map_err(|error| {
        OpenClawAdapterError::Invalid(format!("could not encode OpenClaw configuration: {error}"))
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn model_ref(id: &str) -> String {
    format!("{PROVIDER_ID}/{id}")
}

fn parse_json5_object(bytes: Option<&[u8]>) -> Result<Map<String, Value>, OpenClawAdapterError> {
    let Some(bytes) = bytes else {
        return Ok(Map::new());
    };
    let source = std::str::from_utf8(bytes).map_err(|_| {
        OpenClawAdapterError::Invalid("OpenClaw openclaw.json must be UTF-8".into())
    })?;
    json5::from_str::<Value>(source)
        .map_err(|error| {
            OpenClawAdapterError::Invalid(format!(
                "OpenClaw openclaw.json is invalid JSON5: {error}"
            ))
        })?
        .as_object()
        .cloned()
        .ok_or_else(|| {
            OpenClawAdapterError::Invalid("OpenClaw openclaw.json root must be an object".into())
        })
}

fn object_entry<'a>(
    root: &'a mut Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<&'a mut Map<String, Value>, OpenClawAdapterError> {
    root.entry(key)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| OpenClawAdapterError::Invalid(format!("{label} must be an object")))
}

fn validate_loopback_url(value: &str) -> Result<(), OpenClawAdapterError> {
    let url = Url::parse(value).map_err(|_| {
        OpenClawAdapterError::Invalid(format!("invalid OpenClaw gateway URL: {value}"))
    })?;
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
        return Err(OpenClawAdapterError::Invalid(
            "OpenClaw gateway must be a plain HTTP loopback URL without credentials, query, or fragment"
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

fn parse_snapshot(bytes: &[u8]) -> Result<RecoverySnapshot, OpenClawAdapterError> {
    let snapshot: RecoverySnapshot = serde_json::from_slice(bytes).map_err(|error| {
        OpenClawAdapterError::Invalid(format!("OpenClaw recovery snapshot is invalid: {error}"))
    })?;
    if snapshot.version != 1 {
        return Err(OpenClawAdapterError::Invalid(format!(
            "unsupported OpenClaw recovery snapshot version: {}",
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
) -> Result<(), OpenClawAdapterError> {
    write_optional(first_path, Some(first))?;
    write_optional(second_path, Some(second))
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, OpenClawAdapterError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(OpenClawAdapterError::Io {
            operation: "read OpenClaw configuration",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn write_optional(path: &Path, bytes: Option<&[u8]>) -> Result<(), OpenClawAdapterError> {
    match bytes {
        Some(bytes) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|source| OpenClawAdapterError::Io {
                    operation: "create OpenClaw configuration directory",
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            crate::storage::atomic_replace(path, bytes).map_err(|source| OpenClawAdapterError::Io {
                operation: "write OpenClaw configuration",
                path: path.to_path_buf(),
                source,
            })
        }
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(OpenClawAdapterError::Io {
                operation: "remove OpenClaw configuration",
                path: path.to_path_buf(),
                source,
            }),
        },
    }
}

fn combine_rollback(
    primary: OpenClawAdapterError,
    rollback: Result<(), OpenClawAdapterError>,
) -> OpenClawAdapterError {
    match rollback {
        Ok(()) => primary,
        Err(rollback) => {
            OpenClawAdapterError::Invalid(format!("{primary}; rollback also failed: {rollback}"))
        }
    }
}

#[derive(Debug)]
pub enum OpenClawAdapterError {
    HomeDirectoryMissing,
    Invalid(String),
    Drifted,
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl Display for OpenClawAdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HomeDirectoryMissing => formatter.write_str("could not locate the home directory"),
            Self::Invalid(message) => formatter.write_str(message),
            Self::Drifted => formatter.write_str(
                "OpenClaw configuration differs from the last GrillForge apply; restore or resolve the drift before continuing",
            ),
            Self::Io {
                operation,
                path,
                source,
            } => write!(formatter, "{operation} failed for {}: {source}", path.display()),
        }
    }
}

impl Error for OpenClawAdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
