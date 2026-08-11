use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::fs;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use url::Url;

pub const PROFILE_ID: &str = "7c9949e4-173b-4c52-991d-c8cfb24b22f6";
pub const PROFILE_NAME: &str = "GrillForge";
const CONFIG_FILE: &str = "claude_desktop_config.json";
const SNAPSHOT_FILE: &str = "claude-desktop.snapshot.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeDesktopPaths {
    pub normal_config_path: PathBuf,
    pub threep_config_path: PathBuf,
    pub config_library_path: PathBuf,
    pub profile_path: PathBuf,
    pub meta_path: PathBuf,
}

pub fn macos_paths_from_home(home: impl AsRef<Path>) -> ClaudeDesktopPaths {
    let support = home.as_ref().join("Library/Application Support");
    paths_from_dirs(support.join("Claude"), support.join("Claude-3p"))
}

pub fn windows_paths_from_local_app_data(local_app_data: impl AsRef<Path>) -> ClaudeDesktopPaths {
    paths_from_dirs(
        local_app_data.as_ref().join("Claude"),
        local_app_data.as_ref().join("Claude-3p"),
    )
}

#[allow(clippy::needless_return)]
pub fn current_claude_desktop_paths() -> Result<ClaudeDesktopPaths, ClaudeDesktopAdapterError> {
    #[cfg(target_os = "macos")]
    {
        return dirs::home_dir()
            .map(macos_paths_from_home)
            .ok_or(ClaudeDesktopAdapterError::HomeDirectoryMissing);
    }
    #[cfg(windows)]
    {
        let local_app_data = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join("AppData/Local")))
            .ok_or(ClaudeDesktopAdapterError::HomeDirectoryMissing)?;
        return Ok(windows_paths_from_local_app_data(local_app_data));
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Err(ClaudeDesktopAdapterError::UnsupportedPlatform)
    }
}

fn paths_from_dirs(normal_dir: PathBuf, threep_dir: PathBuf) -> ClaudeDesktopPaths {
    let config_library_path = threep_dir.join("configLibrary");
    ClaudeDesktopPaths {
        normal_config_path: normal_dir.join(CONFIG_FILE),
        threep_config_path: threep_dir.join(CONFIG_FILE),
        profile_path: config_library_path.join(format!("{PROFILE_ID}.json")),
        meta_path: config_library_path.join("_meta.json"),
        config_library_path,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeDesktopDetection {
    pub executable_path: PathBuf,
}

pub fn detect_macos_claude_client_in(
    application_dirs: &[PathBuf],
) -> Option<ClaudeDesktopDetection> {
    application_dirs.iter().find_map(|directory| {
        let executable = directory.join("Claude.app/Contents/MacOS/Claude");
        executable.is_file().then_some(ClaudeDesktopDetection {
            executable_path: executable,
        })
    })
}

pub fn detect_windows_claude_client_in(
    installation_roots: &[PathBuf],
) -> Option<ClaudeDesktopDetection> {
    installation_roots.iter().find_map(|root| {
        [
            root.join("Programs/Claude/Claude.exe"),
            root.join("Claude/Claude.exe"),
            root.join("Claude.exe"),
        ]
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map(|executable_path| ClaudeDesktopDetection { executable_path })
    })
}

#[allow(clippy::needless_return)]
pub fn detect_claude_client() -> Option<ClaudeDesktopDetection> {
    #[cfg(target_os = "macos")]
    {
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join("Applications"));
        }
        return detect_macos_claude_client_in(&roots);
    }
    #[cfg(windows)]
    {
        let roots = ["LOCALAPPDATA", "PROGRAMFILES"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        return detect_windows_claude_client_in(&roots);
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        None
    }
}

pub fn is_claude_desktop_route_id(value: &str) -> bool {
    let Some(tail) = value
        .strip_prefix("anthropic/claude-")
        .or_else(|| value.strip_prefix("claude-"))
    else {
        return false;
    };
    !value.contains("[1m]")
        && ["sonnet-", "opus-", "haiku-", "fable-"].iter().any(|role| {
            tail.strip_prefix(role).is_some_and(|rest| {
                !rest.is_empty()
                    && !rest.starts_with('-')
                    && !rest.ends_with('-')
                    && !rest.contains("--")
                    && rest.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'-' | b'_' | b'.')
                    })
            })
        })
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeDesktopRouteSpec {
    model: String,
    label: Option<String>,
    supports_1m: bool,
}

impl ClaudeDesktopRouteSpec {
    pub fn new(
        model: impl Into<String>,
        label: Option<impl Into<String>>,
        supports_1m: bool,
    ) -> Self {
        Self {
            model: model.into(),
            label: label.map(Into::into),
            supports_1m,
        }
    }
}

impl Debug for ClaudeDesktopRouteSpec {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClaudeDesktopRouteSpec")
            .field("model", &self.model)
            .field("label", &self.label)
            .field("supports_1m", &self.supports_1m)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeDesktopRequest {
    gateway_base_url: String,
    bearer_token: String,
    routes: Vec<ClaudeDesktopRouteSpec>,
}

impl ClaudeDesktopRequest {
    pub fn new(
        gateway_base_url: impl Into<String>,
        bearer_token: impl Into<String>,
        routes: Vec<ClaudeDesktopRouteSpec>,
    ) -> Self {
        Self {
            gateway_base_url: gateway_base_url.into(),
            bearer_token: bearer_token.into(),
            routes,
        }
    }
}

impl Debug for ClaudeDesktopRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClaudeDesktopRequest")
            .field("gateway_base_url", &self.gateway_base_url)
            .field("bearer_token", &"[REDACTED]")
            .field("routes", &self.routes)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeDesktopTakeoverStatus {
    Inactive,
    Active,
    Drifted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeDesktopStatus {
    pub snapshot_present: bool,
    pub takeover: ClaudeDesktopTakeoverStatus,
    pub differences: Vec<String>,
}

#[derive(Debug)]
pub struct ClaudeDesktopAdapter {
    paths: ClaudeDesktopPaths,
    snapshot_path: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
struct RecoverySnapshot {
    version: u8,
    original: ManagedFiles,
    expected: ManagedFiles,
    active: ClaudeDesktopRequest,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedFiles {
    normal: Option<Vec<u8>>,
    threep: Option<Vec<u8>>,
    profile: Option<Vec<u8>>,
    meta: Option<Vec<u8>>,
}

impl ClaudeDesktopAdapter {
    pub fn new(paths: ClaudeDesktopPaths, grillforge_root: impl Into<PathBuf>) -> Self {
        Self {
            paths,
            snapshot_path: grillforge_root.into().join(SNAPSHOT_FILE),
        }
    }

    pub fn snapshot_path(&self) -> &Path {
        &self.snapshot_path
    }

    pub fn apply(&self, request: ClaudeDesktopRequest) -> Result<(), ClaudeDesktopAdapterError> {
        validate_request(&request)?;
        let before = self.capture_files()?;
        let previous_snapshot = read_optional(&self.snapshot_path)?;
        let existing_snapshot = self.read_snapshot()?;
        let original = existing_snapshot
            .as_ref()
            .map(|snapshot| snapshot.original.clone())
            .unwrap_or_else(|| before.clone());
        let expected = self.build_expected(&before, &request)?;
        let snapshot = RecoverySnapshot {
            version: 1,
            original,
            expected: expected.clone(),
            active: request,
        };
        self.write_snapshot(&snapshot)?;
        if let Err(apply) = self
            .write_files(&expected)
            .and_then(|_| self.verify_files(&expected))
        {
            let rollback = self
                .write_files(&before)
                .and_then(|_| restore_optional(&self.snapshot_path, previous_snapshot.as_deref()));
            return Err(combine_rollback(apply, rollback));
        }
        Ok(())
    }

    pub fn disable(&self) -> Result<(), ClaudeDesktopAdapterError> {
        let snapshot = self.read_snapshot()?.ok_or_else(|| {
            ClaudeDesktopAdapterError::SnapshotMissing(self.snapshot_path.clone())
        })?;
        let before = self.capture_files()?;
        if let Err(apply) = self
            .write_files(&snapshot.original)
            .and_then(|_| self.verify_files(&snapshot.original))
        {
            return Err(combine_rollback(apply, self.write_files(&before)));
        }
        if let Err(source) = fs::remove_file(&self.snapshot_path) {
            let apply = ClaudeDesktopAdapterError::WriteConfiguration {
                path: self.snapshot_path.clone(),
                source,
            };
            return Err(combine_rollback(apply, self.write_files(&before)));
        }
        Ok(())
    }

    pub fn status(&self) -> Result<ClaudeDesktopStatus, ClaudeDesktopAdapterError> {
        let current = self.capture_files()?;
        let snapshot = self.read_snapshot()?;
        let mut differences = Vec::new();
        let takeover = match &snapshot {
            Some(snapshot) if current == snapshot.expected => ClaudeDesktopTakeoverStatus::Active,
            Some(snapshot) => {
                for (label, actual, expected) in [
                    (
                        "Claude/claude_desktop_config.json",
                        &current.normal,
                        &snapshot.expected.normal,
                    ),
                    (
                        "Claude-3p/claude_desktop_config.json",
                        &current.threep,
                        &snapshot.expected.threep,
                    ),
                    (
                        "Claude-3p/configLibrary/GrillForge.json",
                        &current.profile,
                        &snapshot.expected.profile,
                    ),
                    (
                        "Claude-3p/configLibrary/_meta.json",
                        &current.meta,
                        &snapshot.expected.meta,
                    ),
                ] {
                    if actual != expected {
                        differences.push(label.to_string());
                    }
                }
                ClaudeDesktopTakeoverStatus::Drifted
            }
            None if self.has_managed_artifacts(&current)? => ClaudeDesktopTakeoverStatus::Drifted,
            None => ClaudeDesktopTakeoverStatus::Inactive,
        };
        Ok(ClaudeDesktopStatus {
            snapshot_present: snapshot.is_some(),
            takeover,
            differences,
        })
    }

    fn build_expected(
        &self,
        current: &ManagedFiles,
        request: &ClaudeDesktopRequest,
    ) -> Result<ManagedFiles, ClaudeDesktopAdapterError> {
        if let Some(profile) = current.profile.as_deref() {
            parse_object(profile, &self.paths.profile_path)?;
        }
        let normal = Some(with_deployment_mode(
            current.normal.as_deref(),
            &self.paths.normal_config_path,
            "3p",
        )?);
        let threep = Some(with_deployment_mode(
            current.threep.as_deref(),
            &self.paths.threep_config_path,
            "3p",
        )?);
        let profile = Some(serialize_json(
            &self.paths.profile_path,
            &gateway_profile(request),
        )?);
        let meta = Some(with_profile_meta(
            current.meta.as_deref(),
            &self.paths.meta_path,
        )?);
        Ok(ManagedFiles {
            normal,
            threep,
            profile,
            meta,
        })
    }

    fn capture_files(&self) -> Result<ManagedFiles, ClaudeDesktopAdapterError> {
        Ok(ManagedFiles {
            normal: read_optional(&self.paths.normal_config_path)?,
            threep: read_optional(&self.paths.threep_config_path)?,
            profile: read_optional(&self.paths.profile_path)?,
            meta: read_optional(&self.paths.meta_path)?,
        })
    }

    fn write_files(&self, files: &ManagedFiles) -> Result<(), ClaudeDesktopAdapterError> {
        for (path, contents) in [
            (&self.paths.normal_config_path, files.normal.as_deref()),
            (&self.paths.threep_config_path, files.threep.as_deref()),
            (&self.paths.profile_path, files.profile.as_deref()),
            (&self.paths.meta_path, files.meta.as_deref()),
        ] {
            if read_optional(path)?.as_deref() != contents {
                restore_optional(path, contents)?;
            }
        }
        Ok(())
    }

    fn verify_files(&self, expected: &ManagedFiles) -> Result<(), ClaudeDesktopAdapterError> {
        if self.capture_files()? == *expected {
            Ok(())
        } else {
            Err(ClaudeDesktopAdapterError::VerificationFailed)
        }
    }

    fn has_managed_artifacts(
        &self,
        files: &ManagedFiles,
    ) -> Result<bool, ClaudeDesktopAdapterError> {
        if files.profile.is_some() {
            return Ok(true);
        }
        let Some(meta) = files.meta.as_deref() else {
            return Ok(false);
        };
        let value = parse_object(meta, &self.paths.meta_path)?;
        Ok(value
            .get("entries")
            .and_then(Value::as_array)
            .is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.get("id").and_then(Value::as_str) == Some(PROFILE_ID))
            }))
    }

    fn read_snapshot(&self) -> Result<Option<RecoverySnapshot>, ClaudeDesktopAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(None);
        };
        let snapshot: RecoverySnapshot = serde_json::from_slice(&bytes)
            .map_err(|_| ClaudeDesktopAdapterError::InvalidSnapshot(self.snapshot_path.clone()))?;
        if snapshot.version != 1 || validate_request(&snapshot.active).is_err() {
            return Err(ClaudeDesktopAdapterError::InvalidSnapshot(
                self.snapshot_path.clone(),
            ));
        }
        Ok(Some(snapshot))
    }

    fn write_snapshot(&self, snapshot: &RecoverySnapshot) -> Result<(), ClaudeDesktopAdapterError> {
        let bytes = serde_json::to_vec_pretty(snapshot).map_err(|source| {
            ClaudeDesktopAdapterError::SerializeConfiguration {
                path: self.snapshot_path.clone(),
                source,
            }
        })?;
        write_atomic(&self.snapshot_path, &bytes)
    }
}

#[derive(Debug)]
pub enum ClaudeDesktopAdapterError {
    UnsupportedPlatform,
    HomeDirectoryMissing,
    InvalidGateway(String),
    MissingBearerToken,
    MissingRoutes,
    InvalidRouteId(String),
    InvalidRouteLabel(String),
    DuplicateRouteId(String),
    InvalidConfiguration(PathBuf),
    InvalidSnapshot(PathBuf),
    SnapshotMissing(PathBuf),
    VerificationFailed,
    ApplyRollbackFailed {
        apply: Box<ClaudeDesktopAdapterError>,
        rollback: Box<ClaudeDesktopAdapterError>,
    },
    ReadConfiguration {
        path: PathBuf,
        source: io::Error,
    },
    WriteConfiguration {
        path: PathBuf,
        source: io::Error,
    },
    SerializeConfiguration {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl Display for ClaudeDesktopAdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform => formatter
                .write_str("Claude Client 3P configuration is supported only on macOS and Windows"),
            Self::HomeDirectoryMissing => {
                formatter.write_str("could not resolve the Claude Client configuration directory")
            }
            Self::InvalidGateway(url) => write!(
                formatter,
                "Claude Client gateway must be an HTTP loopback URL: {url}"
            ),
            Self::MissingBearerToken => {
                formatter.write_str("Claude Client gateway bearer token is required")
            }
            Self::MissingRoutes => {
                formatter.write_str("Claude Client requires at least one model route")
            }
            Self::InvalidRouteId(id) => {
                write!(formatter, "unsafe Claude Client model route id: {id}")
            }
            Self::InvalidRouteLabel(id) => {
                write!(
                    formatter,
                    "Claude Client model route label is invalid: {id}"
                )
            }
            Self::DuplicateRouteId(id) => {
                write!(formatter, "duplicate Claude Client model route id: {id}")
            }
            Self::InvalidConfiguration(path) => write!(
                formatter,
                "Claude Client configuration must be a valid JSON object: {}",
                path.display()
            ),
            Self::InvalidSnapshot(path) => write!(
                formatter,
                "GrillForge Claude Client snapshot is invalid: {}",
                path.display()
            ),
            Self::SnapshotMissing(path) => write!(
                formatter,
                "GrillForge Claude Client snapshot does not exist: {}",
                path.display()
            ),
            Self::VerificationFailed => {
                formatter.write_str("Claude Client configuration verification failed")
            }
            Self::ApplyRollbackFailed { apply, rollback } => {
                write!(formatter, "{apply}; rollback failed: {rollback}")
            }
            Self::ReadConfiguration { path, source } => write!(
                formatter,
                "failed to read Claude Client configuration {}: {source}",
                path.display()
            ),
            Self::WriteConfiguration { path, source } => write!(
                formatter,
                "failed to write Claude Client configuration {}: {source}",
                path.display()
            ),
            Self::SerializeConfiguration { path, source } => write!(
                formatter,
                "failed to serialize Claude Client configuration {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for ClaudeDesktopAdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadConfiguration { source, .. } | Self::WriteConfiguration { source, .. } => {
                Some(source)
            }
            Self::SerializeConfiguration { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn validate_request(request: &ClaudeDesktopRequest) -> Result<(), ClaudeDesktopAdapterError> {
    let url = Url::parse(&request.gateway_base_url)
        .map_err(|_| ClaudeDesktopAdapterError::InvalidGateway(request.gateway_base_url.clone()))?;
    let loopback = match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback()),
        None => false,
    };
    if !matches!(url.scheme(), "http" | "https") || !loopback {
        return Err(ClaudeDesktopAdapterError::InvalidGateway(
            request.gateway_base_url.clone(),
        ));
    }
    if request.bearer_token.trim().is_empty()
        || request
            .bearer_token
            .chars()
            .any(|character| character.is_control())
    {
        return Err(ClaudeDesktopAdapterError::MissingBearerToken);
    }
    if request.routes.is_empty() {
        return Err(ClaudeDesktopAdapterError::MissingRoutes);
    }
    let mut route_ids = HashSet::new();
    for route in &request.routes {
        if !is_claude_desktop_route_id(&route.model) {
            return Err(ClaudeDesktopAdapterError::InvalidRouteId(
                route.model.clone(),
            ));
        }
        if !route_ids.insert(route.model.as_str()) {
            return Err(ClaudeDesktopAdapterError::DuplicateRouteId(
                route.model.clone(),
            ));
        }
        if route
            .label
            .as_deref()
            .is_some_and(|label| label.trim().is_empty() || label.chars().any(char::is_control))
        {
            return Err(ClaudeDesktopAdapterError::InvalidRouteLabel(
                route.model.clone(),
            ));
        }
    }
    Ok(())
}

fn gateway_profile(request: &ClaudeDesktopRequest) -> Value {
    let models = request
        .routes
        .iter()
        .map(|route| {
            if route.label.is_none() && !route.supports_1m {
                return Value::String(route.model.clone());
            }
            let mut model = Map::new();
            model.insert("name".to_string(), Value::String(route.model.clone()));
            if let Some(label) = route.label.as_deref() {
                model.insert(
                    "labelOverride".to_string(),
                    Value::String(label.trim().to_string()),
                );
            }
            if route.supports_1m {
                model.insert("supports1m".to_string(), Value::Bool(true));
            }
            Value::Object(model)
        })
        .collect::<Vec<_>>();
    json!({
        "coworkEgressAllowedHosts": ["*"],
        "disableDeploymentModeChooser": true,
        "inferenceGatewayApiKey": request.bearer_token,
        "inferenceGatewayAuthScheme": "bearer",
        "inferenceGatewayBaseUrl": request.gateway_base_url,
        "inferenceProvider": "gateway",
        "inferenceModels": models,
    })
}

fn with_deployment_mode(
    bytes: Option<&[u8]>,
    path: &Path,
    mode: &str,
) -> Result<Vec<u8>, ClaudeDesktopAdapterError> {
    let mut value = match bytes {
        Some(bytes) => parse_object(bytes, path)?,
        None => Map::new(),
    };
    value.insert(
        "deploymentMode".to_string(),
        Value::String(mode.to_string()),
    );
    serialize_json(path, &Value::Object(value))
}

fn with_profile_meta(
    bytes: Option<&[u8]>,
    path: &Path,
) -> Result<Vec<u8>, ClaudeDesktopAdapterError> {
    let mut value = match bytes {
        Some(bytes) => parse_object(bytes, path)?,
        None => Map::new(),
    };
    let mut entries = match value.remove("entries") {
        Some(Value::Array(entries)) => entries,
        Some(_) => {
            return Err(ClaudeDesktopAdapterError::InvalidConfiguration(
                path.to_path_buf(),
            ));
        }
        None => Vec::new(),
    };
    entries.retain(|entry| entry.get("id").and_then(Value::as_str) != Some(PROFILE_ID));
    entries.push(json!({"id": PROFILE_ID, "name": PROFILE_NAME}));
    value.insert("entries".to_string(), Value::Array(entries));
    value.insert(
        "appliedId".to_string(),
        Value::String(PROFILE_ID.to_string()),
    );
    serialize_json(path, &Value::Object(value))
}

fn parse_object(
    bytes: &[u8],
    path: &Path,
) -> Result<Map<String, Value>, ClaudeDesktopAdapterError> {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| ClaudeDesktopAdapterError::InvalidConfiguration(path.to_path_buf()))
}

fn serialize_json(path: &Path, value: &Value) -> Result<Vec<u8>, ClaudeDesktopAdapterError> {
    serde_json::to_vec_pretty(value).map_err(|source| {
        ClaudeDesktopAdapterError::SerializeConfiguration {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, ClaudeDesktopAdapterError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ClaudeDesktopAdapterError::ReadConfiguration {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn restore_optional(path: &Path, contents: Option<&[u8]>) -> Result<(), ClaudeDesktopAdapterError> {
    match contents {
        Some(contents) => write_atomic(path, contents),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(ClaudeDesktopAdapterError::WriteConfiguration {
                path: path.to_path_buf(),
                source,
            }),
        },
    }
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), ClaudeDesktopAdapterError> {
    let parent = path
        .parent()
        .ok_or_else(|| ClaudeDesktopAdapterError::WriteConfiguration {
            path: path.to_path_buf(),
            source: io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"),
        })?;
    fs::create_dir_all(parent).map_err(|source| ClaudeDesktopAdapterError::WriteConfiguration {
        path: path.to_path_buf(),
        source,
    })?;
    crate::storage::atomic_replace(path, contents).map_err(|source| {
        ClaudeDesktopAdapterError::WriteConfiguration {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn combine_rollback(
    apply: ClaudeDesktopAdapterError,
    rollback: Result<(), ClaudeDesktopAdapterError>,
) -> ClaudeDesktopAdapterError {
    match rollback {
        Ok(()) => apply,
        Err(rollback) => ClaudeDesktopAdapterError::ApplyRollbackFailed {
            apply: Box::new(apply),
            rollback: Box::new(rollback),
        },
    }
}
