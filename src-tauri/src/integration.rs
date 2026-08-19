use crate::adapters::claude_code::{
    ClaudeCodeAdapter, ClaudeCodeTakeoverStatus, ClaudeNativeModel, EnableRequest, MODEL_SLOT_IDS,
    detect_claude_cli, discover_claude_native_models,
};
use crate::application::ControlPlaneService;
use crate::application::ControlPlaneState;
use crate::gateway::GatewayStatus;
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
    pub native_model_slots: BTreeMap<String, String>,
    pub supported_model_slots: Vec<&'static str>,
    pub native_models: Vec<ClaudeNativeModel>,
    pub native_models_error: Option<String>,
    pub native_current_model: Option<String>,
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
    claude_config_root: PathBuf,
    claude_state_path: PathBuf,
    claude_desktop_cache: Option<PathBuf>,
    activated_this_session: AtomicBool,
}

impl IntegrationService {
    pub fn new(
        claude_config_root: impl Into<PathBuf>,
        grillforge_root: impl Into<PathBuf>,
    ) -> Self {
        let claude_config_root = claude_config_root.into();
        let grillforge_root = grillforge_root.into();
        Self {
            adapter: ClaudeCodeAdapter::new(&claude_config_root, &grillforge_root),
            claude_state_path: claude_config_root
                .parent()
                .unwrap_or(Path::new("."))
                .join(".claude.json"),
            claude_desktop_cache: claude_config_root
                .parent()
                .map(|home| home.join("Library/Application Support/Claude/Local Storage/leveldb")),
            claude_config_root,
            activated_this_session: AtomicBool::new(false),
        }
    }

    pub fn with_native_catalog_paths(
        mut self,
        state_path: impl Into<PathBuf>,
        desktop_cache: Option<PathBuf>,
    ) -> Self {
        self.claude_state_path = state_path.into();
        self.claude_desktop_cache = desktop_cache;
        self
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
        let request = match main {
            None => EnableRequest::native(),
            Some(main) => EnableRequest::managed_main_only(gateway_base_url, main),
        };
        let native_main = state.claude_native_model_slots.get("main").cloned();
        let native_slots = state
            .claude_native_model_slots
            .iter()
            .filter(|(slot, _)| slot.as_str() != "main")
            .map(|(slot, model)| (slot.clone(), model.clone()))
            .collect();
        let request = request
            .with_model_routes(gateway_base_url, model_routes)
            .with_native_models(native_main, native_slots);
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
                if state_uses_gateway(state) {
                    if let Some(native_base_url) = self.native_upstream_base_url()? {
                        gateway.set_native_base_url(&native_base_url)?;
                    } else {
                        gateway.use_official_native_base_url();
                    }
                    gateway.activate(state)?;
                }
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
        let (native_models, native_models_error, native_current_model) =
            match discover_claude_native_models(
                &self.claude_config_root,
                &self.claude_state_path,
                self.claude_desktop_cache.as_deref(),
            ) {
                Ok(catalog) => (catalog.models, None, catalog.cli_current_model),
                Err(error) => (Vec::new(), Some(error.to_string()), None),
            };
        Ok(IntegrationStatus {
            snapshot_present: status.snapshot_present,
            takeover,
            differences: status.differences,
            managed_main_alias: status.managed_main_alias,
            native_model_slots: status.native_model_slots,
            supported_model_slots: MODEL_SLOT_IDS.to_vec(),
            native_models,
            native_models_error,
            native_current_model,
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

fn state_uses_gateway(state: &ControlPlaneState) -> bool {
    state.main_model_id.is_some() || !state.model_slots.is_empty()
}

pub fn default_claude_config_root(home: &Path) -> PathBuf {
    home.join(".claude")
}

#[tauri::command]
pub fn integration_status(
    integration: State<'_, IntegrationService>,
) -> Result<IntegrationStatus, String> {
    integration.status()
}

#[tauri::command]
pub fn detect_claude_code() -> Result<ClaudeCliStatus, String> {
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
    let state = control_plane.state()?;
    if state_uses_gateway(&state) {
        if let Some(native_base_url) = integration.native_upstream_base_url()? {
            gateway.set_native_base_url(&native_base_url)?;
        } else {
            gateway.use_official_native_base_url();
        }
    }
    let status = integration.apply(&state, &gateway.base_url)?;
    if state_uses_gateway(&state) {
        gateway.activate(&state).map_err(|error| {
            let restore = integration.disable();
            match restore {
                Ok(_) => error,
                Err(restore_error) => {
                    format!("{error}; Claude Code restore also failed: {restore_error}")
                }
            }
        })?;
    } else {
        gateway.deactivate();
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
