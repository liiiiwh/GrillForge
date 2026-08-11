use crate::core::provider::is_slug;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};
use toml_edit::{DocumentMut, Item, Table, value};
use url::Url;

const SNAPSHOT_FILE: &str = "codex.snapshot.json";
const CLI_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexPaths {
    pub config_path: PathBuf,
    pub agents_dir: PathBuf,
}

impl CodexPaths {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        let config_path = config_path.into();
        let agents_dir = config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("agents");
        Self {
            config_path,
            agents_dir,
        }
    }
}

pub fn paths_from_home(home: impl AsRef<Path>) -> CodexPaths {
    CodexPaths::new(home.as_ref().join(".codex/config.toml"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexCliDetection {
    pub path: PathBuf,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodexNativeModel {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexConfiguredModel {
    pub model: String,
    pub provider: Option<String>,
}

pub fn detect_codex_cli() -> Result<Option<CodexCliDetection>, CodexAdapterError> {
    let executable = if cfg!(windows) { "codex.exe" } else { "codex" };
    let mut candidates = env::var_os("PATH")
        .map(|path| {
            env::split_paths(&path)
                .map(|dir| dir.join(executable))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from(
            "/Applications/ChatGPT.app/Contents/Resources/codex",
        ));
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join("Applications/ChatGPT.app/Contents/Resources/codex"));
        }
    }

    crate::cli_discovery::first_valid_candidate_across_sources(
        candidates,
        || {
            crate::cli_discovery::login_shell_candidates(executable).map_err(|error| {
                CodexAdapterError::Invalid(format!(
                    "discover Codex CLI through the login shell: {error}"
                ))
            })
        },
        |path| inspect_codex_cli(path),
    )
}

pub fn detect_codex_cli_in(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<Option<CodexCliDetection>, CodexAdapterError> {
    crate::cli_discovery::first_valid_candidate(candidates, |path| inspect_codex_cli(path))
}

pub fn inspect_codex_cli(path: impl AsRef<Path>) -> Result<CodexCliDetection, CodexAdapterError> {
    let path = path.as_ref().to_path_buf();
    let mut command =
        crate::cli_discovery::version_command(&path).map_err(|source| CodexAdapterError::Io {
            operation: "prepare Codex CLI inspection",
            path: path.clone(),
            source,
        })?;
    let mut child = command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| CodexAdapterError::Io {
            operation: "inspect Codex CLI",
            path: path.clone(),
            source,
        })?;
    let deadline = Instant::now() + CLI_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().map_err(|source| CodexAdapterError::Io {
            operation: "inspect Codex CLI",
            path: path.clone(),
            source,
        })? {
            let output = child
                .wait_with_output()
                .map_err(|source| CodexAdapterError::Io {
                    operation: "inspect Codex CLI",
                    path: path.clone(),
                    source,
                })?;
            if !status.success() {
                return Err(CodexAdapterError::Invalid(format!(
                    "Codex CLI did not return a version: {}",
                    path.display()
                )));
            }
            let version = String::from_utf8(output.stdout)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    CodexAdapterError::Invalid(format!(
                        "Codex CLI did not return a version: {}",
                        path.display()
                    ))
                })?;
            return Ok(CodexCliDetection { path, version });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CodexAdapterError::Invalid(format!(
                "Codex CLI version check timed out: {}",
                path.display()
            )));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub fn inspect_codex_native_models(
    path: impl AsRef<Path>,
) -> Result<Vec<CodexNativeModel>, CodexAdapterError> {
    #[derive(Deserialize)]
    struct Catalog {
        models: Vec<CatalogModel>,
    }
    #[derive(Deserialize)]
    struct CatalogModel {
        slug: String,
        display_name: String,
        visibility: String,
    }

    let path = path.as_ref().to_path_buf();
    let mut command =
        crate::cli_discovery::version_command(&path).map_err(|source| CodexAdapterError::Io {
            operation: "prepare Codex model catalog inspection",
            path: path.clone(),
            source,
        })?;
    let mut child = command
        .args(["debug", "models", "--bundled"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| CodexAdapterError::Io {
            operation: "inspect Codex model catalog",
            path: path.clone(),
            source,
        })?;
    let mut stdout = child.stdout.take().ok_or_else(|| {
        CodexAdapterError::Invalid("Codex CLI model catalog stdout is unavailable".into())
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| {
        CodexAdapterError::Invalid("Codex CLI model catalog stderr is unavailable".into())
    })?;
    let stdout_reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).map(|_| output)
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::new();
        stderr.read_to_end(&mut output).map(|_| output)
    });
    let deadline = Instant::now() + CLI_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().map_err(|source| CodexAdapterError::Io {
            operation: "inspect Codex model catalog",
            path: path.clone(),
            source,
        })? {
            let stdout = join_catalog_reader(stdout_reader, &path)?;
            let _stderr = join_catalog_reader(stderr_reader, &path)?;
            if !status.success() {
                return Err(CodexAdapterError::Invalid(format!(
                    "Codex CLI did not return its bundled model catalog: {}",
                    path.display()
                )));
            }
            let catalog: Catalog = serde_json::from_slice(&stdout).map_err(|_| {
                CodexAdapterError::Invalid(format!(
                    "Codex CLI returned an invalid bundled model catalog: {}",
                    path.display()
                ))
            })?;
            let mut models = catalog
                .models
                .into_iter()
                .filter(|model| model.visibility == "list")
                .map(|model| CodexNativeModel {
                    id: model.slug,
                    name: model.display_name,
                })
                .collect::<Vec<_>>();
            models.sort_by(|left, right| left.name.cmp(&right.name));
            return Ok(models);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(CodexAdapterError::Invalid(format!(
                "Codex CLI model catalog check timed out: {}",
                path.display()
            )));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn join_catalog_reader(
    reader: thread::JoinHandle<io::Result<Vec<u8>>>,
    path: &Path,
) -> Result<Vec<u8>, CodexAdapterError> {
    reader
        .join()
        .map_err(|_| {
            CodexAdapterError::Invalid(format!(
                "Codex CLI model catalog reader failed: {}",
                path.display()
            ))
        })?
        .map_err(|source| CodexAdapterError::Io {
            operation: "read Codex model catalog",
            path: path.to_path_buf(),
            source,
        })
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexProviderRequest {
    id: String,
    name: String,
    base_url: String,
    bearer_token: String,
}

impl CodexProviderRequest {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        base_url: impl Into<String>,
        bearer_token: impl Into<String>,
    ) -> Result<Self, CodexAdapterError> {
        let id = id.into();
        if !is_slug(&id) {
            return Err(CodexAdapterError::Invalid(
                "Codex provider id must be a lowercase slug".into(),
            ));
        }
        let name = name.into();
        validate_text(&name, "Codex provider name")?;
        let base_url = base_url.into();
        validate_endpoint(&base_url)?;
        let bearer_token = bearer_token.into();
        validate_secret(&bearer_token)?;
        Ok(Self {
            id,
            name,
            base_url,
            bearer_token,
        })
    }

    fn config_id(&self) -> String {
        if self.id == "grillforge" {
            self.id.clone()
        } else {
            format!("grillforge_{}", self.id)
        }
    }
}

impl Debug for CodexProviderRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexProviderRequest")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("bearer_token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodexModelSelection {
    Native {
        model: String,
    },
    Existing {
        model: String,
        provider: String,
    },
    Managed {
        provider: CodexProviderRequest,
        model: String,
    },
}

impl CodexModelSelection {
    pub fn native(model: impl Into<String>) -> Result<Self, CodexAdapterError> {
        let model = model.into();
        validate_model(&model)?;
        Ok(Self::Native { model })
    }

    pub fn existing(
        model: impl Into<String>,
        provider: Option<&str>,
    ) -> Result<Self, CodexAdapterError> {
        let model = model.into();
        validate_model(&model)?;
        let provider = provider.unwrap_or("openai").to_string();
        validate_text(&provider, "Codex model provider")?;
        Ok(Self::Existing { model, provider })
    }

    pub fn managed(
        provider: CodexProviderRequest,
        model: impl Into<String>,
    ) -> Result<Self, CodexAdapterError> {
        let model = model.into();
        validate_model(&model)?;
        Ok(Self::Managed { provider, model })
    }

    fn model(&self) -> &str {
        match self {
            Self::Native { model } | Self::Existing { model, .. } | Self::Managed { model, .. } => {
                model
            }
        }
    }

    fn provider_id(&self) -> String {
        match self {
            Self::Native { .. } => "openai".into(),
            Self::Existing { provider, .. } => provider.clone(),
            Self::Managed { provider, .. } => provider.config_id(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexRequest {
    main: CodexModelSelection,
    default_subagent: Option<CodexModelSelection>,
    custom_agents: BTreeMap<String, CodexModelSelection>,
}

impl CodexRequest {
    pub fn new(
        base_url: impl Into<String>,
        bearer_token: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, CodexAdapterError> {
        let provider =
            CodexProviderRequest::new("grillforge", "GrillForge", base_url, bearer_token)?;
        Self::from_selections(
            CodexModelSelection::managed(provider, model)?,
            None,
            BTreeMap::new(),
        )
    }

    pub fn native(model: impl Into<String>) -> Result<Self, CodexAdapterError> {
        Self::from_selections(CodexModelSelection::native(model)?, None, BTreeMap::new())
    }

    pub fn from_selections(
        main: CodexModelSelection,
        default_subagent: Option<CodexModelSelection>,
        custom_agents: BTreeMap<String, CodexModelSelection>,
    ) -> Result<Self, CodexAdapterError> {
        if default_subagent
            .as_ref()
            .is_some_and(|selection| selection.provider_id() != main.provider_id())
        {
            return Err(CodexAdapterError::Invalid(
                "Codex default SubAgent model must use the same provider as the main model".into(),
            ));
        }
        for name in custom_agents.keys() {
            if !is_slug(name) {
                return Err(CodexAdapterError::Invalid(format!(
                    "Codex custom Agent name must be a lowercase slug: {name}"
                )));
            }
        }
        Ok(Self {
            main,
            default_subagent,
            custom_agents,
        })
    }
}

impl Debug for CodexRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexRequest")
            .field("main", &self.main)
            .field("default_subagent", &self.default_subagent)
            .field("custom_agents", &self.custom_agents)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexCustomAgent {
    pub name: String,
    pub description: String,
    pub configured_model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexTakeoverStatus {
    Inactive,
    Active,
    Drifted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexStatus {
    pub snapshot_present: bool,
    pub takeover: CodexTakeoverStatus,
}

#[derive(Debug)]
pub struct CodexAdapter {
    paths: CodexPaths,
    snapshot_path: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
struct RecoverySnapshot {
    version: u8,
    original: Option<Vec<u8>>,
    expected: Vec<u8>,
    #[serde(default)]
    agents: Vec<AgentRecoverySnapshot>,
}

#[derive(Clone, Serialize, Deserialize)]
struct AgentRecoverySnapshot {
    file_name: String,
    original: Vec<u8>,
    expected: Vec<u8>,
}

impl CodexAdapter {
    pub fn new(paths: CodexPaths, grillforge_root: impl Into<PathBuf>) -> Self {
        Self {
            paths,
            snapshot_path: grillforge_root.into().join(SNAPSHOT_FILE),
        }
    }

    pub fn apply(&self, request: CodexRequest) -> Result<CodexStatus, CodexAdapterError> {
        let current = read_optional(&self.paths.config_path)?;
        let previous_snapshot = read_optional(&self.snapshot_path)?;
        let previous = previous_snapshot
            .as_deref()
            .map(parse_snapshot)
            .transpose()?;
        if let Some(snapshot) = &previous {
            if !snapshot_matches(snapshot, &self.paths)? {
                return Err(CodexAdapterError::Drifted);
            }
        }
        let original = previous
            .as_ref()
            .map(|snapshot| snapshot.original.clone())
            .unwrap_or_else(|| current.clone());
        let expected = project_config(original.as_deref(), &request)?;
        let agents = self.project_agents(previous.as_ref(), &request.custom_agents)?;
        let snapshot = RecoverySnapshot {
            version: 2,
            original,
            expected: expected.clone(),
            agents,
        };
        let snapshot_bytes = serde_json::to_vec_pretty(&snapshot).map_err(|error| {
            CodexAdapterError::Invalid(format!("could not encode Codex snapshot: {error}"))
        })?;
        let current_agents = snapshot
            .agents
            .iter()
            .map(|agent| {
                let path = self.paths.agents_dir.join(&agent.file_name);
                read_optional(&path).map(|bytes| (path, bytes))
            })
            .collect::<Result<Vec<_>, _>>()?;

        if let Err(error) = self.write_applied(&snapshot_bytes, &snapshot) {
            let rollback_config = write_optional(&self.paths.config_path, current.as_deref());
            let rollback_snapshot =
                write_optional(&self.snapshot_path, previous_snapshot.as_deref());
            let rollback_agents = current_agents
                .iter()
                .try_for_each(|(path, bytes)| write_optional(path, bytes.as_deref()));
            return Err(combine_rollback(
                error,
                rollback_config.and(rollback_snapshot).and(rollback_agents),
            ));
        }
        if !snapshot_matches(&snapshot, &self.paths)? {
            return Err(CodexAdapterError::Invalid(
                "Codex apply verification failed".into(),
            ));
        }
        self.status()
    }

    pub fn disable(&self) -> Result<CodexStatus, CodexAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(CodexStatus {
                snapshot_present: false,
                takeover: CodexTakeoverStatus::Inactive,
            });
        };
        let snapshot = parse_snapshot(&bytes)?;
        if !snapshot_matches(&snapshot, &self.paths)? {
            return Err(CodexAdapterError::Drifted);
        }
        write_optional(&self.paths.config_path, snapshot.original.as_deref())?;
        for agent in &snapshot.agents {
            write_optional(
                &self.paths.agents_dir.join(&agent.file_name),
                Some(&agent.original),
            )?;
        }
        if read_optional(&self.paths.config_path)? != snapshot.original {
            return Err(CodexAdapterError::Invalid(
                "Codex restore verification failed; recovery snapshot was retained".into(),
            ));
        }
        for agent in &snapshot.agents {
            if read_optional(&self.paths.agents_dir.join(&agent.file_name))?.as_deref()
                != Some(agent.original.as_slice())
            {
                return Err(CodexAdapterError::Invalid(
                    "Codex Agent restore verification failed; recovery snapshot was retained"
                        .into(),
                ));
            }
        }
        fs::remove_file(&self.snapshot_path).map_err(|source| CodexAdapterError::Io {
            operation: "remove Codex recovery snapshot",
            path: self.snapshot_path.clone(),
            source,
        })?;
        self.status()
    }

    pub fn status(&self) -> Result<CodexStatus, CodexAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(CodexStatus {
                snapshot_present: false,
                takeover: CodexTakeoverStatus::Inactive,
            });
        };
        let snapshot = parse_snapshot(&bytes)?;
        Ok(CodexStatus {
            snapshot_present: true,
            takeover: if snapshot_matches(&snapshot, &self.paths)? {
                CodexTakeoverStatus::Active
            } else {
                CodexTakeoverStatus::Drifted
            },
        })
    }

    pub fn configured_model(&self) -> Result<Option<CodexConfiguredModel>, CodexAdapterError> {
        let Some(bytes) = read_optional(&self.paths.config_path)? else {
            return Ok(None);
        };
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| CodexAdapterError::Invalid("Codex config.toml must be UTF-8".into()))?;
        let document = text.parse::<DocumentMut>().map_err(|error| {
            CodexAdapterError::Invalid(format!("Codex config.toml is invalid TOML: {error}"))
        })?;
        let Some(model) = document.get("model").and_then(Item::as_str) else {
            return Ok(None);
        };
        validate_model(model)?;
        let provider = document
            .get("model_provider")
            .and_then(Item::as_str)
            .map(str::to_string);
        if let Some(provider) = &provider {
            validate_text(provider, "Codex model provider")?;
        }
        Ok(Some(CodexConfiguredModel {
            model: model.to_string(),
            provider,
        }))
    }

    pub fn custom_agents(&self) -> Result<Vec<CodexCustomAgent>, CodexAdapterError> {
        Ok(self
            .agent_files()?
            .into_iter()
            .map(|agent| agent.definition)
            .collect())
    }

    fn project_agents(
        &self,
        previous: Option<&RecoverySnapshot>,
        requested: &BTreeMap<String, CodexModelSelection>,
    ) -> Result<Vec<AgentRecoverySnapshot>, CodexAdapterError> {
        let discovered = self
            .agent_files()?
            .into_iter()
            .map(|agent| (agent.definition.name.clone(), agent))
            .collect::<BTreeMap<_, _>>();
        for name in requested.keys() {
            if !discovered.contains_key(name) {
                return Err(CodexAdapterError::Invalid(format!(
                    "Codex custom Agent no longer exists: {name}"
                )));
            }
        }
        let previous_by_file = previous
            .map(|snapshot| {
                snapshot
                    .agents
                    .iter()
                    .map(|agent| (agent.file_name.clone(), agent))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let mut files = previous_by_file.keys().cloned().collect::<BTreeSet<_>>();
        files.extend(
            requested
                .keys()
                .filter_map(|name| discovered.get(name))
                .map(|agent| agent.file_name.clone()),
        );
        files
            .into_iter()
            .map(|file_name| {
                let (name, original) = if let Some(previous) = previous_by_file.get(&file_name) {
                    let definition = parse_agent_file(&file_name, &previous.original)?;
                    (definition.name, previous.original.clone())
                } else {
                    let agent = discovered
                        .values()
                        .find(|agent| agent.file_name == file_name)
                        .ok_or_else(|| {
                            CodexAdapterError::Invalid(format!(
                                "Codex custom Agent file disappeared: {file_name}"
                            ))
                        })?;
                    (agent.definition.name.clone(), agent.bytes.clone())
                };
                let expected = requested
                    .get(&name)
                    .map(|selection| project_agent(&file_name, &original, selection))
                    .transpose()?
                    .unwrap_or_else(|| original.clone());
                Ok(AgentRecoverySnapshot {
                    file_name,
                    original,
                    expected,
                })
            })
            .collect()
    }

    fn write_applied(
        &self,
        snapshot_bytes: &[u8],
        snapshot: &RecoverySnapshot,
    ) -> Result<(), CodexAdapterError> {
        write_optional(&self.snapshot_path, Some(snapshot_bytes))?;
        write_optional(&self.paths.config_path, Some(&snapshot.expected))?;
        for agent in &snapshot.agents {
            write_optional(
                &self.paths.agents_dir.join(&agent.file_name),
                Some(&agent.expected),
            )?;
        }
        Ok(())
    }

    fn agent_files(&self) -> Result<Vec<CodexAgentFile>, CodexAdapterError> {
        let entries = match fs::read_dir(&self.paths.agents_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(CodexAdapterError::Io {
                    operation: "read Codex Agent directory",
                    path: self.paths.agents_dir.clone(),
                    source,
                });
            }
        };
        let mut agents = Vec::new();
        let mut names = BTreeSet::new();
        for entry in entries {
            let entry = entry.map_err(|source| CodexAdapterError::Io {
                operation: "read Codex Agent directory entry",
                path: self.paths.agents_dir.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("toml") || !path.is_file()
            {
                continue;
            }
            let file_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| {
                    CodexAdapterError::Invalid("Codex Agent filename must be UTF-8".into())
                })?
                .to_string();
            let bytes = fs::read(&path).map_err(|source| CodexAdapterError::Io {
                operation: "read Codex Agent configuration",
                path: path.clone(),
                source,
            })?;
            let definition = parse_agent_file(&file_name, &bytes)?;
            if !names.insert(definition.name.clone()) {
                return Err(CodexAdapterError::Invalid(format!(
                    "duplicate Codex custom Agent name: {}",
                    definition.name
                )));
            }
            agents.push(CodexAgentFile {
                file_name,
                bytes,
                definition,
            });
        }
        agents.sort_by(|left, right| left.definition.name.cmp(&right.definition.name));
        Ok(agents)
    }
}

struct CodexAgentFile {
    file_name: String,
    bytes: Vec<u8>,
    definition: CodexCustomAgent,
}

fn project_config(
    original: Option<&[u8]>,
    request: &CodexRequest,
) -> Result<Vec<u8>, CodexAdapterError> {
    let text = match original {
        Some(bytes) => std::str::from_utf8(bytes)
            .map_err(|_| CodexAdapterError::Invalid("Codex config.toml must be UTF-8".into()))?,
        None => "",
    };
    let mut document = text.parse::<DocumentMut>().map_err(|error| {
        CodexAdapterError::Invalid(format!("Codex config.toml is invalid TOML: {error}"))
    })?;
    document["model"] = value(request.main.model());
    document["model_provider"] = value(request.main.provider_id());
    if let Some(default) = &request.default_subagent {
        let agents = document
            .entry("agents")
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| CodexAdapterError::Invalid("Codex agents must be a table".into()))?;
        agents["default_subagent_model"] = value(default.model());
    }
    let providers = document
        .entry("model_providers")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| {
            CodexAdapterError::Invalid("Codex model_providers must be a table".into())
        })?;
    let selections = std::iter::once(&request.main)
        .chain(request.default_subagent.iter())
        .chain(request.custom_agents.values());
    for selection in selections {
        let CodexModelSelection::Managed { provider, .. } = selection else {
            continue;
        };
        let mut configured = Table::new();
        configured["name"] = value(&provider.name);
        configured["base_url"] = value(&provider.base_url);
        configured["wire_api"] = value("responses");
        configured["experimental_bearer_token"] = value(&provider.bearer_token);
        providers[&provider.config_id()] = Item::Table(configured);
    }
    Ok(document.to_string().into_bytes())
}

fn project_agent(
    file_name: &str,
    original: &[u8],
    selection: &CodexModelSelection,
) -> Result<Vec<u8>, CodexAdapterError> {
    parse_agent_file(file_name, original)?;
    let text = std::str::from_utf8(original).map_err(|_| {
        CodexAdapterError::Invalid(format!("Codex Agent file must be UTF-8: {file_name}"))
    })?;
    let mut document = text.parse::<DocumentMut>().map_err(|error| {
        CodexAdapterError::Invalid(format!(
            "Codex Agent file is invalid TOML ({file_name}): {error}"
        ))
    })?;
    document["model"] = value(selection.model());
    document["model_provider"] = value(selection.provider_id());
    Ok(document.to_string().into_bytes())
}

fn parse_agent_file(file_name: &str, bytes: &[u8]) -> Result<CodexCustomAgent, CodexAdapterError> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        CodexAdapterError::Invalid(format!("Codex Agent file must be UTF-8: {file_name}"))
    })?;
    let document = text.parse::<DocumentMut>().map_err(|error| {
        CodexAdapterError::Invalid(format!(
            "Codex Agent file is invalid TOML ({file_name}): {error}"
        ))
    })?;
    let required = |field: &str| {
        document
            .get(field)
            .and_then(Item::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                CodexAdapterError::Invalid(format!(
                    "Codex Agent file {file_name} must define {field}"
                ))
            })
    };
    let name = required("name")?.to_string();
    if !is_slug(&name) {
        return Err(CodexAdapterError::Invalid(format!(
            "Codex custom Agent name must be a lowercase slug: {name}"
        )));
    }
    required("developer_instructions")?;
    Ok(CodexCustomAgent {
        name,
        description: required("description")?.to_string(),
        configured_model: document
            .get("model")
            .and_then(Item::as_str)
            .map(str::to_string),
    })
}

fn snapshot_matches(
    snapshot: &RecoverySnapshot,
    paths: &CodexPaths,
) -> Result<bool, CodexAdapterError> {
    if read_optional(&paths.config_path)?.as_deref() != Some(snapshot.expected.as_slice()) {
        return Ok(false);
    }
    for agent in &snapshot.agents {
        if read_optional(&paths.agents_dir.join(&agent.file_name))?.as_deref()
            != Some(agent.expected.as_slice())
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_endpoint(value: &str) -> Result<(), CodexAdapterError> {
    let url = Url::parse(value)
        .map_err(|_| CodexAdapterError::Invalid(format!("invalid Codex endpoint: {value}")))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(CodexAdapterError::Invalid(format!(
            "invalid Codex endpoint: {value}"
        )));
    }
    Ok(())
}

fn validate_text(value: &str, field: &str) -> Result<(), CodexAdapterError> {
    if value.trim().is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(CodexAdapterError::Invalid(format!(
            "{field} must not be empty, padded, or contain control characters"
        )));
    }
    Ok(())
}

fn validate_model(value: &str) -> Result<(), CodexAdapterError> {
    validate_text(value, "Codex model")
}

fn validate_secret(value: &str) -> Result<(), CodexAdapterError> {
    validate_text(value, "Codex bearer token")
}

fn parse_snapshot(bytes: &[u8]) -> Result<RecoverySnapshot, CodexAdapterError> {
    let snapshot: RecoverySnapshot = serde_json::from_slice(bytes).map_err(|error| {
        CodexAdapterError::Invalid(format!("Codex recovery snapshot is invalid: {error}"))
    })?;
    if !matches!(snapshot.version, 1 | 2) {
        return Err(CodexAdapterError::Invalid(format!(
            "unsupported Codex recovery snapshot version: {}",
            snapshot.version
        )));
    }
    Ok(snapshot)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, CodexAdapterError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CodexAdapterError::Io {
            operation: "read Codex configuration",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn write_optional(path: &Path, bytes: Option<&[u8]>) -> Result<(), CodexAdapterError> {
    match bytes {
        Some(bytes) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|source| CodexAdapterError::Io {
                    operation: "create Codex configuration directory",
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            crate::storage::atomic_replace(path, bytes).map_err(|source| CodexAdapterError::Io {
                operation: "write Codex configuration",
                path: path.to_path_buf(),
                source,
            })
        }
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(CodexAdapterError::Io {
                operation: "remove Codex configuration",
                path: path.to_path_buf(),
                source,
            }),
        },
    }
}

fn combine_rollback(
    primary: CodexAdapterError,
    rollback: Result<(), CodexAdapterError>,
) -> CodexAdapterError {
    match rollback {
        Ok(()) => primary,
        Err(rollback) => {
            CodexAdapterError::Invalid(format!("{primary}; rollback also failed: {rollback}"))
        }
    }
}

#[derive(Debug)]
pub enum CodexAdapterError {
    Invalid(String),
    Drifted,
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl Display for CodexAdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Drifted => formatter.write_str(
                "Codex configuration differs from the last GrillForge apply; resolve the drift before continuing",
            ),
            Self::Io {
                operation,
                path,
                source,
            } => write!(formatter, "{operation} failed for {}: {source}", path.display()),
        }
    }
}

impl Error for CodexAdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
