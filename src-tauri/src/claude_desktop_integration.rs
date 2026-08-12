#[cfg(windows)]
use crate::adapters::claude_desktop::windows_paths_from_local_app_data;
use crate::adapters::claude_desktop::{
    ClaudeDesktopAdapter, ClaudeDesktopPaths, ClaudeDesktopRequest, ClaudeDesktopRouteSpec,
    ClaudeDesktopTakeoverStatus, detect_claude_client, macos_paths_from_home,
};
use crate::application::{ControlPlaneService, ControlPlaneState};
use crate::extension_integration::ExtensionIntegrationService;
use crate::gateway::{GatewayStatus, RouteSpec};
use crate::integration::IntegrationTakeover;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::State;
use uuid::Uuid;

pub const DESKTOP_MODEL_SLOTS: [(&str, &str); 4] = [
    ("sonnet", "claude-sonnet-5"),
    ("opus", "claude-opus-5"),
    ("fable", "claude-fable-5"),
    ("haiku", "claude-haiku-4-5"),
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeDesktopIntegrationStatus {
    pub installed: bool,
    pub executable_path: Option<String>,
    pub snapshot_present: bool,
    pub takeover: IntegrationTakeover,
    pub differences: Vec<String>,
    pub configured_routes: Vec<String>,
    pub supported_model_slots: Vec<&'static str>,
}

#[derive(Clone)]
struct ActiveConfiguration {
    routes: Vec<RouteSpec>,
    token: String,
}

pub struct ClaudeDesktopIntegrationService {
    adapter: ClaudeDesktopAdapter,
    activated_this_session: AtomicBool,
    active: Mutex<Option<ActiveConfiguration>>,
}

impl ClaudeDesktopIntegrationService {
    pub fn new(paths: ClaudeDesktopPaths, grillforge_root: impl Into<PathBuf>) -> Self {
        let grillforge_root = grillforge_root.into();
        Self {
            adapter: ClaudeDesktopAdapter::new(paths, &grillforge_root),
            activated_this_session: AtomicBool::new(false),
            active: Mutex::new(None),
        }
    }

    pub fn apply(
        &self,
        state: &ControlPlaneState,
        gateway: &GatewayStatus,
    ) -> Result<ClaudeDesktopIntegrationStatus, String> {
        let mut gateway_routes = Vec::new();
        let mut profile_routes = Vec::new();
        for (slot, route_id) in DESKTOP_MODEL_SLOTS {
            let Some(model_id) = state.claude_desktop_model_slots.get(slot) else {
                continue;
            };
            let model = state
                .models
                .iter()
                .find(|model| &model.id == model_id)
                .ok_or_else(|| {
                    format!("Claude Client {slot} 槽位引用了不存在的模型: {model_id}")
                })?;
            let label = format!("{} · GrillForge", model.name);
            gateway_routes.push(RouteSpec {
                route_id: route_id.into(),
                model_id: model.id.clone(),
                label_override: Some(label.clone()),
                supports_1m: false,
            });
            profile_routes.push(ClaudeDesktopRouteSpec::new(route_id, Some(label), false));
        }
        if gateway_routes.is_empty() {
            return Err("Claude Client 至少需要配置一个对话/Cowork 模型槽位".into());
        }

        let token = Uuid::new_v4().simple().to_string();
        let previous = self
            .active
            .lock()
            .map_err(|_| "Claude Client 活动路由锁已损坏".to_string())?
            .clone();
        gateway.activate_claude_desktop(gateway_routes.clone(), &token)?;
        let profile_url = format!("{}/claude-desktop", gateway.base_url.trim_end_matches('/'));
        if let Err(error) = self.adapter.apply(ClaudeDesktopRequest::new(
            profile_url,
            &token,
            profile_routes,
        )) {
            let restore = match previous {
                Some(previous) => gateway.activate_claude_desktop(previous.routes, &previous.token),
                None => {
                    gateway.deactivate_claude_desktop();
                    Ok(())
                }
            };
            return Err(match restore {
                Ok(()) => error.to_string(),
                Err(restore_error) => {
                    format!("{error}; Claude Client 上一条路由恢复失败: {restore_error}")
                }
            });
        }
        *self
            .active
            .lock()
            .map_err(|_| "Claude Client 活动路由锁已损坏".to_string())? =
            Some(ActiveConfiguration {
                routes: gateway_routes,
                token,
            });
        self.activated_this_session.store(true, Ordering::Release);
        self.status()
    }

    pub fn status(&self) -> Result<ClaudeDesktopIntegrationStatus, String> {
        let adapter = self.adapter.status().map_err(|error| error.to_string())?;
        let takeover = match adapter.takeover {
            ClaudeDesktopTakeoverStatus::Inactive => IntegrationTakeover::Inactive,
            ClaudeDesktopTakeoverStatus::Drifted => IntegrationTakeover::Drifted,
            ClaudeDesktopTakeoverStatus::Active
                if adapter.snapshot_present
                    && !self.activated_this_session.load(Ordering::Acquire) =>
            {
                IntegrationTakeover::ReapplyRequired
            }
            ClaudeDesktopTakeoverStatus::Active => IntegrationTakeover::Active,
        };
        let detection = detect_claude_client();
        let configured_routes = self
            .active
            .lock()
            .map_err(|_| "Claude Client 活动路由锁已损坏".to_string())?
            .as_ref()
            .map(|active| {
                active
                    .routes
                    .iter()
                    .map(|route| route.route_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        Ok(ClaudeDesktopIntegrationStatus {
            installed: detection.is_some(),
            executable_path: detection.map(|item| item.executable_path.display().to_string()),
            snapshot_present: adapter.snapshot_present,
            takeover,
            differences: adapter.differences,
            configured_routes,
            supported_model_slots: DESKTOP_MODEL_SLOTS.iter().map(|(slot, _)| *slot).collect(),
        })
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
            ClaudeDesktopTakeoverStatus::Inactive | ClaudeDesktopTakeoverStatus::Drifted => {
                Ok(false)
            }
            ClaudeDesktopTakeoverStatus::Active => {
                self.apply(state, gateway)?;
                Ok(true)
            }
        }
    }

    pub fn disable(
        &self,
        gateway: &GatewayStatus,
    ) -> Result<ClaudeDesktopIntegrationStatus, String> {
        let current = self.status()?;
        if current.snapshot_present {
            self.adapter.disable().map_err(|error| error.to_string())?;
        } else if current.takeover != IntegrationTakeover::Inactive {
            return Err("Claude Client 存在 GrillForge 配置，但缺少可恢复快照".into());
        }
        gateway.deactivate_claude_desktop();
        *self
            .active
            .lock()
            .map_err(|_| "Claude Client 活动路由锁已损坏".to_string())? = None;
        self.activated_this_session.store(false, Ordering::Release);
        self.status()
    }
}

pub fn default_claude_desktop_paths(home: &Path) -> ClaudeDesktopPaths {
    #[cfg(target_os = "macos")]
    {
        macos_paths_from_home(home)
    }
    #[cfg(windows)]
    {
        let local_app_data = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Local"));
        windows_paths_from_local_app_data(local_app_data)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        macos_paths_from_home(home)
    }
}

#[tauri::command]
pub async fn claude_desktop_status(
    integration: State<'_, ClaudeDesktopIntegrationService>,
) -> Result<ClaudeDesktopIntegrationStatus, String> {
    integration.status()
}

#[tauri::command]
pub fn apply_claude_desktop(
    integration: State<'_, ClaudeDesktopIntegrationService>,
    control_plane: State<'_, ControlPlaneService>,
    extensions: State<'_, ExtensionIntegrationService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<ClaudeDesktopIntegrationStatus, String> {
    let state = control_plane.state()?;
    let status = extensions.with_suspended_client(&state, &gateway, "claude_desktop", || {
        integration.apply(&state, &gateway)
    })?;
    if let Err(error) = control_plane.set_client_integration_enabled("claude_desktop", true) {
        let restore = extensions.with_suspended_client(
            &control_plane.state()?,
            &gateway,
            "claude_desktop",
            || integration.disable(&gateway),
        );
        return Err(match restore {
            Ok(_) => error,
            Err(restore_error) => {
                format!("{error}; Claude Client restore also failed: {restore_error}")
            }
        });
    }
    Ok(status)
}

#[tauri::command]
pub fn disable_claude_desktop(
    integration: State<'_, ClaudeDesktopIntegrationService>,
    control_plane: State<'_, ControlPlaneService>,
    extensions: State<'_, ExtensionIntegrationService>,
    gateway: State<'_, GatewayStatus>,
) -> Result<ClaudeDesktopIntegrationStatus, String> {
    let state = control_plane.state()?;
    let status = extensions.with_suspended_client(&state, &gateway, "claude_desktop", || {
        integration.disable(&gateway)
    })?;
    control_plane.set_client_integration_enabled("claude_desktop", false)?;
    Ok(status)
}
