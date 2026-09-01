pub mod cli;
pub mod db;
mod editor;
pub mod error;
mod mapping;
mod mcp;
pub mod runner;
pub mod skills;
mod source;
mod sync;
#[cfg(test)]
pub mod test_utils;
mod tray;
mod worktree;

use tauri::{Manager, WindowEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(mcp::McpState::default())
        .manage(runner::RunnerState::default())
        .on_window_event(|window, event| {
            // Intercept the close button: when `close_to_tray` is enabled (the
            // default), hide the window instead of quitting. The user quits
            // via the tray menu's "Quit" item.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let close_to_tray = read_setting(app, "close_to_tray")
                    .map(|v| v != "false")
                    .unwrap_or(true);
                if close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            // Run DB migrations before anything else touches the database.
            let conn = db::open_db(app.handle())?;
            db::run_migrations(&conn)?;
            // Reap stale active runs left by a crashed/killed tasker
            // (fire-and-forget — best-effort, never blocks startup).
            if let Err(e) = runner::cleanup_dangling(&conn) {
                eprintln!("cleanup_dangling on startup failed: {e}");
            }
            drop(conn);

            // Wire the AppHandle into the run core so it can push events.
            let _ = app
                .state::<runner::RunnerState>()
                .core
                .set_app(app.handle().clone());

            // Install the system tray icon + menu.
            tray::install(app.handle())?;

            // Auto-start the MCP server if the user previously enabled it.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = handle.state::<mcp::McpState>();
                let (enabled, port) = read_mcp_settings(&handle).unwrap_or((false, 27816));
                if enabled {
                    let _ = mcp::apply(&handle, &state, true, port).await;
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            db::cards::list_cards,
            db::cards::list_cards_by_column,
            db::cards::create_local_card,
            db::cards::update_card,
            db::cards::move_card,
            db::cards::delete_card,
            db::cards::is_card_locked_cmd,
            db::cards::delete_all_source_cards,
            db::cards::get_card_by_source_ref,
            db::settings::get_setting,
            db::settings::set_setting,
            db::settings::get_all_settings,
            db::settings::save_settings,
            db::settings::get_snapshot,
            db::settings::save_snapshot,
            db::settings::list_tree_sources,
            db::settings::add_tree_source,
            db::settings::update_tree_source,
            db::settings::delete_tree_source,
            db::settings::list_sources,
            db::settings::add_source,
            db::settings::update_source,
            db::settings::delete_source,
            db::settings::get_source,
            editor::open_in_editor,
            source::fetch_source_cards,
            source::fetch_source_options,
            source::list_source_types,
            source::sync_source,
            source::resolve_conflicts,
            source::preview_jql,
            mcp::mcp_apply,
            mcp::mcp_status,
            runner::acp_list_agents,
            runner::acp_register_agent,
            runner::acp_update_agent,
            runner::acp_delete_agent,
            runner::acp_list_skills,
            runner::acp_list_active_runs,
            runner::acp_create_run,
            runner::acp_resume_run,
            runner::acp_get_run,
            runner::acp_get_run_for_card,
            runner::acp_latest_run_for_card,
            runner::acp_list_updates,
            runner::acp_has_updates,
            runner::acp_list_runs,
            runner::acp_list_recent_runs,
            runner::acp_cleanup,
            runner::acp_cancel_run,
            runner::acp_respond_permission,
            runner::acp_send_followup,
            runner::acp_diff_main,
            runner::acp_merge,
            runner::acp_remove_worktree,
            runner::acp_delete_run
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Read a single key from the settings table via rusqlite, so the backend can
/// read config before the frontend has loaded. Returns `None` if the DB or row
/// is missing.
pub fn read_setting<R: tauri::Runtime>(app: &tauri::AppHandle<R>, key: &str) -> Option<String> {
    let dir = app.path().app_config_dir().ok()?;
    let db_path = dir.join("tasker.db");
    if !db_path.exists() {
        return None;
    }
    // Use the shared opener so WAL + busy_timeout + FK pragmas match the
    // rest of the app (avoids locked/FK-disabled reads).
    let conn = db::open_db_path(&db_path).ok()?;
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
        r.get::<_, String>(0)
    })
    .ok()
}

/// Read `mcp_enabled` + `mcp_port` from the settings table via rusqlite, so the
/// backend can auto-start the MCP server before the frontend has loaded.
fn read_mcp_settings<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Option<(bool, u16)> {
    let enabled = read_setting(app, "mcp_enabled")
        .map(|v| v == "true")
        .unwrap_or(false);
    let port = read_setting(app, "mcp_port")
        .and_then(|v| v.parse().ok())
        .unwrap_or(27816);
    Some((enabled, port))
}
