use crate::application::{
    ControlPlaneService, ControlPlaneState, ExtensionSubAgentInput, PublicExtensionSubAgent,
};
use crate::gateway::{AgentRuntimeRoute, GatewayStatus};
use crate::local_agents::{discover_claude_builtin_agents, discover_claude_code_agents};
use crate::mcp_mount::McpMountManager;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

pub struct ExtensionIntegrationService {
    mounts: McpMountManager,
    claude_root: PathBuf,
    claude_runtime: Option<PathBuf>,
    pi_settings_path: Option<PathBuf>,
    tokens: Mutex<HashMap<String, String>>,
    operation_lock: Mutex<()>,
}

impl ExtensionIntegrationService {
    pub fn new(
        mounts: McpMountManager,
        claude_root: impl Into<PathBuf>,
        claude_runtime: Option<PathBuf>,
        pi_settings_path: Option<PathBuf>,
    ) -> Self {
        Self {
            mounts,
            claude_root: claude_root.into(),
            claude_runtime,
            pi_settings_path,
            tokens: Mutex::new(HashMap::new()),
            operation_lock: Mutex::new(()),
        }
    }

    pub fn set_binding(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
        client_id: &str,
        extension_id: &str,
        enabled: bool,
    ) -> Result<ControlPlaneState, String> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| "live configuration operation lock is poisoned".to_string())?;
        let previous = control.state()?;
        let was_enabled = previous
            .client_extension_subagent_ids
            .get(client_id)
            .is_some_and(|ids| ids.iter().any(|id| id == extension_id));
        if was_enabled == enabled {
            self.reconcile_client(&previous, gateway, client_id)?;
            return Ok(previous);
        }
        let updated =
            control.set_client_extension_subagent_enabled(client_id, extension_id, enabled)?;
        if let Err(error) = self.reconcile_client(&updated, gateway, client_id) {
            let rollback = control
                .set_client_extension_subagent_enabled(client_id, extension_id, was_enabled)
                .and_then(|restored| {
                    self.reconcile_client(&restored, gateway, client_id)?;
                    Ok(restored)
                });
            return Err(match rollback {
                Ok(_) => error,
                Err(rollback_error) => {
                    format!("{error}; extension binding rollback failed: {rollback_error}")
                }
            });
        }
        Ok(updated)
    }

    pub fn update_extension(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
        input: ExtensionSubAgentInput,
    ) -> Result<ControlPlaneState, String> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| "live configuration operation lock is poisoned".to_string())?;
        let previous = control.state()?;
        let original = previous
            .extension_subagents
            .iter()
            .find(|extension| extension.id == input.id)
            .cloned()
            .ok_or_else(|| format!("unknown extension SubAgent: {}", input.id))?;
        let bound_clients = bound_clients(&previous, &input.id);
        let updated = control.update_extension_subagent(input)?;
        if let Err(error) = self.reconcile_clients(&updated, gateway, &bound_clients) {
            let rollback = control
                .update_extension_subagent(extension_input(original))
                .and_then(|restored| {
                    self.reconcile_clients(&restored, gateway, &bound_clients)?;
                    Ok(restored)
                });
            return Err(match rollback {
                Ok(_) => error,
                Err(rollback_error) => {
                    format!("{error}; extension update rollback failed: {rollback_error}")
                }
            });
        }
        Ok(updated)
    }

    pub fn reconcile_all(
        &self,
        state: &ControlPlaneState,
        gateway: &GatewayStatus,
    ) -> Result<(), String> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| "live configuration operation lock is poisoned".to_string())?;
        self.reconcile_all_unlocked(state, gateway)
    }

    pub fn reconcile_all_from_control(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<(), String> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| "live configuration operation lock is poisoned".to_string())?;
        let state = control.state()?;
        self.reconcile_all_unlocked(&state, gateway)
    }

    fn reconcile_all_unlocked(
        &self,
        state: &ControlPlaneState,
        gateway: &GatewayStatus,
    ) -> Result<(), String> {
        for client_id in self.mounts.supported_clients() {
            self.reconcile_client(state, gateway, &client_id)?;
        }
        Ok(())
    }

    fn reconcile_clients(
        &self,
        state: &ControlPlaneState,
        gateway: &GatewayStatus,
        client_ids: &[String],
    ) -> Result<(), String> {
        for client_id in client_ids {
            self.reconcile_client(state, gateway, client_id)?;
        }
        Ok(())
    }

    pub fn restore_live_mounts(&self, gateway: &GatewayStatus) -> Result<(), String> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| "live configuration operation lock is poisoned".to_string())?;
        self.restore_live_mounts_unlocked(gateway)
    }

    pub fn restore_clients_then_reconcile<T>(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| "live configuration operation lock is poisoned".to_string())?;
        self.restore_live_mounts_unlocked(gateway)?;
        let result = operation();
        let reconcile = control
            .state()
            .and_then(|state| self.reconcile_all_unlocked(&state, gateway));
        match (result, reconcile) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(reconcile_error)) => Err(format!(
                "{error}; extension MCP startup restore also failed: {reconcile_error}"
            )),
        }
    }

    fn restore_live_mounts_unlocked(&self, gateway: &GatewayStatus) -> Result<(), String> {
        for client_id in self.mounts.supported_clients() {
            gateway.deactivate_client_agent_broker(&client_id);
            self.mounts.unmount(&client_id)?;
        }
        self.tokens
            .lock()
            .map_err(|_| "extension MCP token lock is poisoned".to_string())?
            .clear();
        Ok(())
    }

    pub fn with_suspended_client<T>(
        &self,
        state: &ControlPlaneState,
        gateway: &GatewayStatus,
        client_id: &str,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| "live configuration operation lock is poisoned".to_string())?;
        self.suspend_client(gateway, client_id)?;
        let result = operation();
        let reconcile = self.reconcile_client(state, gateway, client_id);
        match (result, reconcile) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(reconcile_error)) => Err(format!(
                "{error}; extension MCP restore also failed: {reconcile_error}"
            )),
        }
    }

    fn suspend_client(&self, gateway: &GatewayStatus, client_id: &str) -> Result<(), String> {
        if !self
            .mounts
            .supported_clients()
            .iter()
            .any(|candidate| candidate == client_id)
        {
            return Err(format!(
                "client {client_id} does not provide a verified MCP configuration format"
            ));
        }
        gateway.deactivate_client_agent_broker(client_id);
        self.mounts.unmount(client_id)?;
        self.tokens
            .lock()
            .map_err(|_| "extension MCP token lock is poisoned".to_string())?
            .remove(client_id);
        Ok(())
    }

    pub fn reconcile_client(
        &self,
        state: &ControlPlaneState,
        gateway: &GatewayStatus,
        client_id: &str,
    ) -> Result<(), String> {
        if !self
            .mounts
            .supported_clients()
            .iter()
            .any(|candidate| candidate == client_id)
        {
            return Err(format!(
                "client {client_id} does not provide a verified MCP configuration format"
            ));
        }
        let ids = state
            .client_extension_subagent_ids
            .get(client_id)
            .cloned()
            .unwrap_or_default();
        if ids.is_empty() {
            gateway.deactivate_client_agent_broker(client_id);
            self.mounts.unmount(client_id)?;
            self.tokens
                .lock()
                .map_err(|_| "extension MCP token lock is poisoned".to_string())?
                .remove(client_id);
            return Ok(());
        }
        if client_id == "pi" {
            let settings = self
                .pi_settings_path
                .as_deref()
                .ok_or_else(|| "Pi MCP extension settings path is not configured".to_string())?;
            if !crate::mcp_mount::pi_mcp_extension_installed(settings)? {
                return Err(
                    "Pi needs pi-mcp-extension before it can use extension SubAgents".into(),
                );
            }
        }
        let mut discovered = discover_claude_code_agents(&self.claude_root)?
            .into_iter()
            .map(|agent| agent.agent_id)
            .collect::<HashSet<_>>();
        let mut routes = Vec::with_capacity(ids.len());
        for id in ids {
            let extension = state
                .extension_subagents
                .iter()
                .find(|extension| extension.id == id)
                .ok_or_else(|| format!("unknown extension SubAgent: {id}"))?;
            if extension.source_client_id != "claude_code" {
                return Err(format!(
                    "extension SubAgent {} uses an unsupported source client: {}",
                    extension.id, extension.source_client_id
                ));
            }
            if !discovered.contains(&extension.source_agent_id) {
                let runtime = self.claude_runtime.clone().or_else(|| {
                    crate::adapters::claude_code::detect_claude_cli()
                        .ok()
                        .flatten()
                        .map(|detection| detection.path)
                });
                let builtins = runtime
                    .as_deref()
                    .map(discover_claude_builtin_agents)
                    .transpose()
                    .map_err(|error| {
                        format!(
                            "extension SubAgent {} source Agent does not exist or could not be verified: {}; {error}",
                            extension.id, extension.source_agent_id
                        )
                    })?
                    .unwrap_or_default();
                discovered.extend(builtins.into_iter().map(|agent| agent.agent_id));
                if !discovered.contains(&extension.source_agent_id) {
                    return Err(format!(
                        "extension SubAgent {} source Agent does not exist: {}",
                        extension.id, extension.source_agent_id
                    ));
                }
            }
            routes.push(AgentRuntimeRoute {
                extension_id: extension.id.clone(),
                source_client_id: extension.source_client_id.clone(),
                source_agent_id: extension.source_agent_id.clone(),
                model_id: extension.model_id.clone(),
            });
        }
        let token = {
            let mut tokens = self
                .tokens
                .lock()
                .map_err(|_| "extension MCP token lock is poisoned".to_string())?;
            tokens
                .entry(client_id.to_string())
                .or_insert_with(|| Uuid::new_v4().to_string())
                .clone()
        };
        let url = format!("{}/mcp/{client_id}", gateway.base_url.trim_end_matches('/'));
        self.mounts.mount(client_id, &url, &token)?;
        let runtime = match &self.claude_runtime {
            Some(runtime) => runtime.clone(),
            None => crate::adapters::claude_code::detect_claude_cli()
                .map_err(|error| error.to_string())?
                .map(|detection| detection.path)
                .ok_or_else(|| {
                    "Claude Code CLI is required to run extension SubAgents".to_string()
                })?,
        };
        gateway.activate_client_agent_broker(
            client_id,
            state,
            &token,
            &runtime,
            &self.claude_root,
            routes,
        )
    }
}

fn bound_clients(state: &ControlPlaneState, extension_id: &str) -> Vec<String> {
    state
        .client_extension_subagent_ids
        .iter()
        .filter(|(_, ids)| ids.iter().any(|id| id == extension_id))
        .map(|(client_id, _)| client_id.clone())
        .collect()
}

fn extension_input(extension: PublicExtensionSubAgent) -> ExtensionSubAgentInput {
    ExtensionSubAgentInput {
        id: extension.id,
        name: extension.name,
        source_client_id: extension.source_client_id,
        source_agent_id: extension.source_agent_id,
        model_id: extension.model_id,
        capabilities: extension.capabilities,
    }
}

pub fn default_claude_runtime_path(path: Option<&Path>) -> Result<PathBuf, String> {
    path.map(Path::to_path_buf)
        .ok_or_else(|| "Claude Code CLI is required to run extension SubAgents".to_string())
}

#[tauri::command]
pub fn set_client_extension_binding(
    control: tauri::State<'_, ControlPlaneService>,
    integration: tauri::State<'_, ExtensionIntegrationService>,
    gateway: tauri::State<'_, GatewayStatus>,
    client_id: String,
    extension_subagent_id: String,
    enabled: bool,
) -> Result<ControlPlaneState, String> {
    integration.set_binding(
        &control,
        &gateway,
        &client_id,
        &extension_subagent_id,
        enabled,
    )
}

#[tauri::command]
pub fn update_extension_subagent(
    control: tauri::State<'_, ControlPlaneService>,
    integration: tauri::State<'_, ExtensionIntegrationService>,
    gateway: tauri::State<'_, GatewayStatus>,
    input: ExtensionSubAgentInput,
) -> Result<ControlPlaneState, String> {
    integration.update_extension(&control, &gateway, input)
}
