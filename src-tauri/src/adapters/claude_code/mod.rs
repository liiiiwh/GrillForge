use std::collections::BTreeMap;
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
const SNAPSHOT_FILE: &str = "claude-code.snapshot.json";
const MANAGED_ENVIRONMENT_KEYS: [&str; 7] = [
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
];
pub const MODEL_SLOT_IDS: [&str; 5] = ["sonnet", "opus", "fable", "haiku", "subagent_default"];
const MODEL_SLOTS: [(&str, &str); 5] = [
    ("sonnet", "ANTHROPIC_DEFAULT_SONNET_MODEL"),
    ("opus", "ANTHROPIC_DEFAULT_OPUS_MODEL"),
    ("fable", "ANTHROPIC_DEFAULT_FABLE_MODEL"),
    ("haiku", "ANTHROPIC_DEFAULT_HAIKU_MODEL"),
    ("subagent_default", "CLAUDE_CODE_SUBAGENT_MODEL"),
];
const CLI_VERSION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnableRequest {
    gateway_base_url: String,
    main_route: Option<String>,
    model_routes: BTreeMap<String, String>,
    native_main_model: Option<String>,
    native_model_slots: BTreeMap<String, String>,
}

impl EnableRequest {
    pub fn managed_main_only(
        gateway_base_url: impl Into<String>,
        main_route: impl Into<String>,
    ) -> Self {
        Self {
            gateway_base_url: gateway_base_url.into(),
            main_route: Some(main_route.into()),
            model_routes: BTreeMap::new(),
            native_main_model: None,
            native_model_slots: BTreeMap::new(),
        }
    }

    pub fn native() -> Self {
        Self {
            gateway_base_url: String::new(),
            main_route: None,
            model_routes: BTreeMap::new(),
            native_main_model: None,
            native_model_slots: BTreeMap::new(),
        }
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

    pub fn with_native_models(
        mut self,
        main: Option<String>,
        slots: BTreeMap<String, String>,
    ) -> Self {
        self.native_main_model = main;
        self.native_model_slots = slots;
        self
    }

    fn takes_over(&self) -> bool {
        self.uses_gateway()
            || self.native_main_model.is_some()
            || !self.native_model_slots.is_empty()
    }

    fn uses_gateway(&self) -> bool {
        self.main_route.is_some() || !self.model_routes.is_empty()
    }
}

impl ActiveConfiguration {
    fn from_request(request: &EnableRequest) -> Self {
        Self {
            base_url: request.gateway_base_url.clone(),
            main_route: request.main_route.clone(),
            model_routes: request.model_routes.clone(),
            native_main_model: request.native_main_model.clone(),
            native_model_slots: request.native_model_slots.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeCodeOperation {
    SetModel { value: String },
    RemoveModel,
    SetEnvironment { key: String, value: String },
    RemoveEnvironment { key: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeCodeSnapshot {
    version: u8,
    settings: Option<Vec<u8>>,
    environment: BTreeMap<String, Option<String>>,
    active: ActiveConfiguration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ActiveConfiguration {
    base_url: String,
    main_route: Option<String>,
    #[serde(default)]
    model_routes: BTreeMap<String, String>,
    #[serde(default)]
    native_main_model: Option<String>,
    #[serde(default)]
    native_model_slots: BTreeMap<String, String>,
}

impl ActiveConfiguration {
    fn uses_gateway(&self) -> bool {
        self.main_route.is_some() || !self.model_routes.is_empty()
    }
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
    pub native_model_slots: BTreeMap<String, String>,
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
    rollback: Vec<FileSnapshot>,
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
        let current_settings_model = settings.get("model").and_then(serde_json::Value::as_str);
        let current_model_routes = MODEL_SLOTS
            .into_iter()
            .map(|(slot, key)| (slot, environment_value(environment, key)))
            .collect::<BTreeMap<_, _>>();
        let managed_main_alias = current_main.filter(|alias| is_route_alias(alias));
        let mut native_model_slots = current_model_routes
            .iter()
            .filter_map(|(slot, model)| {
                model
                    .filter(|model| !is_route_alias(model))
                    .map(|model| ((*slot).to_string(), model.to_string()))
            })
            .collect::<BTreeMap<_, _>>();
        if let Some(model) = current_settings_model {
            native_model_slots.insert("main".into(), model.into());
        }

        let snapshot = self.read_snapshot()?;
        let mut differences = Vec::new();
        let takeover = match &snapshot {
            Some(snapshot) => {
                let original_settings_model = original_settings_model(snapshot)?;
                let expected_settings_model = snapshot
                    .active
                    .native_main_model
                    .as_deref()
                    .or(original_settings_model.as_deref());
                if current_settings_model != expected_settings_model {
                    differences.push("model".into());
                }
                let expected_main = snapshot
                    .active
                    .main_route
                    .as_deref()
                    .or_else(|| original_environment(snapshot, "ANTHROPIC_MODEL"));
                for (slot, key) in MODEL_SLOTS {
                    let expected = snapshot
                        .active
                        .model_routes
                        .get(slot)
                        .map(String::as_str)
                        .or_else(|| {
                            snapshot
                                .active
                                .native_model_slots
                                .get(slot)
                                .map(String::as_str)
                        })
                        .or_else(|| original_environment(snapshot, key));
                    if current_model_routes.get(slot).copied().flatten() != expected {
                        differences.push(key.to_string());
                    }
                }
                let expected_base_url = snapshot
                    .active
                    .uses_gateway()
                    .then_some(snapshot.active.base_url.as_str())
                    .or_else(|| original_environment(snapshot, "ANTHROPIC_BASE_URL"));
                if current_base_url != expected_base_url {
                    differences.push("ANTHROPIC_BASE_URL".into());
                }
                if current_main != expected_main {
                    differences.push("ANTHROPIC_MODEL".into());
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
                    || current_model_routes
                        .values()
                        .flatten()
                        .any(|alias| is_route_alias(alias));
                if has_managed_configuration {
                    if managed_main_alias.is_some() {
                        differences.push("ANTHROPIC_MODEL".into());
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
            native_model_slots,
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
        let settings_path = self.config_dir.join(SETTINGS_FILE);
        let current = snapshot_file(&settings_path)?;
        let original = FileSnapshot {
            path: settings_path.clone(),
            contents: snapshot.settings.clone(),
        };
        if let Err(error) = restore_snapshot(&original).and_then(|_| {
            if snapshot_file(&settings_path)?.contents == original.contents {
                Ok(())
            } else {
                Err(ClaudeCodeAdapterError::VerificationFailed(settings_path))
            }
        }) {
            let rollback = restore_snapshot(&current);
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
        if request.uses_gateway() {
            validate_gateway(&request.gateway_base_url)?;
        }
        if request.main_route.is_some() && request.native_main_model.is_some() {
            return Err(ClaudeCodeAdapterError::ConflictingModelSelection(
                "main".into(),
            ));
        }
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
        if let Some(model) = request.native_main_model.as_deref() {
            validate_native_model(model)?;
        }
        for (slot, model) in &request.native_model_slots {
            if model_slot_environment_key(slot).is_none() {
                return Err(ClaudeCodeAdapterError::InvalidModelSlot(slot.clone()));
            }
            if request.model_routes.contains_key(slot) {
                return Err(ClaudeCodeAdapterError::ConflictingModelSelection(
                    slot.clone(),
                ));
            }
            validate_native_model(model)?;
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
                version: 2,
                settings: settings_snapshot.contents.clone(),
                environment: capture_environment(&settings),
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
        let mut operations = Vec::new();
        if request.uses_gateway() {
            operations.push(ClaudeCodeOperation::SetEnvironment {
                key: "ANTHROPIC_BASE_URL".to_string(),
                value: request.gateway_base_url,
            });
        } else if had_snapshot {
            push_restore_environment(&mut operations, &snapshot, "ANTHROPIC_BASE_URL");
        }
        match request.native_main_model {
            Some(model) => operations.push(ClaudeCodeOperation::SetModel { value: model }),
            None if had_snapshot => push_restore_model(&mut operations, &snapshot)?,
            None => {}
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
                None => match request.native_model_slots.get(slot) {
                    Some(model) => operations.push(ClaudeCodeOperation::SetEnvironment {
                        key: key.to_string(),
                        value: model.clone(),
                    }),
                    None if had_snapshot => {
                        push_restore_environment(&mut operations, &snapshot, key)
                    }
                    None => {}
                },
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
        ClaudeCodePlan {
            operations,
            snapshot: None,
        }
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
        if snapshot.version != 2
            || (snapshot.active.uses_gateway()
                && validate_gateway(&snapshot.active.base_url).is_err())
            || snapshot
                .active
                .main_route
                .as_deref()
                .is_some_and(|alias| !is_route_alias(alias))
            || snapshot.active.model_routes.iter().any(|(slot, alias)| {
                model_slot_environment_key(slot).is_none() || !is_route_alias(alias)
            })
            || snapshot
                .active
                .native_main_model
                .as_deref()
                .is_some_and(|model| validate_native_model(model).is_err())
            || snapshot
                .active
                .native_model_slots
                .iter()
                .any(|(slot, model)| {
                    model_slot_environment_key(slot).is_none()
                        || validate_native_model(model).is_err()
                        || snapshot.active.model_routes.contains_key(slot)
                })
            || snapshot
                .environment
                .keys()
                .any(|key| !MANAGED_ENVIRONMENT_KEYS.contains(&key.as_str()))
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
        for operation in plan.operations() {
            match operation {
                ClaudeCodeOperation::SetModel { value } => {
                    settings.insert("model".into(), serde_json::Value::String(value.clone()));
                    changes_settings = true;
                }
                ClaudeCodeOperation::RemoveModel => {
                    settings.remove("model");
                    changes_settings = true;
                }
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
        let rollback = changes_settings
            .then_some(settings_snapshot)
            .into_iter()
            .collect();
        Ok(PreparedPlan { settings, rollback })
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
                ClaudeCodeOperation::SetModel { value } => {
                    settings.get("model").and_then(serde_json::Value::as_str)
                        == Some(value.as_str())
                }
                ClaudeCodeOperation::RemoveModel => !settings.contains_key("model"),
                ClaudeCodeOperation::SetEnvironment { key, value } => {
                    environment_value(environment, key) == Some(value.as_str())
                }
                ClaudeCodeOperation::RemoveEnvironment { key } => {
                    environment_value(environment, key).is_none()
                }
            };
            if !verified {
                let path = match operation {
                    ClaudeCodeOperation::SetModel { .. }
                    | ClaudeCodeOperation::RemoveModel
                    | ClaudeCodeOperation::SetEnvironment { .. }
                    | ClaudeCodeOperation::RemoveEnvironment { .. } => settings_path.clone(),
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
    InvalidModelSlot(String),
    InvalidRouteAlias(String),
    InvalidNativeModel(String),
    ConflictingModelSelection(String),
    ApplyRollbackFailed {
        apply: Box<ClaudeCodeAdapterError>,
        rollback: Box<ClaudeCodeAdapterError>,
    },
    InvalidSettings(PathBuf),
    InvalidSnapshot(PathBuf),
    SnapshotMissing(PathBuf),
    VerificationFailed(PathBuf),
    CliVersionFailed(PathBuf),
    CliTimedOut(PathBuf),
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
            Self::InvalidModelSlot(slot) => {
                write!(formatter, "unsupported Claude Code model slot: {slot}")
            }
            Self::InvalidRouteAlias(alias) => write!(
                formatter,
                "model route alias must be a safe grillforge/ identifier: {alias}"
            ),
            Self::InvalidNativeModel(model) => {
                write!(formatter, "unsupported Claude Code native model: {model}")
            }
            Self::ConflictingModelSelection(slot) => write!(
                formatter,
                "Claude Code model slot cannot be both native and managed: {slot}"
            ),
            Self::ApplyRollbackFailed { apply, rollback } => {
                write!(formatter, "{apply}; rollback failed: {rollback}")
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

fn push_restore_model(
    operations: &mut Vec<ClaudeCodeOperation>,
    snapshot: &ClaudeCodeSnapshot,
) -> Result<(), ClaudeCodeAdapterError> {
    match original_settings_model(snapshot)? {
        Some(value) => operations.push(ClaudeCodeOperation::SetModel { value }),
        None => operations.push(ClaudeCodeOperation::RemoveModel),
    }
    Ok(())
}

fn original_settings_model(
    snapshot: &ClaudeCodeSnapshot,
) -> Result<Option<String>, ClaudeCodeAdapterError> {
    let Some(contents) = snapshot.settings.as_deref() else {
        return Ok(None);
    };
    let settings = serde_json::from_slice::<serde_json::Value>(contents)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| ClaudeCodeAdapterError::InvalidSnapshot(PathBuf::from(SNAPSHOT_FILE)))?;
    match settings.get("model") {
        Some(serde_json::Value::String(model)) => Ok(Some(model.clone())),
        None => Ok(None),
        Some(_) => Err(ClaudeCodeAdapterError::InvalidSnapshot(PathBuf::from(
            SNAPSHOT_FILE,
        ))),
    }
}

fn validate_native_model(value: &str) -> Result<(), ClaudeCodeAdapterError> {
    if matches!(value, "default" | "sonnet" | "opus" | "fable" | "haiku") {
        Ok(())
    } else {
        Err(ClaudeCodeAdapterError::InvalidNativeModel(
            value.to_string(),
        ))
    }
}

fn model_slot_environment_key(slot: &str) -> Option<&'static str> {
    MODEL_SLOTS
        .iter()
        .find_map(|(candidate, key)| (*candidate == slot).then_some(*key))
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
