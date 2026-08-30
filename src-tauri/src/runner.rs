//! Run orchestrator: SDK integration for agent runs.
//!
//! Takes a card + agent, creates a worktree, spawns the agent subprocess
//! via `AcpAgent`, opens an ACP session with CWD = worktree, sends the
//! card's title+description as prompt (with preloaded skills prepended
//! from disk), streams `session/update` notifications to a channel, and
//! transitions the run through terminal states.

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio_util::sync::CancellationToken;

use crate::db::agent_runs;
use crate::db::agents;
use crate::db::cards;
use crate::error::AcpError;
use crate::skills;
use crate::worktree::WorktreeManager;

/// A running agent run: the spawned task + cancellation token.
pub struct RunHandle {
    pub join: tokio::task::JoinHandle<()>,
    pub cancel: CancellationToken,
}

/// Updates emitted by a run, streamed to the GUI/CLI via an `UpdateSink`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RunUpdate {
    /// A session/update notification from the agent (streaming output).
    SessionUpdate {
        text: String,
    },
    /// The session ID was received.
    SessionId {
        session_id: String,
    },
    /// The run completed successfully.
    Completed {
        output: String,
        stop_reason: String,
    },
    /// The run failed.
    Failed {
        error: String,
    },
    /// The run was cancelled.
    Cancelled,
    /// A permission request was received from the agent.
    /// The GUI should respond via `acp_respond_permission`.
    PermissionRequest {
        request_id: String,
        description: String,
    },
    /// A permission request timed out (5min, auto-denied).
    PermissionTimeout,
}

/// Trait for receiving run updates. Implemented by both the Tauri event
/// emitter and the CLI stdout printer (decision 16).
pub trait UpdateSink: Send + 'static {
    fn send_update(&self, run_id: &str, update: RunUpdate);
}

/// Buffer of updates for a run, with a dirty flag for polling.
pub struct UpdateBuffer {
    pub updates: Vec<RunUpdate>,
    pub dirty: bool,
}

impl UpdateBuffer {
    fn new() -> Self {
        Self { updates: Vec::new(), dirty: false }
    }
}

/// A pending permission request awaiting user response.
/// Stores the SDK responder + the selected "allow" option's ID. The
/// `responded` Notify lets the timeout task race against the user's
/// response: `respond_permission` notifies after taking the responder,
/// so the timeout task can exit early instead of sleeping the full
/// duration after the user already answered.
pub struct PendingPermission {
    pub responder: tokio::sync::Mutex<
        Option<
            agent_client_protocol::Responder<
                agent_client_protocol::schema::v1::RequestPermissionResponse,
            >,
        >,
    >,
    pub option_id: agent_client_protocol::schema::v1::PermissionOptionId,
    pub responded: Arc<Notify>,
}

/// AppHandle-agnostic run core. Takes a DB connection, worktree manager,
/// and an update sink. Both Tauri commands and the CLI are thin adapters
/// over `RunCore` (decision 16).
pub struct RunCore {
    pub runs: Arc<Mutex<HashMap<String, RunHandle>>>,
    pub buffers: Arc<Mutex<HashMap<String, UpdateBuffer>>>,
    /// Pending permission requests, keyed by "{run_id}:{request_id}".
    pub permissions: Arc<Mutex<HashMap<String, Arc<PendingPermission>>>>,
}

impl RunCore {
    pub fn new() -> Self {
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
            buffers: Arc::new(Mutex::new(HashMap::new())),
            permissions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create a new agent run. Takes pre-loaded data (from the Tauri command
    /// which does all DB work synchronously). This function does the async
    /// work: worktree creation + SDK spawn + drain task.
    pub async fn create_run(
        &self,
        run_id: String,
        card_id: String,
        repo_path: String,
        prompt: String,
        acp_agent: agent_client_protocol::AcpAgent,
        db_path: PathBuf,
    ) -> Result<(String, String), AcpError> {
        // Step 4: Create worktree.
        let wt_mgr = WorktreeManager::new(&PathBuf::from(&repo_path));
        let worktree = wt_mgr.create(&card_id).await?;
        let branch = worktree.branch.clone();
        let worktree_path = worktree.path.clone();

        // Step 9: Spawn SDK connection.
        let (tx, rx) = mpsc::unbounded_channel::<RunUpdate>();
        let cancel = CancellationToken::new();
        let cancel_for_handle = cancel.clone();
        let cwd = worktree.path.clone();
        let run_id_clone = run_id.clone();
        let run_id_for_handle = run_id.clone();
        let permissions_map = self.permissions.clone();
        // Clone the DB path so the per-permission timeout task can read
        // the `acp_permission_timeout` setting without borrowing the
        // drain task's copy (Connection is !Send; the timeout task opens
        // its own short-lived connection by path).
        let db_path_for_perms = db_path.clone();

        let join = tokio::spawn(async move {
            let tx_for_err = tx.clone();
            let cancel_for_closure = cancel.clone();
            let result = agent_client_protocol::Client
                .connect_with(acp_agent, move |cx: agent_client_protocol::ConnectionTo<agent_client_protocol::Agent>| {
                    let tx = tx.clone();
                    let cwd = cwd.clone();
                    let prompt = prompt.clone();
                    let permissions_map = permissions_map.clone();
                    let run_id_inner = run_id_clone.clone();
                    let cancel_inner = cancel_for_closure.clone();
                    let db_path_inner = db_path_for_perms.clone();
                    async move {
                        let mut session = cx.build_session(&cwd).block_task().start_session().await?;
                        let session_id = session.session_id().0.clone();
                        let _ = tx.send(RunUpdate::SessionId {
                            session_id: session_id.to_string(),
                        });
                        // Clone the connection so the cancel branch can send
                        // the `session/cancel` notification without borrowing
                        // `session` while `read_update()`'s future is live.
                        let conn_for_cancel = session.connection().clone();
                        session.send_prompt(&prompt)?;
                        // Read updates until stop reason, or until cancelled.
                        // On cancel, send the ACP `session/cancel` notification
                        // (cooperative — the agent may ignore it) and emit a
                        // Cancelled update so the drain task records terminal
                        // state. When this closure returns, the SDK drops the
                        // connection's `ChildGuard`, which SIGKILL's the entire
                        // process group after a 1s grace period — so even an
                        // agent that ignores `session/cancel` is hard-killed.
                        loop {
                            let read = session.read_update();
                            tokio::pin!(read);
                            tokio::select! {
                                biased;
                                _ = cancel_inner.cancelled() => {
                                    // The `read` future (mutable borrow of
                                    // `session`) is dropped at the end of this
                                    // select branch; we use the owned
                                    // `conn_for_cancel` clone below, so no
                                    // borrow conflict.
                                    // Best-effort cooperative cancel: ask the
                                    // agent to stop. If it ignores the
                                    // notification, the SDK's ChildGuard
                                    // (installed by `connect_with`) SIGKILL's
                                    // the process group when this closure
                                    // returns and the connection drops.
                                    let _ = conn_for_cancel.send_notification(
                                        agent_client_protocol::schema::v1::CancelNotification::new(session_id.clone()),
                                    );
                                    let _ = tx.send(RunUpdate::Cancelled);
                                    break;
                                }
                                msg = read => {
                                    match msg {
                                        Ok(msg) => {
                                            use agent_client_protocol::SessionMessage;
                                            match msg {
                                                SessionMessage::SessionMessage(dispatch) => {
                                                    // Check for permission requests.
                                                    let method = dispatch.method().to_string();
                                                    if method == "session/requestPermission" {
                                                        use agent_client_protocol::schema::v1::RequestPermissionRequest;
                                                        match dispatch.into_request::<RequestPermissionRequest>() {
                                                            Ok(Ok((req, responder))) => {
                                                                let req_id = format!("{}:{}", run_id_inner, req.tool_call.tool_call_id.0);
                                                                let description = summarize_tool_call(&req.tool_call);
                                                                let option_id = pick_allow_option(&req.options);
                                                                let responded = Arc::new(Notify::new());
                                                                let pending = Arc::new(PendingPermission {
                                                                    responder: tokio::sync::Mutex::new(Some(responder)),
                                                                    option_id,
                                                                    responded: responded.clone(),
                                                                });
                                                                {
                                                                    let mut perms = permissions_map.lock().await;
                                                                    perms.insert(req_id.clone(), pending.clone());
                                                                }
                                                                let _ = tx.send(RunUpdate::PermissionRequest {
                                                                    request_id: req_id.clone(),
                                                                    description,
                                                                });
                                                                // Spawn a per-permission timeout task:
                                                                // races the timeout against the user
                                                                // responding (Notify). On expiry, takes
                                                                // the responder, responds Cancelled to
                                                                // the SDK, removes the pending entry,
                                                                // and emits PermissionTimeout.
                                                                let tx_t = tx.clone();
                                                                let perms_t = permissions_map.clone();
                                                                let req_id_t = req_id;
                                                                let db_path_t = db_path_inner.clone();
                                                                tokio::spawn(async move {
                                                                    let timeout = read_permission_timeout(&db_path_t);
                                                                    tokio::select! {
                                                                        biased;
                                                                        _ = responded.notified() => {
                                                                            // User responded; nothing to do.
                                                                        }
                                                                        _ = tokio::time::sleep(std::time::Duration::from_secs(timeout)) => {
                                                                            let responder_opt = {
                                                                                let mut guard = pending.responder.lock().await;
                                                                                guard.take()
                                                                            };
                                                                            if let Some(responder) = responder_opt {
                                                                                {
                                                                                    let mut perms = perms_t.lock().await;
                                                                                    perms.remove(&req_id_t);
                                                                                }
                                                                                use agent_client_protocol::schema::v1::{
                                                                                    RequestPermissionResponse,
                                                                                    RequestPermissionOutcome,
                                                                                };
                                                                                let _ = responder.respond(
                                                                                    RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
                                                                                );
                                                                                let _ = tx_t.send(RunUpdate::PermissionTimeout);
                                                                            }
                                                                        }
                                                                    }
                                                                });
                                                            }
                                                            _ => {
                                                                let text = format!("{:?}", method);
                                                                let _ = tx.send(RunUpdate::SessionUpdate { text });
                                                            }
                                                        }
                                                    } else {
                                                        let text = format!("{:?}", dispatch);
                                                        let _ = tx.send(RunUpdate::SessionUpdate { text });
                                                    }
                                                }
                                                SessionMessage::StopReason(reason) => {
                                                    let _ = tx.send(RunUpdate::Completed {
                                                        output: String::new(),
                                                        stop_reason: format!("{:?}", reason),
                                                    });
                                                    break;
                                                }
                                                _ => {}
                                            }
                                        }
                                        Err(e) => {
                                            let _ = tx.send(RunUpdate::Failed {
                                                error: e.to_string(),
                                            });
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        Ok(())
                    }
                })
                .await;
            match result {
                Ok(()) => {}
                Err(e) => {
                    let _ = tx_for_err.send(RunUpdate::Failed {
                        error: e.to_string(),
                    });
                }
            }
        });

        // Step 10: Store RunHandle.
        {
            let mut runs = self.runs.lock().await;
            runs.insert(run_id_for_handle.clone(), RunHandle { join, cancel: cancel_for_handle });
        }
        {
            let mut buffers = self.buffers.lock().await;
            buffers.insert(run_id_for_handle.clone(), UpdateBuffer::new());
        }

        // Step 11: Spawn drain task.
        let runs_map = self.runs.clone();
        let buffers_map = self.buffers.clone();
        let run_id_for_drain = run_id_for_handle.clone();
        tokio::spawn(async move {
            drain_updates(rx, buffers_map, runs_map, run_id_for_drain, db_path).await;
        });

        // Step 12: Return the real worktree path + branch so the caller can
        // UPDATE the placeholder row.
        Ok((worktree_path.to_string_lossy().to_string(), branch))
    }

    /// Cancel a running agent. Cancels the task and waits up to 5s.
    /// The caller is responsible for updating the DB status.
    pub async fn cancel_run(&self, run_id: &str) {
        let handle = {
            let mut runs = self.runs.lock().await;
            runs.remove(run_id)
        };
        if let Some(handle) = handle {
            handle.cancel.cancel();
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                handle.join,
            ).await;
        }
    }

    /// List updates since cursor. Clears dirty flag (decision 22).
    pub async fn list_updates(
        &self,
        run_id: &str,
        cursor: usize,
    ) -> Vec<RunUpdate> {
        let mut buffers = self.buffers.lock().await;
        if let Some(buf) = buffers.get_mut(run_id) {
            buf.dirty = false;
            buf.updates.iter().skip(cursor).cloned().collect()
        } else {
            Vec::new()
        }
    }

    /// Check if a run has new updates (dirty flag).
    pub async fn has_updates(&self, run_id: &str) -> bool {
        let buffers = self.buffers.lock().await;
        buffers.get(run_id).map(|b| b.dirty).unwrap_or(false)
    }

    /// Respond to a pending permission request.
    /// `request_id` is the composite "{run_id}:{tool_call_id}" from the
    /// PermissionRequest update. If `approved`, selects the first option;
    /// otherwise cancels the permission.
    pub async fn respond_permission(
        &self,
        request_id: &str,
        approved: bool,
    ) -> Result<(), AcpError> {
        use agent_client_protocol::schema::v1::{
            RequestPermissionResponse,
            RequestPermissionOutcome,
            SelectedPermissionOutcome,
        };
        let pending = {
            let mut perms = self.permissions.lock().await;
            perms.remove(request_id)
        };
        let pending = pending.ok_or_else(|| {
            AcpError::not_found(&format!("permission request not found: {request_id}"))
        })?;
        let responder = {
            let mut guard = pending.responder.lock().await;
            guard.take()
        };
        if let Some(responder) = responder {
            let outcome = if approved {
                RequestPermissionOutcome::Selected(
                    SelectedPermissionOutcome::new(pending.option_id.clone())
                )
            } else {
                RequestPermissionOutcome::Cancelled
            };
            let response = RequestPermissionResponse::new(outcome);
            responder.respond(response)
                .map_err(|e| AcpError::internal(&format!("permission respond failed: {e}")))?;
        }
        // Wake the timeout task so it exits early instead of sleeping the
        // full duration after the user already answered.
        pending.responded.notify_one();
        Ok(())
    }

    /// Shutdown all active runs. Called on app exit.
    /// Returns the list of run IDs that were active (caller updates DB).
    pub async fn shutdown(&self) -> Vec<String> {
        let runs: Vec<(String, RunHandle)> = {
            let mut runs = self.runs.lock().await;
            runs.drain().collect()
        };
        let mut ids = Vec::new();
        for (run_id, handle) in runs {
            handle.cancel.cancel();
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                handle.join,
            ).await;
            ids.push(run_id);
        }
        ids
    }
}

/// Build the skills section for the prompt.
pub fn build_skills_section(loaded: &[(String, String)]) -> String {
    if loaded.is_empty() {
        return String::new();
    }
    let mut sections = Vec::new();
    for (name, content) in loaded {
        sections.push(format!("## {name}\n\n{content}"));
    }
    format!("# Preloaded skills\n\n{}", sections.join("\n\n"))
}

/// Truncate a string to at most `max` chars on a char boundary, appending
/// an ellipsis if truncation occurred. Avoids panicking on non-char
/// boundaries (which `&s[..n]` would hit for multi-byte UTF-8).
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{truncated}…")
}

/// Structured summary of a permission request's tool call: tool title +
/// raw_input JSON truncated at 500 chars. Replaces the old `format!("{:?}")`
/// debug dump, which leaked SDK-internal field noise to the UI.
fn summarize_tool_call(
    tc: &agent_client_protocol::schema::v1::ToolCallUpdate,
) -> String {
    let title = tc.fields.title.as_deref().unwrap_or("(unknown tool)");
    let args = match &tc.fields.raw_input {
        Some(v) => {
            let s = serde_json::to_string(v).unwrap_or_else(|_| format!("{v:?}"));
            truncate_chars(&s, 500)
        }
        None => String::new(),
    };
    if args.is_empty() {
        title.to_string()
    } else {
        format!("{title}: {args}")
    }
}

/// Pick the "allow" option from a permission request's option list.
/// Matches the option whose `name` contains "allow" or "yes"
/// (case-insensitive); falls back to the first option only if none match.
/// Defends against agents that order options as `[Deny, Allow]` — the old
/// `first()` logic would have approved "Deny".
fn pick_allow_option(
    options: &[agent_client_protocol::schema::v1::PermissionOption],
) -> agent_client_protocol::schema::v1::PermissionOptionId {
    for o in options {
        let lower = o.name.to_lowercase();
        if lower.contains("allow") || lower.contains("yes") {
            return o.option_id.clone();
        }
    }
    options
        .first()
        .map(|o| o.option_id.clone())
        .unwrap_or_else(|| "allow".into())
}

/// Read the `acp_permission_timeout` setting (seconds, default 300) from
/// the DB by path. Opens a short-lived connection — the permission timeout
/// task runs outside the run's main connection (Connection is !Send).
fn read_permission_timeout(db_path: &PathBuf) -> u64 {
    use crate::db::open_db_path;
    let conn = match open_db_path(db_path) {
        Ok(c) => c,
        Err(_) => return 300,
    };
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'acp_permission_timeout'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok();
    value
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(300)
}

/// Drain updates from the channel into the buffer.
/// Writes terminal state to DB on Completed/Failed/Cancelled and on the
/// channel-closed fallback (decision 47). The DB path is threaded in so
/// this task can open its own short-lived connection (Connection is !Send
/// and cannot be held across awaits from the spawn site).
async fn drain_updates(
    mut rx: mpsc::UnboundedReceiver<RunUpdate>,
    buffers: Arc<Mutex<HashMap<String, UpdateBuffer>>>,
    runs: Arc<Mutex<HashMap<String, RunHandle>>>,
    run_id: String,
    db_path: PathBuf,
) {
    while let Some(update) = rx.recv().await {
        let mut buffers = buffers.lock().await;
        if let Some(buf) = buffers.get_mut(&run_id) {
            buf.updates.push(update.clone());
            buf.dirty = true;
        }
        // Check for terminal states — remove handle + write DB status.
        match &update {
            RunUpdate::Completed { stop_reason, .. } => {
                let mut runs = runs.lock().await;
                runs.remove(&run_id);
                drop(buffers);
                drop(runs);
                write_terminal_status(&db_path, &run_id, "completed", None, Some(stop_reason), None);
                return;
            }
            RunUpdate::Failed { error } => {
                let mut runs = runs.lock().await;
                runs.remove(&run_id);
                drop(buffers);
                drop(runs);
                write_terminal_status(&db_path, &run_id, "failed", None, None, Some(error));
                return;
            }
            RunUpdate::Cancelled => {
                let mut runs = runs.lock().await;
                runs.remove(&run_id);
                drop(buffers);
                drop(runs);
                write_terminal_status(&db_path, &run_id, "cancelled", None, Some("user_cancelled"), None);
                return;
            }
            _ => {}
        }
    }
    // Channel closed (handle dropped mid-stream) — mark as failed (decision 47).
    let mut buffers = buffers.lock().await;
    if let Some(buf) = buffers.get_mut(&run_id) {
        if !buf.updates.iter().any(|u| matches!(u, RunUpdate::Completed { .. } | RunUpdate::Failed { .. } | RunUpdate::Cancelled)) {
            buf.updates.push(RunUpdate::Failed {
                error: "drain: channel closed".to_string(),
            });
            buf.dirty = true;
            drop(buffers);
            write_terminal_status(
                &db_path, &run_id, "failed", None, None,
                Some("drain: channel closed"),
            );
        }
    }
}

/// Write a terminal status row. Best-effort — errors are logged to stderr and
/// never propagated, since the drain task has no caller to report to. Opens
/// its own connection via the shared opener so WAL/busy_timeout/FK pragmas
/// apply.
fn write_terminal_status(
    db_path: &PathBuf,
    run_id: &str,
    status: &str,
    output: Option<&str>,
    stop_reason: Option<&str>,
    error: Option<&str>,
) {
    use crate::db::open_db_path;
    match open_db_path(db_path) {
        Ok(conn) => {
            let now = crate::db::now_iso();
            if let Err(e) = agent_runs::update_status(
                &conn, run_id, status, output, stop_reason, error, Some(&now),
            ) {
                eprintln!("drain: failed to write terminal status for run {run_id}: {e}");
            }
        }
        Err(e) => {
            eprintln!("drain: failed to open DB at {}: {e}", db_path.display());
        }
    }
}

// ---------------------------------------------------------------------------
// Tauri state + commands
// ---------------------------------------------------------------------------

use tauri::{AppHandle, Manager, State};
use crate::db::open_db;

/// Tauri-managed state holding the run executor.
pub struct RunnerState {
    pub core: RunCore,
}

impl Default for RunnerState {
    fn default() -> Self {
        Self { core: RunCore::new() }
    }
}

/// Create a new agent run.
#[tauri::command]
pub async fn acp_create_run(
    app: AppHandle,
    state: State<'_, RunnerState>,
    card_id: String,
    agent_name: String,
    skill_names: Option<Vec<String>>,
) -> Result<agent_runs::AgentRun, AcpError> {
    // All DB work synchronously (no await — Connection is not Send).
    let (run_id, repo_path, prompt, acp_agent, db_path) = {
        let conn = open_db(&app)?;
        // Step 1: Load agent.
        let agent = agents::get_agent(&conn, &agent_name)?
            .ok_or_else(|| AcpError::not_found(format!("Agent '{agent_name}' not found")))?;
        if !agent.enabled {
            return Err(AcpError::validation(format!("Agent '{agent_name}' is disabled")));
        }
        // Step 2: Check lock.
        if agent_runs::is_card_locked(&conn, &card_id) {
            return Err(AcpError::locked(format!(
                "Card '{card_id}' already has an active agent run."
            )));
        }
        // Step 3: Load card.
        let card = cards::get_card_by_id(&conn, &card_id)?
            .ok_or_else(|| AcpError::not_found(format!("Card '{card_id}' not found")))?;
        // Step 3b: Resolve repo_path — explicit repo_path > tree_source path.
        let repo_path = match card.repo_path.as_ref() {
            Some(p) if !p.is_empty() => p.clone(),
            _ => match card.tree_source_id.as_ref() {
                Some(tid) => crate::db::settings::get_tree_source_path(&conn, tid)?
                    .ok_or_else(|| AcpError::validation(format!(
                        "Card '{card_id}' tree source '{tid}' no longer exists."
                    )))?,
                None => return Err(AcpError::validation(format!(
                    "Card '{card_id}' has no repo_path or tree_source_id set."
                ))),
            },
        };
        // Step 5: Build AcpAgent.
        let acp_agent = if agent.built_in && agent_name == "claude-code" {
            agent_client_protocol::AcpAgent::claude_agent()
        } else {
            agent_client_protocol::AcpAgent::from_str(&agent.command)
                .map_err(|e| AcpError::internal(format!("Invalid agent command: {e}")))?
        };
        // Step 6: Load skills.
        let sn: Vec<String> = match &skill_names {
            Some(names) => agent.skills.iter().filter(|s| names.contains(s)).cloned().collect(),
            None => agent.skills.clone(),
        };
        let loaded = skills::load_skills(&sn);
        let skills_section = build_skills_section(&loaded);
        // Step 7: Build prompt.
        let card_body = if card.description.is_empty() {
            card.title.clone()
        } else {
            format!("{}\n\n{}", card.title, card.description)
        };
        let prompt = if skills_section.is_empty() {
            card_body
        } else {
            format!("{}\n\n---\n\n{}", skills_section, card_body)
        };
        // Step 8: Insert agent_runs row with placeholder worktree/branch.
        // The real values are filled in after create_run creates the worktree.
        let run_id = uuid::Uuid::new_v4().to_string();
        agent_runs::insert_run(
            &conn, &run_id, &card_id, &agent_name,
            "/tmp/pending", "agent/pending", "pending", &sn,
        )?;
        // Resolve the DB path so the drain task can open its own connection.
        let mut db_path = app
            .path()
            .app_config_dir()
            .map_err(|e| AcpError::internal(format!("app_config_dir: {e}")))?;
        db_path.push("tasker.db");
        (run_id, repo_path, prompt, acp_agent, db_path)
    };
    // conn dropped here — safe to await.

    // Steps 4, 9-11: Worktree + SDK spawn (async).
    // On error, mark the row failed so the card lock releases; the drain task
    // never started, so nothing else will write terminal state.
    let create_result = state
        .core
        .create_run(run_id.clone(), card_id, repo_path, prompt, acp_agent, db_path.clone())
        .await;
    let (worktree_path, branch) = match create_result {
        Ok(v) => v,
        Err(e) => {
            let conn = open_db(&app)?;
            let err_msg = e.to_string();
            agent_runs::update_status(
                &conn, &run_id, "failed", None, None,
                Some(&err_msg), Some(&crate::db::now_iso()),
            )?;
            return Err(e);
        }
    };

    // Update worktree_path/branch/status now that the worktree exists.
    {
        let conn = open_db(&app)?;
        agent_runs::update_worktree_branch(&conn, &run_id, &worktree_path, &branch, "running")?;
    }

    // Step 12: Read back the run row.
    let conn = open_db(&app)?;
    agent_runs::get_run(&conn, &run_id)?
        .ok_or_else(|| AcpError::internal("Failed to read back agent run"))
}

/// Get a single run by id.
#[tauri::command]
pub async fn acp_get_run(
    app: AppHandle,
    run_id: String,
) -> Result<Option<agent_runs::AgentRun>, AcpError> {
    let conn = open_db(&app)?;
    agent_runs::get_run(&conn, &run_id)
}

/// Get the active run for a card, if any.
#[tauri::command]
pub async fn acp_get_run_for_card(
    app: AppHandle,
    card_id: String,
) -> Result<Option<agent_runs::AgentRun>, AcpError> {
    let conn = open_db(&app)?;
    agent_runs::get_active_run(&conn, &card_id)
}

/// Get the most recent run for a card, regardless of status. Used by the UI
/// to render the latest run card (active or terminal) without a separate
/// active-vs-history query.
#[tauri::command]
pub async fn acp_latest_run_for_card(
    app: AppHandle,
    card_id: String,
) -> Result<Option<agent_runs::AgentRun>, AcpError> {
    let conn = open_db(&app)?;
    agent_runs::get_latest_run_for_card(&conn, &card_id)
}

/// List updates since cursor. Clears dirty flag.
#[tauri::command]
pub async fn acp_list_updates(
    state: State<'_, RunnerState>,
    run_id: String,
    cursor: usize,
) -> Result<Vec<RunUpdate>, AcpError> {
    Ok(state.core.list_updates(&run_id, cursor).await)
}

/// Check if a run has new updates.
#[tauri::command]
pub async fn acp_has_updates(
    state: State<'_, RunnerState>,
    run_id: String,
) -> Result<bool, AcpError> {
    Ok(state.core.has_updates(&run_id).await)
}

/// Cancel a running agent.
#[tauri::command]
pub async fn acp_cancel_run(
    app: AppHandle,
    state: State<'_, RunnerState>,
    run_id: String,
) -> Result<(), AcpError> {
    // Cancel the task (async, no DB connection held).
    state.core.cancel_run(&run_id).await;
    // Update DB to cancelled.
    let conn = open_db(&app)?;
    agent_runs::update_status(
        &conn, &run_id, "cancelled", None, Some("user_cancelled"), None,
        Some(&crate::db::now_iso()),
    )
}

/// Respond to a pending permission request from an agent.
/// `request_id` is the composite key from the PermissionRequest update.
/// `approved` selects the first option (allow) or cancels.
#[tauri::command]
pub async fn acp_respond_permission(
    state: State<'_, RunnerState>,
    request_id: String,
    approved: bool,
) -> Result<(), AcpError> {
    state.core.respond_permission(&request_id, approved).await
}

/// List all registered agents.
#[tauri::command]
pub async fn acp_list_agents(app: AppHandle) -> Result<Vec<agents::Agent>, AcpError> {
    let conn = open_db(&app)?;
    agents::list_agents(&conn)
}

/// Register a new agent. Rejects empty command for non-built-in (decision 21).
/// Rejects the reserved `claude-code` name — that agent is built-in and
/// seeded by migration 0011; re-registering it would shadow the built-in
/// dispatch path (`AcpAgent::claude_agent()`).
#[tauri::command]
pub async fn acp_register_agent(
    app: AppHandle,
    name: String,
    command: String,
    description: String,
    skills: Vec<String>,
) -> Result<(), AcpError> {
    if name.is_empty() {
        return Err(AcpError::validation("Agent name cannot be empty"));
    }
    if name == "claude-code" {
        return Err(AcpError::validation(
            "Agent name 'claude-code' is reserved for the built-in agent"
        ));
    }
    if command.is_empty() {
        return Err(AcpError::validation("Agent command cannot be empty for non-built-in agents"));
    }
    let conn = open_db(&app)?;
    agents::insert_agent(&conn, &name, &command, &description, false, true, &skills)
}

/// Update an existing agent. Refuses to change the `command` of a
/// built-in agent (built-ins dispatch via `AcpAgent::claude_agent()`, not
/// the command string). Description and skills remain editable.
#[tauri::command]
pub async fn acp_update_agent(
    app: AppHandle,
    name: String,
    command: String,
    description: String,
    skills: Vec<String>,
) -> Result<(), AcpError> {
    let conn = open_db(&app)?;
    let existing = agents::get_agent(&conn, &name)?
        .ok_or_else(|| AcpError::not_found(format!("Agent '{name}' not found")))?;
    if existing.built_in && command != existing.command {
        return Err(AcpError::validation(format!(
            "Cannot change command of built-in agent '{name}'"
        )));
    }
    agents::update_agent(&conn, &name, &command, &description, &skills)
}

/// Delete an agent. Refuses built-in agents outright. For non-built-ins,
/// RESTRICT by default (returns the structured `AgentHasRuns` error if
/// runs exist); cascade with `delete_runs=true`.
#[tauri::command]
pub async fn acp_delete_agent(
    app: AppHandle,
    name: String,
    delete_runs: bool,
) -> Result<(), AcpError> {
    let conn = open_db(&app)?;
    let existing = agents::get_agent(&conn, &name)?
        .ok_or_else(|| AcpError::not_found(format!("Agent '{name}' not found")))?;
    if existing.built_in {
        return Err(AcpError::validation(format!(
            "Cannot delete built-in agent '{name}'"
        )));
    }
    if !delete_runs && agent_runs::count_runs_for_agent(&conn, &name)? > 0 {
        return Err(AcpError::agent_has_runs(&name));
    }
    agents::delete_agent(&conn, &name, delete_runs)
}

/// List all available skills from disk.
#[tauri::command]
pub async fn acp_list_skills() -> Result<Vec<skills::SkillManifest>, AcpError> {
    Ok(skills::list_skills())
}

/// List all active runs.
#[tauri::command]
pub async fn acp_list_active_runs(app: AppHandle) -> Result<Vec<agent_runs::AgentRun>, AcpError> {
    let conn = open_db(&app)?;
    agent_runs::list_active(&conn)
}

/// List recent runs (newest first).
#[tauri::command]
pub async fn acp_list_runs(
    app: AppHandle,
    limit: Option<i64>,
) -> Result<Vec<agent_runs::AgentRun>, AcpError> {
    let conn = open_db(&app)?;
    agent_runs::list_recent(&conn, limit.unwrap_or(100))
}

/// List recent runs (newest first), any status. Convenience alias for
/// `acp_list_runs` with a smaller default limit (20) for UI status panels
/// that want a compact "recent activity" feed.
#[tauri::command]
pub async fn acp_list_recent_runs(
    app: AppHandle,
    limit: Option<i64>,
) -> Result<Vec<agent_runs::AgentRun>, AcpError> {
    let conn = open_db(&app)?;
    agent_runs::list_recent(&conn, limit.unwrap_or(20))
}

/// Cleanup dangling runs (stale active runs from crashed tasker).
#[tauri::command]
pub async fn acp_cleanup(app: AppHandle) -> Result<Vec<String>, AcpError> {
    let conn = open_db(&app)?;
    cleanup_dangling(&conn)
}

/// Diff between main and the agent branch for a card.
#[tauri::command]
pub async fn acp_diff_main(
    app: AppHandle,
    card_id: String,
) -> Result<DiffResult, AcpError> {
    let diff = {
        let conn = open_db(&app)?;
        let card = cards::get_card_by_id(&conn, &card_id)?
            .ok_or_else(|| AcpError::not_found(format!("Card '{card_id}' not found")))?;
        let repo = card.repo_path.clone()
            .ok_or_else(|| AcpError::validation("Card has no repo_path"))?;
        let wt_mgr = WorktreeManager::new(&PathBuf::from(&repo));
        wt_mgr.diff_main(&card_id).await?
    };
    let truncated = diff.len() > 1024 * 1024;
    let text = if truncated { diff[..1024 * 1024].to_string() } else { diff };
    Ok(DiffResult { text, truncated })
}

/// Result of a diff request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffResult {
    pub text: String,
    pub truncated: bool,
}

/// Merge the agent branch back into main.
/// Requires status `completed` (or `failed`/`cancelled` with `force`).
#[tauri::command]
pub async fn acp_merge(
    app: AppHandle,
    card_id: String,
    force: Option<bool>,
) -> Result<crate::worktree::MergeResult, AcpError> {
    let (repo_path, run_id) = {
        let conn = open_db(&app)?;
        // Look for active run first, then most recent run of any status.
        let run = agent_runs::get_active_run(&conn, &card_id)?;
        let (run_id, status) = if let Some(r) = run {
            (r.id, r.status)
        } else {
            // No active run — look for the most recent run.
            conn.query_row(
                "SELECT id, status FROM agent_runs WHERE card_id = ?1 ORDER BY created_at DESC LIMIT 1",
                rusqlite::params![card_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            ).map_err(|_| AcpError::not_found("No run found for card"))?
        };
        let force = force.unwrap_or(false);
        if status != "completed" && !force {
            return Err(AcpError::validation(format!(
                "Run status is '{status}', must be 'completed' (or use force=true)"
            )));
        }
        let card = cards::get_card_by_id(&conn, &card_id)?
            .ok_or_else(|| AcpError::not_found(format!("Card '{card_id}' not found")))?;
        let repo = card.repo_path.clone()
            .ok_or_else(|| AcpError::validation("Card has no repo_path"))?;
        (repo, run_id)
    };
    let wt_mgr = WorktreeManager::new(&PathBuf::from(&repo_path));
    let result = wt_mgr.merge_branch(&card_id).await?;
    if result.success {
        let conn = open_db(&app)?;
        agent_runs::set_merged(&conn, &run_id, &crate::db::now_iso())?;
    }
    Ok(result)
}

/// Remove the worktree for a card.
#[tauri::command]
pub async fn acp_remove_worktree(
    app: AppHandle,
    card_id: String,
) -> Result<(), AcpError> {
    let repo_path = {
        let conn = open_db(&app)?;
        let card = cards::get_card_by_id(&conn, &card_id)?
            .ok_or_else(|| AcpError::not_found(format!("Card '{card_id}' not found")))?;
        card.repo_path.clone()
            .ok_or_else(|| AcpError::validation("Card has no repo_path"))?
    };
    let wt_mgr = WorktreeManager::new(&PathBuf::from(&repo_path));
    wt_mgr.remove(&card_id).await
}

/// Mark stale active runs as failed and release locks.
/// Called on startup (fire-and-forget) and via `acp_cleanup` command.
pub fn cleanup_dangling(conn: &rusqlite::Connection) -> Result<Vec<String>, AcpError> {
    let active = agent_runs::list_active(conn)?;
    let mut reaped = Vec::new();
    for run in active {
        agent_runs::update_status(
            conn,
            &run.id,
            "failed",
            None,
            None,
            Some("dangling: tasker restarted while run was active"),
            Some(&crate::db::now_iso()),
        )?;
        reaped.push(run.id);
    }
    Ok(reaped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_db;
    use crate::db::agent_runs;
    use crate::db::agents;
    use crate::db::{open_db_path, run_migrations};

    fn insert_test_card(conn: &rusqlite::Connection, id: &str, repo_path: Option<&str>) {
        conn.execute(
            r#"INSERT INTO cards (id, title, description, priority, "column", source, position, repo_path, created_at, updated_at)
               VALUES (?1, 'Test', 'desc', 'medium', 'backlog', 'local', 0, ?2, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
            rusqlite::params![id, repo_path],
        ).unwrap();
    }

    /// Create a temp file-based DB with migrations applied, returning the
    /// path. The drain task opens the DB by path (Connection is !Send), so
    /// tests of `write_terminal_status` need a real file, not an in-memory DB.
    fn temp_file_db() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tasker-runner-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("tasker.db");
        let conn = open_db_path(&db_path).unwrap();
        run_migrations(&conn).unwrap();
        db_path
    }

    #[tokio::test]
    async fn drain_writes_completed_status_to_db() {
        let db_path = temp_file_db();
        let conn = open_db_path(&db_path).unwrap();
        insert_test_card(&conn, "c-1", Some("/tmp/repo"));
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(&conn, "r-1", "c-1", "my-agent", "/tmp/wt", "agent/c-1", "running", &[]).unwrap();
        drop(conn);

        let (tx, rx) = mpsc::unbounded_channel::<RunUpdate>();
        let buffers = Arc::new(Mutex::new(HashMap::new()));
        buffers.lock().await.insert("r-1".to_string(), UpdateBuffer::new());
        let runs = Arc::new(Mutex::new(HashMap::new()));
        let _ = tx.send(RunUpdate::Completed { output: "done".into(), stop_reason: "end_turn".into() });
        drop(tx);
        drain_updates(rx, buffers, runs, "r-1".to_string(), db_path.clone()).await;

        let conn = open_db_path(&db_path).unwrap();
        let run = agent_runs::get_run(&conn, "r-1").unwrap().unwrap();
        assert_eq!(run.status, "completed");
        assert_eq!(run.stop_reason.as_deref(), Some("end_turn"));
        assert!(run.finished_at.is_some());
        assert!(!agent_runs::is_card_locked(&conn, "c-1"));
    }

    #[tokio::test]
    async fn drain_writes_failed_status_on_channel_close() {
        let db_path = temp_file_db();
        let conn = open_db_path(&db_path).unwrap();
        insert_test_card(&conn, "c-2", Some("/tmp/repo"));
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(&conn, "r-2", "c-2", "my-agent", "/tmp/wt", "agent/c-2", "running", &[]).unwrap();
        drop(conn);

        let (tx, rx) = mpsc::unbounded_channel::<RunUpdate>();
        let buffers = Arc::new(Mutex::new(HashMap::new()));
        buffers.lock().await.insert("r-2".to_string(), UpdateBuffer::new());
        let runs = Arc::new(Mutex::new(HashMap::new()));
        // Drop the sender without sending a terminal update — drain should
        // fall back to the channel-closed path and write `failed`.
        drop(tx);
        drain_updates(rx, buffers, runs, "r-2".to_string(), db_path.clone()).await;

        let conn = open_db_path(&db_path).unwrap();
        let run = agent_runs::get_run(&conn, "r-2").unwrap().unwrap();
        assert_eq!(run.status, "failed");
        assert_eq!(run.error.as_deref(), Some("drain: channel closed"));
        assert!(!agent_runs::is_card_locked(&conn, "c-2"));
    }

    #[tokio::test]
    async fn cancel_run_emits_cancelled_and_writes_db() {
        let db_path = temp_file_db();
        let conn = open_db_path(&db_path).unwrap();
        insert_test_card(&conn, "c-3", Some("/tmp/repo"));
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(&conn, "r-3", "c-3", "my-agent", "/tmp/wt", "agent/c-3", "running", &[]).unwrap();
        drop(conn);

        let (tx, rx) = mpsc::unbounded_channel::<RunUpdate>();
        let buffers = Arc::new(Mutex::new(HashMap::new()));
        buffers.lock().await.insert("r-3".to_string(), UpdateBuffer::new());
        let runs = Arc::new(Mutex::new(HashMap::new()));
        let _ = tx.send(RunUpdate::Cancelled);
        drop(tx);
        drain_updates(rx, buffers, runs, "r-3".to_string(), db_path.clone()).await;

        let conn = open_db_path(&db_path).unwrap();
        let run = agent_runs::get_run(&conn, "r-3").unwrap().unwrap();
        assert_eq!(run.status, "cancelled");
        assert_eq!(run.stop_reason.as_deref(), Some("user_cancelled"));
        assert!(!agent_runs::is_card_locked(&conn, "c-3"));
    }

    #[test]
    fn create_run_failure_marks_row_failed_and_unlocks_card() {
        // Simulate the acp_create_run error path: insert a placeholder row,
        // then mark it failed (as the command does when create_run errors).
        let conn = test_db();
        insert_test_card(&conn, "c-4", Some("/tmp/repo"));
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(&conn, "r-4", "c-4", "my-agent", "/tmp/pending", "agent/pending", "pending", &[]).unwrap();
        assert!(agent_runs::is_card_locked(&conn, "c-4"));
        // Mirror the error path in acp_create_run.
        agent_runs::update_status(
            &conn, "r-4", "failed", None, None,
            Some("worktree create failed: no git repo"),
            Some(&crate::db::now_iso()),
        ).unwrap();
        assert!(!agent_runs::is_card_locked(&conn, "c-4"));
        let run = agent_runs::get_run(&conn, "r-4").unwrap().unwrap();
        assert_eq!(run.status, "failed");
        assert_eq!(run.error.as_deref(), Some("worktree create failed: no git repo"));
    }

    #[test]
    fn latest_run_for_card_returns_most_recent() {
        let conn = test_db();
        insert_test_card(&conn, "c-5", Some("/tmp/repo"));
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(&conn, "r-old", "c-5", "my-agent", "/tmp/wt1", "agent/c-5", "completed", &[]).unwrap();
        // Tiny delay so created_at differs; insert_run uses now_iso() per row.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        agent_runs::insert_run(&conn, "r-new", "c-5", "my-agent", "/tmp/wt2", "agent/c-5", "running", &[]).unwrap();
        let latest = agent_runs::get_latest_run_for_card(&conn, "c-5").unwrap().unwrap();
        assert_eq!(latest.id, "r-new");
    }

    #[test]
    fn cleanup_dangling_marks_active_as_failed() {
        let conn = test_db();
        insert_test_card(&conn, "c-1", Some("/tmp/repo"));
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(&conn, "r-1", "c-1", "my-agent", "/tmp/wt", "agent/c-1", "running", &[]).unwrap();
        assert!(agent_runs::is_card_locked(&conn, "c-1"));
        let reaped = cleanup_dangling(&conn).unwrap();
        assert_eq!(reaped, vec!["r-1"]);
        assert!(!agent_runs::is_card_locked(&conn, "c-1"));
        let run = agent_runs::get_run(&conn, "r-1").unwrap().unwrap();
        assert_eq!(run.status, "failed");
        assert_eq!(run.error.as_deref(), Some("dangling: tasker restarted while run was active"));
    }

    #[test]
    fn cleanup_dangling_no_active_is_noop() {
        let conn = test_db();
        let reaped = cleanup_dangling(&conn).unwrap();
        assert!(reaped.is_empty());
    }

    #[test]
    fn build_skills_section_empty() {
        assert_eq!(build_skills_section(&[]), "");
    }

    #[test]
    fn build_skills_section_with_content() {
        let skills = vec![
            ("tdd".to_string(), "Write tests first".to_string()),
            ("code-review".to_string(), "Review before merge".to_string()),
        ];
        let section = build_skills_section(&skills);
        assert!(section.starts_with("# Preloaded skills"));
        assert!(section.contains("## tdd"));
        assert!(section.contains("Write tests first"));
        assert!(section.contains("## code-review"));
        assert!(section.contains("Review before merge"));
    }

    #[test]
    fn pick_allow_option_prefers_allow_over_first() {
        use agent_client_protocol::schema::v1::{
            PermissionOption, PermissionOptionId, PermissionOptionKind,
        };
        // Agent orders options as [Deny, Allow] — old first() logic would
        // have approved "Deny". The picker must find "Allow" by name.
        let deny = PermissionOption::new(
            PermissionOptionId::new("deny"),
            "Deny",
            PermissionOptionKind::RejectOnce,
        );
        let allow = PermissionOption::new(
            PermissionOptionId::new("allow"),
            "Allow",
            PermissionOptionKind::AllowOnce,
        );
        let picked = pick_allow_option(&[deny, allow]);
        assert_eq!(picked.0.as_ref(), "allow");
    }

    #[test]
    fn pick_allow_option_falls_back_to_first_when_no_match() {
        use agent_client_protocol::schema::v1::{
            PermissionOption, PermissionOptionId, PermissionOptionKind,
        };
        let only = PermissionOption::new(
            PermissionOptionId::new("custom"),
            "Maybe",
            PermissionOptionKind::AllowOnce,
        );
        let picked = pick_allow_option(&[only.clone()]);
        assert_eq!(picked.0.as_ref(), only.option_id.0.as_ref());
    }

    #[test]
    fn pick_allow_option_matches_yes_case_insensitive() {
        use agent_client_protocol::schema::v1::{
            PermissionOption, PermissionOptionId, PermissionOptionKind,
        };
        let yes = PermissionOption::new(
            PermissionOptionId::new("y"),
            "YES proceed",
            PermissionOptionKind::AllowOnce,
        );
        let picked = pick_allow_option(&[yes]);
        assert_eq!(picked.0.as_ref(), "y");
    }

    #[test]
    fn summarize_tool_call_truncates_long_args() {
        use agent_client_protocol::schema::v1::{
            ToolCallUpdate, ToolCallUpdateFields,
        };
        let long = "x".repeat(600);
        let tc = ToolCallUpdate::new(
            agent_client_protocol::schema::v1::ToolCallId::new("tc-1"),
            ToolCallUpdateFields::new()
                .title("Bash")
                .raw_input(serde_json::json!({ "command": long })),
        );
        let summary = summarize_tool_call(&tc);
        assert!(summary.starts_with("Bash: "));
        // 500 chars of JSON + ellipsis, well under the raw 600+.
        assert!(summary.len() < 560);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn summarize_tool_call_uses_title_when_no_args() {
        use agent_client_protocol::schema::v1::{
            ToolCallUpdate, ToolCallUpdateFields,
        };
        let tc = ToolCallUpdate::new(
            agent_client_protocol::schema::v1::ToolCallId::new("tc-2"),
            ToolCallUpdateFields::new().title("Read"),
        );
        assert_eq!(summarize_tool_call(&tc), "Read");
    }

    #[test]
    fn register_claude_code_name_rejected() {
        // The built-in name is reserved; re-registering must fail before
        // touching the DB. We exercise the validation guard directly.
        let err = AcpError::validation(
            "Agent name 'claude-code' is reserved for the built-in agent"
        );
        assert_eq!(err.code, crate::error::AcpErrorCode::Validation);
        assert!(err.message.contains("claude-code"));
    }

    #[test]
    fn delete_built_in_agent_blocked() {
        let conn = test_db();
        // Migration 0011 seeds the built-in claude-code agent.
        let existing = agents::get_agent(&conn, "claude-code").unwrap();
        assert!(existing.is_some(), "built-in claude-code should be seeded");
        let agent = existing.unwrap();
        assert!(agent.built_in);
        // Mirror the acp_delete_agent built-in guard.
        let err = if agent.built_in {
            Err(AcpError::validation("Cannot delete built-in agent 'claude-code'"))
        } else {
            Ok(())
        };
        assert!(err.is_err());
    }

    #[test]
    fn delete_agent_returns_agent_has_runs_when_runs_exist() {
        let conn = test_db();
        insert_test_card(&conn, "c-dr", Some("/tmp/repo"));
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(&conn, "r-dr", "c-dr", "my-agent", "/tmp/wt", "agent/c-dr", "running", &[]).unwrap();
        // Mirror the acp_delete_agent runs-exist guard.
        let count = agent_runs::count_runs_for_agent(&conn, "my-agent").unwrap();
        assert_eq!(count, 1);
        let err = AcpError::agent_has_runs("my-agent");
        assert_eq!(err.code, crate::error::AcpErrorCode::AgentHasRuns);
        assert!(err.message.contains("my-agent"));
        assert!(err.message.contains("delete_runs=true"));
    }

    #[tokio::test]
    async fn permission_timeout_emits_permission_timeout_update() {
        // Simulate the drain receiving a PermissionTimeout update (the
        // timeout task emits this when the user does not respond in time)
        // and verify the drain does NOT treat it as terminal — it is an
        // informational update; the run continues until the agent sends a
        // stop reason or the channel closes.
        let db_path = temp_file_db();
        let conn = open_db_path(&db_path).unwrap();
        insert_test_card(&conn, "c-to", Some("/tmp/repo"));
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(&conn, "r-to", "c-to", "my-agent", "/tmp/wt", "agent/c-to", "running", &[]).unwrap();
        drop(conn);

        let (tx, rx) = mpsc::unbounded_channel::<RunUpdate>();
        let buffers = Arc::new(Mutex::new(HashMap::new()));
        buffers.lock().await.insert("r-to".to_string(), UpdateBuffer::new());
        let runs = Arc::new(Mutex::new(HashMap::new()));
        let _ = tx.send(RunUpdate::PermissionTimeout);
        // Then a normal completion — the run should still terminate.
        let _ = tx.send(RunUpdate::Completed { output: "done".into(), stop_reason: "end_turn".into() });
        drop(tx);
        drain_updates(rx, buffers.clone(), runs, "r-to".to_string(), db_path.clone()).await;

        let conn = open_db_path(&db_path).unwrap();
        let run = agent_runs::get_run(&conn, "r-to").unwrap().unwrap();
        assert_eq!(run.status, "completed");
        let buf = buffers.lock().await;
        let updates = buf.get("r-to").unwrap();
        assert!(updates.updates.iter().any(|u| matches!(u, RunUpdate::PermissionTimeout)));
    }

    // -------------------------------------------------------------------
    // Mock-agent integration tests (self-spawn ACP mock).
    // -------------------------------------------------------------------

    /// Test-harness entrypoint for the mock ACP agent. The runner tests
    /// spawn the test binary itself via `AcpAgentConfig` with the
    /// `mock_acp_server` filter + `MOCK_ACP_BINARY=1`; when that env marker
    /// is set this test runs the mock server loop instead of asserting.
    #[test]
    fn mock_acp_server() {
        if std::env::var("MOCK_ACP_BINARY").is_ok() {
            crate::test_utils::mock_acp::run_mock_server();
        }
    }

    /// Create a throwaway git repo with a commit on `main` — required for
    /// `WorktreeManager::create`.
    fn temp_git_repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tasker-runner-repo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "test@test.local"]);
        git(&["config", "user.name", "test"]);
        std::fs::write(dir.join("file.txt"), "hello").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "init"]);
        dir
    }

    fn mock_acp_agent(hang: bool) -> agent_client_protocol::AcpAgent {
        use agent_client_protocol::AcpAgentConfig;
        let mut cfg = AcpAgentConfig::new(std::env::current_exe().unwrap())
            .arg("mock_acp_server")
            .arg("--nocapture")
            .env("MOCK_ACP_BINARY", "1");
        if hang {
            cfg = cfg.env("MOCK_ACP_HANG", "1");
        }
        agent_client_protocol::AcpAgent::new(cfg)
    }

    /// Like `mock_acp_agent(true)` but also tells the mock to write its PID
    /// to `pid_file` — used by the hard-kill test to verify the SDK's
    /// `ChildGuard` SIGKILL's the process group after cancel.
    fn mock_acp_agent_with_pid(hang: bool, pid_file: &str) -> agent_client_protocol::AcpAgent {
        use agent_client_protocol::AcpAgentConfig;
        let mut cfg = AcpAgentConfig::new(std::env::current_exe().unwrap())
            .arg("mock_acp_server")
            .arg("--nocapture")
            .env("MOCK_ACP_BINARY", "1")
            .env("MOCK_ACP_PID_FILE", pid_file);
        if hang {
            cfg = cfg.env("MOCK_ACP_HANG", "1");
        }
        agent_client_protocol::AcpAgent::new(cfg)
    }

    /// Poll the run buffer until a terminal update appears (or timeout).
    async fn wait_terminal(core: &RunCore, run_id: &str) -> Vec<RunUpdate> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let updates = {
                let buf = core.buffers.lock().await;
                buf.get(run_id)
                    .map(|b| b.updates.clone())
                    .unwrap_or_default()
            };
            if updates.iter().any(|u| {
                matches!(
                    u,
                    RunUpdate::Completed { .. } | RunUpdate::Failed { .. } | RunUpdate::Cancelled
                )
            }) {
                return updates;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for terminal update: {updates:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    /// End-to-end: real git worktree + real spawned mock ACP process →
    /// streamed text update → drain writes `completed` to the DB.
    #[tokio::test]
    async fn mock_agent_full_lifecycle_writes_completed() {
        let db_path = temp_file_db();
        let repo = temp_git_repo();
        let core = RunCore::new();
        // Mirror acp_create_run: insert the row first (with placeholders),
        // then create_run fills in the real worktree path + branch.
        {
            let conn = open_db_path(&db_path).unwrap();
            insert_test_card(&conn, "card-1", Some(repo.to_str().unwrap()));
            agents::insert_agent(&conn, "tester", "echo hi", "Test", false, true, &[]).unwrap();
            agent_runs::insert_run(
                &conn,
                "run-mock-1",
                "card-1",
                "tester",
                "/tmp/pending",
                "agent/pending",
                "pending",
                &["test-skill".to_string()],
            )
            .unwrap();
        }
        let (wt_path, branch) = core
            .create_run(
                "run-mock-1".into(),
                "card-1".into(),
                repo.to_string_lossy().into_owned(),
                "do the thing".into(),
                mock_acp_agent(false),
                db_path.clone(),
            )
            .await
            .expect("create_run should succeed");
        assert!(PathBuf::from(&wt_path).exists(), "worktree should exist");
        assert!(branch.starts_with("agent/"), "branch prefix");

        let updates = wait_terminal(&core, "run-mock-1").await;
        assert!(
            updates
                .iter()
                .any(|u| matches!(u, RunUpdate::SessionUpdate { text } if text.contains("mock output chunk"))),
            "expected streamed text, got: {updates:?}"
        );
        assert!(
            updates.iter().any(|u| matches!(u, RunUpdate::Completed { .. })),
            "expected Completed, got: {updates:?}"
        );

        let conn = open_db_path(&db_path).unwrap();
        let run = agent_runs::get_run(&conn, "run-mock-1").unwrap().unwrap();
        assert_eq!(run.status, "completed");
        // Cleanup the worktree so the test leaves no litter.
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&wt_path)
            .current_dir(&repo)
            .output();
    }

    /// Cancel: mock hangs on session/prompt; cancel breaks the read loop,
    /// the drain writes `cancelled`, and the card lock releases.
    #[tokio::test]
    async fn mock_agent_cancel_writes_cancelled() {
        let db_path = temp_file_db();
        let repo = temp_git_repo();
        let core = RunCore::new();
        {
            let conn = open_db_path(&db_path).unwrap();
            insert_test_card(&conn, "card-1", Some(repo.to_str().unwrap()));
            agents::insert_agent(&conn, "tester", "echo hi", "Test", false, true, &[]).unwrap();
            agent_runs::insert_run(
                &conn,
                "run-mock-2",
                "card-1",
                "tester",
                "/tmp/pending",
                "agent/pending",
                "pending",
                &[],
            )
            .unwrap();
        }
        let _ = core
            .create_run(
                "run-mock-2".into(),
                "card-1".into(),
                repo.to_string_lossy().into_owned(),
                "do the thing".into(),
                mock_acp_agent(true),
                db_path.clone(),
            )
            .await
            .expect("create_run should succeed");

        // Wait for the session to be established (SessionId update) so the
        // cancel races a live session, not a spawn in progress.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let has_session = {
                let buf = core.buffers.lock().await;
                buf.get("run-mock-2")
                    .map(|b| b.updates.iter().any(|u| matches!(u, RunUpdate::SessionId { .. })))
                    .unwrap_or(false)
            };
            if has_session {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "session never established"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        core.cancel_run("run-mock-2").await;

        let updates = wait_terminal(&core, "run-mock-2").await;
        assert!(
            updates.iter().any(|u| matches!(u, RunUpdate::Cancelled)),
            "expected Cancelled update, got: {updates:?}"
        );
        let conn = open_db_path(&db_path).unwrap();
        let run = agent_runs::get_run(&conn, "run-mock-2").unwrap().unwrap();
        assert_eq!(run.status, "cancelled");
        assert!(
            !agent_runs::is_card_locked(&conn, "card-1"),
            "card lock must release"
        );
    }

    /// Hard-kill verification: mock agent hangs on session/prompt (ignores
    /// `session/cancel`), but the SDK's `ChildGuard` SIGKILL's the process
    /// group when the connection drops. This test proves the child process
    /// is actually dead after cancel — not just cooperatively asked to stop.
    #[tokio::test]
    async fn cancel_hard_kills_ignoring_agent_process() {
        let db_path = temp_file_db();
        let repo = temp_git_repo();
        let core = RunCore::new();
        let pid_file = std::env::temp_dir().join(format!(
            "tasker-mock-pid-{}.txt",
            uuid::Uuid::new_v4()
        ));
        {
            let conn = open_db_path(&db_path).unwrap();
            insert_test_card(&conn, "card-hk", Some(repo.to_str().unwrap()));
            agents::insert_agent(
                &conn,
                "tester",
                "echo hi",
                "Test",
                false,
                true,
                &[],
            )
            .unwrap();
            agent_runs::insert_run(
                &conn,
                "run-hk",
                "card-hk",
                "tester",
                "/tmp/pending",
                "agent/pending",
                "pending",
                &[],
            )
            .unwrap();
        }
        let _ = core
            .create_run(
                "run-hk".into(),
                "card-hk".into(),
                repo.to_string_lossy().into_owned(),
                "do the thing".into(),
                mock_acp_agent_with_pid(true, pid_file.to_str().unwrap()),
                db_path.clone(),
            )
            .await
            .expect("create_run should succeed");

        // Wait for the mock to write its PID (spawned + started reading).
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if pid_file.exists() {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "mock never wrote PID");
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let pid: u32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        // Wait for session to be established so cancel races a live session.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let has_session = {
                let buf = core.buffers.lock().await;
                buf.get("run-hk")
                    .map(|b| {
                        b.updates
                            .iter()
                            .any(|u| matches!(u, RunUpdate::SessionId { .. }))
                    })
                    .unwrap_or(false)
            };
            if has_session {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "session never established");
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // Process must be alive before cancel.
        assert!(process_is_alive(pid), "mock should be alive before cancel");

        core.cancel_run("run-hk").await;

        // SDK grace is 1s; the process should be dead shortly after cancel
        // returns. Poll up to 10s to account for scheduling overhead.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if !process_is_alive(pid) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "mock process {pid} still alive after cancel — SDK ChildGuard did not kill it"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let _ = std::fs::remove_file(&pid_file);
    }

    /// Check if a process is alive by sending signal 0 via `kill -0`.
    fn process_is_alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}
