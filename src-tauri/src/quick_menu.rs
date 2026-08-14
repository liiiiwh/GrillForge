use crate::application::{ControlPlaneService, ControlPlaneState};
use crate::claude_desktop_integration::ClaudeDesktopIntegrationService;
use crate::client_integrations::{
    GeminiIntegrationService, GrokBuildIntegrationService, HermesIntegrationService,
    KimiCodeIntegrationService, OpenCodeIntegrationService,
};
use crate::codex_integration::CodexIntegrationService;
use crate::extension_integration::ExtensionIntegrationService;
use crate::gateway::GatewayStatus;
use crate::integration::IntegrationService;
use crate::pi_integration::PiIntegrationService;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{App, AppHandle, Manager, Runtime};

const TRAY_ID: &str = "grillforge-quick-menu";
const OPEN_ID: &str = "quick.open";
const REFRESH_ID: &str = "quick.refresh";
const QUIT_ID: &str = "quick.quit";
const ACTION_PREFIX: &str = "quick.action.";

const CLIENTS: [(&str, &str); 9] = [
    ("claude_code", "Claude Code"),
    ("claude_desktop", "Claude Client"),
    ("codex", "Codex"),
    ("pi", "Pi"),
    ("gemini", "Gemini CLI"),
    ("grok_build", "Grok Build"),
    ("opencode", "OpenCode"),
    ("hermes", "Hermes"),
    ("kimi_code", "Kimi Code"),
];

const MCP_CLIENTS: [&str; 7] = [
    "claude_code",
    "claude_desktop",
    "codex",
    "gemini",
    "opencode",
    "kimi_code",
    "pi",
];

#[derive(Debug, Clone, Default)]
struct ClientSnapshot {
    installed: bool,
    native_models: Vec<(String, String)>,
    native_current: Option<String>,
    custom_agents: Vec<String>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
enum QuickAction {
    SelectModel {
        client_id: String,
        slot_id: String,
        provider_id: Option<String>,
        model_id: Option<String>,
    },
    ApplyClient {
        client_id: String,
    },
    SetMounted {
        client_id: String,
        mounted: bool,
    },
    SetBinding {
        client_id: String,
        extension_id: String,
        enabled: bool,
    },
}

#[derive(Default)]
struct QuickMenuController {
    snapshots: Mutex<BTreeMap<String, ClientSnapshot>>,
    actions: Mutex<HashMap<String, QuickAction>>,
    error: Mutex<Option<String>>,
    busy: AtomicBool,
}

struct MenuActions {
    values: HashMap<String, QuickAction>,
    next: usize,
}

impl MenuActions {
    fn new() -> Self {
        Self {
            values: HashMap::new(),
            next: 0,
        }
    }

    fn insert(&mut self, action: QuickAction) -> String {
        let id = format!("{ACTION_PREFIX}{}", self.next);
        self.next += 1;
        self.values.insert(id.clone(), action);
        id
    }
}

pub fn initialize<R: Runtime>(app: &App<R>) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    {
        app.manage(QuickMenuController::default());
        let menu = loading_menu(app)?;
        let mut tray = TrayIconBuilder::with_id(TRAY_ID)
            .menu(&menu)
            .tooltip("GrillForge")
            .show_menu_on_left_click(true)
            .on_menu_event(|app, event| handle_menu_event(app, event.id().as_ref()));
        if let Some(icon) = app.default_window_icon() {
            tray = tray.icon(icon.clone()).icon_as_template(false);
        }
        tray.build(app)?;
        refresh(app.handle().clone(), true);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn loading_menu<R: Runtime>(app: &App<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(
        app,
        OPEN_ID,
        "打开主界面",
        true,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::new(app, "正在加载客户端…", false, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        QUIT_ID,
        "退出",
        true,
        None::<&str>,
    )?)?;
    Ok(menu)
}

#[cfg(target_os = "macos")]
fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    match id {
        OPEN_ID => show_main_window(app),
        REFRESH_ID => refresh(app.clone(), true),
        QUIT_ID => app.exit(0),
        _ if id.starts_with(ACTION_PREFIX) => {
            let action = app
                .state::<QuickMenuController>()
                .actions
                .lock()
                .ok()
                .and_then(|actions| actions.get(id).cloned());
            if let Some(action) = action {
                execute_action(app.clone(), action);
            }
        }
        _ => {}
    }
}

#[cfg(target_os = "macos")]
fn execute_action<R: Runtime>(app: AppHandle<R>, action: QuickAction) {
    let controller = app.state::<QuickMenuController>();
    if controller.busy.swap(true, Ordering::AcqRel) {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let result = apply_action(&app, action);
        set_error(&app, result.err());
        if let Err(error) = rebuild_menu(&app) {
            set_error(&app, Some(format!("刷新快捷菜单失败：{error}")));
        }
        app.state::<QuickMenuController>()
            .busy
            .store(false, Ordering::Release);
    });
}

#[cfg(target_os = "macos")]
fn apply_action<R: Runtime>(app: &AppHandle<R>, action: QuickAction) -> Result<(), String> {
    let control = app.state::<ControlPlaneService>();
    match action {
        QuickAction::SelectModel {
            client_id,
            slot_id,
            provider_id,
            model_id,
        } => {
            select_model(&control, &client_id, &slot_id, provider_id, model_id)?;
        }
        QuickAction::ApplyClient { client_id } => apply_client(app, &client_id)?,
        QuickAction::SetMounted { client_id, mounted } => {
            let extensions = app.state::<ExtensionIntegrationService>();
            let gateway = app.state::<GatewayStatus>();
            if mounted {
                extensions.mount_client(&control, &gateway, &client_id)?;
            } else {
                extensions.unmount_client(&control, &gateway, &client_id)?;
            }
        }
        QuickAction::SetBinding {
            client_id,
            extension_id,
            enabled,
        } => {
            app.state::<ExtensionIntegrationService>().set_binding(
                &control,
                &app.state::<GatewayStatus>(),
                &client_id,
                &extension_id,
                enabled,
            )?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn select_model(
    control: &ControlPlaneService,
    client_id: &str,
    slot_id: &str,
    provider_id: Option<String>,
    model_id: Option<String>,
) -> Result<(), String> {
    let native = provider_id.is_none();
    match (client_id, slot_id, native) {
        ("claude_code", "main", true) => {
            control.set_claude_native_model("main".into(), model_id)?;
        }
        ("claude_code", "main", false) => {
            control.set_main_model(model_id)?;
        }
        ("claude_code", slot, true) => {
            control.set_claude_native_model(slot.into(), model_id)?;
        }
        ("claude_code", slot, false) => {
            control.set_model_slot(slot.into(), model_id)?;
        }
        ("claude_desktop", slot, false) => {
            control.set_claude_desktop_model_slot(slot.into(), model_id)?;
        }
        ("codex", "main", true) => {
            control.set_codex_native_main_model(model_id)?;
        }
        ("codex", "main", false) => {
            control.set_codex_main_model(model_id)?;
        }
        ("codex", "default_subagent", true) => {
            control.set_codex_native_default_subagent_model(model_id)?;
        }
        ("codex", "default_subagent", false) => {
            control.set_codex_default_subagent_model(model_id)?;
        }
        ("codex", slot, true) if slot.starts_with("agent:") => {
            control.set_codex_native_custom_agent_model(
                slot.trim_start_matches("agent:").into(),
                model_id,
            )?;
        }
        ("codex", slot, false) if slot.starts_with("agent:") => {
            control
                .set_codex_custom_agent_model(slot.trim_start_matches("agent:").into(), model_id)?;
        }
        ("pi", "main", _) => {
            control.set_pi_main_model(model_id)?;
        }
        (client, "main", _) => {
            control.set_client_main_model(client.into(), model_id)?;
        }
        _ => return Err(format!("客户端 {client_id} 不支持模型槽位 {slot_id}")),
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn apply_client<R: Runtime>(app: &AppHandle<R>, client_id: &str) -> Result<(), String> {
    let control = app.state::<ControlPlaneService>();
    let gateway = app.state::<GatewayStatus>();
    let extensions = app.state::<ExtensionIntegrationService>();
    match client_id {
        "claude_code" => crate::integration::apply_claude_code(
            app.state::<IntegrationService>(),
            control,
            gateway,
        )
        .map(|_| ()),
        "claude_desktop" => crate::claude_desktop_integration::apply_claude_desktop(
            app.state::<ClaudeDesktopIntegrationService>(),
            control,
            extensions,
            gateway,
        )
        .map(|_| ()),
        "codex" => crate::codex_integration::apply_codex(
            app.state::<CodexIntegrationService>(),
            control,
            extensions,
            gateway,
        )
        .map(|_| ()),
        "pi" => {
            crate::pi_integration::apply_pi(app.state::<PiIntegrationService>(), control, gateway)
                .map(|_| ())
        }
        "gemini" => crate::client_integrations::apply_gemini(
            app.state::<GeminiIntegrationService>(),
            control,
            extensions,
            gateway,
        )
        .map(|_| ()),
        "grok_build" => crate::client_integrations::apply_grok_build(
            app.state::<GrokBuildIntegrationService>(),
            control,
            gateway,
        )
        .map(|_| ()),
        "opencode" => crate::client_integrations::apply_opencode(
            app.state::<OpenCodeIntegrationService>(),
            control,
            extensions,
            gateway,
        )
        .map(|_| ()),
        "hermes" => crate::client_integrations::apply_hermes(
            app.state::<HermesIntegrationService>(),
            control,
            gateway,
        )
        .map(|_| ()),
        "kimi_code" => crate::client_integrations::apply_kimi_code(
            app.state::<KimiCodeIntegrationService>(),
            control,
            gateway,
        )
        .map(|_| ()),
        _ => Err(format!("不支持的客户端：{client_id}")),
    }
}

#[cfg(target_os = "macos")]
fn refresh<R: Runtime>(app: AppHandle<R>, inspect_clients: bool) {
    let controller = app.state::<QuickMenuController>();
    if controller.busy.swap(true, Ordering::AcqRel) {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        if inspect_clients {
            let snapshots = collect_snapshots(&app);
            if let Ok(mut current) = app.state::<QuickMenuController>().snapshots.lock() {
                *current = snapshots;
            }
        }
        if let Err(error) = rebuild_menu(&app) {
            set_error(&app, Some(format!("刷新快捷菜单失败：{error}")));
        }
        app.state::<QuickMenuController>()
            .busy
            .store(false, Ordering::Release);
    });
}

#[cfg(target_os = "macos")]
fn collect_snapshots<R: Runtime>(app: &AppHandle<R>) -> BTreeMap<String, ClientSnapshot> {
    let control = app.state::<ControlPlaneService>();
    let mut snapshots = BTreeMap::new();

    let mut claude = ClientSnapshot::default();
    match crate::adapters::claude_code::detect_claude_cli() {
        Ok(detection) => claude.installed = detection.is_some(),
        Err(error) => claude.error = Some(error.to_string()),
    }
    match app.state::<IntegrationService>().status() {
        Ok(status) => {
            claude.native_current = status.native_current_model;
            claude.native_models = status
                .native_models
                .into_iter()
                .map(|model| (model.id, model.name))
                .collect();
        }
        Err(error) => claude.error = Some(error),
    }
    snapshots.insert("claude_code".into(), claude);

    let mut desktop = ClientSnapshot::default();
    match app.state::<ClaudeDesktopIntegrationService>().status() {
        Ok(status) => {
            desktop.installed = status.installed;
            desktop.native_current = status.native_current_model;
            desktop.native_models = status
                .native_models
                .into_iter()
                .map(|model| (model.id, model.name))
                .collect();
        }
        Err(error) => desktop.error = Some(error),
    }
    snapshots.insert("claude_desktop".into(), desktop);

    let mut codex = ClientSnapshot::default();
    match app.state::<CodexIntegrationService>().status(&control) {
        Ok(status) => {
            codex.installed = status.installed;
            codex.native_current = status.current_config_model;
            codex.native_models = status
                .native_models
                .into_iter()
                .map(|model| (model.id, model.name))
                .collect();
            codex.custom_agents = status
                .custom_agents
                .into_iter()
                .map(|agent| agent.name)
                .collect();
        }
        Err(error) => codex.error = Some(error),
    }
    snapshots.insert("codex".into(), codex);

    let state = control.state();
    let mut pi = ClientSnapshot::default();
    match state
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|state| app.state::<PiIntegrationService>().status(state))
    {
        Ok(status) => pi.installed = status.installed,
        Err(error) => pi.error = Some(error),
    }
    snapshots.insert("pi".into(), pi);

    collect_generic_snapshot(
        &mut snapshots,
        "gemini",
        app.state::<GeminiIntegrationService>().status(&control),
    );
    collect_generic_snapshot(
        &mut snapshots,
        "grok_build",
        app.state::<GrokBuildIntegrationService>().status(&control),
    );
    collect_generic_snapshot(
        &mut snapshots,
        "opencode",
        app.state::<OpenCodeIntegrationService>().status(&control),
    );
    collect_generic_snapshot(
        &mut snapshots,
        "hermes",
        app.state::<HermesIntegrationService>().status(&control),
    );
    match app.state::<KimiCodeIntegrationService>().status(&control) {
        Ok(status) => {
            snapshots.insert(
                "kimi_code".into(),
                ClientSnapshot {
                    installed: status.client.installed,
                    ..ClientSnapshot::default()
                },
            );
        }
        Err(error) => {
            snapshots.insert(
                "kimi_code".into(),
                ClientSnapshot {
                    error: Some(error),
                    ..ClientSnapshot::default()
                },
            );
        }
    }
    snapshots
}

#[cfg(target_os = "macos")]
fn collect_generic_snapshot(
    snapshots: &mut BTreeMap<String, ClientSnapshot>,
    id: &str,
    status: Result<crate::client_integrations::ClientIntegrationStatus, String>,
) {
    let snapshot = match status {
        Ok(status) => ClientSnapshot {
            installed: status.installed,
            ..ClientSnapshot::default()
        },
        Err(error) => ClientSnapshot {
            error: Some(error),
            ..ClientSnapshot::default()
        },
    };
    snapshots.insert(id.into(), snapshot);
}

#[cfg(target_os = "macos")]
fn rebuild_menu<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let state = app.state::<ControlPlaneService>().state()?;
    let snapshots = app
        .state::<QuickMenuController>()
        .snapshots
        .lock()
        .map_err(|_| "快捷菜单客户端状态锁已损坏".to_string())?
        .clone();
    let error = app
        .state::<QuickMenuController>()
        .error
        .lock()
        .map_err(|_| "快捷菜单错误状态锁已损坏".to_string())?
        .clone();
    let mut actions = MenuActions::new();
    let menu = Menu::new(app).map_err(|error| error.to_string())?;
    menu.append(
        &MenuItem::with_id(app, OPEN_ID, "打开主界面", true, None::<&str>)
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if let Some(error) = error {
        menu.append(
            &MenuItem::new(app, format!("操作失败：{error}"), false, None::<&str>)
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    }
    menu.append(&PredefinedMenuItem::separator(app).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;

    for (client_id, name) in CLIENTS {
        let snapshot = snapshots.get(client_id).cloned().unwrap_or_default();
        let label = if snapshot.error.is_some() {
            format!("{name}（检测失败）")
        } else if snapshot.installed {
            name.to_string()
        } else {
            format!("{name}（未安装）")
        };
        let client_menu =
            Submenu::new(app, label, snapshot.installed).map_err(|error| error.to_string())?;
        append_client_menu(
            app,
            &client_menu,
            &state,
            client_id,
            &snapshot,
            &mut actions,
        )?;
        menu.append(&client_menu)
            .map_err(|error| error.to_string())?;
    }

    menu.append(&PredefinedMenuItem::separator(app).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    menu.append(
        &MenuItem::with_id(app, REFRESH_ID, "刷新客户端", true, None::<&str>)
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    menu.append(
        &MenuItem::with_id(app, QUIT_ID, "退出", true, None::<&str>)
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    *app.state::<QuickMenuController>()
        .actions
        .lock()
        .map_err(|_| "快捷菜单动作锁已损坏".to_string())? = actions.values;
    app.tray_by_id(TRAY_ID)
        .ok_or_else(|| "快捷菜单图标不存在".to_string())?
        .set_menu(Some(menu))
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn append_client_menu<R: Runtime>(
    app: &AppHandle<R>,
    menu: &Submenu<R>,
    state: &ControlPlaneState,
    client_id: &str,
    snapshot: &ClientSnapshot,
    actions: &mut MenuActions,
) -> Result<(), String> {
    let model_menu = Submenu::new(app, "模型配置", true).map_err(|error| error.to_string())?;
    for (slot_id, slot_name) in client_slots(state, client_id, snapshot) {
        append_slot_menu(
            app,
            &model_menu,
            state,
            client_id,
            &slot_id,
            &slot_name,
            snapshot,
            actions,
        )?;
    }
    menu.append(&model_menu)
        .map_err(|error| error.to_string())?;

    if MCP_CLIENTS.contains(&client_id) {
        let extension_menu =
            Submenu::new(app, "扩展 Agent", true).map_err(|error| error.to_string())?;
        let mounted = state
            .mcp_mounted_client_ids
            .iter()
            .any(|value| value == client_id);
        let mount_id = actions.insert(QuickAction::SetMounted {
            client_id: client_id.into(),
            mounted: !mounted,
        });
        extension_menu
            .append(
                &CheckMenuItem::with_id(app, mount_id, "启用扩展", true, mounted, None::<&str>)
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        extension_menu
            .append(&PredefinedMenuItem::separator(app).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
        let bindings = state
            .client_extension_subagent_ids
            .get(client_id)
            .cloned()
            .unwrap_or_default();
        for extension in &state.extension_subagents {
            let enabled = bindings.contains(&extension.id);
            let id = actions.insert(QuickAction::SetBinding {
                client_id: client_id.into(),
                extension_id: extension.id.clone(),
                enabled: !enabled,
            });
            extension_menu
                .append(
                    &CheckMenuItem::with_id(app, id, &extension.name, true, enabled, None::<&str>)
                        .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
        }
        if state.extension_subagents.is_empty() {
            extension_menu
                .append(
                    &MenuItem::new(app, "尚未定义扩展 Agent", false, None::<&str>)
                        .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
        }
        menu.append(&extension_menu)
            .map_err(|error| error.to_string())?;
    }

    menu.append(&PredefinedMenuItem::separator(app).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let apply_id = actions.insert(QuickAction::ApplyClient {
        client_id: client_id.into(),
    });
    menu.append(
        &MenuItem::with_id(app, apply_id, "应用配置", true, None::<&str>)
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if let Some(error) = &snapshot.error {
        menu.append(
            &MenuItem::new(app, format!("检测错误：{error}"), false, None::<&str>)
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn client_slots(
    state: &ControlPlaneState,
    client_id: &str,
    snapshot: &ClientSnapshot,
) -> Vec<(String, String)> {
    match client_id {
        "claude_code" => vec![
            ("main".into(), "默认模型".into()),
            ("sonnet".into(), "Sonnet".into()),
            ("opus".into(), "Opus".into()),
            ("fable".into(), "Fable".into()),
            ("haiku".into(), "Haiku".into()),
            ("subagent_default".into(), "SubAgent 默认".into()),
        ],
        "claude_desktop" => vec![
            ("sonnet".into(), "Sonnet".into()),
            ("opus".into(), "Opus".into()),
            ("fable".into(), "Fable".into()),
            ("haiku".into(), "Haiku".into()),
        ],
        "codex" => {
            let mut slots = vec![
                ("main".into(), "默认模型".into()),
                ("default_subagent".into(), "SubAgent 默认".into()),
            ];
            slots.extend(
                snapshot
                    .custom_agents
                    .iter()
                    .map(|name| (format!("agent:{name}"), name.clone())),
            );
            slots
        }
        _ if state.client_configurations.contains_key(client_id) || client_id == "pi" => {
            vec![("main".into(), "默认模型".into())]
        }
        _ => Vec::new(),
    }
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn append_slot_menu<R: Runtime>(
    app: &AppHandle<R>,
    parent: &Submenu<R>,
    state: &ControlPlaneState,
    client_id: &str,
    slot_id: &str,
    slot_name: &str,
    snapshot: &ClientSnapshot,
    actions: &mut MenuActions,
) -> Result<(), String> {
    let slot_menu = Submenu::new(app, slot_name, true).map_err(|error| error.to_string())?;
    let current = current_selection(state, client_id, slot_id);

    let native_menu = Submenu::new(app, "跟随原生", client_id != "claude_desktop")
        .map_err(|error| error.to_string())?;
    if client_id != "claude_desktop" {
        let default_id = actions.insert(QuickAction::SelectModel {
            client_id: client_id.into(),
            slot_id: slot_id.into(),
            provider_id: None,
            model_id: None,
        });
        native_menu
            .append(
                &CheckMenuItem::with_id(
                    app,
                    default_id,
                    "默认",
                    true,
                    current.is_none(),
                    None::<&str>,
                )
                .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        for (model_id, name) in &snapshot.native_models {
            let id = actions.insert(QuickAction::SelectModel {
                client_id: client_id.into(),
                slot_id: slot_id.into(),
                provider_id: None,
                model_id: Some(model_id.clone()),
            });
            native_menu
                .append(
                    &CheckMenuItem::with_id(
                        app,
                        id,
                        name,
                        true,
                        current.as_deref() == Some(model_id),
                        None::<&str>,
                    )
                    .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
        }
    } else {
        native_menu
            .append(
                &CheckMenuItem::new(
                    app,
                    snapshot
                        .native_current
                        .as_deref()
                        .unwrap_or("当前 Claude Client 模型"),
                    false,
                    current.is_none(),
                    None::<&str>,
                )
                .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
    }
    slot_menu
        .append(&native_menu)
        .map_err(|error| error.to_string())?;

    for provider in state.providers.iter().filter(|provider| provider.enabled) {
        let provider_menu =
            Submenu::new(app, &provider.name, true).map_err(|error| error.to_string())?;
        let models = state
            .models
            .iter()
            .filter(|model| model.provider_id == provider.id)
            .collect::<Vec<_>>();
        for model in &models {
            let id = actions.insert(QuickAction::SelectModel {
                client_id: client_id.into(),
                slot_id: slot_id.into(),
                provider_id: Some(provider.id.clone()),
                model_id: Some(model.id.clone()),
            });
            provider_menu
                .append(
                    &CheckMenuItem::with_id(
                        app,
                        id,
                        &model.name,
                        true,
                        current.as_deref() == Some(&model.id),
                        None::<&str>,
                    )
                    .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
        }
        if models.is_empty() {
            provider_menu
                .append(
                    &MenuItem::new(app, "无可用模型", false, None::<&str>)
                        .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
        }
        slot_menu
            .append(&provider_menu)
            .map_err(|error| error.to_string())?;
    }
    parent.append(&slot_menu).map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn current_selection(state: &ControlPlaneState, client_id: &str, slot_id: &str) -> Option<String> {
    match (client_id, slot_id) {
        ("claude_code", "main") => state
            .main_model_id
            .clone()
            .or_else(|| state.claude_native_model_slots.get("main").cloned()),
        ("claude_code", slot) => state
            .model_slots
            .get(slot)
            .cloned()
            .or_else(|| state.claude_native_model_slots.get(slot).cloned()),
        ("claude_desktop", slot) => state.claude_desktop_model_slots.get(slot).cloned(),
        ("codex", "main") => state
            .codex_main_model_id
            .clone()
            .or_else(|| state.codex_native_model_slots.get("main").cloned()),
        ("codex", "default_subagent") => state
            .codex_agent_model_ids
            .get("default_subagent")
            .cloned()
            .or_else(|| {
                state
                    .codex_native_model_slots
                    .get("default_subagent")
                    .cloned()
            }),
        ("codex", slot) if slot.starts_with("agent:") => {
            let agent = slot.trim_start_matches("agent:");
            state.codex_agent_model_ids.get(agent).cloned().or_else(|| {
                state
                    .codex_native_model_slots
                    .get(&format!("agent_{agent}"))
                    .cloned()
            })
        }
        ("pi", "main") => state.pi_main_model_id.clone(),
        (client, "main") => state
            .client_configurations
            .get(client)
            .and_then(|config| config.main_model_id.clone()),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn set_error<R: Runtime>(app: &AppHandle<R>, error: Option<String>) {
    if let Ok(mut current) = app.state::<QuickMenuController>().error.lock() {
        *current = error;
    }
}

#[cfg(target_os = "macos")]
fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
