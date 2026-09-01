//! System tray icon with a Show/Hide/Quit menu.
//!
//! Left-click toggles the main window; right-click opens the menu. Built once
//! in `setup` and lives for the app's lifetime.

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime,
};

/// Tray icon hover tooltip.
const TOOLTIP: &str = "Kansolo";

/// Build and install the tray icon + menu. Called once from `setup`.
///
/// Returns an error (rather than panicking) if the default window icon
/// isn't configured, so a misconfigured build surfaces as a setup failure
/// instead of a crash.
pub fn install<R: Runtime>(app: &AppHandle<R>) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "Hide", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hide, &quit])?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "default window icon must be configured")?;

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip(TOOLTIP)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_window(app),
            "hide" => hide_window(app),
            "quit" => quit_app(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Bring the main window to the foreground.
fn show_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// Hide the main window while the app keeps running in the tray.
fn hide_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
}

/// Toggle main window visibility.
fn toggle_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("main") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.set_focus();
        }
    }
}

/// Quit the app: cancel active runs so card locks release, then exit.
/// Best-effort — shutdown is bounded by a 5s timeout per run.
fn quit_app<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        // Cancel active runs and collect their IDs.
        let active_ids: Vec<String> = match handle.try_state::<crate::runner::RunnerState>() {
            Some(state) => state.core.shutdown().await,
            None => Vec::new(),
        };
        // Mark each reaped run failed so the card lock releases.
        if !active_ids.is_empty() {
            if let Ok(conn) = crate::db::open_db(&handle) {
                let now = crate::db::now_iso();
                for id in &active_ids {
                    let _ = crate::db::agent_runs::update_status(
                        &conn,
                        id,
                        "failed",
                        None,
                        None,
                        Some("tasker quitting"),
                        Some(&now),
                    );
                }
            }
        }
        handle.exit(0);
    });
}
