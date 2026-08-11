use crate::adapters::claude_code::{
    ClaudeCodeAdapter, ClaudeCodeTakeoverStatus, EnableRequest, MODEL_SLOT_IDS, WorkerModel,
    WorkerStrategy, detect_claude_cli,
};
use crate::application::ControlPlaneService;
use crate::application::ControlPlaneState;
use crate::gateway::GatewayStatus;
use crate::skills::SkillInstaller;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::State;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationTakeover {
    Inactive,
    Active,
    ReapplyRequired,
    Drifted,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationStatus {
    pub snapshot_present: bool,
    pub takeover: IntegrationTakeover,
    pub differences: Vec<String>,
    pub managed_main_alias: Option<String>,
    pub forced_worker_alias: Option<String>,
    pub generated_agent_names: Vec<String>,
    pub selector_skill_installed: bool,
    pub supported_model_slots: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCliStatus {
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
}

pub struct IntegrationService {
    adapter: ClaudeCodeAdapter,
    skill_root: PathBuf,
    activated_this_session: AtomicBool,
}

impl IntegrationService {
    pub fn new(
        claude_config_root: impl Into<PathBuf>,
        grillforge_root: impl Into<PathBuf>,
    ) -> Self {
        let claude_config_root = claude_config_root.into();
        Self {
            adapter: ClaudeCodeAdapter::new(&claude_config_root, grillforge_root),
            skill_root: claude_config_root.join("skills"),
            activated_this_session: AtomicBool::new(false),
        }
    }

    pub fn apply(
        &self,
        state: &ControlPlaneState,
        gateway_base_url: &str,
    ) -> Result<IntegrationStatus, String> {
        let main = state
            .main_model_id
            .as_ref()
            .map(|id| route_for_model(state, id))
            .transpose()?;
        let model_routes = state
            .model_slots
            .iter()
            .map(|(slot, id)| route_for_model(state, id).map(|route| (slot.clone(), route)))
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let mut workers = if state.subagents.is_empty() && state.worker_mode {
            state
                .models
                .iter()
                .filter(|model| model.worker_enabled)
                .map(|model| WorkerModel::new(&model.id, &model.route_alias))
                .collect::<Vec<_>>()
        } else {
            state
                .subagents
                .iter()
                .filter(|subagent| subagent.enabled)
                .map(|subagent| {
                    WorkerModel::new(&subagent.id, format!("grillforge/{}", subagent.model_id))
                        .with_capabilities(subagent.capabilities.clone())
                })
                .collect::<Vec<_>>()
        };
        if state.native_subagent_enabled
            && (main.is_some() || !model_routes.is_empty() || !workers.is_empty())
        {
            workers.push(WorkerModel::native_default());
        }

        if !workers.is_empty() {
            SkillInstaller::install(&self.skill_root).map_err(|error| error.to_string())?;
        }
        let selector_binary = if workers.is_empty() {
            None
        } else {
            Some(
                std::env::current_exe()
                    .map_err(|error| format!("could not locate GrillForge executable: {error}"))?
                    .display()
                    .to_string(),
            )
        };

        let request = match (main, workers.len()) {
            (None, 0) => EnableRequest::native_main_without_workers(),
            (Some(main), 0) => EnableRequest::managed_main_only(gateway_base_url, main),
            (None, _) => EnableRequest::native_main(
                gateway_base_url,
                workers,
                WorkerStrategy::SelectablePool,
            ),
            (Some(main), _) => EnableRequest::managed_main(
                gateway_base_url,
                main,
                workers,
                WorkerStrategy::SelectablePool,
            ),
        };
        let request = request.with_model_routes(gateway_base_url, model_routes);
        let request = match selector_binary {
            Some(path) => request.with_selector_binary(path),
            None => request,
        };
        self.adapter
            .enable(request)
            .map_err(|error| error.to_string())?;
        self.activated_this_session.store(true, Ordering::Release);
        self.status()
    }

    pub fn recovery_pending(&self) -> bool {
        self.adapter.snapshot_path().is_file()
    }

    pub fn resume_if_applied(
        &self,
        state: &ControlPlaneState,
        gateway: &GatewayStatus,
    ) -> Result<bool, String> {
        match self
            .adapter
            .status()
            .map_err(|error| error.to_string())?
            .takeover
        {
            ClaudeCodeTakeoverStatus::Inactive | ClaudeCodeTakeoverStatus::Drifted => Ok(false),
            ClaudeCodeTakeoverStatus::Active => {
                if let Some(native_base_url) = self.native_upstream_base_url()? {
                    gateway.set_native_base_url(&native_base_url)?;
                } else {
                    gateway.use_official_native_base_url();
                }
                gateway.activate(state)?;
                self.activated_this_session.store(true, Ordering::Release);
                Ok(true)
            }
        }
    }

    pub fn native_upstream_base_url(&self) -> Result<Option<String>, String> {
        self.adapter
            .native_upstream_base_url()
            .map_err(|error| error.to_string())
    }

    pub fn status(&self) -> Result<IntegrationStatus, String> {
        let status = self.adapter.status().map_err(|error| error.to_string())?;
        let takeover = match status.takeover {
            ClaudeCodeTakeoverStatus::Inactive => IntegrationTakeover::Inactive,
            ClaudeCodeTakeoverStatus::Drifted => IntegrationTakeover::Drifted,
            ClaudeCodeTakeoverStatus::Active
                if status.snapshot_present
                    && !self.activated_this_session.load(Ordering::Acquire) =>
            {
                IntegrationTakeover::ReapplyRequired
            }
            ClaudeCodeTakeoverStatus::Active => IntegrationTakeover::Active,
        };
        Ok(IntegrationStatus {
            snapshot_present: status.snapshot_present,
            takeover,
            differences: status.differences,
            managed_main_alias: status.managed_main_alias,
            forced_worker_alias: status.forced_worker_alias,
            generated_agent_names: status.generated_agent_names,
            selector_skill_installed: self
                .skill_root
                .join("grillforge-model-selector/SKILL.md")
                .is_file(),
            supported_model_slots: MODEL_SLOT_IDS.to_vec(),
        })
    }

    pub fn disable(&self) -> Result<IntegrationStatus, String> {
        let current = self.status()?;
        match (current.snapshot_present, current.takeover) {
            (true, _) => self.adapter.disable().map_err(|error| error.to_string())?,
            (false, IntegrationTakeover::Inactive) => return Ok(current),
            (false, IntegrationTakeover::Drifted) => {
                return Err(
                    "Claude Code contains unmanaged GrillForge routes without a recovery snapshot"
                        .to_string(),
                );
            }
            (false, IntegrationTakeover::Active) => {
                return Err("Claude Code active state is missing its recovery snapshot".to_string());
            }
            (false, IntegrationTakeover::ReapplyRequired) => {
                return Err(
                    "Claude Code reapply state is missing its recovery snapshot".to_string()
                );
            }
        }
        self.activated_this_session.store(false, Ordering::Release);
        self.status()
    }
}

fn route_for_model(state: &ControlPlaneState, id: &str) -> Result<String, String> {
    state
        .models
        .iter()
        .find(|model| model.id == id)
        .map(|model| model.route_alias.clone())
        .ok_or_else(|| format!("selected model does not exist: {id}"))
}

pub fn default_claude_config_root(home: &Path) -> PathBuf {
    home.join(".claude")
}

#[tauri::command]
pub async fn integration_status(
    integration: State<'_, IntegrationService>,
) -> Result<IntegrationStatus, String> {
    integration.status()
}

#[tauri::command]
pub async fn detect_claude_code() -> Result<ClaudeCliStatus, String> {
    let detection = detect_claude_cli().map_err(|error| error.to_string())?;
    Ok(match detection {
        Some(detection) => ClaudeCliStatus {
            installed: true,
            path: Some(detection.path.display().to_string()),
            version: Some(detection.version),
        },
        None => ClaudeCliStatus {
            installed: false,
            path: None,
            version: None,
        },
    })
}

#[tauri::command]
pub fn apply_claude_code(
    integration: State<'_, IntegrationService>,
    control_plane: State<'_, ControlPlaneService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<IntegrationStatus, String> {
    if let Some(native_base_url) = integration.native_upstream_base_url()? {
        gateway.set_native_base_url(&native_base_url)?;
    } else {
        gateway.use_official_native_base_url();
    }
    let state = control_plane.state()?;
    let status = integration.apply(&state, &gateway.base_url)?;
    if let Err(error) = gateway.activate(&state) {
        let restore = integration.disable();
        return Err(match restore {
            Ok(_) => error,
            Err(restore_error) => {
                format!("{error}; Claude Code restore also failed: {restore_error}")
            }
        });
    }
    if let Err(error) = control_plane.set_client_integration_enabled("claude_code", true) {
        gateway.deactivate();
        let restore = integration.disable();
        return Err(match restore {
            Ok(_) => error,
            Err(restore_error) => {
                format!("{error}; Claude Code restore also failed: {restore_error}")
            }
        });
    }
    Ok(status)
}

#[tauri::command]
pub fn disable_claude_code(
    integration: State<'_, IntegrationService>,
    control_plane: State<'_, ControlPlaneService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<IntegrationStatus, String> {
    let status = integration.disable()?;
    gateway.deactivate();
    control_plane.set_client_integration_enabled("claude_code", false)?;
    Ok(status)
}
