use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
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
const SNAPSHOT_FILE: &str = "hermes.snapshot.json";
const CLI_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HermesPaths {
    pub config_path: PathBuf,
}

impl HermesPaths {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
        }
    }
}

pub fn paths_from_home(home: impl AsRef<Path>) -> HermesPaths {
    HermesPaths::new(home.as_ref().join(".hermes/config.yaml"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HermesCliDetection {
    pub path: PathBuf,
    pub version: String,
}

pub fn detect_hermes_cli() -> Result<Option<HermesCliDetection>, HermesAdapterError> {
    let executable = if cfg!(windows) {
        "hermes.exe"
    } else {
        "hermes"
    };
    let mut candidates = env::var_os("PATH")
        .map(|path| {
            env::split_paths(&path)
                .map(|directory| directory.join(executable))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        candidates.extend([
            home.join(".local/bin").join(executable),
            home.join(".hermes/hermes-agent/venv/bin").join(executable),
        ]);
    }
    #[cfg(target_os = "macos")]
    candidates.push(PathBuf::from("/usr/local/bin").join(executable));
    crate::cli_discovery::first_valid_candidate_across_sources(
        candidates,
        || {
            crate::cli_discovery::login_shell_candidates(executable).map_err(|error| {
                HermesAdapterError::Invalid(format!(
                    "discover Hermes CLI through the login shell: {error}"
                ))
            })
        },
        |path| inspect_hermes_cli(path),
    )
}

pub fn detect_hermes_cli_in(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<Option<HermesCliDetection>, HermesAdapterError> {
    crate::cli_discovery::first_valid_candidate(candidates, |path| inspect_hermes_cli(path))
}

pub fn inspect_hermes_cli(
    path: impl AsRef<Path>,
) -> Result<HermesCliDetection, HermesAdapterError> {
    let path = path.as_ref().to_path_buf();
    let mut command =
        crate::cli_discovery::version_command(&path).map_err(|source| HermesAdapterError::Io {
            operation: "prepare Hermes CLI inspection",
            path: path.clone(),
            source,
        })?;
    let mut child = command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| HermesAdapterError::Io {
            operation: "inspect Hermes CLI",
            path: path.clone(),
            source,
        })?;
    let deadline = Instant::now() + CLI_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().map_err(|source| HermesAdapterError::Io {
            operation: "inspect Hermes CLI",
            path: path.clone(),
            source,
        })? {
            let output = child
                .wait_with_output()
                .map_err(|source| HermesAdapterError::Io {
                    operation: "inspect Hermes CLI",
                    path: path.clone(),
                    source,
                })?;
            if !status.success() {
                return Err(HermesAdapterError::Invalid(format!(
                    "Hermes CLI did not return a version: {}",
                    path.display()
                )));
            }
            let version = String::from_utf8(output.stdout)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    HermesAdapterError::Invalid(format!(
                        "Hermes CLI did not return a version: {}",
                        path.display()
                    ))
                })?;
            return Ok(HermesCliDetection { path, version });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(HermesAdapterError::Invalid(format!(
                "Hermes CLI version check timed out: {}",
                path.display()
            )));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesModel {
    id: String,
    name: String,
}

impl HermesModel {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Result<Self, HermesAdapterError> {
        let id = id.into();
        if !id.starts_with("grillforge/")
            || id.len() == "grillforge/".len()
            || id.trim() != id
            || id.chars().any(char::is_control)
        {
            return Err(HermesAdapterError::Invalid(format!(
                "Hermes model must use a GrillForge route alias: {id}"
            )));
        }
        let name = name.into();
        if name.trim().is_empty() || name.trim() != name || name.chars().any(char::is_control) {
            return Err(HermesAdapterError::Invalid(format!(
                "Hermes model name is invalid: {id}"
            )));
        }
        Ok(Self { id, name })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermesRequest {
    gateway_base_url: String,
    gateway_token: String,
    models: Vec<HermesModel>,
    default_model: String,
}

impl HermesRequest {
    pub fn new(
        gateway_base_url: impl Into<String>,
        gateway_token: impl Into<String>,
        mut models: Vec<HermesModel>,
        default_model: impl Into<String>,
    ) -> Result<Self, HermesAdapterError> {
        let gateway_base_url = gateway_base_url.into();
        validate_gateway_url(&gateway_base_url)?;
        let gateway_token = gateway_token.into();
        if gateway_token.trim().is_empty()
            || gateway_token.trim() != gateway_token
            || gateway_token.chars().any(char::is_control)
        {
            return Err(HermesAdapterError::Invalid(
                "Hermes gateway token must not be empty, padded, or contain control characters"
                    .into(),
            ));
        }
        if models.is_empty() {
            return Err(HermesAdapterError::Invalid(
                "Hermes requires at least one managed model".into(),
            ));
        }
        models.sort_by(|left, right| left.id.cmp(&right.id));
        if models.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return Err(HermesAdapterError::Invalid(
                "Hermes managed model ids must be unique".into(),
            ));
        }
        let default_model = default_model.into();
        if !models.iter().any(|model| model.id == default_model) {
            return Err(HermesAdapterError::Invalid(
                "Hermes default model must be present in managed models".into(),
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

impl Debug for HermesRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HermesRequest")
            .field("gateway_base_url", &self.gateway_base_url)
            .field("gateway_token", &"[REDACTED]")
            .field("models", &self.models)
            .field("default_model", &self.default_model)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HermesTakeoverStatus {
    Inactive,
    Active,
    Drifted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HermesStatus {
    pub snapshot_present: bool,
    pub takeover: HermesTakeoverStatus,
}

#[derive(Debug)]
pub struct HermesAdapter {
    paths: HermesPaths,
    snapshot_path: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
struct RecoverySnapshot {
    version: u8,
    original: Option<Vec<u8>>,
    expected: Vec<u8>,
}

impl HermesAdapter {
    pub fn new(paths: HermesPaths, grillforge_root: impl Into<PathBuf>) -> Self {
        Self {
            paths,
            snapshot_path: grillforge_root.into().join(SNAPSHOT_FILE),
        }
    }

    pub fn apply(&self, request: HermesRequest) -> Result<HermesStatus, HermesAdapterError> {
        let current = read_optional(&self.paths.config_path)?;
        let previous_snapshot = read_optional(&self.snapshot_path)?;
        let original = match &previous_snapshot {
            Some(bytes) => {
                let snapshot = parse_snapshot(bytes)?;
                if current.as_deref() != Some(snapshot.expected.as_slice()) {
                    return Err(HermesAdapterError::Drifted);
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
            HermesAdapterError::Invalid(format!("could not encode Hermes snapshot: {error}"))
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
            return Err(HermesAdapterError::Invalid(
                "Hermes apply verification failed".into(),
            ));
        }
        self.status()
    }

    pub fn disable(&self) -> Result<HermesStatus, HermesAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(HermesStatus {
                snapshot_present: false,
                takeover: HermesTakeoverStatus::Inactive,
            });
        };
        let snapshot = parse_snapshot(&bytes)?;
        if read_optional(&self.paths.config_path)?.as_deref() != Some(snapshot.expected.as_slice())
        {
            return Err(HermesAdapterError::Drifted);
        }
        write_optional(&self.paths.config_path, snapshot.original.as_deref())?;
        if read_optional(&self.paths.config_path)? != snapshot.original {
            return Err(HermesAdapterError::Invalid(
                "Hermes restore verification failed; recovery snapshot was retained".into(),
            ));
        }
        fs::remove_file(&self.snapshot_path).map_err(|source| HermesAdapterError::Io {
            operation: "remove Hermes recovery snapshot",
            path: self.snapshot_path.clone(),
            source,
        })?;
        self.status()
    }

    pub fn status(&self) -> Result<HermesStatus, HermesAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(HermesStatus {
                snapshot_present: false,
                takeover: HermesTakeoverStatus::Inactive,
            });
        };
        let snapshot = parse_snapshot(&bytes)?;
        Ok(HermesStatus {
            snapshot_present: true,
            takeover: if read_optional(&self.paths.config_path)?.as_deref()
                == Some(snapshot.expected.as_slice())
            {
                HermesTakeoverStatus::Active
            } else {
                HermesTakeoverStatus::Drifted
            },
        })
    }
}

fn project(
    original: Option<&[u8]>,
    request: &HermesRequest,
) -> Result<Vec<u8>, HermesAdapterError> {
    let raw = match original {
        Some(bytes) => std::str::from_utf8(bytes)
            .map_err(|_| HermesAdapterError::Invalid("Hermes config.yaml must be UTF-8".into()))?,
        None => "",
    };
    let config: Value = if raw.trim().is_empty() {
        Value::Mapping(Mapping::new())
    } else {
        serde_yaml::from_str(raw).map_err(|error| {
            HermesAdapterError::Invalid(format!("Hermes config.yaml is invalid YAML: {error}"))
        })?
    };
    let root = config.as_mapping().ok_or_else(|| {
        HermesAdapterError::Invalid("Hermes configuration root must be a YAML mapping".into())
    })?;

    let custom_key = Value::String("custom_providers".into());
    let mut providers = match root.get(&custom_key) {
        Some(value) => value.as_sequence().cloned().ok_or_else(|| {
            HermesAdapterError::Invalid("Hermes custom_providers must be a YAML sequence".into())
        })?,
        None => Vec::new(),
    };
    if providers
        .iter()
        .any(|provider| provider.get("name").is_none())
    {
        return Err(HermesAdapterError::Invalid(
            "every Hermes custom_providers entry must have a name".into(),
        ));
    }
    let models = request
        .models
        .iter()
        .map(|model| {
            (
                Value::String(model.id.clone()),
                Value::Mapping(Mapping::new()),
            )
        })
        .collect::<Mapping>();
    let managed_fields = yaml_mapping([
        ("name", Value::String(PROVIDER_ID.into())),
        ("base_url", Value::String(request.gateway_base_url.clone())),
        ("api_key", Value::String(request.gateway_token.clone())),
        ("api_mode", Value::String("anthropic_messages".into())),
        ("model", Value::String(request.default_model.clone())),
        ("models", Value::Mapping(models)),
    ]);
    if let Some(existing) = providers
        .iter_mut()
        .find(|provider| provider.get("name").and_then(Value::as_str) == Some(PROVIDER_ID))
    {
        let existing = existing.as_mapping_mut().ok_or_else(|| {
            HermesAdapterError::Invalid(
                "the managed Hermes custom provider must be a YAML mapping".into(),
            )
        })?;
        existing.extend(managed_fields);
    } else {
        providers.push(Value::Mapping(managed_fields));
    }

    let model_key = Value::String("model".into());
    let mut model = match root.get(&model_key) {
        Some(value) => value.as_mapping().cloned().ok_or_else(|| {
            HermesAdapterError::Invalid("Hermes model must be a YAML mapping".into())
        })?,
        None => Mapping::new(),
    };
    model.insert(
        Value::String("default".into()),
        Value::String(request.default_model.clone()),
    );
    model.insert(
        Value::String("provider".into()),
        Value::String(PROVIDER_ID.into()),
    );

    let with_providers = replace_section(raw, "custom_providers", &Value::Sequence(providers))?;
    let projected = replace_section(&with_providers, "model", &Value::Mapping(model))?;
    Ok(projected.into_bytes())
}

fn yaml_mapping<const N: usize>(entries: [(&str, Value); N]) -> Mapping {
    entries
        .into_iter()
        .map(|(key, value)| (Value::String(key.into()), value))
        .collect()
}

fn replace_section(raw: &str, key: &str, value: &Value) -> Result<String, HermesAdapterError> {
    let serialized = serde_yaml::to_string(&Value::Mapping(yaml_mapping([(key, value.clone())])))
        .map_err(|error| {
        HermesAdapterError::Invalid(format!("could not encode Hermes {key}: {error}"))
    })?;
    if let Some((start, end)) = find_section(raw, key) {
        let mut output = String::with_capacity(raw.len() + serialized.len());
        output.push_str(&raw[..start]);
        output.push_str(&serialized);
        output.push_str(&raw[end..]);
        Ok(output)
    } else {
        let mut output = raw.to_string();
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&serialized);
        Ok(output)
    }
}

fn find_section(raw: &str, key: &str) -> Option<(usize, usize)> {
    let target = format!("{key}:");
    let mut start = None;
    let mut offset = 0;
    for line in raw.split_inclusive('\n') {
        let clean = line.trim_end_matches(['\r', '\n']);
        let top_level = !clean.is_empty()
            && !clean.starts_with([' ', '\t', '#', '-'])
            && clean.find(':').is_some_and(|colon| {
                clean[colon + 1..].is_empty() || clean[colon + 1..].starts_with([' ', '\t'])
            });
        if start.is_none() && top_level && clean.starts_with(&target) {
            let rest = &clean[target.len()..];
            if rest.is_empty() || rest.starts_with([' ', '\t']) {
                start = Some(offset);
            }
        } else if start.is_some() && top_level {
            return Some((start.expect("set above"), offset));
        }
        offset += line.len();
    }
    start.map(|start| (start, raw.len()))
}

fn validate_gateway_url(value: &str) -> Result<(), HermesAdapterError> {
    let url = Url::parse(value)
        .map_err(|_| HermesAdapterError::Invalid(format!("invalid Hermes gateway URL: {value}")))?;
    let loopback = url
        .host_str()
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .is_some_and(|host| host.is_loopback());
    if url.scheme() != "http"
        || !loopback
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(HermesAdapterError::Invalid(format!(
            "Hermes gateway URL must be an HTTP loopback URL without credentials, query, or fragment: {value}"
        )));
    }
    Ok(())
}

fn parse_snapshot(bytes: &[u8]) -> Result<RecoverySnapshot, HermesAdapterError> {
    let snapshot: RecoverySnapshot = serde_json::from_slice(bytes).map_err(|error| {
        HermesAdapterError::Invalid(format!("Hermes recovery snapshot is invalid: {error}"))
    })?;
    if snapshot.version != 1 {
        return Err(HermesAdapterError::Invalid(format!(
            "unsupported Hermes recovery snapshot version: {}",
            snapshot.version
        )));
    }
    Ok(snapshot)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, HermesAdapterError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(HermesAdapterError::Io {
            operation: "read Hermes configuration",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn write_optional(path: &Path, bytes: Option<&[u8]>) -> Result<(), HermesAdapterError> {
    match bytes {
        Some(bytes) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|source| HermesAdapterError::Io {
                    operation: "create Hermes configuration directory",
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            crate::storage::atomic_replace(path, bytes).map_err(|source| HermesAdapterError::Io {
                operation: "write Hermes configuration",
                path: path.to_path_buf(),
                source,
            })
        }
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(HermesAdapterError::Io {
                operation: "remove Hermes configuration",
                path: path.to_path_buf(),
                source,
            }),
        },
    }
}

fn write_pair(
    first_path: &Path,
    first: &[u8],
    second_path: &Path,
    second: &[u8],
) -> Result<(), HermesAdapterError> {
    write_optional(first_path, Some(first))?;
    write_optional(second_path, Some(second))
}

fn combine_rollback(
    primary: HermesAdapterError,
    rollback: Result<(), HermesAdapterError>,
) -> HermesAdapterError {
    match rollback {
        Ok(()) => primary,
        Err(rollback) => {
            HermesAdapterError::Invalid(format!("{primary}; rollback also failed: {rollback}"))
        }
    }
}

#[derive(Debug)]
pub enum HermesAdapterError {
    Invalid(String),
    Drifted,
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl Display for HermesAdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Drifted => formatter.write_str("Hermes configuration differs from the last GrillForge apply; resolve the drift before continuing"),
            Self::Io { operation, path, source } => write!(formatter, "{operation} failed for {}: {source}", path.display()),
        }
    }
}

impl Error for HermesAdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
