use crate::application::{
    ControlPlaneService, ControlPlaneState, ExtensionSubAgentInput, PublicExtensionSubAgent,
};
use crate::gateway::{AgentRuntimeRoute, AgentSourceRuntime, GatewayStatus};
use crate::local_agents::{
    discover_claude_builtin_agents, discover_claude_code_agents, discover_codex_agents,
    discover_gemini_agents, discover_grok_build_agents, discover_kimi_agents,
    discover_opencode_agents, discover_pi_agents, kimi_user_home,
};
use crate::mcp_mount::McpMountManager;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClientMcpStatus {
    pub client_id: String,
    pub desired_mounted: bool,
    pub mounted: bool,
    pub configuration_changed: bool,
}

pub struct ExtensionIntegrationService {
    mounts: McpMountManager,
    claude_root: PathBuf,
    claude_runtime: Option<PathBuf>,
    codex_root: Option<PathBuf>,
    codex_runtime: Option<PathBuf>,
    gemini_root: Option<PathBuf>,
    gemini_runtime: Option<PathBuf>,
    pi_root: Option<PathBuf>,
    pi_runtime: Option<PathBuf>,
    opencode_root: Option<PathBuf>,
    opencode_runtime: Option<PathBuf>,
    kimi_root: Option<PathBuf>,
    kimi_runtime: Option<PathBuf>,
    grok_build_root: Option<PathBuf>,
    grok_build_runtime: Option<PathBuf>,
    pi_settings_path: Option<PathBuf>,
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
            codex_root: None,
            codex_runtime: None,
            gemini_root: None,
            gemini_runtime: None,
            pi_root: None,
            pi_runtime: None,
            opencode_root: None,
            opencode_runtime: None,
            kimi_root: None,
            kimi_runtime: None,
            grok_build_root: None,
            grok_build_runtime: None,
            pi_settings_path,
            operation_lock: Mutex::new(()),
        }
    }

    pub fn with_codex(
        mut self,
        codex_root: impl Into<PathBuf>,
        codex_runtime: Option<PathBuf>,
    ) -> Self {
        self.codex_root = Some(codex_root.into());
        self.codex_runtime = codex_runtime;
        self
    }

    pub fn with_pi(mut self, pi_root: impl Into<PathBuf>, pi_runtime: Option<PathBuf>) -> Self {
        self.pi_root = Some(pi_root.into());
        self.pi_runtime = pi_runtime;
        self
    }

    pub fn with_gemini(
        mut self,
        gemini_root: impl Into<PathBuf>,
        gemini_runtime: Option<PathBuf>,
    ) -> Self {
        self.gemini_root = Some(gemini_root.into());
        self.gemini_runtime = gemini_runtime;
        self
    }

    pub fn with_opencode(
        mut self,
        opencode_root: impl Into<PathBuf>,
        opencode_runtime: Option<PathBuf>,
    ) -> Self {
        self.opencode_root = Some(opencode_root.into());
        self.opencode_runtime = opencode_runtime;
        self
    }

    pub fn with_kimi(
        mut self,
        kimi_root: impl Into<PathBuf>,
        kimi_runtime: Option<PathBuf>,
    ) -> Self {
        self.kimi_root = Some(kimi_root.into());
        self.kimi_runtime = kimi_runtime;
        self
    }

    pub fn with_grok_build(
        mut self,
        grok_build_root: impl Into<PathBuf>,
        grok_build_runtime: Option<PathBuf>,
    ) -> Self {
        self.grok_build_root = Some(grok_build_root.into());
        self.grok_build_runtime = grok_build_runtime;
        self
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
            if client_mcp_desired(&previous, client_id) {
                self.reconcile_client(&previous, gateway, client_id)?;
            }
            return Ok(previous);
        }
        let updated =
            control.set_client_extension_subagent_enabled(client_id, extension_id, enabled)?;
        if !client_mcp_desired(&updated, client_id) {
            return Ok(updated);
        }
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
        let bound_clients = bound_clients(&previous, &input.id)
            .into_iter()
            .filter(|client_id| client_mcp_desired(&previous, client_id))
            .collect::<Vec<_>>();
        let updated = control.update_extension_subagent(input)?;
        let mut reconciled_clients = Vec::new();
        for client_id in &bound_clients {
            let Err(error) = self.reconcile_client(&updated, gateway, client_id) else {
                reconciled_clients.push(client_id.clone());
                continue;
            };
            let rollback = control
                .update_extension_subagent(extension_input(original))
                .and_then(|restored| {
                    self.reconcile_clients(&restored, gateway, &reconciled_clients)?;
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
        for client_id in &state.mcp_mounted_client_ids {
            self.reconcile_client(state, gateway, client_id)?;
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
            if self.mounts.is_mounted(&client_id)? {
                self.mounts.credential(&client_id)?;
            }
            self.mounts.unmount(&client_id)?;
        }
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
        let was_mounted = client_mcp_desired(state, client_id);
        if was_mounted {
            self.suspend_client(gateway, client_id)?;
        }
        let result = operation();
        let reconcile = if was_mounted {
            self.reconcile_client(state, gateway, client_id)
        } else {
            Ok(())
        };
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
        let mut routes = Vec::with_capacity(ids.len());
        for id in ids {
            let extension = state
                .extension_subagents
                .iter()
                .find(|extension| extension.id == id)
                .ok_or_else(|| format!("unknown extension SubAgent: {id}"))?;
            if !matches!(
                extension.source_client_id.as_str(),
                "claude_code" | "codex" | "gemini" | "pi" | "opencode" | "kimi_code" | "grok_build"
            ) {
                return Err(format!(
                    "extension SubAgent {} uses an unsupported source client: {}",
                    extension.id, extension.source_client_id
                ));
            }
            routes.push(AgentRuntimeRoute {
                extension_id: extension.id.clone(),
                source_client_id: extension.source_client_id.clone(),
                source_agent_id: extension.source_agent_id.clone(),
                model_id: extension.model_id.clone(),
            });
        }
        let token = self.mounts.credential(client_id)?;
        let url = format!("{}/mcp/{client_id}", gateway.base_url.trim_end_matches('/'));
        self.mounts.mount(client_id, &url, &token)?;
        let source_ids = routes
            .iter()
            .map(|route| route.source_client_id.as_str())
            .collect::<HashSet<_>>();
        let mut source_runtimes = Vec::with_capacity(source_ids.len());
        if source_ids.contains("claude_code") {
            let runtime = self.resolve_claude_runtime()?;
            let mut discovered = discover_claude_code_agents(&self.claude_root)?
                .into_iter()
                .map(|agent| agent.agent_id)
                .collect::<HashSet<_>>();
            let needs_builtin_probe = routes.iter().any(|route| {
                route.source_client_id == "claude_code"
                    && !discovered.contains(&route.source_agent_id)
            });
            if needs_builtin_probe {
                discovered.extend(
                    discover_claude_builtin_agents(&runtime)
                        .map_err(|error| {
                            "extension SubAgent source Agent does not exist or could not be verified: "
                                .to_string()
                                + &error
                        })?
                        .into_iter()
                        .map(|agent| agent.agent_id),
                );
            }
            validate_source_agents(&routes, "claude_code", &discovered)?;
            source_runtimes.push(AgentSourceRuntime {
                source_client_id: "claude_code".into(),
                runtime,
                config_root: self.claude_root.clone(),
            });
        }
        if source_ids.contains("codex") {
            let codex_root = self
                .codex_root
                .clone()
                .ok_or_else(|| "Codex configuration root is not configured".to_string())?;
            let runtime = self.resolve_codex_runtime()?;
            let discovered = discover_codex_agents(&codex_root)?
                .into_iter()
                .map(|agent| agent.agent_id)
                .collect::<HashSet<_>>();
            validate_codex_source_agents(&routes, &discovered)?;
            source_runtimes.push(AgentSourceRuntime {
                source_client_id: "codex".into(),
                runtime,
                config_root: codex_root,
            });
        }
        if source_ids.contains("gemini") {
            let gemini_root = self
                .gemini_root
                .clone()
                .ok_or_else(|| "Gemini configuration root is not configured".to_string())?;
            let runtime = self.resolve_gemini_runtime()?;
            let discovered = discover_gemini_agents(&gemini_root)?
                .into_iter()
                .map(|agent| agent.agent_id)
                .collect::<HashSet<_>>();
            validate_gemini_source_agents(&routes, &discovered)?;
            source_runtimes.push(AgentSourceRuntime {
                source_client_id: "gemini".into(),
                runtime,
                config_root: gemini_root,
            });
        }
        if source_ids.contains("pi") {
            let pi_root = self
                .pi_root
                .clone()
                .ok_or_else(|| "Pi configuration root is not configured".to_string())?;
            let runtime = self.resolve_pi_runtime()?;
            let discovered = discover_pi_agents(&pi_root)?
                .into_iter()
                .map(|agent| agent.agent_id)
                .collect::<HashSet<_>>();
            validate_pi_source_agents(&routes, &discovered)?;
            source_runtimes.push(AgentSourceRuntime {
                source_client_id: "pi".into(),
                runtime,
                config_root: pi_root,
            });
        }
        if source_ids.contains("opencode") {
            let opencode_root = self
                .opencode_root
                .clone()
                .ok_or_else(|| "OpenCode configuration root is not configured".to_string())?;
            let runtime = self.resolve_opencode_runtime()?;
            let discovered = discover_opencode_agents(&opencode_root)?
                .into_iter()
                .map(|agent| agent.agent_id)
                .collect::<HashSet<_>>();
            validate_named_project_source_agents(&routes, "opencode", &discovered)?;
            source_runtimes.push(AgentSourceRuntime {
                source_client_id: "opencode".into(),
                runtime,
                config_root: opencode_root,
            });
        }
        if source_ids.contains("kimi_code") {
            let kimi_root = self
                .kimi_root
                .clone()
                .ok_or_else(|| "Kimi Code configuration root is not configured".to_string())?;
            let runtime = self.resolve_kimi_runtime()?;
            let home = kimi_user_home(&kimi_root)?;
            let discovered = discover_kimi_agents(&kimi_root, &home)?
                .into_iter()
                .map(|agent| agent.agent_id)
                .collect::<HashSet<_>>();
            validate_kimi_source_agents(&routes, &discovered)?;
            source_runtimes.push(AgentSourceRuntime {
                source_client_id: "kimi_code".into(),
                runtime,
                config_root: kimi_root,
            });
        }
        if source_ids.contains("grok_build") {
            let grok_build_root = self
                .grok_build_root
                .clone()
                .ok_or_else(|| "Grok Build configuration root is not configured".to_string())?;
            let runtime = self.resolve_grok_build_runtime()?;
            let cwd = std::env::current_dir()
                .map_err(|error| format!("current project directory is unavailable: {error}"))?;
            let discovered = discover_grok_build_agents(&runtime, &cwd)?
                .into_iter()
                .map(|agent| agent.agent_id)
                .collect::<HashSet<_>>();
            validate_named_project_source_agents(&routes, "grok_build", &discovered)?;
            source_runtimes.push(AgentSourceRuntime {
                source_client_id: "grok_build".into(),
                runtime,
                config_root: grok_build_root,
            });
        }
        gateway.activate_client_agent_broker_with_sources(
            client_id,
            state,
            &token,
            source_runtimes,
            routes,
        )
    }

    pub fn mount_client(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
        client_id: &str,
    ) -> Result<ClientMcpStatus, String> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| "live configuration operation lock is poisoned".to_string())?;
        let previous = control.state()?;
        let was_desired = client_mcp_desired(&previous, client_id);
        let updated = if was_desired {
            previous
        } else {
            control.set_client_mcp_mounted(client_id, true)?
        };
        if let Err(error) = self.reconcile_client(&updated, gateway, client_id) {
            let _ = self.suspend_client(gateway, client_id);
            let rollback = if was_desired {
                Ok(())
            } else {
                control.set_client_mcp_mounted(client_id, false).map(|_| ())
            };
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback_error) => {
                    format!("{error}; MCP mount preference rollback failed: {rollback_error}")
                }
            });
        }
        self.client_status(&updated, client_id)
    }

    pub fn unmount_client(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
        client_id: &str,
    ) -> Result<ClientMcpStatus, String> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| "live configuration operation lock is poisoned".to_string())?;
        let previous = control.state()?;
        let was_desired = client_mcp_desired(&previous, client_id);
        let updated = if was_desired {
            control.set_client_mcp_mounted(client_id, false)?
        } else {
            previous
        };
        if let Err(error) = self.suspend_client(gateway, client_id) {
            let rollback = if was_desired {
                control
                    .set_client_mcp_mounted(client_id, true)
                    .and_then(|restored| self.reconcile_client(&restored, gateway, client_id))
            } else {
                Ok(())
            };
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback_error) => {
                    format!("{error}; MCP unmount preference rollback failed: {rollback_error}")
                }
            });
        }
        self.client_status(&updated, client_id)
    }

    pub fn client_status(
        &self,
        state: &ControlPlaneState,
        client_id: &str,
    ) -> Result<ClientMcpStatus, String> {
        let actual = self.mounts.status(client_id)?;
        Ok(ClientMcpStatus {
            client_id: client_id.to_string(),
            desired_mounted: client_mcp_desired(state, client_id),
            mounted: actual.mounted,
            configuration_changed: actual.configuration_changed,
        })
    }

    pub fn client_statuses(
        &self,
        state: &ControlPlaneState,
    ) -> Result<Vec<ClientMcpStatus>, String> {
        self.mounts
            .supported_clients()
            .into_iter()
            .map(|client_id| self.client_status(state, &client_id))
            .collect()
    }

    fn resolve_claude_runtime(&self) -> Result<PathBuf, String> {
        self.claude_runtime.clone().map_or_else(
            || {
                crate::adapters::claude_code::detect_claude_cli()
                    .map_err(|error| error.to_string())?
                    .map(|detection| detection.path)
                    .ok_or_else(|| {
                        "Claude Code CLI is required to run extension SubAgents".to_string()
                    })
            },
            Ok,
        )
    }

    fn resolve_codex_runtime(&self) -> Result<PathBuf, String> {
        self.codex_runtime.clone().map_or_else(
            || {
                crate::adapters::codex::detect_codex_cli()
                    .map_err(|error| error.to_string())?
                    .map(|detection| detection.path)
                    .ok_or_else(|| "Codex CLI is required to run Codex SubAgents".to_string())
            },
            Ok,
        )
    }

    fn resolve_pi_runtime(&self) -> Result<PathBuf, String> {
        self.pi_runtime.clone().map_or_else(
            || {
                crate::adapters::pi::detect_pi_cli()
                    .map_err(|error| error.to_string())?
                    .map(|detection| detection.path)
                    .ok_or_else(|| "Pi CLI is required to run Pi extension Agents".to_string())
            },
            Ok,
        )
    }

    fn resolve_opencode_runtime(&self) -> Result<PathBuf, String> {
        self.opencode_runtime.clone().map_or_else(
            || {
                crate::adapters::opencode::detect_opencode_cli()
                    .map_err(|error| error.to_string())?
                    .map(|detection| detection.path)
                    .ok_or_else(|| {
                        "OpenCode CLI is required to run OpenCode extension Agents".to_string()
                    })
            },
            Ok,
        )
    }

    fn resolve_kimi_runtime(&self) -> Result<PathBuf, String> {
        self.kimi_runtime.clone().map_or_else(
            || {
                crate::adapters::kimi_code::detect_kimi_code_cli()
                    .map_err(|error| error.to_string())?
                    .map(|detection| detection.path)
                    .ok_or_else(|| {
                        "Kimi Code CLI is required to run Kimi extension Agents".to_string()
                    })
            },
            Ok,
        )
    }

    fn resolve_gemini_runtime(&self) -> Result<PathBuf, String> {
        self.gemini_runtime.clone().map_or_else(
            || {
                crate::adapters::gemini::detect_gemini_cli()
                    .map_err(|error| error.to_string())?
                    .map(|detection| detection.path)
                    .ok_or_else(|| {
                        "Gemini CLI is required to run Gemini extension Agents".to_string()
                    })
            },
            Ok,
        )
    }

    fn resolve_grok_build_runtime(&self) -> Result<PathBuf, String> {
        self.grok_build_runtime.clone().map_or_else(
            || {
                crate::adapters::grok_build::detect_grok_build_cli()
                    .map_err(|error| error.to_string())?
                    .map(|detection| detection.path)
                    .ok_or_else(|| {
                        "Grok Build CLI is required to run Grok Build extension Agents".to_string()
                    })
            },
            Ok,
        )
    }
}

fn validate_source_agents(
    routes: &[AgentRuntimeRoute],
    source_client_id: &str,
    discovered: &HashSet<String>,
) -> Result<(), String> {
    for route in routes
        .iter()
        .filter(|route| route.source_client_id == source_client_id)
    {
        if !discovered.contains(&route.source_agent_id) {
            return Err(format!(
                "extension SubAgent {} source Agent does not exist: {}",
                route.extension_id, route.source_agent_id
            ));
        }
    }
    Ok(())
}

fn validate_codex_source_agents(
    routes: &[AgentRuntimeRoute],
    discovered: &HashSet<String>,
) -> Result<(), String> {
    for route in routes
        .iter()
        .filter(|route| route.source_client_id == "codex")
    {
        if discovered.contains(&route.source_agent_id) {
            continue;
        }
        if route.source_agent_id.is_empty()
            || !route.source_agent_id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
        {
            return Err(format!(
                "extension SubAgent {} has an invalid Codex Agent name: {}",
                route.extension_id, route.source_agent_id
            ));
        }
    }
    Ok(())
}

fn validate_gemini_source_agents(
    routes: &[AgentRuntimeRoute],
    discovered: &HashSet<String>,
) -> Result<(), String> {
    for route in routes
        .iter()
        .filter(|route| route.source_client_id == "gemini")
    {
        if discovered.contains(&route.source_agent_id) {
            continue;
        }
        if route.source_agent_id.is_empty()
            || !route.source_agent_id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
        {
            return Err(format!(
                "extension SubAgent {} has an invalid Gemini Agent name: {}",
                route.extension_id, route.source_agent_id
            ));
        }
    }
    Ok(())
}

fn validate_pi_source_agents(
    routes: &[AgentRuntimeRoute],
    discovered: &HashSet<String>,
) -> Result<(), String> {
    for route in routes.iter().filter(|route| route.source_client_id == "pi") {
        if discovered.contains(&route.source_agent_id) {
            continue;
        }
        if route.source_agent_id.is_empty()
            || !route.source_agent_id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
        {
            return Err(format!(
                "extension SubAgent {} has an invalid Pi Agent name: {}",
                route.extension_id, route.source_agent_id
            ));
        }
    }
    Ok(())
}

fn validate_kimi_source_agents(
    routes: &[AgentRuntimeRoute],
    discovered: &HashSet<String>,
) -> Result<(), String> {
    for route in routes
        .iter()
        .filter(|route| route.source_client_id == "kimi_code")
    {
        if discovered.contains(&route.source_agent_id) {
            continue;
        }
        if route.source_agent_id.is_empty()
            || !route
                .source_agent_id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(format!(
                "extension SubAgent {} has an invalid Kimi Code Agent name: {}",
                route.extension_id, route.source_agent_id
            ));
        }
    }
    Ok(())
}

fn validate_named_project_source_agents(
    routes: &[AgentRuntimeRoute],
    source_client_id: &str,
    discovered: &HashSet<String>,
) -> Result<(), String> {
    for route in routes
        .iter()
        .filter(|route| route.source_client_id == source_client_id)
    {
        if discovered.contains(&route.source_agent_id) {
            continue;
        }
        if route.source_agent_id.is_empty()
            || !route.source_agent_id.split('/').all(|segment| {
                !segment.is_empty()
                    && segment.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'-' | b'_')
                    })
            })
        {
            return Err(format!(
                "extension SubAgent {} has an invalid {source_client_id} Agent name: {}",
                route.extension_id, route.source_agent_id
            ));
        }
    }
    Ok(())
}

fn bound_clients(state: &ControlPlaneState, extension_id: &str) -> Vec<String> {
    state
        .client_extension_subagent_ids
        .iter()
        .filter(|(_, ids)| ids.iter().any(|id| id == extension_id))
        .map(|(client_id, _)| client_id.clone())
        .collect()
}

fn client_mcp_desired(state: &ControlPlaneState, client_id: &str) -> bool {
    state
        .mcp_mounted_client_ids
        .iter()
        .any(|candidate| candidate == client_id)
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

#[tauri::command]
pub fn mount_client_mcp(
    control: tauri::State<'_, ControlPlaneService>,
    integration: tauri::State<'_, ExtensionIntegrationService>,
    gateway: tauri::State<'_, GatewayStatus>,
    client_id: String,
) -> Result<ClientMcpStatus, String> {
    integration.mount_client(&control, &gateway, &client_id)
}

#[tauri::command]
pub fn unmount_client_mcp(
    control: tauri::State<'_, ControlPlaneService>,
    integration: tauri::State<'_, ExtensionIntegrationService>,
    gateway: tauri::State<'_, GatewayStatus>,
    client_id: String,
) -> Result<ClientMcpStatus, String> {
    integration.unmount_client(&control, &gateway, &client_id)
}

#[tauri::command]
pub fn client_mcp_status(
    control: tauri::State<'_, ControlPlaneService>,
    integration: tauri::State<'_, ExtensionIntegrationService>,
    client_id: String,
) -> Result<ClientMcpStatus, String> {
    integration.client_status(&control.state()?, &client_id)
}

#[tauri::command]
pub fn client_mcp_statuses(
    control: tauri::State<'_, ControlPlaneService>,
    integration: tauri::State<'_, ExtensionIntegrationService>,
) -> Result<Vec<ClientMcpStatus>, String> {
    integration.client_statuses(&control.state()?)
}
