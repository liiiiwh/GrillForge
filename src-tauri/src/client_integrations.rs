use crate::adapters::gemini::{
    GeminiAdapter, GeminiPaths, GeminiRequest, GeminiTakeoverStatus, detect_gemini_cli,
};
use crate::adapters::grok_build::{
    GrokBuildAdapter, GrokBuildPaths, GrokBuildRequest, GrokBuildTakeoverStatus,
    detect_grok_build_cli,
};
use crate::adapters::hermes::{
    HermesAdapter, HermesModel, HermesPaths, HermesRequest, HermesTakeoverStatus, detect_hermes_cli,
};
use crate::adapters::kimi_code::{
    KimiCodeAdapter, KimiCodeAgentProfile, KimiCodeModel, KimiCodePaths, KimiCodeRequest,
    KimiCodeTakeoverStatus, detect_kimi_code_cli, discover_kimi_code_agents,
    set_kimi_code_agent_model_preference,
};
use crate::adapters::openclaw::{
    OpenClawAdapter, OpenClawModelSpec, OpenClawPaths, OpenClawRequest, OpenClawTakeoverStatus,
    detect_openclaw_cli,
};
use crate::adapters::opencode::{
    OpenCodeAdapter, OpenCodeModel, OpenCodePaths, OpenCodeRequest, OpenCodeTakeoverStatus,
    detect_opencode_cli,
};
use crate::application::ControlPlaneService;
use crate::gateway::GatewayStatus;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::State;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientIntegrationStatus {
    pub installed: bool,
    pub executable_path: Option<String>,
    pub version: Option<String>,
    pub snapshot_present: bool,
    pub takeover: &'static str,
    pub configured_model_ids: Vec<String>,
    pub main_model_id: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KimiCodeIntegrationStatus {
    #[serde(flatten)]
    pub client: ClientIntegrationStatus,
    pub secondary_model_id: Option<String>,
    pub agents: Vec<KimiCodeAgentProfile>,
}

fn configured(
    control: &ControlPlaneService,
    client_id: &str,
) -> Result<(Option<String>, Vec<String>), String> {
    let state = control.state()?;
    let config = state
        .client_configurations
        .get(client_id)
        .ok_or_else(|| format!("missing client configuration: {client_id}"))?;
    Ok((
        config.main_model_id.clone(),
        config.enabled_model_ids.clone(),
    ))
}

fn takeover_label(active: bool, stale: bool, drifted: bool) -> &'static str {
    if drifted {
        "drifted"
    } else if active && stale {
        "reapply_required"
    } else if active {
        "active"
    } else {
        "inactive"
    }
}

pub struct GeminiIntegrationService {
    adapter: GeminiAdapter,
    activated: AtomicBool,
}

impl GeminiIntegrationService {
    pub fn new(paths: GeminiPaths, root: impl Into<PathBuf>) -> Self {
        Self {
            adapter: GeminiAdapter::new(paths, root),
            activated: AtomicBool::new(false),
        }
    }

    pub fn status(&self, control: &ControlPlaneService) -> Result<ClientIntegrationStatus, String> {
        let detection = detect_gemini_cli().map_err(|error| error.to_string())?;
        let adapter = self.adapter.status().map_err(|error| error.to_string())?;
        let (main_model_id, configured_model_ids) = configured(control, "gemini")?;
        let active = adapter.takeover == GeminiTakeoverStatus::Active;
        Ok(ClientIntegrationStatus {
            installed: detection.is_some(),
            executable_path: detection
                .as_ref()
                .map(|value| value.path.display().to_string()),
            version: detection.map(|value| value.version),
            snapshot_present: adapter.snapshot_present,
            takeover: takeover_label(
                active,
                !self.activated.load(Ordering::Acquire),
                adapter.takeover == GeminiTakeoverStatus::Drifted,
            ),
            configured_model_ids,
            main_model_id,
        })
    }

    pub fn apply(&self, control: &ControlPlaneService) -> Result<ClientIntegrationStatus, String> {
        if detect_gemini_cli()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err("未检测到 Gemini CLI；安装后才能应用配置".into());
        }
        let selection = control.client_selection("gemini")?;
        self.adapter
            .apply(
                GeminiRequest::new(
                    selection.provider.endpoint,
                    selection.provider.api_key,
                    selection.main_model.upstream_id,
                )
                .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        self.activated.store(true, Ordering::Release);
        self.status(control)
    }

    pub fn disable(
        &self,
        control: &ControlPlaneService,
    ) -> Result<ClientIntegrationStatus, String> {
        self.adapter.disable().map_err(|error| error.to_string())?;
        self.activated.store(false, Ordering::Release);
        self.status(control)
    }

    pub fn recovery_pending(&self) -> bool {
        self.adapter
            .status()
            .is_ok_and(|status| status.snapshot_present)
    }

    pub fn resume_if_applied(&self) -> Result<bool, String> {
        match self
            .adapter
            .status()
            .map_err(|error| error.to_string())?
            .takeover
        {
            GeminiTakeoverStatus::Inactive | GeminiTakeoverStatus::Drifted => Ok(false),
            GeminiTakeoverStatus::Active => {
                self.activated.store(true, Ordering::Release);
                Ok(true)
            }
        }
    }
}

pub struct GrokBuildIntegrationService {
    adapter: GrokBuildAdapter,
    activated: AtomicBool,
}

impl GrokBuildIntegrationService {
    pub fn new(paths: GrokBuildPaths, root: impl Into<PathBuf>) -> Self {
        Self {
            adapter: GrokBuildAdapter::new(paths, root),
            activated: AtomicBool::new(false),
        }
    }

    pub fn status(&self, control: &ControlPlaneService) -> Result<ClientIntegrationStatus, String> {
        let detection = detect_grok_build_cli().map_err(|error| error.to_string())?;
        let adapter = self.adapter.status().map_err(|error| error.to_string())?;
        let (main_model_id, configured_model_ids) = configured(control, "grok_build")?;
        let active = adapter.takeover == GrokBuildTakeoverStatus::Active;
        Ok(ClientIntegrationStatus {
            installed: detection.is_some(),
            executable_path: detection
                .as_ref()
                .map(|value| value.path.display().to_string()),
            version: detection.map(|value| value.version),
            snapshot_present: adapter.snapshot_present,
            takeover: takeover_label(
                active,
                !self.activated.load(Ordering::Acquire),
                adapter.takeover == GrokBuildTakeoverStatus::Drifted,
            ),
            configured_model_ids,
            main_model_id,
        })
    }

    pub fn apply(&self, control: &ControlPlaneService) -> Result<ClientIntegrationStatus, String> {
        if detect_grok_build_cli()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err("未检测到 Grok Build CLI；安装后才能应用配置".into());
        }
        let selection = control.client_selection("grok_build")?;
        self.adapter
            .apply(
                GrokBuildRequest::new(
                    selection.provider.endpoint,
                    selection.provider.api_key,
                    selection.main_model.upstream_id,
                    selection.main_model.display_name,
                )
                .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        self.activated.store(true, Ordering::Release);
        self.status(control)
    }

    pub fn disable(
        &self,
        control: &ControlPlaneService,
    ) -> Result<ClientIntegrationStatus, String> {
        self.adapter.disable().map_err(|error| error.to_string())?;
        self.activated.store(false, Ordering::Release);
        self.status(control)
    }

    pub fn recovery_pending(&self) -> bool {
        self.adapter
            .status()
            .is_ok_and(|status| status.snapshot_present)
    }

    pub fn resume_if_applied(&self) -> Result<bool, String> {
        match self
            .adapter
            .status()
            .map_err(|error| error.to_string())?
            .takeover
        {
            GrokBuildTakeoverStatus::Inactive | GrokBuildTakeoverStatus::Drifted => Ok(false),
            GrokBuildTakeoverStatus::Active => {
                self.activated.store(true, Ordering::Release);
                Ok(true)
            }
        }
    }
}

pub struct OpenCodeIntegrationService {
    adapter: OpenCodeAdapter,
    activated: AtomicBool,
}

impl OpenCodeIntegrationService {
    pub fn new(paths: OpenCodePaths, root: impl Into<PathBuf>) -> Self {
        Self {
            adapter: OpenCodeAdapter::new(paths, root),
            activated: AtomicBool::new(false),
        }
    }

    pub fn status(&self, control: &ControlPlaneService) -> Result<ClientIntegrationStatus, String> {
        let detection = detect_opencode_cli().map_err(|error| error.to_string())?;
        let adapter = self.adapter.status().map_err(|error| error.to_string())?;
        let (main_model_id, configured_model_ids) = configured(control, "opencode")?;
        let active = adapter.takeover == OpenCodeTakeoverStatus::Active;
        Ok(ClientIntegrationStatus {
            installed: detection.is_some(),
            executable_path: detection
                .as_ref()
                .map(|value| value.path.display().to_string()),
            version: detection.map(|value| value.version),
            snapshot_present: adapter.snapshot_present,
            takeover: takeover_label(
                active,
                !self.activated.load(Ordering::Acquire),
                adapter.takeover == OpenCodeTakeoverStatus::Drifted,
            ),
            configured_model_ids,
            main_model_id,
        })
    }

    pub fn apply(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<ClientIntegrationStatus, String> {
        if detect_opencode_cli()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err("未检测到 OpenCode CLI；安装后才能应用配置".into());
        }
        self.activate(control, gateway)?;
        self.status(control)
    }

    fn activate(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<(), String> {
        let selection = control.client_selection("opencode")?;
        let token = uuid::Uuid::new_v4().to_string();
        let models = selection
            .enabled_models
            .iter()
            .map(|model| {
                OpenCodeModel::new(format!("grillforge/{}", model.id), &model.display_name)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let request = OpenCodeRequest::new(
            format!(
                "{}/clients/opencode/v1",
                gateway.base_url.trim_end_matches('/')
            ),
            &token,
            models,
            format!("grillforge/{}", selection.main_model.id),
        )
        .map_err(|error| error.to_string())?;
        self.adapter
            .apply(request)
            .map_err(|error| error.to_string())?;
        let ids = selection
            .enabled_models
            .iter()
            .map(|model| model.id.clone())
            .collect();
        if let Err(error) = gateway.activate_client("opencode", ids, &token) {
            let restore = self
                .adapter
                .disable()
                .map_err(|restore| restore.to_string());
            return Err(match restore {
                Ok(_) => error,
                Err(restore) => format!("{error}; OpenCode 配置回滚也失败: {restore}"),
            });
        }
        self.activated.store(true, Ordering::Release);
        Ok(())
    }

    pub fn disable(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<ClientIntegrationStatus, String> {
        self.adapter.disable().map_err(|error| error.to_string())?;
        gateway.deactivate_client("opencode");
        self.activated.store(false, Ordering::Release);
        self.status(control)
    }

    pub fn recovery_pending(&self) -> bool {
        self.adapter
            .status()
            .is_ok_and(|status| status.snapshot_present)
    }

    pub fn resume_if_applied(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<bool, String> {
        match self
            .adapter
            .status()
            .map_err(|error| error.to_string())?
            .takeover
        {
            OpenCodeTakeoverStatus::Inactive | OpenCodeTakeoverStatus::Drifted => Ok(false),
            OpenCodeTakeoverStatus::Active => {
                self.activate(control, gateway)?;
                Ok(true)
            }
        }
    }
}

pub struct OpenClawIntegrationService {
    adapter: OpenClawAdapter,
    activated: AtomicBool,
}

impl OpenClawIntegrationService {
    pub fn new(paths: OpenClawPaths, root: impl Into<PathBuf>) -> Self {
        Self {
            adapter: OpenClawAdapter::new(paths, root),
            activated: AtomicBool::new(false),
        }
    }

    pub fn status(&self, control: &ControlPlaneService) -> Result<ClientIntegrationStatus, String> {
        let detection = detect_openclaw_cli().map_err(|error| error.to_string())?;
        let adapter = self.adapter.status().map_err(|error| error.to_string())?;
        let (main_model_id, configured_model_ids) = configured(control, "openclaw")?;
        let active = adapter.takeover == OpenClawTakeoverStatus::Active;
        Ok(ClientIntegrationStatus {
            installed: detection.is_some(),
            executable_path: detection
                .as_ref()
                .map(|value| value.path.display().to_string()),
            version: detection.map(|value| value.version),
            snapshot_present: adapter.snapshot_present,
            takeover: takeover_label(
                active,
                !self.activated.load(Ordering::Acquire),
                adapter.takeover == OpenClawTakeoverStatus::Drifted,
            ),
            configured_model_ids,
            main_model_id,
        })
    }

    pub fn apply(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<ClientIntegrationStatus, String> {
        if detect_openclaw_cli()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err("未检测到 OpenClaw CLI；安装后才能应用配置".into());
        }
        self.activate(control, gateway)?;
        self.status(control)
    }

    fn activate(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<(), String> {
        let selection = control.client_selection("openclaw")?;
        let token = uuid::Uuid::new_v4().to_string();
        let models = selection
            .enabled_models
            .iter()
            .map(|model| {
                let mut input = vec!["text".into()];
                if model
                    .capabilities
                    .iter()
                    .any(|value| matches!(value.as_str(), "image" | "vision"))
                {
                    input.push("image".into());
                }
                OpenClawModelSpec::new(
                    format!("grillforge/{}", model.id),
                    &model.display_name,
                    model.capabilities.iter().any(|value| value == "reasoning")
                        || !model.protocol_capabilities.is_empty(),
                    input,
                    128_000,
                    16_384,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let primary = format!("grillforge/{}", selection.main_model.id);
        let fallbacks = selection
            .enabled_models
            .iter()
            .filter(|model| model.id != selection.main_model.id)
            .map(|model| format!("grillforge/{}", model.id))
            .collect();
        let request = OpenClawRequest::new(
            format!(
                "{}/clients/openclaw",
                gateway.base_url.trim_end_matches('/')
            ),
            &token,
            models,
            primary,
            fallbacks,
        )
        .map_err(|error| error.to_string())?;
        self.adapter
            .apply(request)
            .map_err(|error| error.to_string())?;
        let ids = selection
            .enabled_models
            .iter()
            .map(|model| model.id.clone())
            .collect();
        if let Err(error) = gateway.activate_client("openclaw", ids, &token) {
            let restore = self
                .adapter
                .disable()
                .map_err(|restore| restore.to_string());
            return Err(match restore {
                Ok(_) => error,
                Err(restore) => format!("{error}; OpenClaw 配置回滚也失败: {restore}"),
            });
        }
        self.activated.store(true, Ordering::Release);
        Ok(())
    }

    pub fn disable(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<ClientIntegrationStatus, String> {
        self.adapter.disable().map_err(|error| error.to_string())?;
        gateway.deactivate_client("openclaw");
        self.activated.store(false, Ordering::Release);
        self.status(control)
    }

    pub fn recovery_pending(&self) -> bool {
        self.adapter
            .status()
            .is_ok_and(|status| status.snapshot_present)
    }

    pub fn resume_if_applied(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<bool, String> {
        match self
            .adapter
            .status()
            .map_err(|error| error.to_string())?
            .takeover
        {
            OpenClawTakeoverStatus::Inactive | OpenClawTakeoverStatus::Drifted => Ok(false),
            OpenClawTakeoverStatus::Active => {
                self.activate(control, gateway)?;
                Ok(true)
            }
        }
    }
}

pub struct HermesIntegrationService {
    adapter: HermesAdapter,
    activated: AtomicBool,
}

impl HermesIntegrationService {
    pub fn new(paths: HermesPaths, root: impl Into<PathBuf>) -> Self {
        Self {
            adapter: HermesAdapter::new(paths, root),
            activated: AtomicBool::new(false),
        }
    }

    pub fn status(&self, control: &ControlPlaneService) -> Result<ClientIntegrationStatus, String> {
        let detection = detect_hermes_cli().map_err(|error| error.to_string())?;
        let adapter = self.adapter.status().map_err(|error| error.to_string())?;
        let (main_model_id, configured_model_ids) = configured(control, "hermes")?;
        let active = adapter.takeover == HermesTakeoverStatus::Active;
        Ok(ClientIntegrationStatus {
            installed: detection.is_some(),
            executable_path: detection
                .as_ref()
                .map(|value| value.path.display().to_string()),
            version: detection.map(|value| value.version),
            snapshot_present: adapter.snapshot_present,
            takeover: takeover_label(
                active,
                !self.activated.load(Ordering::Acquire),
                adapter.takeover == HermesTakeoverStatus::Drifted,
            ),
            configured_model_ids,
            main_model_id,
        })
    }

    pub fn apply(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<ClientIntegrationStatus, String> {
        if detect_hermes_cli()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err("未检测到 Hermes CLI；安装后才能应用配置".into());
        }
        self.activate(control, gateway)?;
        self.status(control)
    }

    fn activate(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<(), String> {
        let selection = control.client_selection("hermes")?;
        let token = uuid::Uuid::new_v4().to_string();
        let models = selection
            .enabled_models
            .iter()
            .map(|model| HermesModel::new(format!("grillforge/{}", model.id), &model.display_name))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let request = HermesRequest::new(
            format!(
                "{}/clients/hermes/v1",
                gateway.base_url.trim_end_matches('/')
            ),
            &token,
            models,
            format!("grillforge/{}", selection.main_model.id),
        )
        .map_err(|error| error.to_string())?;
        self.adapter
            .apply(request)
            .map_err(|error| error.to_string())?;
        let ids = selection
            .enabled_models
            .iter()
            .map(|model| model.id.clone())
            .collect();
        if let Err(error) = gateway.activate_client("hermes", ids, &token) {
            let restore = self
                .adapter
                .disable()
                .map_err(|restore| restore.to_string());
            return Err(match restore {
                Ok(_) => error,
                Err(restore) => format!("{error}; Hermes 配置回滚也失败: {restore}"),
            });
        }
        self.activated.store(true, Ordering::Release);
        Ok(())
    }

    pub fn disable(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<ClientIntegrationStatus, String> {
        self.adapter.disable().map_err(|error| error.to_string())?;
        gateway.deactivate_client("hermes");
        self.activated.store(false, Ordering::Release);
        self.status(control)
    }

    pub fn recovery_pending(&self) -> bool {
        self.adapter
            .status()
            .is_ok_and(|status| status.snapshot_present)
    }

    pub fn resume_if_applied(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<bool, String> {
        match self
            .adapter
            .status()
            .map_err(|error| error.to_string())?
            .takeover
        {
            HermesTakeoverStatus::Inactive | HermesTakeoverStatus::Drifted => Ok(false),
            HermesTakeoverStatus::Active => {
                self.activate(control, gateway)?;
                Ok(true)
            }
        }
    }
}

pub struct KimiCodeIntegrationService {
    adapter: KimiCodeAdapter,
    paths: KimiCodePaths,
    activated: AtomicBool,
}

impl KimiCodeIntegrationService {
    pub fn new(paths: KimiCodePaths, root: impl Into<PathBuf>) -> Self {
        Self {
            adapter: KimiCodeAdapter::new(paths.clone(), root),
            paths,
            activated: AtomicBool::new(false),
        }
    }

    pub fn status(
        &self,
        control: &ControlPlaneService,
    ) -> Result<KimiCodeIntegrationStatus, String> {
        let detection = detect_kimi_code_cli().map_err(|error| error.to_string())?;
        let adapter = self.adapter.status().map_err(|error| error.to_string())?;
        let (main_model_id, configured_model_ids) = configured(control, "kimi_code")?;
        let secondary_model_id = control
            .state()?
            .client_configurations
            .get("kimi_code")
            .and_then(|configuration| configuration.secondary_model_id.clone());
        let active = adapter.takeover == KimiCodeTakeoverStatus::Active;
        Ok(KimiCodeIntegrationStatus {
            client: ClientIntegrationStatus {
                installed: detection.is_some(),
                executable_path: detection
                    .as_ref()
                    .map(|value| value.path.display().to_string()),
                version: detection.map(|value| value.version),
                snapshot_present: adapter.snapshot_present,
                takeover: takeover_label(
                    active,
                    !self.activated.load(Ordering::Acquire),
                    adapter.takeover == KimiCodeTakeoverStatus::Drifted,
                ),
                configured_model_ids,
                main_model_id,
            },
            secondary_model_id,
            agents: discover_kimi_code_agents(&self.paths).map_err(|error| error.to_string())?,
        })
    }

    pub fn apply(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<KimiCodeIntegrationStatus, String> {
        if detect_kimi_code_cli()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err("未检测到 Kimi Code CLI；安装后才能应用配置".into());
        }
        self.activate(control, gateway)?;
        self.status(control)
    }

    pub fn set_agent_model_preference(
        &self,
        name: &str,
        preference: &str,
    ) -> Result<Vec<KimiCodeAgentProfile>, String> {
        set_kimi_code_agent_model_preference(&self.paths, name, preference)
            .map_err(|error| error.to_string())?;
        discover_kimi_code_agents(&self.paths).map_err(|error| error.to_string())
    }

    fn activate(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<(), String> {
        let selection = control.client_selection("kimi_code")?;
        let configuration = control
            .state()?
            .client_configurations
            .get("kimi_code")
            .cloned()
            .ok_or_else(|| "Kimi Code configuration is missing".to_string())?;
        let token = uuid::Uuid::new_v4().to_string();
        let models = selection
            .enabled_models
            .iter()
            .map(|model| {
                let mut capabilities = vec!["tool_use"];
                if model
                    .capabilities
                    .iter()
                    .any(|capability| matches!(capability.as_str(), "image" | "vision"))
                {
                    capabilities.push("image_in");
                }
                if model
                    .capabilities
                    .iter()
                    .any(|capability| capability == "video")
                {
                    capabilities.push("video_in");
                }
                if model
                    .capabilities
                    .iter()
                    .any(|capability| matches!(capability.as_str(), "reasoning" | "thinking"))
                    || !model.protocol_capabilities.is_empty()
                {
                    capabilities.push("thinking");
                }
                KimiCodeModel::new(
                    format!("grillforge/{}", model.id),
                    &model.display_name,
                    capabilities,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let request = KimiCodeRequest::new(
            format!(
                "{}/clients/kimi-code",
                gateway.base_url.trim_end_matches('/')
            ),
            &token,
            models,
            format!("grillforge/{}", selection.main_model.id),
            configuration
                .secondary_model_id
                .map(|id| format!("grillforge/{id}")),
        )
        .map_err(|error| error.to_string())?;
        self.adapter
            .apply(request)
            .map_err(|error| error.to_string())?;
        let ids = selection
            .enabled_models
            .iter()
            .map(|model| model.id.clone())
            .collect();
        if let Err(error) = gateway.activate_client("kimi-code", ids, &token) {
            let restore = self
                .adapter
                .disable()
                .map_err(|restore| restore.to_string());
            return Err(match restore {
                Ok(_) => error,
                Err(restore) => format!("{error}; Kimi Code 配置回滚也失败: {restore}"),
            });
        }
        self.activated.store(true, Ordering::Release);
        Ok(())
    }

    pub fn disable(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<KimiCodeIntegrationStatus, String> {
        self.adapter.disable().map_err(|error| error.to_string())?;
        gateway.deactivate_client("kimi-code");
        self.activated.store(false, Ordering::Release);
        self.status(control)
    }

    pub fn recovery_pending(&self) -> bool {
        self.adapter
            .status()
            .is_ok_and(|status| status.snapshot_present)
    }

    pub fn resume_if_applied(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<bool, String> {
        match self
            .adapter
            .status()
            .map_err(|error| error.to_string())?
            .takeover
        {
            KimiCodeTakeoverStatus::Inactive | KimiCodeTakeoverStatus::Drifted => Ok(false),
            KimiCodeTakeoverStatus::Active => {
                self.activate(control, gateway)?;
                Ok(true)
            }
        }
    }
}

#[tauri::command]
pub async fn gemini_status(
    service: State<'_, GeminiIntegrationService>,
    control: State<'_, ControlPlaneService>,
) -> Result<ClientIntegrationStatus, String> {
    service.status(&control)
}

#[tauri::command]
pub fn apply_gemini(
    service: State<'_, GeminiIntegrationService>,
    control: State<'_, ControlPlaneService>,
) -> Result<ClientIntegrationStatus, String> {
    let status = service.apply(&control)?;
    if let Err(error) = control.set_client_integration_enabled("gemini", true) {
        let _ = service.disable(&control);
        return Err(error);
    }
    Ok(status)
}

#[tauri::command]
pub fn disable_gemini(
    service: State<'_, GeminiIntegrationService>,
    control: State<'_, ControlPlaneService>,
) -> Result<ClientIntegrationStatus, String> {
    let status = service.disable(&control)?;
    control.set_client_integration_enabled("gemini", false)?;
    Ok(status)
}

#[tauri::command]
pub async fn grok_build_status(
    service: State<'_, GrokBuildIntegrationService>,
    control: State<'_, ControlPlaneService>,
) -> Result<ClientIntegrationStatus, String> {
    service.status(&control)
}

#[tauri::command]
pub fn apply_grok_build(
    service: State<'_, GrokBuildIntegrationService>,
    control: State<'_, ControlPlaneService>,
) -> Result<ClientIntegrationStatus, String> {
    let status = service.apply(&control)?;
    if let Err(error) = control.set_client_integration_enabled("grok_build", true) {
        let _ = service.disable(&control);
        return Err(error);
    }
    Ok(status)
}

#[tauri::command]
pub fn disable_grok_build(
    service: State<'_, GrokBuildIntegrationService>,
    control: State<'_, ControlPlaneService>,
) -> Result<ClientIntegrationStatus, String> {
    let status = service.disable(&control)?;
    control.set_client_integration_enabled("grok_build", false)?;
    Ok(status)
}

#[tauri::command]
pub async fn opencode_status(
    service: State<'_, OpenCodeIntegrationService>,
    control: State<'_, ControlPlaneService>,
) -> Result<ClientIntegrationStatus, String> {
    service.status(&control)
}

#[tauri::command]
pub fn apply_opencode(
    service: State<'_, OpenCodeIntegrationService>,
    control: State<'_, ControlPlaneService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<ClientIntegrationStatus, String> {
    let status = service.apply(&control, &gateway)?;
    if let Err(error) = control.set_client_integration_enabled("opencode", true) {
        let _ = service.disable(&control, &gateway);
        return Err(error);
    }
    Ok(status)
}

#[tauri::command]
pub fn disable_opencode(
    service: State<'_, OpenCodeIntegrationService>,
    control: State<'_, ControlPlaneService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<ClientIntegrationStatus, String> {
    let status = service.disable(&control, &gateway)?;
    control.set_client_integration_enabled("opencode", false)?;
    Ok(status)
}

#[tauri::command]
pub async fn openclaw_status(
    service: State<'_, OpenClawIntegrationService>,
    control: State<'_, ControlPlaneService>,
) -> Result<ClientIntegrationStatus, String> {
    service.status(&control)
}

#[tauri::command]
pub fn apply_openclaw(
    service: State<'_, OpenClawIntegrationService>,
    control: State<'_, ControlPlaneService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<ClientIntegrationStatus, String> {
    let status = service.apply(&control, &gateway)?;
    if let Err(error) = control.set_client_integration_enabled("openclaw", true) {
        let _ = service.disable(&control, &gateway);
        return Err(error);
    }
    Ok(status)
}

#[tauri::command]
pub fn disable_openclaw(
    service: State<'_, OpenClawIntegrationService>,
    control: State<'_, ControlPlaneService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<ClientIntegrationStatus, String> {
    let status = service.disable(&control, &gateway)?;
    control.set_client_integration_enabled("openclaw", false)?;
    Ok(status)
}

#[tauri::command]
pub async fn hermes_status(
    service: State<'_, HermesIntegrationService>,
    control: State<'_, ControlPlaneService>,
) -> Result<ClientIntegrationStatus, String> {
    service.status(&control)
}

#[tauri::command]
pub fn apply_hermes(
    service: State<'_, HermesIntegrationService>,
    control: State<'_, ControlPlaneService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<ClientIntegrationStatus, String> {
    let status = service.apply(&control, &gateway)?;
    if let Err(error) = control.set_client_integration_enabled("hermes", true) {
        let _ = service.disable(&control, &gateway);
        return Err(error);
    }
    Ok(status)
}

#[tauri::command]
pub fn disable_hermes(
    service: State<'_, HermesIntegrationService>,
    control: State<'_, ControlPlaneService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<ClientIntegrationStatus, String> {
    let status = service.disable(&control, &gateway)?;
    control.set_client_integration_enabled("hermes", false)?;
    Ok(status)
}

#[tauri::command]
pub async fn kimi_code_status(
    service: State<'_, KimiCodeIntegrationService>,
    control: State<'_, ControlPlaneService>,
) -> Result<KimiCodeIntegrationStatus, String> {
    service.status(&control)
}

#[tauri::command]
pub fn apply_kimi_code(
    service: State<'_, KimiCodeIntegrationService>,
    control: State<'_, ControlPlaneService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<KimiCodeIntegrationStatus, String> {
    let status = service.apply(&control, &gateway)?;
    if let Err(error) = control.set_client_integration_enabled("kimi_code", true) {
        let _ = service.disable(&control, &gateway);
        return Err(error);
    }
    Ok(status)
}

#[tauri::command]
pub fn disable_kimi_code(
    service: State<'_, KimiCodeIntegrationService>,
    control: State<'_, ControlPlaneService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<KimiCodeIntegrationStatus, String> {
    let status = service.disable(&control, &gateway)?;
    control.set_client_integration_enabled("kimi_code", false)?;
    Ok(status)
}

#[tauri::command]
pub fn set_kimi_code_agent_model_preference_command(
    service: State<'_, KimiCodeIntegrationService>,
    name: String,
    preference: String,
) -> Result<Vec<KimiCodeAgentProfile>, String> {
    service.set_agent_model_preference(&name, &preference)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{ModelInput, ProviderInput};
    use crate::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
    use crate::gateway::Gateway;
    use std::fs;

    fn gateway_control(root: &std::path::Path) -> ControlPlaneService {
        let control = ControlPlaneService::new(root);
        control
            .save_provider(ProviderInput {
                id: "local".into(),
                name: "Local".into(),
                protocol: Protocol::OpenAiChatCompletions,
                endpoint: "http://127.0.0.1:9".into(),
                endpoint_mode: EndpointMode::BaseUrl,
                api_key_placement: ApiKeyPlacement::None,
                api_key: None,
                enabled: true,
                models_url: None,
            })
            .unwrap();
        control
            .save_model(ModelInput {
                id: "coder".into(),
                name: "Coder".into(),
                upstream_id: "coder-upstream".into(),
                provider_id: "local".into(),
                capabilities: vec!["coding".into()],
                protocol_capabilities: vec![],
            })
            .unwrap();
        for client in ["opencode", "openclaw", "hermes", "kimi_code"] {
            control
                .set_client_model_enabled(client.into(), "coder".into(), true)
                .unwrap();
            control
                .set_client_main_model(client.into(), Some("coder".into()))
                .unwrap();
        }
        control
    }

    #[test]
    fn restart_marks_unchanged_direct_client_configurations_active_without_cli_detection() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("grillforge");
        let gemini_paths = GeminiPaths::new(
            temp.path().join("home/.gemini/.env"),
            temp.path().join("home/.gemini/settings.json"),
        );
        let gemini = GeminiIntegrationService::new(gemini_paths, &root);
        gemini
            .adapter
            .apply(GeminiRequest::new("http://127.0.0.1:9", "token", "gemini").unwrap())
            .unwrap();
        let grok_paths = GrokBuildPaths::new(temp.path().join("home/.grok/config.toml"));
        let grok = GrokBuildIntegrationService::new(grok_paths, &root);
        grok.adapter
            .apply(GrokBuildRequest::new("http://127.0.0.1:9", "token", "grok", "Grok").unwrap())
            .unwrap();

        assert!(gemini.resume_if_applied().unwrap());
        assert!(grok.resume_if_applied().unwrap());
        assert!(gemini.activated.load(Ordering::Acquire));
        assert!(grok.activated.load(Ordering::Acquire));
    }

    #[test]
    fn restart_rebuilds_routes_for_every_unchanged_gateway_client() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("grillforge");
        let control = gateway_control(&root);
        let gateway = Gateway::new(&root).status("http://127.0.0.1:15721".into());

        let opencode = OpenCodeIntegrationService::new(
            OpenCodePaths::new(temp.path().join("home/opencode.json")),
            &root,
        );
        opencode
            .adapter
            .apply(
                OpenCodeRequest::new(
                    "http://127.0.0.1:15721/clients/opencode/v1",
                    "old-token",
                    vec![OpenCodeModel::new("grillforge/coder", "Coder").unwrap()],
                    "grillforge/coder",
                )
                .unwrap(),
            )
            .unwrap();
        let openclaw = OpenClawIntegrationService::new(
            OpenClawPaths::new(temp.path().join("home/openclaw.json")),
            &root,
        );
        openclaw
            .adapter
            .apply(
                OpenClawRequest::new(
                    "http://127.0.0.1:15721/clients/openclaw",
                    "old-token",
                    vec![
                        OpenClawModelSpec::new(
                            "grillforge/coder",
                            "Coder",
                            false,
                            vec!["text".into()],
                            128_000,
                            16_384,
                        )
                        .unwrap(),
                    ],
                    "grillforge/coder",
                    vec![],
                )
                .unwrap(),
            )
            .unwrap();
        let hermes = HermesIntegrationService::new(
            HermesPaths::new(temp.path().join("home/hermes.yaml")),
            &root,
        );
        hermes
            .adapter
            .apply(
                HermesRequest::new(
                    "http://127.0.0.1:15721/clients/hermes/v1",
                    "old-token",
                    vec![HermesModel::new("grillforge/coder", "Coder").unwrap()],
                    "grillforge/coder",
                )
                .unwrap(),
            )
            .unwrap();

        assert!(opencode.resume_if_applied(&control, &gateway).unwrap());
        assert!(openclaw.resume_if_applied(&control, &gateway).unwrap());
        assert!(hermes.resume_if_applied(&control, &gateway).unwrap());
        assert!(opencode.activated.load(Ordering::Acquire));
        assert!(openclaw.activated.load(Ordering::Acquire));
        assert!(hermes.activated.load(Ordering::Acquire));
    }

    #[test]
    fn restart_never_overwrites_a_drifted_gateway_client_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("grillforge");
        let control = gateway_control(&root);
        let gateway = Gateway::new(&root).status("http://127.0.0.1:15721".into());
        let paths = OpenCodePaths::new(temp.path().join("home/opencode.json"));
        let service = OpenCodeIntegrationService::new(paths.clone(), &root);
        service
            .adapter
            .apply(
                OpenCodeRequest::new(
                    "http://127.0.0.1:15721/clients/opencode/v1",
                    "old-token",
                    vec![OpenCodeModel::new("grillforge/coder", "Coder").unwrap()],
                    "grillforge/coder",
                )
                .unwrap(),
            )
            .unwrap();
        fs::write(&paths.config_path, b"{ userChanged: true }").unwrap();
        let changed = fs::read(&paths.config_path).unwrap();

        assert!(!service.resume_if_applied(&control, &gateway).unwrap());
        assert_eq!(fs::read(paths.config_path).unwrap(), changed);
        assert!(!service.activated.load(Ordering::Acquire));
    }

    #[test]
    fn kimi_code_activation_writes_primary_secondary_and_rebuilds_gateway_routes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("grillforge");
        let control = gateway_control(&root);
        control
            .set_client_secondary_model("kimi_code".into(), Some("coder".into()))
            .unwrap();
        let gateway = Gateway::new(&root).status("http://127.0.0.1:15721".into());
        let paths = KimiCodePaths::new(
            temp.path().join("home/.kimi-code/config.toml"),
            temp.path().join("home/.kimi-code/agents"),
            temp.path().join("home/.agents/agents"),
        );
        let service = KimiCodeIntegrationService::new(paths.clone(), &root);

        service.activate(&control, &gateway).unwrap();

        let config = fs::read_to_string(paths.config_path)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert_eq!(config["default_model"].as_str(), Some("grillforge/coder"));
        assert_eq!(
            config["secondary_model"]["model"].as_str(),
            Some("grillforge/coder")
        );
        assert!(service.activated.load(Ordering::Acquire));
    }
}
