use crate::adapters::pi::{
    PiAdapter, PiCliDetection, PiModelSpec, PiPaths, PiRequest, PiTakeoverStatus, detect_pi_cli,
    inspect_pi_cli,
};
use crate::application::{ControlPlaneService, ControlPlaneState};
use crate::gateway::GatewayStatus;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::State;

pub struct PiIntegrationService {
    adapter: PiAdapter,
    cli_path: Option<PathBuf>,
    activated_this_session: AtomicBool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiIntegrationStatus {
    pub installed: bool,
    pub executable_path: Option<String>,
    pub version: Option<String>,
    pub snapshot_present: bool,
    pub takeover: &'static str,
    pub configured_model_ids: Vec<String>,
    pub default_model_id: Option<String>,
}

impl PiIntegrationService {
    pub fn new(paths: PiPaths, grillforge_root: impl Into<PathBuf>) -> Self {
        Self {
            adapter: PiAdapter::new(paths, grillforge_root),
            cli_path: None,
            activated_this_session: AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    pub fn with_cli_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.cli_path = Some(path.into());
        self
    }

    pub fn status(&self, state: &ControlPlaneState) -> Result<PiIntegrationStatus, String> {
        let detection = self.detect().map_err(|error| error.to_string())?;
        let adapter_status = self.adapter.status().map_err(|error| error.to_string())?;
        let takeover = match adapter_status.takeover {
            PiTakeoverStatus::Inactive => "inactive",
            PiTakeoverStatus::Drifted => "drifted",
            PiTakeoverStatus::Active if !self.activated_this_session.load(Ordering::Acquire) => {
                "reapply_required"
            }
            PiTakeoverStatus::Active => "active",
        };
        Ok(PiIntegrationStatus {
            installed: detection.is_some(),
            executable_path: detection
                .as_ref()
                .map(|detection| detection.path.display().to_string()),
            version: detection.map(|detection| detection.version),
            snapshot_present: adapter_status.snapshot_present,
            takeover,
            configured_model_ids: state.pi_enabled_model_ids.clone(),
            default_model_id: state.pi_main_model_id.clone(),
        })
    }

    pub fn apply(
        &self,
        state: &ControlPlaneState,
        gateway: &GatewayStatus,
    ) -> Result<PiIntegrationStatus, String> {
        if self.detect().map_err(|error| error.to_string())?.is_none() {
            return Err("未检测到 Pi CLI；安装 Pi 后才能应用配置".into());
        }
        self.activate(state, gateway)?;
        self.status(state)
    }

    fn activate(&self, state: &ControlPlaneState, gateway: &GatewayStatus) -> Result<(), String> {
        if state.pi_enabled_model_ids.is_empty() {
            return Err("Pi 至少需要一个已启用模型".into());
        }
        let models = state
            .pi_enabled_model_ids
            .iter()
            .map(|id| {
                let model = state
                    .models
                    .iter()
                    .find(|model| &model.id == id)
                    .ok_or_else(|| format!("Pi references unknown model: {id}"))?;
                let mut input = vec!["text".into()];
                if model
                    .capabilities
                    .iter()
                    .any(|capability| matches!(capability.as_str(), "image" | "vision"))
                {
                    input.push("image".into());
                }
                PiModelSpec::new(
                    format!("grillforge/{}", model.id),
                    &model.name,
                    model
                        .capabilities
                        .iter()
                        .any(|capability| capability == "reasoning")
                        || !model.protocol_capabilities.is_empty(),
                    input,
                    // Pi requires both limits; an unknown model keeps the previous
                    // defaults rather than blocking the whole configuration.
                    model.context_window.unwrap_or(128_000),
                    model.max_output_tokens.unwrap_or(16_384),
                )
                .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let default_model = state
            .pi_main_model_id
            .as_ref()
            .map(|id| format!("grillforge/{id}"));
        let token = uuid::Uuid::new_v4().to_string();
        let request = PiRequest::new(
            format!("{}/pi", gateway.base_url.trim_end_matches('/')),
            &token,
            models,
            default_model,
        )
        .map_err(|error| error.to_string())?;

        self.adapter
            .apply(request)
            .map_err(|error| error.to_string())?;
        if let Err(error) = gateway.activate_pi(state.pi_enabled_model_ids.clone(), &token) {
            let restore = self
                .adapter
                .disable()
                .map_err(|restore| restore.to_string());
            return Err(match restore {
                Ok(_) => error,
                Err(restore) => format!("{error}; Pi 配置回滚也失败: {restore}"),
            });
        }
        self.activated_this_session.store(true, Ordering::Release);
        Ok(())
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
            PiTakeoverStatus::Inactive | PiTakeoverStatus::Drifted => Ok(false),
            PiTakeoverStatus::Active => {
                self.activate(state, gateway)?;
                Ok(true)
            }
        }
    }

    pub fn disable(
        &self,
        state: &ControlPlaneState,
        gateway: &GatewayStatus,
    ) -> Result<PiIntegrationStatus, String> {
        self.adapter.disable().map_err(|error| error.to_string())?;
        gateway.deactivate_pi();
        self.activated_this_session.store(false, Ordering::Release);
        self.status(state)
    }

    pub fn recovery_pending(&self) -> bool {
        self.adapter
            .status()
            .is_ok_and(|status| status.snapshot_present)
    }

    fn detect(&self) -> Result<Option<PiCliDetection>, crate::adapters::pi::PiAdapterError> {
        match &self.cli_path {
            Some(path) => inspect_pi_cli(path).map(Some),
            None => detect_pi_cli(),
        }
    }
}

#[tauri::command]
pub fn pi_status(
    integration: State<'_, PiIntegrationService>,
    control_plane: State<'_, ControlPlaneService>,
) -> Result<PiIntegrationStatus, String> {
    integration.status(&control_plane.state()?)
}

#[tauri::command]
pub fn apply_pi(
    integration: State<'_, PiIntegrationService>,
    control_plane: State<'_, ControlPlaneService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<PiIntegrationStatus, String> {
    let state = control_plane.state()?;
    let status = integration.apply(&state, &gateway)?;
    if let Err(error) = control_plane.set_client_integration_enabled("pi", true) {
        let restore = integration.disable(&state, &gateway);
        return Err(match restore {
            Ok(_) => error,
            Err(restore_error) => format!("{error}; Pi restore also failed: {restore_error}"),
        });
    }
    Ok(status)
}

#[tauri::command]
pub fn disable_pi(
    integration: State<'_, PiIntegrationService>,
    control_plane: State<'_, ControlPlaneService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<PiIntegrationStatus, String> {
    let status = integration.disable(&control_plane.state()?, &gateway)?;
    control_plane.set_client_integration_enabled("pi", false)?;
    Ok(status)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::application::{ModelInput, ProviderInput};
    use crate::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
    use crate::gateway::Gateway;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn apply_and_disable_connect_control_plane_gateway_and_pi_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("grillforge");
        let pi_root = temp.path().join("home/.pi/agent");
        let cli = temp.path().join("pi");
        fs::write(&cli, "#!/bin/sh\nprintf 'pi 0.42.0\\n'\n").unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();

        let control = ControlPlaneService::new(&root);
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
                context_window: None,
                max_output_tokens: None,
            })
            .unwrap();
        control.set_pi_model_enabled("coder".into(), true).unwrap();
        let state = control.set_pi_main_model(Some("coder".into())).unwrap();

        let gateway = Gateway::new(&root);
        let gateway_status = gateway.status("http://127.0.0.1:15721".into());
        let integration = PiIntegrationService::new(
            PiPaths::new(pi_root.join("models.json"), pi_root.join("settings.json")),
            &root,
        )
        .with_cli_path(cli);

        let status = integration.apply(&state, &gateway_status).unwrap();
        assert_eq!(status.takeover, "active");
        let models = fs::read_to_string(pi_root.join("models.json")).unwrap();
        assert!(models.contains("grillforge/coder"));
        assert!(!models.contains("coder-upstream"));
        assert!(!models.contains("test-key"));

        let status = integration.disable(&state, &gateway_status).unwrap();
        assert_eq!(status.takeover, "inactive");
        assert!(!pi_root.join("models.json").exists());
        assert!(!pi_root.join("settings.json").exists());
    }

    #[test]
    fn restart_resumes_an_unchanged_applied_pi_configuration_without_cli_detection() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("grillforge");
        let pi_root = temp.path().join("home/.pi/agent");
        let control = ControlPlaneService::new(&root);
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
                context_window: None,
                max_output_tokens: None,
            })
            .unwrap();
        control.set_pi_model_enabled("coder".into(), true).unwrap();
        let state = control.set_pi_main_model(Some("coder".into())).unwrap();
        let paths = PiPaths::new(pi_root.join("models.json"), pi_root.join("settings.json"));
        let first = PiIntegrationService::new(paths.clone(), &root);

        let request = PiRequest::new(
            "http://127.0.0.1:15721/pi",
            "old-token",
            vec![
                PiModelSpec::new(
                    "grillforge/coder",
                    "Coder",
                    false,
                    vec!["text".into()],
                    128_000,
                    16_384,
                )
                .unwrap(),
            ],
            Some("grillforge/coder".into()),
        )
        .unwrap();
        first.adapter.apply(request).unwrap();

        let restarted_gateway = Gateway::new(&root);
        let restarted_status = restarted_gateway.status("http://127.0.0.1:15721".into());
        let restarted = PiIntegrationService::new(paths, &root);
        assert!(
            restarted
                .resume_if_applied(&state, &restarted_status)
                .unwrap()
        );
        assert!(restarted.activated_this_session.load(Ordering::Acquire));
    }

    #[test]
    fn restart_does_not_overwrite_a_drifted_pi_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("grillforge");
        let pi_root = temp.path().join("home/.pi/agent");
        let paths = PiPaths::new(pi_root.join("models.json"), pi_root.join("settings.json"));
        let adapter = PiAdapter::new(paths.clone(), &root);
        adapter
            .apply(
                PiRequest::new(
                    "http://127.0.0.1:15721/pi",
                    "old-token",
                    vec![
                        PiModelSpec::new(
                            "grillforge/coder",
                            "Coder",
                            false,
                            vec!["text".into()],
                            128_000,
                            16_384,
                        )
                        .unwrap(),
                    ],
                    Some("grillforge/coder".into()),
                )
                .unwrap(),
            )
            .unwrap();
        fs::write(&paths.settings_path, b"{\"userChanged\":true}").unwrap();
        let changed = fs::read(&paths.settings_path).unwrap();
        let control = ControlPlaneService::new(&root);
        let gateway = Gateway::new(&root).status("http://127.0.0.1:15721".into());
        let restarted = PiIntegrationService::new(paths.clone(), &root);

        assert!(
            !restarted
                .resume_if_applied(&control.state().unwrap(), &gateway)
                .unwrap()
        );
        assert_eq!(fs::read(paths.settings_path).unwrap(), changed);
        assert!(!restarted.activated_this_session.load(Ordering::Acquire));
    }
}
