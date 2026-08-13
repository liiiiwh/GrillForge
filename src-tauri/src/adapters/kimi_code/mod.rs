use serde::{Deserialize, Serialize};
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
use toml_edit::{Array, DocumentMut, Item, Table, value};
use url::Url;

const SNAPSHOT_FILE: &str = "kimi-code.snapshot.json";
const PROVIDER_ID: &str = "grillforge";
const CLI_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiCodePaths {
    pub config_path: PathBuf,
}

impl KimiCodePaths {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
        }
    }
}

pub fn paths_from_home(home: impl AsRef<Path>) -> KimiCodePaths {
    let home = home.as_ref();
    KimiCodePaths::new(home.join(".kimi-code/config.toml"))
}

pub fn mcp_path_from_home(home: impl AsRef<Path>) -> PathBuf {
    home.as_ref().join(".kimi-code/mcp.json")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiCodeCliDetection {
    pub path: PathBuf,
    pub version: String,
}

pub fn detect_kimi_code_cli() -> Result<Option<KimiCodeCliDetection>, KimiCodeAdapterError> {
    let executable = if cfg!(windows) { "kimi.exe" } else { "kimi" };
    let mut candidates = env::var_os("PATH")
        .map(|path| {
            env::split_paths(&path)
                .map(|directory| directory.join(executable))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        candidates.extend(crate::cli_discovery::node_cli_candidates_from_home(
            home, executable,
        ));
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
                KimiCodeAdapterError::Invalid(format!(
                    "discover Kimi Code CLI through the login shell: {error}"
                ))
            })
        },
        |path| inspect_kimi_code_cli(path),
    )
}

pub fn detect_kimi_code_cli_in(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<Option<KimiCodeCliDetection>, KimiCodeAdapterError> {
    crate::cli_discovery::first_valid_candidate(candidates, |path| inspect_kimi_code_cli(path))
}

pub fn inspect_kimi_code_cli(
    path: impl AsRef<Path>,
) -> Result<KimiCodeCliDetection, KimiCodeAdapterError> {
    let path = path.as_ref().to_path_buf();
    let mut command = crate::cli_discovery::version_command(&path).map_err(|source| {
        KimiCodeAdapterError::Io {
            operation: "prepare Kimi Code CLI inspection",
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
        .map_err(|source| KimiCodeAdapterError::Io {
            operation: "inspect Kimi Code CLI",
            path: path.clone(),
            source,
        })?;
    let deadline = Instant::now() + CLI_TIMEOUT;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|source| KimiCodeAdapterError::Io {
                operation: "inspect Kimi Code CLI",
                path: path.clone(),
                source,
            })?
        {
            let output = child
                .wait_with_output()
                .map_err(|source| KimiCodeAdapterError::Io {
                    operation: "inspect Kimi Code CLI",
                    path: path.clone(),
                    source,
                })?;
            if !status.success() {
                return Err(KimiCodeAdapterError::Invalid(format!(
                    "Kimi Code CLI did not return a version: {}",
                    path.display()
                )));
            }
            let version = String::from_utf8(output.stdout)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    KimiCodeAdapterError::Invalid(format!(
                        "Kimi Code CLI did not return a version: {}",
                        path.display()
                    ))
                })?;
            return Ok(KimiCodeCliDetection { path, version });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(KimiCodeAdapterError::Invalid(format!(
                "Kimi Code CLI version check timed out: {}",
                path.display()
            )));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KimiCodeAgentProfile {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KimiCodeModel {
    id: String,
    capabilities: Vec<String>,
}

impl KimiCodeModel {
    pub fn new(
        id: impl Into<String>,
        capabilities: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, KimiCodeAdapterError> {
        let id = id.into();
        if !valid_route(&id) {
            return Err(KimiCodeAdapterError::Invalid(format!(
                "Kimi Code model must use a GrillForge route alias: {id}"
            )));
        }
        let mut capabilities = capabilities.into_iter().map(Into::into).collect::<Vec<_>>();
        if capabilities.iter().any(|capability| {
            !matches!(
                capability.as_str(),
                "thinking" | "always_thinking" | "image_in" | "video_in" | "audio_in" | "tool_use"
            )
        }) {
            return Err(KimiCodeAdapterError::Invalid(format!(
                "Kimi Code model capabilities are invalid: {id}"
            )));
        }
        if !capabilities
            .iter()
            .any(|capability| capability == "tool_use")
        {
            capabilities.push("tool_use".into());
        }
        capabilities.sort();
        capabilities.dedup();
        Ok(Self { id, capabilities })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KimiCodeRequest {
    gateway_base_url: String,
    gateway_token: String,
    models: Vec<KimiCodeModel>,
    default_model: String,
}

impl KimiCodeRequest {
    pub fn new(
        gateway_base_url: impl Into<String>,
        gateway_token: impl Into<String>,
        mut models: Vec<KimiCodeModel>,
        default_model: impl Into<String>,
    ) -> Result<Self, KimiCodeAdapterError> {
        let gateway_base_url = gateway_base_url.into();
        validate_gateway_url(&gateway_base_url)?;
        let gateway_token = gateway_token.into();
        if gateway_token.trim().is_empty()
            || gateway_token.trim() != gateway_token
            || gateway_token.chars().any(char::is_control)
        {
            return Err(KimiCodeAdapterError::Invalid(
                "Kimi Code gateway token is invalid".into(),
            ));
        }
        if models.is_empty() {
            return Err(KimiCodeAdapterError::Invalid(
                "Kimi Code requires at least one managed model".into(),
            ));
        }
        models.sort_by(|left, right| left.id.cmp(&right.id));
        if models.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return Err(KimiCodeAdapterError::Invalid(
                "Kimi Code managed model ids must be unique".into(),
            ));
        }
        let default_model = default_model.into();
        if !models.iter().any(|model| model.id == default_model) {
            return Err(KimiCodeAdapterError::Invalid(
                "Kimi Code default model must be present in managed models".into(),
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

impl Debug for KimiCodeRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("KimiCodeRequest")
            .field("gateway_base_url", &self.gateway_base_url)
            .field("gateway_token", &"[REDACTED]")
            .field("models", &self.models)
            .field("default_model", &self.default_model)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KimiCodeTakeoverStatus {
    Inactive,
    Active,
    Drifted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiCodeStatus {
    pub snapshot_present: bool,
    pub takeover: KimiCodeTakeoverStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Snapshot {
    original: Option<Vec<u8>>,
    applied: Vec<u8>,
}

pub struct KimiCodeAdapter {
    paths: KimiCodePaths,
    snapshot_path: PathBuf,
}

impl KimiCodeAdapter {
    pub fn new(paths: KimiCodePaths, grillforge_root: impl Into<PathBuf>) -> Self {
        Self {
            paths,
            snapshot_path: grillforge_root.into().join(SNAPSHOT_FILE),
        }
    }

    pub fn snapshot_path(&self) -> &Path {
        &self.snapshot_path
    }

    pub fn apply(&self, request: KimiCodeRequest) -> Result<KimiCodeStatus, KimiCodeAdapterError> {
        let current = read_optional(&self.paths.config_path)?;
        let existing = self.read_snapshot()?;
        if let Some(snapshot) = &existing {
            if current.as_deref() != Some(snapshot.applied.as_slice()) {
                return Err(KimiCodeAdapterError::Drifted);
            }
        }
        let original = existing
            .map(|snapshot| snapshot.original)
            .unwrap_or(current);
        let applied = render_config(original.as_deref(), &request)?;
        let snapshot = Snapshot {
            original,
            applied: applied.clone(),
        };
        if let Some(parent) = self.paths.config_path.parent() {
            fs::create_dir_all(parent).map_err(|source| KimiCodeAdapterError::Io {
                operation: "create Kimi Code configuration directory",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        write_json(&self.snapshot_path, &snapshot)?;
        if let Err(error) = crate::storage::atomic_replace(&self.paths.config_path, &applied) {
            let _ = fs::remove_file(&self.snapshot_path);
            return Err(KimiCodeAdapterError::Io {
                operation: "apply Kimi Code configuration",
                path: self.paths.config_path.clone(),
                source: error,
            });
        }
        self.status()
    }

    pub fn disable(&self) -> Result<KimiCodeStatus, KimiCodeAdapterError> {
        let Some(snapshot) = self.read_snapshot()? else {
            return self.status();
        };
        if read_optional(&self.paths.config_path)?.as_deref() != Some(snapshot.applied.as_slice()) {
            return Err(KimiCodeAdapterError::Drifted);
        }
        match snapshot.original {
            Some(original) => crate::storage::atomic_replace(&self.paths.config_path, &original)
                .map_err(|source| KimiCodeAdapterError::Io {
                    operation: "restore Kimi Code configuration",
                    path: self.paths.config_path.clone(),
                    source,
                })?,
            None => match fs::remove_file(&self.paths.config_path) {
                Ok(()) => {}
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(KimiCodeAdapterError::Io {
                        operation: "remove Kimi Code configuration",
                        path: self.paths.config_path.clone(),
                        source,
                    });
                }
            },
        }
        fs::remove_file(&self.snapshot_path).map_err(|source| KimiCodeAdapterError::Io {
            operation: "remove Kimi Code snapshot",
            path: self.snapshot_path.clone(),
            source,
        })?;
        self.status()
    }

    pub fn status(&self) -> Result<KimiCodeStatus, KimiCodeAdapterError> {
        let Some(snapshot) = self.read_snapshot()? else {
            return Ok(KimiCodeStatus {
                snapshot_present: false,
                takeover: KimiCodeTakeoverStatus::Inactive,
            });
        };
        let current = read_optional(&self.paths.config_path)?;
        Ok(KimiCodeStatus {
            snapshot_present: true,
            takeover: if current.as_deref() == Some(snapshot.applied.as_slice()) {
                KimiCodeTakeoverStatus::Active
            } else {
                KimiCodeTakeoverStatus::Drifted
            },
        })
    }

    fn read_snapshot(&self) -> Result<Option<Snapshot>, KimiCodeAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| KimiCodeAdapterError::Invalid("Kimi Code snapshot is invalid".into()))
    }
}

fn render_config(
    original: Option<&[u8]>,
    request: &KimiCodeRequest,
) -> Result<Vec<u8>, KimiCodeAdapterError> {
    let text = match original {
        Some(bytes) => std::str::from_utf8(bytes).map_err(|_| {
            KimiCodeAdapterError::Invalid("Kimi Code config.toml is not UTF-8".into())
        })?,
        None => "",
    };
    let mut document = text.parse::<DocumentMut>().map_err(|error| {
        KimiCodeAdapterError::Invalid(format!("Kimi Code config.toml is invalid: {error}"))
    })?;
    if document
        .get("providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(PROVIDER_ID))
        .is_some()
    {
        return Err(KimiCodeAdapterError::Invalid(
            "Kimi Code already contains an unmanaged GrillForge provider".into(),
        ));
    }

    document["default_model"] = value(&request.default_model);
    document["experimental"]["secondary-model"] = value(true);
    document["providers"][PROVIDER_ID]["type"] = value("anthropic");
    document["providers"][PROVIDER_ID]["base_url"] = value(&request.gateway_base_url);
    document["providers"][PROVIDER_ID]["api_key"] = value(&request.gateway_token);
    for model in &request.models {
        document["models"][&model.id]["provider"] = value(PROVIDER_ID);
        document["models"][&model.id]["model"] = value(&model.id);
        document["models"][&model.id]["max_context_size"] = value(128_000_i64);
        let mut capabilities = Array::new();
        for capability in &model.capabilities {
            capabilities.push(capability.as_str());
        }
        document["models"][&model.id]["capabilities"] = value(capabilities);
    }
    let mut secondary = Table::new();
    secondary["default_model"] = value(&request.default_model);
    let mut pool = Table::new();
    for model in &request.models {
        pool[&model.id] = value("");
    }
    secondary["models"] = Item::Table(pool);
    document["secondary_model"] = Item::Table(secondary);
    Ok(document.to_string().into_bytes())
}

fn validate_gateway_url(value: &str) -> Result<(), KimiCodeAdapterError> {
    let url = Url::parse(value)
        .map_err(|_| KimiCodeAdapterError::Invalid("Kimi Code gateway URL is invalid".into()))?;
    let host = url
        .host_str()
        .ok_or_else(|| KimiCodeAdapterError::Invalid("Kimi Code gateway URL has no host".into()))?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if url.scheme() != "http" || !loopback || url.query().is_some() || url.fragment().is_some() {
        return Err(KimiCodeAdapterError::Invalid(
            "Kimi Code gateway URL must be a loopback HTTP URL".into(),
        ));
    }
    Ok(())
}

fn valid_route(value: &str) -> bool {
    value.starts_with("grillforge/")
        && value.len() > "grillforge/".len()
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, KimiCodeAdapterError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(KimiCodeAdapterError::Io {
            operation: "read Kimi Code file",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn write_json(path: &Path, snapshot: &Snapshot) -> Result<(), KimiCodeAdapterError> {
    let bytes = serde_json::to_vec(snapshot)
        .map_err(|_| KimiCodeAdapterError::Invalid("Kimi Code snapshot is invalid".into()))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| KimiCodeAdapterError::Io {
            operation: "create Kimi Code snapshot directory",
            path: parent.to_path_buf(),
            source,
        })?;
    }
    crate::storage::atomic_replace(path, &bytes).map_err(|source| KimiCodeAdapterError::Io {
        operation: "write Kimi Code snapshot",
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Debug)]
pub enum KimiCodeAdapterError {
    Invalid(String),
    Drifted,
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl Display for KimiCodeAdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Drifted => formatter.write_str("Kimi Code managed configuration was modified"),
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "{operation} failed for {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for KimiCodeAdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
