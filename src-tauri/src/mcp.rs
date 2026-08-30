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
use rusqlite::params;

/// Settings key under which the MCP bearer token is persisted.
const MCP_TOKEN_KEY: &str = "mcp_token";

/// Read the persisted MCP bearer token from the settings table, if any.
/// Returns `None` when the row is absent or empty.
fn read_mcp_token(db_path: &PathBuf) -> Option<String> {
    let conn = open_db_path(db_path).ok()?;
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![MCP_TOKEN_KEY],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .filter(|t| !t.is_empty())
}

/// Load the persisted MCP bearer token, or generate + persist a fresh one.
/// The token is a UUID v4 string — random enough for a local-only bearer
/// secret. Lives entirely in `mcp.rs` (no `lib.rs` involvement).
fn load_or_create_token(db_path: &PathBuf) -> Result<String, String> {
    if let Some(t) = read_mcp_token(db_path) {
        return Ok(t);
    }
    let token = uuid::Uuid::new_v4().to_string();
    let conn = open_db_path(db_path)?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = ?2",
        params![MCP_TOKEN_KEY, &token],
    )
    .map_err(|e| e.to_string())?;
    Ok(token)
}

/// axum middleware: reject requests whose `Authorization` header is not
/// `Bearer <token>`. Used to gate the local MCP HTTP endpoint.
async fn require_bearer(
    axum::extract::State(token): axum::extract::State<Arc<String>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let expected = format!("Bearer {token}");
    let authorized = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .map(|h| h == expected)
        .unwrap_or(false);
    if authorized {
        next.run(request).await
    } else {
        axum::response::Response::builder()
            .status(axum::http::StatusCode::UNAUTHORIZED)
            .body(axum::body::Body::empty())
            .unwrap()
    }
}

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
    /// Whether a bearer token is persisted for the MCP server. The token
    /// itself is never exposed to the UI — only its presence.
    pub has_token: bool,
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
                          tree_source_id, repo_path, source_instance_id, created_at, updated_at
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
                    source_instance_id: r.get(11)?,
                    created_at: r.get(12)?,
                    updated_at: r.get(13)?,
                })
            })
            .map_err(|e| McpError::internal_error("db_query_failed", Some(serde_json::json!({ "error": e.to_string() }))))?;
        let cards: Vec<Card> = {
            let mut out = Vec::new();
            for r in rows {
                let row = r.map_err(|e| McpError::internal_error(
                    "db_row_failed",
                    Some(serde_json::json!({ "error": e.to_string() })),
                ))?;
                out.push(Card::from(row));
            }
            out
        };
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
                          tree_source_id, repo_path, source_instance_id, created_at, updated_at
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
                        source_instance_id: r.get(11)?,
                        created_at: r.get(12)?,
                        updated_at: r.get(13)?,
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
/// signalling readiness via a oneshot channel. Every request must present
/// `Authorization: Bearer <token>` (see [`require_bearer`]).
async fn start_server(
    db_path: PathBuf,
    port: u16,
    token: Arc<String>,
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
        let router = axum::Router::new()
            .nest_service("/mcp", service)
            .layer(axum::middleware::from_fn_with_state(token, require_bearer));
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
        let has_token = db_path_for(app)
            .map(|p| read_mcp_token(&p).is_some())
            .unwrap_or(false);
        return Ok(McpStatus { running: false, port: None, has_token });
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
    let token = load_or_create_token(&db_path)?;
    let (join, cancel) = start_server(db_path, port, Arc::new(token)).await?;
    *guard = Some(McpHandle { join, cancel });
    Ok(McpStatus { running: true, port: Some(port), has_token: true })
}

/// Report the current running state. `has_token` reflects whether a bearer
/// token is persisted in settings — the token itself is never exposed.
pub async fn status<R: Runtime>(app: &AppHandle<R>, state: &State<'_, McpState>) -> McpStatus {
    let guard = state.server.lock().await;
    let has_token = db_path_for(app)
        .map(|p| read_mcp_token(&p).is_some())
        .unwrap_or(false);
    if guard.is_some() {
        // Port is not tracked separately here; the caller knows the configured port.
        McpStatus { running: true, port: None, has_token }
    } else {
        McpStatus { running: false, port: None, has_token }
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
pub async fn mcp_status<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, McpState>,
) -> Result<McpStatus, String> {
    Ok(status(&app, &state).await)
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

    /// Issue a raw HTTP/1.1 GET to `addr/mcp` with the given `Authorization`
    /// header value (empty string => no header). Returns the full response.
    async fn raw_http_get(addr: std::net::SocketAddr, auth: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let req = if auth.is_empty() {
            format!("GET /mcp HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")
        } else {
            format!(
                "GET /mcp HTTP/1.1\r\nHost: {addr}\r\nAuthorization: {auth}\r\nConnection: close\r\n\r\n"
            )
        };
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        String::from_utf8_lossy(&buf).to_string()
    }

    /// The MCP endpoint must reject unauthenticated requests with 401 and
    /// accept only the correct bearer token.
    #[tokio::test]
    async fn mcp_request_without_token_is_401() {
        let token = Arc::new("test-secret".to_string());
        let app = axum::Router::new()
            .route("/mcp", axum::routing::any(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(token, require_bearer));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let cancel = CancellationToken::new();
        let ct = cancel.clone();
        let join = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move { ct.cancelled().await })
                .await;
        });

        // No Authorization header -> 401.
        let resp = raw_http_get(addr, "").await;
        assert!(resp.starts_with("HTTP/1.1 401"), "no-auth: {resp}");
        // Correct bearer -> 200.
        let resp = raw_http_get(addr, "Bearer test-secret").await;
        assert!(resp.starts_with("HTTP/1.1 200"), "correct: {resp}");
        // Wrong bearer -> 401.
        let resp = raw_http_get(addr, "Bearer wrong").await;
        assert!(resp.starts_with("HTTP/1.1 401"), "wrong: {resp}");

        cancel.cancel();
        let _ = join.await;
    }
}
