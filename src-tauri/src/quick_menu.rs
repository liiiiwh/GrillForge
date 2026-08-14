use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

const TRAY_ID: &str = "grillforge-quick-menu";
const OPEN_ID: &str = "quick.open";
const QUIT_ID: &str = "quick.quit";

pub fn initialize<R: Runtime>(app: &App<R>) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let panel = WebviewWindowBuilder::new(
            app,
            "quick-menu",
            WebviewUrl::App("index.html?window=quick-menu".into()),
        )
        .title("GrillForge")
        .inner_size(520.0, 680.0)
        .min_inner_size(420.0, 420.0)
        .decorations(false)
        .resizable(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build()?;
        let panel_for_focus = panel.clone();
        panel.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Focused(false)) {
                let _ = panel_for_focus.hide();
            }
        });

        let menu = Menu::new(app)?;
        menu.append(&MenuItem::with_id(
            app,
            OPEN_ID,
            "打开主界面",
            true,
            None::<&str>,
        )?)?;
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        menu.append(&MenuItem::with_id(
            app,
            QUIT_ID,
            "退出",
            true,
            None::<&str>,
        )?)?;

        let mut tray = TrayIconBuilder::with_id(TRAY_ID)
            .menu(&menu)
            .tooltip("GrillForge")
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| match event.id().as_ref() {
                OPEN_ID => show_main_window_inner(app),
                QUIT_ID => app.exit(0),
                _ => {}
            })
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    position,
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    toggle_quick_panel(tray.app_handle(), position.x, position.y);
                }
            });
        if let Some(icon) = app.default_window_icon() {
            tray = tray.icon(icon.clone()).icon_as_template(false);
        }
        tray.build(app)?;
    }

    Ok(())
}

#[tauri::command]
pub fn show_main_window(app: AppHandle) {
    show_main_window_inner(&app);
}

#[cfg(target_os = "macos")]
fn toggle_quick_panel<R: Runtime>(app: &AppHandle<R>, click_x: f64, click_y: f64) {
    let Some(panel) = app.get_webview_window("quick-menu") else {
        return;
    };
    if panel.is_visible().unwrap_or(false) {
        let _ = panel.hide();
        return;
    }
    let panel_width = panel.outer_size().map(|size| size.width).unwrap_or(520) as f64;
    let x = (click_x - panel_width + 28.0).round() as i32;
    let y = (click_y + 12.0).round() as i32;
    let _ = panel.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(
        x, y,
    )));
    let _ = panel.show();
    let _ = panel.set_focus();
}

#[cfg(target_os = "macos")]
fn show_main_window_inner<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
