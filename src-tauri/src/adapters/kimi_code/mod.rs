use serde::{Deserialize, Serialize};
use serde_yaml::Value as YamlValue;
use std::collections::BTreeMap;
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
use toml_edit::{Array, DocumentMut, Item, value};
use url::Url;

const SNAPSHOT_FILE: &str = "kimi-code.snapshot.json";
const PROVIDER_ID: &str = "grillforge";
const CLI_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiCodePaths {
    pub config_path: PathBuf,
    pub user_agents_dir: PathBuf,
    pub shared_agents_dir: PathBuf,
}

impl KimiCodePaths {
    pub fn new(
        config_path: impl Into<PathBuf>,
        user_agents_dir: impl Into<PathBuf>,
        shared_agents_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            config_path: config_path.into(),
            user_agents_dir: user_agents_dir.into(),
            shared_agents_dir: shared_agents_dir.into(),
        }
    }
}

pub fn paths_from_home(home: impl AsRef<Path>) -> KimiCodePaths {
    let home = home.as_ref();
    KimiCodePaths::new(
        home.join(".kimi-code/config.toml"),
        home.join(".kimi-code/agents"),
        home.join(".agents/agents"),
    )
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
    pub model_preference: Option<String>,
    pub built_in: bool,
    pub source: Option<PathBuf>,
}

pub fn discover_kimi_code_agents(
    paths: &KimiCodePaths,
) -> Result<Vec<KimiCodeAgentProfile>, KimiCodeAdapterError> {
    let mut agents = BTreeMap::from([
        (
            "coder".to_string(),
            built_in_agent("coder", "General coding tasks"),
        ),
        (
            "explore".to_string(),
            built_in_agent("explore", "Codebase exploration"),
        ),
        (
            "plan".to_string(),
            built_in_agent("plan", "Implementation planning"),
        ),
    ]);
    load_agent_directory(&paths.shared_agents_dir, &mut agents)?;
    load_agent_directory(&paths.user_agents_dir, &mut agents)?;
    Ok(agents.into_values().collect())
}

fn built_in_agent(name: &str, description: &str) -> KimiCodeAgentProfile {
    KimiCodeAgentProfile {
        name: name.into(),
        description: description.into(),
        model_preference: None,
        built_in: true,
        source: None,
    }
}

fn load_agent_directory(
    root: &Path,
    agents: &mut BTreeMap<String, KimiCodeAgentProfile>,
) -> Result<(), KimiCodeAdapterError> {
    if !root.exists() {
        return Ok(());
    }
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|source| KimiCodeAdapterError::Io {
            operation: "read Kimi Code agent directory",
            path: directory.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| KimiCodeAdapterError::Io {
                operation: "read Kimi Code agent directory entry",
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|value| value.to_str()) == Some("md") {
                files.push(path);
            }
        }
    }
    files.sort();
    for path in files {
        let profile = parse_agent_profile(&path)?;
        agents.insert(profile.name.clone(), profile);
    }
    Ok(())
}

fn parse_agent_profile(path: &Path) -> Result<KimiCodeAgentProfile, KimiCodeAdapterError> {
    let text = fs::read_to_string(path).map_err(|source| KimiCodeAdapterError::Io {
        operation: "read Kimi Code agent",
        path: path.to_path_buf(),
        source,
    })?;
    parse_agent_profile_text(path, &text)
}

fn parse_agent_profile_text(
    path: &Path,
    text: &str,
) -> Result<KimiCodeAgentProfile, KimiCodeAdapterError> {
    let Some(rest) = text.strip_prefix("---\n") else {
        return Err(KimiCodeAdapterError::Invalid(format!(
            "Kimi Code agent has no YAML frontmatter: {}",
            path.display()
        )));
    };
    let Some((frontmatter, _body)) = rest.split_once("\n---\n") else {
        return Err(KimiCodeAdapterError::Invalid(format!(
            "Kimi Code agent frontmatter is not terminated: {}",
            path.display()
        )));
    };
    let yaml: YamlValue = serde_yaml::from_str(frontmatter).map_err(|error| {
        KimiCodeAdapterError::Invalid(format!(
            "Kimi Code agent frontmatter is invalid at {}: {error}",
            path.display()
        ))
    })?;
    let mapping = yaml.as_mapping().ok_or_else(|| {
        KimiCodeAdapterError::Invalid(format!(
            "Kimi Code agent frontmatter must be a mapping: {}",
            path.display()
        ))
    })?;
    let string = |key: &str| {
        mapping
            .get(YamlValue::String(key.into()))
            .and_then(YamlValue::as_str)
            .map(str::to_string)
    };
    let name = string("name").ok_or_else(|| {
        KimiCodeAdapterError::Invalid(format!(
            "Kimi Code agent name is missing: {}",
            path.display()
        ))
    })?;
    let description = string("description").unwrap_or_default();
    let model_preference = string("model_preference");
    if model_preference
        .as_deref()
        .is_some_and(|value| !matches!(value, "primary" | "secondary"))
    {
        return Err(KimiCodeAdapterError::Invalid(format!(
            "Kimi Code agent model_preference must be primary or secondary: {}",
            path.display()
        )));
    }
    Ok(KimiCodeAgentProfile {
        name,
        description,
        model_preference,
        built_in: false,
        source: Some(path.to_path_buf()),
    })
}

pub fn set_kimi_code_agent_model_preference(
    paths: &KimiCodePaths,
    name: &str,
    preference: &str,
) -> Result<KimiCodeAgentProfile, KimiCodeAdapterError> {
    if !matches!(preference, "primary" | "secondary") {
        return Err(KimiCodeAdapterError::Invalid(
            "Kimi Code agent model preference must be primary or secondary".into(),
        ));
    }
    let agent = discover_kimi_code_agents(paths)?
        .into_iter()
        .find(|agent| agent.name == name)
        .ok_or_else(|| {
            KimiCodeAdapterError::Invalid(format!("Kimi Code agent not found: {name}"))
        })?;
    let path = agent.source.ok_or_else(|| {
        KimiCodeAdapterError::Invalid(format!(
            "Kimi Code built-in agent model preference is managed by the client: {name}"
        ))
    })?;
    let original = fs::read_to_string(&path).map_err(|source| KimiCodeAdapterError::Io {
        operation: "read Kimi Code agent",
        path: path.clone(),
        source,
    })?;
    let rest = original.strip_prefix("---\n").ok_or_else(|| {
        KimiCodeAdapterError::Invalid(format!(
            "Kimi Code agent has no YAML frontmatter: {}",
            path.display()
        ))
    })?;
    let (frontmatter, body) = rest.split_once("\n---\n").ok_or_else(|| {
        KimiCodeAdapterError::Invalid(format!(
            "Kimi Code agent frontmatter is not terminated: {}",
            path.display()
        ))
    })?;
    let mut lines = frontmatter.lines().map(str::to_owned).collect::<Vec<_>>();
    if let Some(line) = lines
        .iter_mut()
        .find(|line| line.starts_with("model_preference:"))
    {
        *line = format!("model_preference: {preference}");
    } else {
        lines.push(format!("model_preference: {preference}"));
    }
    let projected = format!("---\n{}\n---\n{body}", lines.join("\n"));
    let profile = parse_agent_profile_text(&path, &projected)?;
    if profile.name != name || profile.model_preference.as_deref() != Some(preference) {
        return Err(KimiCodeAdapterError::Invalid(format!(
            "Kimi Code agent projection did not preserve the selected agent: {name}"
        )));
    }
    crate::storage::atomic_replace(&path, projected.as_bytes()).map_err(|source| {
        KimiCodeAdapterError::Io {
            operation: "write Kimi Code agent model preference",
            path: path.clone(),
            source,
        }
    })?;
    let verified = fs::read_to_string(&path).map_err(|source| KimiCodeAdapterError::Io {
        operation: "verify Kimi Code agent model preference",
        path: path.clone(),
        source,
    })?;
    if verified != projected {
        return Err(KimiCodeAdapterError::Invalid(format!(
            "Kimi Code agent model preference verification failed: {}",
            path.display()
        )));
    }
    Ok(profile)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KimiCodeModel {
    id: String,
    name: String,
    capabilities: Vec<String>,
}

impl KimiCodeModel {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        capabilities: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, KimiCodeAdapterError> {
        let id = id.into();
        if !valid_route(&id) {
            return Err(KimiCodeAdapterError::Invalid(format!(
                "Kimi Code model must use a GrillForge route alias: {id}"
            )));
        }
        let name = name.into();
        if name.trim().is_empty() || name.trim() != name || name.chars().any(char::is_control) {
            return Err(KimiCodeAdapterError::Invalid(format!(
                "Kimi Code model name is invalid: {id}"
            )));
        }
        let capabilities = capabilities.into_iter().map(Into::into).collect::<Vec<_>>();
        if capabilities.iter().any(|capability| {
            capability.trim().is_empty()
                || capability.trim() != capability
                || capability.chars().any(char::is_control)
        }) {
            return Err(KimiCodeAdapterError::Invalid(format!(
                "Kimi Code model capabilities are invalid: {id}"
            )));
        }
        Ok(Self {
            id,
            name,
            capabilities,
        })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KimiCodeRequest {
    gateway_base_url: String,
    gateway_token: String,
    models: Vec<KimiCodeModel>,
    primary_model: String,
    secondary_model: Option<String>,
}

impl KimiCodeRequest {
    pub fn new(
        gateway_base_url: impl Into<String>,
        gateway_token: impl Into<String>,
        mut models: Vec<KimiCodeModel>,
        primary_model: impl Into<String>,
        secondary_model: Option<impl Into<String>>,
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
        let primary_model = primary_model.into();
        if !models.iter().any(|model| model.id == primary_model) {
            return Err(KimiCodeAdapterError::Invalid(
                "Kimi Code primary model must be present in managed models".into(),
            ));
        }
        let secondary_model = secondary_model.map(Into::into);
        if secondary_model
            .as_ref()
            .is_some_and(|id| !models.iter().any(|model| &model.id == id))
        {
            return Err(KimiCodeAdapterError::Invalid(
                "Kimi Code secondary model must be present in managed models".into(),
            ));
        }
        Ok(Self {
            gateway_base_url,
            gateway_token,
            models,
            primary_model,
            secondary_model,
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
            .field("primary_model", &self.primary_model)
            .field("secondary_model", &self.secondary_model)
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

    document["default_model"] = value(&request.primary_model);
    document["providers"][PROVIDER_ID]["type"] = value("anthropic");
    document["providers"][PROVIDER_ID]["base_url"] = value(&request.gateway_base_url);
    document["providers"][PROVIDER_ID]["api_key"] = value(&request.gateway_token);
    for model in &request.models {
        document["models"][&model.id]["provider"] = value(PROVIDER_ID);
        document["models"][&model.id]["model"] = value(&model.id);
        document["models"][&model.id]["display_name"] = value(&model.name);
        document["models"][&model.id]["max_context_size"] = value(128_000_i64);
        let mut capabilities = Array::new();
        for capability in &model.capabilities {
            capabilities.push(capability.as_str());
        }
        document["models"][&model.id]["capabilities"] = value(capabilities);
    }
    match &request.secondary_model {
        Some(model) => document["secondary_model"]["model"] = value(model),
        None => {
            document.remove("secondary_model");
        }
    }
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
