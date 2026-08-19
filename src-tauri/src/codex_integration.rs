use crate::adapters::codex::{
    CodexAdapter, CodexCliDetection, CodexCustomAgent, CodexNativeModel, CodexPaths,
    CodexTakeoverStatus, detect_codex_cli, inspect_codex_native_models,
};
use crate::application::ControlPlaneService;
use crate::extension_integration::ExtensionIntegrationService;
use crate::gateway::GatewayStatus;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::State;

pub struct CodexIntegrationService {
    adapter: CodexAdapter,
    activated_this_session: AtomicBool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexIntegrationStatus {
    pub installed: bool,
    pub executable_path: Option<String>,
    pub version: Option<String>,
    pub snapshot_present: bool,
    pub takeover: &'static str,
    pub configured_model_id: Option<String>,
    pub current_config_model: Option<String>,
    pub current_config_provider: Option<String>,
    pub supported_protocols: Vec<&'static str>,
    pub native_models: Vec<CodexNativeModel>,
    pub native_models_error: Option<String>,
    pub custom_agents: Vec<CodexCustomAgent>,
}

impl CodexIntegrationService {
    pub fn new(paths: CodexPaths, grillforge_root: impl Into<PathBuf>) -> Self {
        Self {
            adapter: CodexAdapter::new(paths, grillforge_root),
            activated_this_session: AtomicBool::new(false),
        }
    }

    pub fn status(&self, control: &ControlPlaneService) -> Result<CodexIntegrationStatus, String> {
        let detection = detect_codex_cli().map_err(|error| error.to_string())?;
        let (native_models, native_models_error) = inspect_native_models_for(detection.as_ref());
        let adapter = self.adapter.status().map_err(|error| error.to_string())?;
        let configured = self
            .adapter
            .configured_model()
            .map_err(|error| error.to_string())?;
        let state = control.state()?;
        let takeover = match adapter.takeover {
            CodexTakeoverStatus::Inactive => "inactive",
            CodexTakeoverStatus::Drifted => "drifted",
            CodexTakeoverStatus::Active if !self.activated_this_session.load(Ordering::Acquire) => {
                "reapply_required"
            }
            CodexTakeoverStatus::Active => "active",
        };
        Ok(CodexIntegrationStatus {
            installed: detection.is_some(),
            executable_path: detection
                .as_ref()
                .map(|detection| detection.path.display().to_string()),
            version: detection.map(|detection| detection.version),
            snapshot_present: adapter.snapshot_present,
            takeover,
            configured_model_id: state.codex_main_model_id,
            current_config_model: configured.as_ref().map(|value| value.model.clone()),
            current_config_provider: configured.and_then(|value| value.provider),
            supported_protocols: vec!["open_ai_responses", "open_ai_chat_completions"],
            native_models,
            native_models_error,
            custom_agents: self
                .adapter
                .custom_agents()
                .map_err(|error| error.to_string())?,
        })
    }

    pub fn apply(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<CodexIntegrationStatus, String> {
        if detect_codex_cli()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err("未检测到 Codex CLI；安装 Codex 后才能应用配置".into());
        }
        self.activate(control, gateway)?;
        self.activated_this_session.store(true, Ordering::Release);
        self.status(control)
    }

    fn activate(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<(), String> {
        let model_ids = control.codex_route_model_ids()?;
        let token = uuid::Uuid::new_v4().to_string();
        let current = self
            .adapter
            .configured_model()
            .map_err(|error| error.to_string())?;
        self.adapter
            .apply(control.codex_request(&gateway.base_url, &token, current.as_ref())?)
            .map_err(|error| error.to_string())?;
        if model_ids.is_empty() {
            gateway.deactivate_codex();
            return Ok(());
        }
        if let Err(error) = gateway.activate_codex(model_ids, &token) {
            let restore = self
                .adapter
                .disable()
                .map_err(|restore| restore.to_string());
            return Err(match restore {
                Ok(_) => error,
                Err(restore) => format!("{error}; Codex 配置回滚也失败: {restore}"),
            });
        }
        Ok(())
    }

    pub fn disable(
        &self,
        control: &ControlPlaneService,
        gateway: &GatewayStatus,
    ) -> Result<CodexIntegrationStatus, String> {
        self.adapter.disable().map_err(|error| error.to_string())?;
        gateway.deactivate_codex();
        self.activated_this_session.store(false, Ordering::Release);
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
            CodexTakeoverStatus::Inactive | CodexTakeoverStatus::Drifted => Ok(false),
            CodexTakeoverStatus::Active => {
                self.activate(control, gateway)?;
                self.activated_this_session.store(true, Ordering::Release);
                Ok(true)
            }
        }
    }
}

fn inspect_native_models_for(
    detection: Option<&CodexCliDetection>,
) -> (Vec<CodexNativeModel>, Option<String>) {
    detection
        .map(
            |detection| match inspect_codex_native_models(&detection.path) {
                Ok(models) => (models, None),
                Err(error) => (Vec::new(), Some(error.to_string())),
            },
        )
        .unwrap_or_default()
}

#[tauri::command(async)]
pub fn codex_status(
    integration: State<'_, CodexIntegrationService>,
    control: State<'_, ControlPlaneService>,
) -> Result<CodexIntegrationStatus, String> {
    integration.status(&control)
}

#[tauri::command(async)]
pub fn apply_codex(
    integration: State<'_, CodexIntegrationService>,
    control: State<'_, ControlPlaneService>,
    extensions: State<'_, ExtensionIntegrationService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<CodexIntegrationStatus, String> {
    let state = control.state()?;
    let status = extensions.with_suspended_client(&state, &gateway, "codex", || {
        integration.apply(&control, &gateway)
    })?;
    if let Err(error) = control.set_client_integration_enabled("codex", true) {
        let restore =
            extensions.with_suspended_client(&control.state()?, &gateway, "codex", || {
                integration.disable(&control, &gateway)
            });
        return Err(match restore {
            Ok(_) => error,
            Err(restore_error) => format!("{error}; Codex restore also failed: {restore_error}"),
        });
    }
    Ok(status)
}

#[tauri::command(async)]
pub fn disable_codex(
    integration: State<'_, CodexIntegrationService>,
    control: State<'_, ControlPlaneService>,
    extensions: State<'_, ExtensionIntegrationService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<CodexIntegrationStatus, String> {
    let state = control.state()?;
    let status = extensions.with_suspended_client(&state, &gateway, "codex", || {
        integration.disable(&control, &gateway)
    })?;
    control.set_client_integration_enabled("codex", false)?;
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::codex::CodexRequest;
    use crate::application::{ModelInput, ProviderInput};
    use crate::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
    use crate::gateway::Gateway;
    use std::fs;

    fn configured_control(root: &std::path::Path) -> ControlPlaneService {
        let control = ControlPlaneService::new(root);
        control
            .save_provider(ProviderInput {
                id: "provider".into(),
                name: "Provider".into(),
                protocol: Protocol::OpenAiResponses,
                endpoint: "http://127.0.0.1:9".into(),
                endpoint_mode: EndpointMode::BaseUrl,
                api_key_placement: ApiKeyPlacement::Bearer,
                api_key: Some("test-key".into()),
                enabled: true,
                models_url: None,
            })
            .unwrap();
        control
            .save_model(ModelInput {
                id: "coder".into(),
                name: "Coder".into(),
                upstream_id: "coder".into(),
                provider_id: "provider".into(),
                capabilities: vec!["coding".into()],
                protocol_capabilities: vec![],
                context_window: None,
                max_output_tokens: None,
            })
            .unwrap();
        control.set_codex_main_model(Some("coder".into())).unwrap();
        control
    }

    #[cfg(unix)]
    #[test]
    fn model_catalog_failure_does_not_hide_codex_status() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("codex");
        fs::write(
            &executable,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo codex-cli-test; exit 0; fi\necho catalog-failed >&2\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let service = CodexIntegrationService::new(
            CodexPaths::new(temp.path().join("home/.codex/config.toml")),
            temp.path().join("grillforge"),
        );
        let control = ControlPlaneService::new(temp.path().join("grillforge"));
        let detection = crate::adapters::codex::inspect_codex_cli(&executable).unwrap();

        let (native_models, native_models_error) = inspect_native_models_for(Some(&detection));

        assert!(native_models.is_empty());
        assert!(native_models_error.unwrap().contains("model catalog"));
        assert!(service.adapter.status().is_ok());
        assert!(control.state().is_ok());
    }

    #[test]
    fn restart_marks_an_unchanged_applied_codex_configuration_active_without_cli_detection() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("grillforge");
        let paths = CodexPaths::new(temp.path().join("home/.codex/config.toml"));
        CodexAdapter::new(paths.clone(), &root)
            .apply(CodexRequest::new("http://127.0.0.1:15721/v1", "token", "coder").unwrap())
            .unwrap();
        let restarted = CodexIntegrationService::new(paths, &root);
        let control = configured_control(&root);
        let gateway = Gateway::new(&root).status("http://127.0.0.1:15721".into());

        assert!(restarted.resume_if_applied(&control, &gateway).unwrap());
        assert!(restarted.activated_this_session.load(Ordering::Acquire));
    }

    #[test]
    fn restart_does_not_mark_a_drifted_codex_configuration_active() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("grillforge");
        let paths = CodexPaths::new(temp.path().join("home/.codex/config.toml"));
        CodexAdapter::new(paths.clone(), &root)
            .apply(CodexRequest::new("http://127.0.0.1:15721/v1", "token", "coder").unwrap())
            .unwrap();
        fs::write(&paths.config_path, "model = \"user-change\"\n").unwrap();
        let changed = fs::read(&paths.config_path).unwrap();
        let restarted = CodexIntegrationService::new(paths.clone(), &root);
        let control = configured_control(&root);
        let gateway = Gateway::new(&root).status("http://127.0.0.1:15721".into());

        assert!(!restarted.resume_if_applied(&control, &gateway).unwrap());
        assert_eq!(fs::read(paths.config_path).unwrap(), changed);
        assert!(!restarted.activated_this_session.load(Ordering::Acquire));
    }
}
