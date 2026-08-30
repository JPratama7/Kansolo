//! Embedded MCP (Model Context Protocol) server over streamable HTTP.
//!
//! Exposes three tools to external MCP clients:
//!   - `list_cards`  — return every card, ordered by column then position.
//!   - `get_card`    — return a single card by id.
//!   - `move_card`   — move a card to a column + position.
//!
//! The server reads/writes the same SQLite database file that
//! `tauri-plugin-sql` uses (`{app_config_dir}/tasker.db`). rusqlite opens the
//! file with WAL + a busy timeout so it coexists with the sqlx connection pool
//! the frontend uses.
//!
//! Lifecycle is driven from the TypeScript side via the `mcp_apply` and
//! `mcp_status` Tauri commands; the running axum task + its cancellation token
//! live in [`McpState`] managed by the Tauri app.

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    tool, tool_handler, tool_router,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::db::{open_db_path, Card, CardRow, now_iso};

/// A running MCP server instance: the spawned axum task plus the token used to
/// ask it to stop gracefully.
struct McpHandle {
    join: tokio::task::JoinHandle<()>,
    cancel: CancellationToken,
}

/// Tauri-managed state holding the optional running server.
#[derive(Default)]
pub struct McpState {
    server: Mutex<Option<McpHandle>>,
}

/// JSON shape returned by `mcp_status` / `mcp_apply`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub running: bool,
    pub port: Option<u16>,
}

// ---------------------------------------------------------------------------
// MCP tool parameter shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct GetCardParams {
    /// The card id (UUID).
    #[schemars(description = "The card id (UUID).")]
    id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MoveCardParams {
    /// The card id (UUID).
    #[schemars(description = "The card id (UUID).")]
    id: String,
    /// Target column: `backlog`, `ongoing`, or `done`.
    #[schemars(description = "Target column: 'backlog', 'ongoing', or 'done'.")]
    column: String,
    /// New zero-based position within the target column.
    #[schemars(description = "New zero-based position within the target column.")]
    position: i64,
}

// ---------------------------------------------------------------------------
// MCP service
// ---------------------------------------------------------------------------

/// The MCP service handler. Holds the DB path and the rmcp tool router.
#[derive(Clone)]
struct KansoloMcp {
    db_path: Arc<PathBuf>,
    tool_router: ToolRouter<KansoloMcp>,
}

#[tool_router]
impl KansoloMcp {
    fn new(db_path: PathBuf) -> Self {
        Self {
            db_path: Arc::new(db_path),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "List all kanban cards, ordered by column then position.")]
    async fn list_cards(&self) -> Result<CallToolResult, McpError> {
        let conn = open_db_path(&self.db_path).map_err(|e| McpError::internal_error(
            "db_open_failed",
            Some(serde_json::json!({ "error": e })),
        ))?;
        let mut stmt = conn
            .prepare(
                r#"SELECT id, title, description, priority, "column", source, position,
                          source_ref, source_status,
                          tree_source_id, repo_path, created_at, updated_at
                   FROM cards
                   ORDER BY "column", position ASC"#,
            )
            .map_err(|e| McpError::internal_error("db_prepare_failed", Some(serde_json::json!({ "error": e.to_string() }))))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(CardRow {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    description: r.get(2)?,
                    priority: r.get(3)?,
                    column: r.get(4)?,
                    source: r.get(5)?,
                    position: r.get(6)?,
                    source_ref: r.get(7)?,
                    source_status: r.get(8)?,
                    tree_source_id: r.get(9)?,
                    repo_path: r.get(10)?,
                    created_at: r.get(11)?,
                    updated_at: r.get(12)?,
                })
            })
            .map_err(|e| McpError::internal_error("db_query_failed", Some(serde_json::json!({ "error": e.to_string() }))))?;
        let cards: Vec<Card> = rows.filter_map(Result::ok).map(Card::from).collect();
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&cards)
                .map_err(|e| McpError::internal_error("serialize_failed", Some(serde_json::json!({ "error": e.to_string() }))))?,
        )]))
    }

    #[tool(description = "Get a single kanban card by id.")]
    async fn get_card(
        &self,
        Parameters(GetCardParams { id }): Parameters<GetCardParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = open_db_path(&self.db_path).map_err(|e| McpError::internal_error(
            "db_open_failed",
            Some(serde_json::json!({ "error": e })),
        ))?;
        let row = conn
            .query_row(
                r#"SELECT id, title, description, priority, "column", source, position,
                          source_ref, source_status,
                          tree_source_id, repo_path, created_at, updated_at
                   FROM cards WHERE id = ?1"#,
                [&id],
                |r| {
                    Ok(CardRow {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        description: r.get(2)?,
                        priority: r.get(3)?,
                        column: r.get(4)?,
                        source: r.get(5)?,
                        position: r.get(6)?,
                        source_ref: r.get(7)?,
                        source_status: r.get(8)?,
                        tree_source_id: r.get(9)?,
                        repo_path: r.get(10)?,
                        created_at: r.get(11)?,
                        updated_at: r.get(12)?,
                    })
                },
            );
        match row {
            Ok(r) => {
                let card = Card::from(r);
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&card)
                        .map_err(|e| McpError::internal_error("serialize_failed", Some(serde_json::json!({ "error": e.to_string() }))))?,
                )]))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(CallToolResult::success(vec![
                Content::text(format!("No card with id {id}.")),
            ])),
            Err(e) => Err(McpError::internal_error(
                "db_query_failed",
                Some(serde_json::json!({ "error": e.to_string() })),
            )),
        }
    }

    #[tool(description = "Move a card to a target column and position.")]
    async fn move_card(
        &self,
        Parameters(MoveCardParams { id, column, position }): Parameters<MoveCardParams>,
    ) -> Result<CallToolResult, McpError> {
        if !matches!(column.as_str(), "backlog" | "ongoing" | "done") {
            return Err(McpError::invalid_params(
                "column must be 'backlog', 'ongoing', or 'done'",
                None,
            ));
        }
        let conn = open_db_path(&self.db_path).map_err(|e| McpError::internal_error(
            "db_open_failed",
            Some(serde_json::json!({ "error": e })),
        ))?;
        let now = now_iso();
        let updated = conn
            .execute(
                r#"UPDATE cards SET "column" = ?1, position = ?2, updated_at = ?3 WHERE id = ?4"#,
                rusqlite::params![&column, position, &now, &id],
            )
            .map_err(|e| McpError::internal_error("db_update_failed", Some(serde_json::json!({ "error": e.to_string() }))))?;
        if updated == 0 {
            Ok(CallToolResult::success(vec![Content::text(format!(
                "No card with id {id}."
            ))]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(format!(
                "Moved card {id} to {column} at position {position}."
            ))]))
        }
    }
}

#[tool_handler]
impl ServerHandler for KansoloMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_03_26,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation {
                name: "kansolo-mcp".to_string(),
                version: "0.1.0".to_string(),
                title: None,
                website_url: None,
                icons: None,
            },
            instructions: Some(
                "Kansolo kanban board. Tools: list_cards, get_card, move_card.".to_string(),
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Lifecycle: start / stop the axum server
// ---------------------------------------------------------------------------

/// Spawn the MCP HTTP server on `127.0.0.1:{port}`. Returns once the server is
/// bound (or fails to bind) by running the bind inside the spawned task and
/// signalling readiness via a oneshot channel.
async fn start_server(
    db_path: PathBuf,
    port: u16,
) -> Result<(tokio::task::JoinHandle<()>, CancellationToken), String> {
    let cancel = CancellationToken::new();
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    let ct = cancel.clone();

    let join = tokio::spawn(async move {
        let service = StreamableHttpService::new(
            move || Ok(KansoloMcp::new(db_path.clone())),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default(),
        );
        let router = axum::Router::new().nest_service("/mcp", service);
        let bind = format!("127.0.0.1:{port}");
        let listener = match tokio::net::TcpListener::bind(&bind).await {
            Ok(l) => {
                let _ = tx.send(Ok(()));
                l
            }
            Err(e) => {
                let _ = tx.send(Err(format!("failed to bind {bind}: {e}")));
                return;
            }
        };
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move { ct.cancelled().await })
            .await;
    });

    match rx.await {
        Ok(Ok(())) => Ok((join, cancel)),
        Ok(Err(e)) => {
            let _ = join.await;
            Err(e)
        }
        Err(_) => Err("MCP server startup channel closed unexpectedly".to_string()),
    }
}

/// Resolve the SQLite file path the SQL plugin uses: `{app_config_dir}/tasker.db`.
fn db_path_for<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("could not resolve app config dir: {e}"))?;
    Ok(dir.join("tasker.db"))
}

/// Apply the desired MCP server state. Starts, stops, or restarts the server so
/// that the running state matches `enabled` + `port`.
pub async fn apply<R: Runtime>(
    app: &AppHandle<R>,
    state: &State<'_, McpState>,
    enabled: bool,
    port: u16,
) -> Result<McpStatus, String> {
    let mut guard = state.server.lock().await;
    let currently_running = guard.is_some();

    if !enabled {
        if let Some(handle) = guard.take() {
            handle.cancel.cancel();
            let _ = handle.join.await;
        }
        return Ok(McpStatus { running: false, port: None });
    }

    if currently_running {
        // No change needed if the port matches the one we bound. We don't track
        // the bound port separately, so restart to honor a potential port change.
        if let Some(handle) = guard.take() {
            handle.cancel.cancel();
            let _ = handle.join.await;
        }
    }

    let db_path = db_path_for(app)?;
    let (join, cancel) = start_server(db_path, port).await?;
    *guard = Some(McpHandle { join, cancel });
    Ok(McpStatus { running: true, port: Some(port) })
}

/// Report the current running state.
pub async fn status(state: &State<'_, McpState>) -> McpStatus {
    let guard = state.server.lock().await;
    if guard.is_some() {
        // Port is not tracked separately here; the caller knows the configured port.
        McpStatus { running: true, port: None }
    } else {
        McpStatus { running: false, port: None }
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Apply the MCP server configuration: start, stop, or restart so the running
/// state matches `enabled` + `port`.
#[tauri::command]
pub async fn mcp_apply<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, McpState>,
    enabled: bool,
    port: u16,
) -> Result<McpStatus, String> {
    apply(&app, &state, enabled, port).await
}

/// Report whether the MCP server is currently running.
#[tauri::command]
pub async fn mcp_status(state: State<'_, McpState>) -> Result<McpStatus, String> {
    Ok(status(&state).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_timestamp_is_well_formed() {
        let ts = crate::db::now_iso();
        assert_eq!(ts.len(), 20);
        assert!(ts.ends_with('Z'));
        assert_eq!(ts.as_bytes()[4], b'-');
        assert_eq!(ts.as_bytes()[7], b'-');
        assert_eq!(ts.as_bytes()[10], b'T');
        assert_eq!(ts.as_bytes()[13], b':');
        assert_eq!(ts.as_bytes()[16], b':');
    }
}
