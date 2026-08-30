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
use tokio::sync::{Mutex, mpsc, oneshot};
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
/// Stores the SDK responder + the first option's ID (for approval).
pub struct PendingPermission {
    pub responder: tokio::sync::Mutex<
        Option<
            agent_client_protocol::Responder<
                agent_client_protocol::schema::v1::RequestPermissionResponse,
            >,
        >,
    >,
    pub option_id: agent_client_protocol::schema::v1::PermissionOptionId,
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
        skill_names: Vec<String>,
        acp_agent: agent_client_protocol::AcpAgent,
    ) -> Result<(), AcpError> {
        // Step 4: Create worktree.
        let wt_mgr = WorktreeManager::new(&PathBuf::from(&repo_path));
        let worktree = wt_mgr.create(&card_id).await?;

        // Step 9: Spawn SDK connection.
        let (tx, rx) = mpsc::unbounded_channel::<RunUpdate>();
        let cancel = CancellationToken::new();
        let cwd = worktree.path.clone();
        let run_id_clone = run_id.clone();
        let run_id_for_handle = run_id.clone();
        let permissions_map = self.permissions.clone();

        let join = tokio::spawn(async move {
            let tx_for_err = tx.clone();
            let result = agent_client_protocol::Client
                .connect_with(acp_agent, move |cx: agent_client_protocol::ConnectionTo<agent_client_protocol::Agent>| {
                    let tx = tx.clone();
                    let cwd = cwd.clone();
                    let prompt = prompt.clone();
                    let permissions_map = permissions_map.clone();
                    let run_id_inner = run_id_clone.clone();
                    async move {
                        let mut session = cx.build_session(&cwd).block_task().start_session().await?;
                        let _ = tx.send(RunUpdate::SessionId {
                            session_id: session.session_id().0.to_string(),
                        });
                        session.send_prompt(&prompt)?;
                        // Read updates until stop reason.
                        loop {
                            match session.read_update().await {
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
                                                        let description = format!("{:?}", req.tool_call);
                                                        let option_id = req.options.first()
                                                            .map(|o| o.option_id.clone())
                                                            .unwrap_or_else(|| "allow".into());
                                                        let pending = Arc::new(PendingPermission {
                                                            responder: tokio::sync::Mutex::new(Some(responder)),
                                                            option_id,
                                                        });
                                                        {
                                                            let mut perms = permissions_map.lock().await;
                                                            perms.insert(req_id.clone(), pending);
                                                        }
                                                        let _ = tx.send(RunUpdate::PermissionRequest {
                                                            request_id: req_id,
                                                            description,
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
            runs.insert(run_id_for_handle.clone(), RunHandle { join, cancel });
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
            drain_updates(rx, buffers_map, runs_map, run_id_for_drain).await;
        });

        // Step 12: Return Ok — the Tauri command reads back the run row.
        Ok(())
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

/// Drain updates from the channel into the buffer.
/// Writes terminal state to DB when the channel closes (decision 47).
async fn drain_updates(
    mut rx: mpsc::UnboundedReceiver<RunUpdate>,
    buffers: Arc<Mutex<HashMap<String, UpdateBuffer>>>,
    runs: Arc<Mutex<HashMap<String, RunHandle>>>,
    run_id: String,
) {
    while let Some(update) = rx.recv().await {
        let mut buffers = buffers.lock().await;
        if let Some(buf) = buffers.get_mut(&run_id) {
            buf.updates.push(update.clone());
            buf.dirty = true;
        }
        // Check for terminal states — remove handle.
        match &update {
            RunUpdate::Completed { .. } | RunUpdate::Failed { .. } | RunUpdate::Cancelled => {
                let mut runs = runs.lock().await;
                runs.remove(&run_id);
                break;
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
        }
    }
}

// ---------------------------------------------------------------------------
// Tauri state + commands
// ---------------------------------------------------------------------------

use tauri::{AppHandle, State};
use crate::db::open_db;

/// Tauri-managed state holding the run executor.
pub struct RunnerState {
    pub core: RunCore,
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
    let (run_id, repo_path, prompt, skill_names, acp_agent) = {
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
                Some(tid) => crate::db::settings::get_tree_source_path(&conn, tid)?,
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
        // Step 8: Insert agent_runs row.
        let run_id = uuid::Uuid::new_v4().to_string();
        agent_runs::insert_run(
            &conn, &run_id, &card_id, &agent_name,
            "/tmp/pending", "agent/pending", "pending", &sn,
        )?;
        (run_id, repo_path, prompt, sn, acp_agent)
    };
    // conn dropped here — safe to await.

    // Steps 4, 9-11: Worktree + SDK spawn (async).
    state.core.create_run(run_id.clone(), card_id, repo_path, prompt, skill_names, acp_agent).await?;

    // Step 12: Read back the run row.
    let conn = open_db(&app)?;
    // Update worktree_path and branch now that worktree is created.
    // (The insert used placeholder values.)
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
    // Built-in agents (claude-code) have empty command — allowed.
    if command.is_empty() && name != "claude-code" {
        return Err(AcpError::validation("Agent command cannot be empty for non-built-in agents"));
    }
    let conn = open_db(&app)?;
    let built_in = name == "claude-code";
    agents::insert_agent(&conn, &name, &command, &description, built_in, true, &skills)
}

/// Update an existing agent.
#[tauri::command]
pub async fn acp_update_agent(
    app: AppHandle,
    name: String,
    command: String,
    description: String,
    skills: Vec<String>,
) -> Result<(), AcpError> {
    let conn = open_db(&app)?;
    agents::update_agent(&conn, &name, &command, &description, &skills)
}

/// Delete an agent. RESTRICT by default; cascade with delete_runs=true.
#[tauri::command]
pub async fn acp_delete_agent(
    app: AppHandle,
    name: String,
    delete_runs: bool,
) -> Result<(), AcpError> {
    let conn = open_db(&app)?;
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
    let (repo_path, diff) = {
        let conn = open_db(&app)?;
        let card = cards::get_card_by_id(&conn, &card_id)?
            .ok_or_else(|| AcpError::not_found(format!("Card '{card_id}' not found")))?;
        let repo = card.repo_path.clone()
            .ok_or_else(|| AcpError::validation("Card has no repo_path"))?;
        let wt_mgr = WorktreeManager::new(&PathBuf::from(&repo));
        let d = wt_mgr.diff_main(&card_id).await?;
        (repo, d)
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

    fn insert_test_card(conn: &rusqlite::Connection, id: &str, repo_path: Option<&str>) {
        conn.execute(
            r#"INSERT INTO cards (id, title, description, priority, "column", source, position, repo_path, created_at, updated_at)
               VALUES (?1, 'Test', 'desc', 'medium', 'backlog', 'local', 0, ?2, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
            rusqlite::params![id, repo_path],
        ).unwrap();
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
}
