use std::collections::{BTreeMap, HashSet};
use std::env;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use url::Url;

const SETTINGS_FILE: &str = "settings.json";
const AGENT_NAME_PREFIX: &str = "grillforge-worker-";
const OWNERSHIP_MARKER: &[u8] = b"<!-- Managed by GrillForge. -->";
const SNAPSHOT_FILE: &str = "claude-code.snapshot.json";
const MANAGED_ENVIRONMENT_KEYS: [&str; 8] = [
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "GRILLFORGE_BIN",
];
pub const MODEL_SLOT_IDS: [&str; 4] = ["sonnet", "opus", "fable", "haiku"];
const MODEL_SLOTS: [(&str, &str); 4] = [
    ("sonnet", "ANTHROPIC_DEFAULT_SONNET_MODEL"),
    ("opus", "ANTHROPIC_DEFAULT_OPUS_MODEL"),
    ("fable", "ANTHROPIC_DEFAULT_FABLE_MODEL"),
    ("haiku", "ANTHROPIC_DEFAULT_HAIKU_MODEL"),
];
const CLI_VERSION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerStrategy {
    ForcedSingle,
    SelectablePool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerModel {
    id: String,
    route_alias: String,
    capabilities: Vec<String>,
}

impl WorkerModel {
    pub fn new(id: impl Into<String>, route_alias: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            route_alias: route_alias.into(),
            capabilities: Vec::new(),
        }
    }

    pub fn with_capabilities(
        mut self,
        capabilities: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.capabilities = capabilities.into_iter().map(Into::into).collect();
        self
    }

    pub fn native_default() -> Self {
        Self::new("claude-native", "inherit").with_capabilities(["coding", "general"])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnableRequest {
    gateway_base_url: String,
    main_route: Option<String>,
    model_routes: BTreeMap<String, String>,
    workers: Vec<WorkerModel>,
    worker_strategy: Option<WorkerStrategy>,
    selector_binary: Option<String>,
}

impl EnableRequest {
    pub fn native_main(
        gateway_base_url: impl Into<String>,
        workers: Vec<WorkerModel>,
        strategy: WorkerStrategy,
    ) -> Self {
        Self {
            gateway_base_url: gateway_base_url.into(),
            main_route: None,
            model_routes: BTreeMap::new(),
            workers,
            worker_strategy: Some(strategy),
            selector_binary: None,
        }
    }

    pub fn managed_main_only(
        gateway_base_url: impl Into<String>,
        main_route: impl Into<String>,
    ) -> Self {
        Self {
            gateway_base_url: gateway_base_url.into(),
            main_route: Some(main_route.into()),
            model_routes: BTreeMap::new(),
            workers: Vec::new(),
            worker_strategy: None,
            selector_binary: None,
        }
    }

    pub fn managed_main(
        gateway_base_url: impl Into<String>,
        main_route: impl Into<String>,
        workers: Vec<WorkerModel>,
        worker_strategy: WorkerStrategy,
    ) -> Self {
        Self {
            gateway_base_url: gateway_base_url.into(),
            main_route: Some(main_route.into()),
            model_routes: BTreeMap::new(),
            workers,
            worker_strategy: Some(worker_strategy),
            selector_binary: None,
        }
    }

    pub fn native_main_without_workers() -> Self {
        Self {
            gateway_base_url: String::new(),
            main_route: None,
            model_routes: BTreeMap::new(),
            workers: Vec::new(),
            worker_strategy: None,
            selector_binary: None,
        }
    }

    pub fn with_selector_binary(mut self, path: impl Into<String>) -> Self {
        self.selector_binary = Some(path.into());
        self
    }

    pub fn with_model_routes(
        mut self,
        gateway_base_url: impl Into<String>,
        routes: BTreeMap<String, String>,
    ) -> Self {
        if !routes.is_empty() {
            self.gateway_base_url = gateway_base_url.into();
        }
        self.model_routes = routes;
        self
    }

    fn takes_over(&self) -> bool {
        self.main_route.is_some() || !self.model_routes.is_empty() || self.worker_strategy.is_some()
    }
}

impl ActiveConfiguration {
    fn from_request(request: &EnableRequest) -> Self {
        let forced_worker_route = match request.worker_strategy {
            Some(WorkerStrategy::ForcedSingle) => request
                .workers
                .first()
                .map(|worker| worker.route_alias.clone()),
            _ => None,
        };
        let agents = if request.worker_strategy == Some(WorkerStrategy::SelectablePool) {
            request
                .workers
                .iter()
                .map(|worker| {
                    (
                        format!("{}{}.md", AGENT_NAME_PREFIX, worker.id),
                        render_agent(worker),
                    )
                })
                .collect()
        } else {
            BTreeMap::new()
        };
        Self {
            base_url: request.gateway_base_url.clone(),
            main_route: request.main_route.clone(),
            model_routes: request.model_routes.clone(),
            forced_worker_route,
            selector_binary: request.selector_binary.clone(),
            agents,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeCodeOperation {
    SetEnvironment {
        key: String,
        value: String,
    },
    RemoveEnvironment {
        key: String,
    },
    WriteFile {
        path: PathBuf,
        contents: String,
    },
    RemoveFile {
        path: PathBuf,
    },
    RestoreFile {
        path: PathBuf,
        contents: Option<Vec<u8>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeCodeSnapshot {
    version: u8,
    environment: BTreeMap<String, Option<String>>,
    agents: BTreeMap<String, Option<String>>,
    active: ActiveConfiguration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ActiveConfiguration {
    base_url: String,
    main_route: Option<String>,
    #[serde(default)]
    model_routes: BTreeMap<String, String>,
    forced_worker_route: Option<String>,
    #[serde(default)]
    selector_binary: Option<String>,
    agents: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeCodeTakeoverStatus {
    Inactive,
    Active,
    Drifted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCodeStatus {
    pub snapshot_present: bool,
    pub takeover: ClaudeCodeTakeoverStatus,
    pub differences: Vec<String>,
    pub managed_main_alias: Option<String>,
    pub forced_worker_alias: Option<String>,
    pub generated_agent_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCliDetection {
    pub path: PathBuf,
    pub version: String,
}

pub fn detect_claude_cli() -> Result<Option<ClaudeCliDetection>, ClaudeCodeAdapterError> {
    let executable = if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    };
    let mut candidates = env::var_os("PATH")
        .map(|search_path| {
            env::split_paths(&search_path)
                .map(|directory| directory.join(executable))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local/bin").join(executable));
        candidates.push(home.join(".claude/local").join(executable));
    }
    crate::cli_discovery::first_valid_candidate_across_sources(
        candidates,
        || {
            crate::cli_discovery::login_shell_candidates(executable).map_err(|source| {
                ClaudeCodeAdapterError::InspectCli {
                    path: env::var_os("SHELL")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("/bin/sh")),
                    source,
                }
            })
        },
        |path| inspect_claude_cli(path),
    )
}

pub fn inspect_claude_cli(
    path: impl AsRef<Path>,
) -> Result<ClaudeCliDetection, ClaudeCodeAdapterError> {
    let path = path.as_ref().to_path_buf();
    let mut command = crate::cli_discovery::version_command(&path).map_err(|source| {
        ClaudeCodeAdapterError::InspectCli {
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
        .map_err(|source| ClaudeCodeAdapterError::InspectCli {
            path: path.clone(),
            source,
        })?;
    let deadline = Instant::now() + CLI_VERSION_TIMEOUT;
    loop {
        if let Some(status) =
            child
                .try_wait()
                .map_err(|source| ClaudeCodeAdapterError::InspectCli {
                    path: path.clone(),
                    source,
                })?
        {
            let output =
                child
                    .wait_with_output()
                    .map_err(|source| ClaudeCodeAdapterError::InspectCli {
                        path: path.clone(),
                        source,
                    })?;
            if !status.success() {
                return Err(ClaudeCodeAdapterError::CliVersionFailed(path));
            }
            let version = String::from_utf8(output.stdout)
                .ok()
                .map(|version| version.trim().to_string())
                .filter(|version| !version.is_empty())
                .ok_or_else(|| ClaudeCodeAdapterError::CliVersionFailed(path.clone()))?;
            return Ok(ClaudeCliDetection { path, version });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ClaudeCodeAdapterError::CliTimedOut(path));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCodePlan {
    operations: Vec<ClaudeCodeOperation>,
    snapshot: Option<ClaudeCodeSnapshot>,
}

#[derive(Debug)]
struct PreparedPlan {
    settings: Option<(PathBuf, Vec<u8>)>,
    files: Vec<PreparedFileOperation>,
    rollback: Vec<FileSnapshot>,
}

#[derive(Debug)]
enum PreparedFileOperation {
    Write { path: PathBuf, contents: Vec<u8> },
    Remove { path: PathBuf },
}

impl ClaudeCodePlan {
    pub fn operations(&self) -> &[ClaudeCodeOperation] {
        &self.operations
    }

    pub fn snapshot(&self) -> Option<&ClaudeCodeSnapshot> {
        self.snapshot.as_ref()
    }
}

#[derive(Debug)]
pub struct ClaudeCodeAdapter {
    config_dir: PathBuf,
    snapshot_path: PathBuf,
}

impl ClaudeCodeAdapter {
    pub fn new(config_dir: impl Into<PathBuf>, grillforge_config_root: impl Into<PathBuf>) -> Self {
        Self {
            config_dir: config_dir.into(),
            snapshot_path: grillforge_config_root.into().join(SNAPSHOT_FILE),
        }
    }

    pub fn snapshot_path(&self) -> &Path {
        &self.snapshot_path
    }

    pub fn native_upstream_base_url(&self) -> Result<Option<String>, ClaudeCodeAdapterError> {
        if let Some(snapshot) = self.read_snapshot()? {
            return Ok(snapshot
                .environment
                .get("ANTHROPIC_BASE_URL")
                .cloned()
                .flatten());
        }
        let settings_path = self.config_dir.join(SETTINGS_FILE);
        let settings = parse_settings(&snapshot_file(&settings_path)?)?;
        Ok(environment_value(
            settings.get("env").and_then(serde_json::Value::as_object),
            "ANTHROPIC_BASE_URL",
        )
        .map(str::to_owned))
    }

    pub fn status(&self) -> Result<ClaudeCodeStatus, ClaudeCodeAdapterError> {
        let settings_path = self.config_dir.join(SETTINGS_FILE);
        let settings = parse_settings(&snapshot_file(&settings_path)?)?;
        let environment = settings.get("env").and_then(serde_json::Value::as_object);
        let current_base_url = environment_value(environment, "ANTHROPIC_BASE_URL");
        let current_main = environment_value(environment, "ANTHROPIC_MODEL");
        let current_forced = environment_value(environment, "CLAUDE_CODE_SUBAGENT_MODEL");
        let current_selector = environment_value(environment, "GRILLFORGE_BIN");
        let current_model_routes = MODEL_SLOTS
            .into_iter()
            .map(|(slot, key)| (slot, environment_value(environment, key)))
            .collect::<BTreeMap<_, _>>();
        let current_agents = current_owned_agents(&self.config_dir.join("agents"))?;
        let generated_agent_names = current_agents
            .keys()
            .filter_map(|name| name.strip_suffix(".md").map(str::to_owned))
            .collect::<Vec<_>>();
        let managed_main_alias = current_main.filter(|alias| is_route_alias(alias));
        let forced_worker_alias = current_forced.filter(|alias| is_route_alias(alias));

        let snapshot = self.read_snapshot()?;
        let mut differences = Vec::new();
        let takeover = match &snapshot {
            Some(snapshot) => {
                let expected_main = snapshot
                    .active
                    .main_route
                    .as_deref()
                    .or_else(|| original_environment(snapshot, "ANTHROPIC_MODEL"));
                let expected_forced = snapshot
                    .active
                    .forced_worker_route
                    .as_deref()
                    .or_else(|| original_environment(snapshot, "CLAUDE_CODE_SUBAGENT_MODEL"));
                let expected_selector = snapshot
                    .active
                    .selector_binary
                    .as_deref()
                    .or_else(|| original_environment(snapshot, "GRILLFORGE_BIN"));
                for (slot, key) in MODEL_SLOTS {
                    let expected = snapshot
                        .active
                        .model_routes
                        .get(slot)
                        .map(String::as_str)
                        .or_else(|| original_environment(snapshot, key));
                    if current_model_routes.get(slot).copied().flatten() != expected {
                        differences.push(key.to_string());
                    }
                }
                if current_base_url != Some(snapshot.active.base_url.as_str()) {
                    differences.push("ANTHROPIC_BASE_URL".into());
                }
                if current_main != expected_main {
                    differences.push("ANTHROPIC_MODEL".into());
                }
                if current_forced != expected_forced {
                    differences.push("CLAUDE_CODE_SUBAGENT_MODEL".into());
                }
                if current_selector != expected_selector {
                    differences.push("GRILLFORGE_BIN".into());
                }
                let agent_names = current_agents
                    .keys()
                    .chain(snapshot.active.agents.keys())
                    .collect::<HashSet<_>>();
                for name in agent_names {
                    if current_agents.get(name) != snapshot.active.agents.get(name) {
                        differences.push(format!("agents/{name}"));
                    }
                }
                differences.sort();
                if differences.is_empty() {
                    ClaudeCodeTakeoverStatus::Active
                } else {
                    ClaudeCodeTakeoverStatus::Drifted
                }
            }
            None => {
                let has_managed_configuration = managed_main_alias.is_some()
                    || forced_worker_alias.is_some()
                    || current_model_routes
                        .values()
                        .flatten()
                        .any(|alias| is_route_alias(alias))
                    || current_selector.is_some()
                    || !current_agents.is_empty();
                if has_managed_configuration {
                    if managed_main_alias.is_some() {
                        differences.push("ANTHROPIC_MODEL".into());
                    }
                    if forced_worker_alias.is_some() {
                        differences.push("CLAUDE_CODE_SUBAGENT_MODEL".into());
                    }
                    if current_selector.is_some() {
                        differences.push("GRILLFORGE_BIN".into());
                    }
                    for name in current_agents.keys() {
                        differences.push(format!("agents/{name}"));
                    }
                    differences.sort();
                    ClaudeCodeTakeoverStatus::Drifted
                } else {
                    ClaudeCodeTakeoverStatus::Inactive
                }
            }
        };

        Ok(ClaudeCodeStatus {
            snapshot_present: snapshot.is_some(),
            takeover,
            differences,
            managed_main_alias: managed_main_alias.map(str::to_owned),
            forced_worker_alias: forced_worker_alias.map(str::to_owned),
            generated_agent_names,
        })
    }

    pub fn enable(&self, request: EnableRequest) -> Result<(), ClaudeCodeAdapterError> {
        if !request.takes_over() {
            return if self.read_snapshot()?.is_some() {
                self.disable()
            } else {
                Ok(())
            };
        }
        let plan = self.plan_enable(request)?;
        let prepared = self.prepare(&plan)?;
        let previous_snapshot = snapshot_file(&self.snapshot_path)?;
        let snapshot = plan
            .snapshot()
            .expect("enable plans always carry a recovery snapshot");
        self.persist_snapshot(snapshot)?;
        if let Err(error) = self.apply(&prepared).and_then(|_| self.verify_plan(&plan)) {
            let rollback = self
                .rollback(&prepared.rollback)
                .and_then(|_| restore_snapshot(&previous_snapshot));
            return Err(combine_rollback(error, rollback));
        }
        Ok(())
    }

    pub fn disable(&self) -> Result<(), ClaudeCodeAdapterError> {
        let snapshot = self
            .read_snapshot()?
            .ok_or_else(|| ClaudeCodeAdapterError::SnapshotMissing(self.snapshot_path.clone()))?;
        let plan = self.plan_disable(&snapshot);
        let prepared = self.prepare(&plan)?;
        if let Err(error) = self.apply(&prepared).and_then(|_| self.verify_plan(&plan)) {
            let rollback = self.rollback(&prepared.rollback);
            return Err(combine_rollback(error, rollback));
        }
        fs::remove_file(&self.snapshot_path).map_err(|source| {
            ClaudeCodeAdapterError::WriteConfiguration {
                path: self.snapshot_path.clone(),
                source,
            }
        })
    }

    pub fn plan_enable(
        &self,
        request: EnableRequest,
    ) -> Result<ClaudeCodePlan, ClaudeCodeAdapterError> {
        if !request.takes_over() {
            return Ok(match self.read_snapshot()? {
                Some(snapshot) => self.plan_disable(&snapshot),
                None => ClaudeCodePlan {
                    operations: Vec::new(),
                    snapshot: None,
                },
            });
        }
        validate_gateway(&request.gateway_base_url)?;
        if let Some(main_route) = &request.main_route {
            if !is_route_alias(main_route) {
                return Err(ClaudeCodeAdapterError::InvalidRouteAlias(
                    main_route.clone(),
                ));
            }
        }
        for (slot, route) in &request.model_routes {
            if model_slot_environment_key(slot).is_none() {
                return Err(ClaudeCodeAdapterError::InvalidModelSlot(slot.clone()));
            }
            if !is_route_alias(route) {
                return Err(ClaudeCodeAdapterError::InvalidRouteAlias(route.clone()));
            }
        }
        if let Some(worker_strategy) = request.worker_strategy {
            validate_worker_count(worker_strategy, request.workers.len())?;
            validate_workers(&request.workers)?;
        }
        let active = ActiveConfiguration::from_request(&request);

        let settings_path = self.config_dir.join(SETTINGS_FILE);
        let settings_snapshot = snapshot_file(&settings_path)?;
        let settings = parse_settings(&settings_snapshot)?;
        let existing_snapshot = self.read_snapshot()?;
        let had_snapshot = existing_snapshot.is_some();
        let mut snapshot = match existing_snapshot {
            Some(snapshot) => snapshot,
            None => ClaudeCodeSnapshot {
                version: 1,
                environment: capture_environment(&settings),
                agents: BTreeMap::new(),
                active: active.clone(),
            },
        };
        for key in MANAGED_ENVIRONMENT_KEYS {
            snapshot
                .environment
                .entry(key.to_string())
                .or_insert_with(|| {
                    settings
                        .get("env")
                        .and_then(serde_json::Value::as_object)
                        .and_then(|environment| environment.get(key))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                });
        }
        let existing_agents = owned_agent_files(&self.config_dir.join("agents"))?;
        let mut operations = vec![ClaudeCodeOperation::SetEnvironment {
            key: "ANTHROPIC_BASE_URL".to_string(),
            value: request.gateway_base_url,
        }];
        match &request.selector_binary {
            Some(path) if !request.workers.is_empty() => {
                if path.trim().is_empty() || !Path::new(path).is_absolute() {
                    return Err(ClaudeCodeAdapterError::InvalidSelectorBinary(path.clone()));
                }
                operations.push(ClaudeCodeOperation::SetEnvironment {
                    key: "GRILLFORGE_BIN".to_string(),
                    value: path.clone(),
                });
            }
            _ if had_snapshot => {
                push_restore_environment(&mut operations, &snapshot, "GRILLFORGE_BIN")
            }
            _ => {}
        }

        match request.main_route {
            Some(main_route) => operations.push(ClaudeCodeOperation::SetEnvironment {
                key: "ANTHROPIC_MODEL".to_string(),
                value: main_route,
            }),
            None if had_snapshot => {
                push_restore_environment(&mut operations, &snapshot, "ANTHROPIC_MODEL")
            }
            None => {}
        }

        for (slot, key) in MODEL_SLOTS {
            match request.model_routes.get(slot) {
                Some(route) => operations.push(ClaudeCodeOperation::SetEnvironment {
                    key: key.to_string(),
                    value: route.clone(),
                }),
                None if had_snapshot => push_restore_environment(&mut operations, &snapshot, key),
                None => {}
            }
        }

        match request.worker_strategy {
            Some(WorkerStrategy::ForcedSingle) => {
                operations.push(ClaudeCodeOperation::SetEnvironment {
                    key: "CLAUDE_CODE_SUBAGENT_MODEL".to_string(),
                    value: request.workers[0].route_alias.clone(),
                });
                for agent in existing_agents {
                    operations.push(ClaudeCodeOperation::RemoveFile {
                        path: agent.path.clone(),
                    });
                    capture_agent(&mut snapshot, &agent)?;
                }
            }
            Some(WorkerStrategy::SelectablePool) => {
                operations.push(ClaudeCodeOperation::RemoveEnvironment {
                    key: "CLAUDE_CODE_SUBAGENT_MODEL".to_string(),
                });
                let mut desired_paths = HashSet::new();
                for worker in &request.workers {
                    let path = self.agent_path(worker);
                    desired_paths.insert(path.clone());
                    let file_snapshot = snapshot_file(&path)?;
                    validate_agent_ownership(&file_snapshot)?;
                    capture_agent(&mut snapshot, &file_snapshot)?;
                    operations.push(ClaudeCodeOperation::WriteFile {
                        path,
                        contents: render_agent(worker),
                    });
                }
                for agent in existing_agents {
                    if !desired_paths.contains(&agent.path) {
                        operations.push(ClaudeCodeOperation::RemoveFile {
                            path: agent.path.clone(),
                        });
                        capture_agent(&mut snapshot, &agent)?;
                    }
                }
            }
            None => {
                if had_snapshot {
                    push_restore_environment(
                        &mut operations,
                        &snapshot,
                        "CLAUDE_CODE_SUBAGENT_MODEL",
                    );
                }
                for agent in existing_agents {
                    operations.push(ClaudeCodeOperation::RemoveFile {
                        path: agent.path.clone(),
                    });
                    capture_agent(&mut snapshot, &agent)?;
                }
            }
        }

        snapshot.active = active;
        Ok(ClaudeCodePlan {
            operations,
            snapshot: Some(snapshot),
        })
    }

    pub fn plan_disable(&self, snapshot: &ClaudeCodeSnapshot) -> ClaudeCodePlan {
        let mut operations = Vec::new();
        for (key, value) in &snapshot.environment {
            match value.clone() {
                Some(value) => operations.push(ClaudeCodeOperation::SetEnvironment {
                    key: key.clone(),
                    value,
                }),
                None => {
                    operations.push(ClaudeCodeOperation::RemoveEnvironment { key: key.clone() })
                }
            }
        }
        operations.extend(snapshot.agents.iter().map(|(name, contents)| {
            ClaudeCodeOperation::RestoreFile {
                path: self.config_dir.join("agents").join(name),
                contents: contents.clone().map(String::into_bytes),
            }
        }));

        ClaudeCodePlan {
            operations,
            snapshot: None,
        }
    }

    fn agent_path(&self, worker: &WorkerModel) -> PathBuf {
        self.config_dir
            .join("agents")
            .join(format!("{}{}.md", AGENT_NAME_PREFIX, worker.id))
    }

    fn read_snapshot(&self) -> Result<Option<ClaudeCodeSnapshot>, ClaudeCodeAdapterError> {
        let contents = match fs::read(&self.snapshot_path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ClaudeCodeAdapterError::ReadConfiguration {
                    path: self.snapshot_path.clone(),
                    source,
                });
            }
        };
        let snapshot: ClaudeCodeSnapshot = serde_json::from_slice(&contents)
            .map_err(|_| ClaudeCodeAdapterError::InvalidSnapshot(self.snapshot_path.clone()))?;
        if snapshot.version != 1
            || validate_gateway(&snapshot.active.base_url).is_err()
            || snapshot
                .active
                .main_route
                .as_deref()
                .is_some_and(|alias| !is_route_alias(alias))
            || snapshot
                .active
                .forced_worker_route
                .as_deref()
                .is_some_and(|alias| !is_route_alias(alias))
            || snapshot.active.model_routes.iter().any(|(slot, alias)| {
                model_slot_environment_key(slot).is_none() || !is_route_alias(alias)
            })
            || snapshot.active.agents.iter().any(|(name, contents)| {
                !is_managed_agent_name(name)
                    || !contents
                        .as_bytes()
                        .windows(OWNERSHIP_MARKER.len())
                        .any(|part| part == OWNERSHIP_MARKER)
            })
            || snapshot
                .environment
                .keys()
                .any(|key| !MANAGED_ENVIRONMENT_KEYS.contains(&key.as_str()))
            || snapshot
                .agents
                .keys()
                .any(|name| !is_managed_agent_name(name))
        {
            return Err(ClaudeCodeAdapterError::InvalidSnapshot(
                self.snapshot_path.clone(),
            ));
        }
        Ok(Some(snapshot))
    }

    fn persist_snapshot(
        &self,
        snapshot: &ClaudeCodeSnapshot,
    ) -> Result<(), ClaudeCodeAdapterError> {
        let contents = serde_json::to_vec_pretty(snapshot).map_err(|source| {
            ClaudeCodeAdapterError::SerializeConfiguration {
                path: self.snapshot_path.clone(),
                source,
            }
        })?;
        create_parent(&self.snapshot_path)?;
        crate::storage::atomic_replace(&self.snapshot_path, &contents).map_err(|source| {
            ClaudeCodeAdapterError::WriteConfiguration {
                path: self.snapshot_path.clone(),
                source,
            }
        })
    }

    fn prepare(&self, plan: &ClaudeCodePlan) -> Result<PreparedPlan, ClaudeCodeAdapterError> {
        let settings_path = self.config_dir.join(SETTINGS_FILE);
        let settings_snapshot = snapshot_file(&settings_path)?;
        let mut settings = parse_settings(&settings_snapshot)?;
        let mut changes_settings = false;
        let mut files = Vec::new();

        for operation in plan.operations() {
            match operation {
                ClaudeCodeOperation::SetEnvironment { key, value } => {
                    validate_managed_environment_key(key, &settings_path)?;
                    environment_mut(&mut settings, &settings_path)?
                        .insert(key.clone(), serde_json::Value::String(value.clone()));
                    changes_settings = true;
                }
                ClaudeCodeOperation::RemoveEnvironment { key } => {
                    validate_managed_environment_key(key, &settings_path)?;
                    if let Some(environment) = settings
                        .get_mut("env")
                        .and_then(serde_json::Value::as_object_mut)
                    {
                        environment.remove(key);
                    }
                    changes_settings = true;
                }
                ClaudeCodeOperation::WriteFile { path, contents } => {
                    self.preflight_agent(path)?;
                    files.push(PreparedFileOperation::Write {
                        path: path.clone(),
                        contents: contents.as_bytes().to_vec(),
                    });
                }
                ClaudeCodeOperation::RemoveFile { path } => {
                    self.preflight_agent(path)?;
                    files.push(PreparedFileOperation::Remove { path: path.clone() });
                }
                ClaudeCodeOperation::RestoreFile { path, contents } => {
                    self.preflight_agent(path)?;
                    files.push(match contents {
                        Some(contents) => PreparedFileOperation::Write {
                            path: path.clone(),
                            contents: contents.clone(),
                        },
                        None => PreparedFileOperation::Remove { path: path.clone() },
                    });
                }
            }
        }

        let settings = changes_settings
            .then(|| {
                serde_json::to_vec_pretty(&serde_json::Value::Object(settings)).map_err(|source| {
                    ClaudeCodeAdapterError::SerializeConfiguration {
                        path: settings_path.clone(),
                        source,
                    }
                })
            })
            .transpose()?
            .map(|contents| (settings_path, contents));
        let mut rollback = Vec::new();
        if changes_settings {
            rollback.push(settings_snapshot);
        }
        let mut captured = HashSet::new();
        for operation in &files {
            let path = match operation {
                PreparedFileOperation::Write { path, .. }
                | PreparedFileOperation::Remove { path } => path,
            };
            if captured.insert(path.clone()) {
                rollback.push(snapshot_file(path)?);
            }
        }
        Ok(PreparedPlan {
            settings,
            files,
            rollback,
        })
    }

    fn preflight_agent(&self, path: &Path) -> Result<(), ClaudeCodeAdapterError> {
        let expected_directory = self.config_dir.join("agents");
        let valid_path = path.parent() == Some(expected_directory.as_path())
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_managed_agent_name);
        if !valid_path {
            return Err(ClaudeCodeAdapterError::AgentFileCollision(
                path.to_path_buf(),
            ));
        }
        validate_agent_ownership(&snapshot_file(path)?)
    }

    fn apply(&self, prepared: &PreparedPlan) -> Result<(), ClaudeCodeAdapterError> {
        if let Some((path, contents)) = &prepared.settings {
            create_parent(path)?;
            crate::storage::atomic_replace(path, contents).map_err(|source| {
                ClaudeCodeAdapterError::WriteConfiguration {
                    path: path.clone(),
                    source,
                }
            })?;
        }
        for operation in &prepared.files {
            match operation {
                PreparedFileOperation::Write { path, contents } => {
                    create_parent(path)?;
                    crate::storage::atomic_replace(path, contents).map_err(|source| {
                        ClaudeCodeAdapterError::WriteConfiguration {
                            path: path.clone(),
                            source,
                        }
                    })?;
                }
                PreparedFileOperation::Remove { path } => match fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(source) => {
                        return Err(ClaudeCodeAdapterError::WriteConfiguration {
                            path: path.clone(),
                            source,
                        });
                    }
                },
            }
        }
        Ok(())
    }

    fn rollback(&self, snapshots: &[FileSnapshot]) -> Result<(), ClaudeCodeAdapterError> {
        for snapshot in snapshots.iter().rev() {
            restore_snapshot(snapshot)?;
        }
        Ok(())
    }

    fn verify_plan(&self, plan: &ClaudeCodePlan) -> Result<(), ClaudeCodeAdapterError> {
        let settings_path = self.config_dir.join(SETTINGS_FILE);
        let settings = parse_settings(&snapshot_file(&settings_path)?)?;
        let environment = settings.get("env").and_then(serde_json::Value::as_object);
        for operation in plan.operations() {
            let verified = match operation {
                ClaudeCodeOperation::SetEnvironment { key, value } => {
                    environment_value(environment, key) == Some(value.as_str())
                }
                ClaudeCodeOperation::RemoveEnvironment { key } => {
                    environment_value(environment, key).is_none()
                }
                ClaudeCodeOperation::WriteFile { path, contents } => {
                    fs::read(path).is_ok_and(|actual| actual == contents.as_bytes())
                }
                ClaudeCodeOperation::RemoveFile { path } => !path.exists(),
                ClaudeCodeOperation::RestoreFile { path, contents } => match contents {
                    Some(contents) => {
                        fs::read(path).is_ok_and(|actual| actual.as_slice() == contents.as_slice())
                    }
                    None => !path.exists(),
                },
            };
            if !verified {
                let path = match operation {
                    ClaudeCodeOperation::SetEnvironment { .. }
                    | ClaudeCodeOperation::RemoveEnvironment { .. } => settings_path.clone(),
                    ClaudeCodeOperation::WriteFile { path, .. }
                    | ClaudeCodeOperation::RemoveFile { path }
                    | ClaudeCodeOperation::RestoreFile { path, .. } => path.clone(),
                };
                return Err(ClaudeCodeAdapterError::VerificationFailed(path));
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum ClaudeCodeAdapterError {
    InvalidGateway(String),
    InvalidWorkerCount {
        strategy: WorkerStrategy,
        actual: usize,
    },
    InvalidWorkerId(String),
    InvalidWorkerCapability {
        worker_id: String,
        capability: String,
    },
    InvalidModelSlot(String),
    InvalidRouteAlias(String),
    InvalidSelectorBinary(String),
    ApplyRollbackFailed {
        apply: Box<ClaudeCodeAdapterError>,
        rollback: Box<ClaudeCodeAdapterError>,
    },
    DuplicateWorkerId(String),
    DuplicateWorkerCapability {
        worker_id: String,
        capability: String,
    },
    DuplicateRouteAlias(String),
    InvalidSettings(PathBuf),
    InvalidSnapshot(PathBuf),
    SnapshotMissing(PathBuf),
    VerificationFailed(PathBuf),
    CliVersionFailed(PathBuf),
    CliTimedOut(PathBuf),
    AgentFileCollision(PathBuf),
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
    InspectCli {
        path: PathBuf,
        source: io::Error,
    },
}

impl Display for ClaudeCodeAdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidGateway(url) => write!(
                formatter,
                "Claude Code gateway must be an HTTP loopback URL: {url}"
            ),
            Self::InvalidWorkerCount { strategy, actual } => match strategy {
                WorkerStrategy::ForcedSingle => write!(
                    formatter,
                    "forced Worker mode requires exactly one Worker, got {actual}"
                ),
                WorkerStrategy::SelectablePool => write!(
                    formatter,
                    "selectable Worker mode requires at least one Worker, got {actual}"
                ),
            },
            Self::InvalidWorkerId(id) => {
                write!(formatter, "Worker id must be a lowercase slug: {id}")
            }
            Self::InvalidWorkerCapability {
                worker_id,
                capability,
            } => write!(
                formatter,
                "Worker {worker_id} capability must be a lowercase slug: {capability}"
            ),
            Self::InvalidModelSlot(slot) => {
                write!(formatter, "unsupported Claude Code model slot: {slot}")
            }
            Self::InvalidRouteAlias(alias) => write!(
                formatter,
                "Worker route alias must be a safe grillforge/ identifier: {alias}"
            ),
            Self::InvalidSelectorBinary(path) => write!(
                formatter,
                "GrillForge selector binary must use an absolute path: {path}"
            ),
            Self::ApplyRollbackFailed { apply, rollback } => {
                write!(formatter, "{apply}; rollback failed: {rollback}")
            }
            Self::DuplicateWorkerId(id) => write!(formatter, "duplicate Worker id: {id}"),
            Self::DuplicateWorkerCapability {
                worker_id,
                capability,
            } => write!(
                formatter,
                "Worker {worker_id} has duplicate capability: {capability}"
            ),
            Self::DuplicateRouteAlias(alias) => {
                write!(formatter, "duplicate Worker route alias: {alias}")
            }
            Self::InvalidSettings(path) => write!(
                formatter,
                "Claude Code settings must be a valid JSON object: {}",
                path.display()
            ),
            Self::InvalidSnapshot(path) => write!(
                formatter,
                "GrillForge Claude Code snapshot is invalid: {}",
                path.display()
            ),
            Self::SnapshotMissing(path) => write!(
                formatter,
                "GrillForge Claude Code snapshot does not exist: {}",
                path.display()
            ),
            Self::VerificationFailed(path) => write!(
                formatter,
                "Claude Code configuration verification failed: {}",
                path.display()
            ),
            Self::CliVersionFailed(path) => write!(
                formatter,
                "Claude Code CLI did not return a version: {}",
                path.display()
            ),
            Self::CliTimedOut(path) => write!(
                formatter,
                "Claude Code CLI version check timed out: {}",
                path.display()
            ),
            Self::AgentFileCollision(path) => write!(
                formatter,
                "refusing to replace a non-GrillForge Claude Agent: {}",
                path.display()
            ),
            Self::ReadConfiguration { path, source } => write!(
                formatter,
                "failed to read Claude Code configuration {}: {source}",
                path.display()
            ),
            Self::WriteConfiguration { path, source } => write!(
                formatter,
                "failed to write Claude Code configuration {}: {source}",
                path.display()
            ),
            Self::SerializeConfiguration { path, source } => write!(
                formatter,
                "failed to serialize Claude Code configuration {}: {source}",
                path.display()
            ),
            Self::InspectCli { path, source } => write!(
                formatter,
                "failed to inspect Claude Code CLI {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for ClaudeCodeAdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadConfiguration { source, .. }
            | Self::WriteConfiguration { source, .. }
            | Self::InspectCli { source, .. } => Some(source),
            Self::SerializeConfiguration { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn validate_gateway(value: &str) -> Result<(), ClaudeCodeAdapterError> {
    let url =
        Url::parse(value).map_err(|_| ClaudeCodeAdapterError::InvalidGateway(value.to_string()))?;
    let valid_scheme = matches!(url.scheme(), "http" | "https");
    let loopback = match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback()),
        None => false,
    };
    if !valid_scheme || !loopback {
        return Err(ClaudeCodeAdapterError::InvalidGateway(value.to_string()));
    }
    Ok(())
}

fn validate_worker_count(
    strategy: WorkerStrategy,
    actual: usize,
) -> Result<(), ClaudeCodeAdapterError> {
    let valid = match strategy {
        WorkerStrategy::ForcedSingle => actual == 1,
        WorkerStrategy::SelectablePool => actual >= 1,
    };
    if !valid {
        return Err(ClaudeCodeAdapterError::InvalidWorkerCount { strategy, actual });
    }
    Ok(())
}

fn push_restore_environment(
    operations: &mut Vec<ClaudeCodeOperation>,
    snapshot: &ClaudeCodeSnapshot,
    key: &str,
) {
    match snapshot.environment.get(key).cloned().flatten() {
        Some(value) => operations.push(ClaudeCodeOperation::SetEnvironment {
            key: key.to_string(),
            value,
        }),
        None => operations.push(ClaudeCodeOperation::RemoveEnvironment {
            key: key.to_string(),
        }),
    }
}

fn validate_workers(workers: &[WorkerModel]) -> Result<(), ClaudeCodeAdapterError> {
    let mut ids = HashSet::new();
    for worker in workers {
        if !is_slug(&worker.id) || AGENT_NAME_PREFIX.len() + worker.id.len() > 64 {
            return Err(ClaudeCodeAdapterError::InvalidWorkerId(worker.id.clone()));
        }
        if worker.route_alias != "inherit" && !is_route_alias(&worker.route_alias) {
            return Err(ClaudeCodeAdapterError::InvalidRouteAlias(
                worker.route_alias.clone(),
            ));
        }
        if !ids.insert(worker.id.as_str()) {
            return Err(ClaudeCodeAdapterError::DuplicateWorkerId(worker.id.clone()));
        }
        let mut capabilities = HashSet::new();
        for capability in &worker.capabilities {
            if !is_slug(capability) {
                return Err(ClaudeCodeAdapterError::InvalidWorkerCapability {
                    worker_id: worker.id.clone(),
                    capability: capability.clone(),
                });
            }
            if !capabilities.insert(capability) {
                return Err(ClaudeCodeAdapterError::DuplicateWorkerCapability {
                    worker_id: worker.id.clone(),
                    capability: capability.clone(),
                });
            }
        }
    }
    Ok(())
}

fn model_slot_environment_key(slot: &str) -> Option<&'static str> {
    MODEL_SLOTS
        .iter()
        .find_map(|(candidate, key)| (*candidate == slot).then_some(*key))
}

fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn is_route_alias(value: &str) -> bool {
    let Some(route) = value.strip_prefix("grillforge/") else {
        return false;
    };
    !route.is_empty()
        && !route.ends_with('/')
        && !route.contains("//")
        && route.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b'.' | b'/')
        })
}

fn snapshot_file(path: &Path) -> Result<FileSnapshot, ClaudeCodeAdapterError> {
    match fs::read(path) {
        Ok(contents) => Ok(FileSnapshot {
            path: path.to_path_buf(),
            contents: Some(contents),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(FileSnapshot {
            path: path.to_path_buf(),
            contents: None,
        }),
        Err(source) => Err(ClaudeCodeAdapterError::ReadConfiguration {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn restore_snapshot(snapshot: &FileSnapshot) -> Result<(), ClaudeCodeAdapterError> {
    match &snapshot.contents {
        Some(contents) => {
            create_parent(&snapshot.path)?;
            crate::storage::atomic_replace(&snapshot.path, contents).map_err(|source| {
                ClaudeCodeAdapterError::WriteConfiguration {
                    path: snapshot.path.clone(),
                    source,
                }
            })
        }
        None => match fs::remove_file(&snapshot.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(ClaudeCodeAdapterError::WriteConfiguration {
                path: snapshot.path.clone(),
                source,
            }),
        },
    }
}

fn combine_rollback(
    apply: ClaudeCodeAdapterError,
    rollback: Result<(), ClaudeCodeAdapterError>,
) -> ClaudeCodeAdapterError {
    match rollback {
        Ok(()) => apply,
        Err(rollback) => ClaudeCodeAdapterError::ApplyRollbackFailed {
            apply: Box::new(apply),
            rollback: Box::new(rollback),
        },
    }
}

fn owned_agent_files(directory: &Path) -> Result<Vec<FileSnapshot>, ClaudeCodeAdapterError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ClaudeCodeAdapterError::ReadConfiguration {
                path: directory.to_path_buf(),
                source,
            });
        }
    };
    let mut owned = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ClaudeCodeAdapterError::ReadConfiguration {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let candidate = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(AGENT_NAME_PREFIX) && name.ends_with(".md"));
        if !candidate {
            continue;
        }
        let snapshot = snapshot_file(&path)?;
        if snapshot.contents.as_deref().is_some_and(|contents| {
            contents
                .windows(OWNERSHIP_MARKER.len())
                .any(|part| part == OWNERSHIP_MARKER)
        }) {
            owned.push(snapshot);
        }
    }
    owned.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(owned)
}

fn current_owned_agents(
    directory: &Path,
) -> Result<BTreeMap<String, String>, ClaudeCodeAdapterError> {
    owned_agent_files(directory)?
        .into_iter()
        .map(|file| {
            let name = file
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| ClaudeCodeAdapterError::AgentFileCollision(file.path.clone()))?
                .to_string();
            let contents = file
                .contents
                .as_deref()
                .and_then(|contents| std::str::from_utf8(contents).ok())
                .ok_or_else(|| ClaudeCodeAdapterError::AgentFileCollision(file.path.clone()))?
                .to_string();
            Ok((name, contents))
        })
        .collect()
}

fn environment_value<'a>(
    environment: Option<&'a serde_json::Map<String, serde_json::Value>>,
    key: &str,
) -> Option<&'a str> {
    environment
        .and_then(|environment| environment.get(key))
        .and_then(serde_json::Value::as_str)
}

fn original_environment<'a>(snapshot: &'a ClaudeCodeSnapshot, key: &str) -> Option<&'a str> {
    snapshot.environment.get(key).and_then(Option::as_deref)
}

fn parse_settings(
    snapshot: &FileSnapshot,
) -> Result<serde_json::Map<String, serde_json::Value>, ClaudeCodeAdapterError> {
    let Some(contents) = &snapshot.contents else {
        return Ok(serde_json::Map::new());
    };
    let mut settings = serde_json::from_slice::<serde_json::Value>(contents)
        .ok()
        .and_then(|settings| settings.as_object().cloned())
        .ok_or_else(|| ClaudeCodeAdapterError::InvalidSettings(snapshot.path.clone()))?;
    if let Some(environment) = settings.get("env") {
        let environment = environment
            .as_object()
            .ok_or_else(|| ClaudeCodeAdapterError::InvalidSettings(snapshot.path.clone()))?;
        if MANAGED_ENVIRONMENT_KEYS.iter().any(|key| {
            environment
                .get(*key)
                .is_some_and(|value| !value.is_string())
        }) {
            return Err(ClaudeCodeAdapterError::InvalidSettings(
                snapshot.path.clone(),
            ));
        }
    }
    Ok(std::mem::take(&mut settings))
}

fn capture_environment(
    settings: &serde_json::Map<String, serde_json::Value>,
) -> BTreeMap<String, Option<String>> {
    let environment = settings.get("env").and_then(serde_json::Value::as_object);
    MANAGED_ENVIRONMENT_KEYS
        .into_iter()
        .map(|key| {
            let value = environment
                .and_then(|environment| environment.get(key))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            (key.to_string(), value)
        })
        .collect()
}

fn validate_managed_environment_key(
    key: &str,
    settings_path: &Path,
) -> Result<(), ClaudeCodeAdapterError> {
    if MANAGED_ENVIRONMENT_KEYS.contains(&key) {
        Ok(())
    } else {
        Err(ClaudeCodeAdapterError::InvalidSettings(
            settings_path.to_path_buf(),
        ))
    }
}

fn environment_mut<'a>(
    settings: &'a mut serde_json::Map<String, serde_json::Value>,
    settings_path: &Path,
) -> Result<&'a mut serde_json::Map<String, serde_json::Value>, ClaudeCodeAdapterError> {
    if !settings.contains_key("env") {
        settings.insert(
            "env".to_string(),
            serde_json::Value::Object(serde_json::Map::new()),
        );
    }
    settings
        .get_mut("env")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| ClaudeCodeAdapterError::InvalidSettings(settings_path.to_path_buf()))
}

fn create_parent(path: &Path) -> Result<(), ClaudeCodeAdapterError> {
    let parent = path
        .parent()
        .ok_or_else(|| ClaudeCodeAdapterError::WriteConfiguration {
            path: path.to_path_buf(),
            source: io::Error::new(
                io::ErrorKind::InvalidInput,
                "configuration path has no parent",
            ),
        })?;
    fs::create_dir_all(parent).map_err(|source| ClaudeCodeAdapterError::WriteConfiguration {
        path: parent.to_path_buf(),
        source,
    })
}

fn capture_agent(
    snapshot: &mut ClaudeCodeSnapshot,
    file: &FileSnapshot,
) -> Result<(), ClaudeCodeAdapterError> {
    let name = file
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| is_managed_agent_name(name))
        .ok_or_else(|| ClaudeCodeAdapterError::AgentFileCollision(file.path.clone()))?;
    let contents = file
        .contents
        .as_deref()
        .map(|contents| {
            std::str::from_utf8(contents)
                .map(str::to_owned)
                .map_err(|_| ClaudeCodeAdapterError::AgentFileCollision(file.path.clone()))
        })
        .transpose()?;
    snapshot.agents.entry(name.to_string()).or_insert(contents);
    Ok(())
}

fn is_managed_agent_name(name: &str) -> bool {
    name.starts_with(AGENT_NAME_PREFIX)
        && name.ends_with(".md")
        && is_slug(
            name.strip_prefix(AGENT_NAME_PREFIX)
                .and_then(|name| name.strip_suffix(".md"))
                .unwrap_or_default(),
        )
}

fn validate_agent_ownership(snapshot: &FileSnapshot) -> Result<(), ClaudeCodeAdapterError> {
    if snapshot.contents.as_deref().is_some_and(|contents| {
        !contents
            .windows(OWNERSHIP_MARKER.len())
            .any(|part| part == OWNERSHIP_MARKER)
    }) {
        return Err(ClaudeCodeAdapterError::AgentFileCollision(
            snapshot.path.clone(),
        ));
    }
    Ok(())
}

fn render_agent(worker: &WorkerModel) -> String {
    let name = format!("{AGENT_NAME_PREFIX}{}", worker.id);
    let description = if worker.capabilities.is_empty() {
        format!("GrillForge worker {}", worker.id)
    } else {
        format!("GrillForge SubAgent for {}", worker.capabilities.join(", "))
    };
    format!(
        "---\nname: {name}\ndescription: {description}\nmodel: {}\n---\n<!-- Managed by GrillForge. -->\nExecute the delegated task and return a concise result.\n",
        worker.route_alias
    )
}
