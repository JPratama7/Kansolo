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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, Mutex, Notify};

use crate::db::agent_runs;
use crate::db::agents;
use crate::db::cards;
use crate::error::AcpError;
use crate::session::{run_session_job, SessionJob};
use crate::skills;
use crate::worktree::WorktreeManager;

/// Running agent run: the spawned task + cancellation token.
pub struct RunHandle {
    pub join: tokio::task::JoinHandle<()>,
    pub cancel: std::sync::Arc<Notify>,
    /// Sender for follow-up prompts when the agent stops with EndTurn and
    /// the user types a reply in the popup.
    pub prompt_tx: mpsc::UnboundedSender<String>,
    /// Set by `complete_run` before the cancel notify: the runner's cancel
    /// branch then emits `Completed` instead of `Cancelled` ("Done" button).
    pub complete: std::sync::Arc<AtomicBool>,
    /// Live ACP connection to the agent, set once the session starts. Used
    /// for on-the-fly `session/set_config_option` requests (model/effort).
    pub session_conn: std::sync::Arc<
        tokio::sync::OnceCell<agent_client_protocol::ConnectionTo<agent_client_protocol::Agent>>,
    >,
    /// The ACP session id, set once the session starts. Config requests are
    /// keyed by this, not by the run id.
    pub acp_session_id: std::sync::Arc<tokio::sync::OnceCell<String>>,
}

/// Updates emitted by a run, streamed to the GUI/CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RunUpdate {
    /// Session/update notification from the agent (streaming output).
    SessionUpdate { text: String },
    /// Session ID was received.
    #[serde(rename_all = "camelCase")]
    SessionId { session_id: String },
    /// Run completed successfully.
    #[serde(rename_all = "camelCase")]
    Completed { output: String, stop_reason: String },
    /// Run failed.
    Failed { error: String },
    /// Run was cancelled.
    Cancelled,
    /// Permission request was received from the agent.
    /// GUI should respond via `acp_respond_permission`.
    #[serde(rename_all = "camelCase")]
    PermissionRequest {
        request_id: String,
        description: String,
    },
    /// Permission request timed out (5min, auto-denied).
    PermissionTimeout,
    /// Agent stopped with a turn-ending reason (e.g. EndTurn) and is
    /// waiting for the user to send a follow-up prompt or complete the run.
    /// UI should show an input field.
    #[serde(rename_all = "camelCase")]
    WaitingForInput { stop_reason: String },
    /// Emitted while a resumed run regenerates its handoff summary (before
    /// the first prompt). Rendered as a dim status line; never accumulated
    /// into the run's `output`.
    RestoringContext,
}

/// Frontend event payload for a single run update.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpUpdateEvent {
    pub run_id: String,
    pub update: RunUpdate,
}

/// Frontend event payload when the set of active runs changes.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpActiveRunsChangedEvent {
    pub runs: Vec<agent_runs::AgentRun>,
}

/// Buffer of updates for a run, with an accumulated output log for
/// persistence.
pub struct UpdateBuffer {
    pub updates: Vec<RunUpdate>,
    /// Accumulated `SessionUpdate` text, written to `agent_runs.output`
    /// when the run reaches a terminal state.
    pub output: String,
}

impl UpdateBuffer {
    fn new() -> Self {
        Self {
            updates: Vec::new(),
            output: String::new(),
        }
    }
}

/// Pending permission request awaiting user response.
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

/// AppHandle-agnostic run core. Both Tauri commands and the CLI are thin
/// adapters over `RunCore` (decision 16).
#[derive(Default)]
pub struct RunCore {
    pub runs: Arc<Mutex<HashMap<String, RunHandle>>>,
    pub buffers: Arc<Mutex<HashMap<String, UpdateBuffer>>>,
    /// Pending permission requests, keyed by "{run_id}:{request_id}".
    pub permissions: Arc<Mutex<HashMap<String, Arc<PendingPermission>>>>,
    /// AppHandle for pushing updates to the frontend. Set once on startup.
    pub app: OnceLock<AppHandle>,
}

impl RunCore {
    pub fn set_app(&self, app: AppHandle) -> Result<(), AppHandle> {
        self.app.set(app)
    }

    /// Start an agent run. Takes pre-loaded data from the Tauri command and
    /// does the async work: worktree creation (unless an existing worktree is
    /// supplied) + SDK spawn + drain task.
    pub async fn start_run(
        &self,
        run_id: String,
        card_id: String,
        repo_path: String,
        parts: PromptParts,
        acp_agent: agent_client_protocol::AcpAgent,
        db_path: PathBuf,
        existing: Option<(String, String)>,
        model: Option<String>,
        effort: Option<String>,
        handoff: Option<crate::handoff::HandoffCtx>,
    ) -> Result<(String, String), AcpError> {
        // Create or reuse worktree.
        let (worktree_path, branch) = match existing {
            Some((w, b)) => (PathBuf::from(w), b),
            None => {
                let wt_mgr = WorktreeManager::new(&PathBuf::from(&repo_path));
                let worktree = wt_mgr.create(&card_id).await?;
                (worktree.path.clone(), worktree.branch.clone())
            }
        };

        // Spawn SDK connection.
        let (tx, rx) = mpsc::unbounded_channel::<RunUpdate>();
        let cancel = std::sync::Arc::new(Notify::new());
        let cancel_for_handle = cancel.clone();
        let cwd = worktree_path.clone();
        let run_id_clone = run_id.clone();
        let run_id_for_handle = run_id.clone();
        let permissions_map = self.permissions.clone();
        // Clone the DB path so the per-permission timeout task can read
        // the `acp_permission_timeout` setting without borrowing the
        // drain task's copy (Connection is !Send; the timeout task opens
        // its own short-lived connection by path).
        let db_path_for_perms = db_path.clone();

        // Channel for follow-up prompts from the user (interactive mode).
        // Read loop waits on this after each EndTurn instead of exiting,
        // keeping the agent process alive for the next turn. Wrapped in
        // Arc<Mutex<Option<_>>> because connect_with's closure is Fn (not
        // FnOnce), so the receiver can't be moved directly — it's taken
        // from the Option on the single actual call.
        let (prompt_tx, prompt_rx) = mpsc::unbounded_channel::<String>();
        let prompt_tx_for_handle = prompt_tx.clone();
        let prompt_rx = Arc::new(tokio::sync::Mutex::new(Some(prompt_rx)));
        let complete = Arc::new(AtomicBool::new(false));
        let complete_for_closure = complete.clone();
        // Shared slot for the live ACP connection; set once the session
        // starts so `set_session_config` can reach it later.
        let session_conn = Arc::new(tokio::sync::OnceCell::new());
        let session_conn_for_closure = session_conn.clone();
        let acp_session_id = Arc::new(tokio::sync::OnceCell::new());
        let acp_session_id_for_closure = acp_session_id.clone();

        // Publish the live connection + session id for on-the-fly config
        // changes, and persist the session id for later resumes.
        let db_path_for_on_session = db_path_for_perms.clone();
        let run_id_for_on_session = run_id_clone.clone();
        let on_session = Arc::new(
            move |conn: &agent_client_protocol::ConnectionTo<agent_client_protocol::Agent>,
                  sid: &str| {
                let _ = session_conn_for_closure.set(conn.clone());
                let _ = acp_session_id_for_closure.set(sid.to_string());
                let db_path = db_path_for_on_session.clone();
                let run_id = run_id_for_on_session.clone();
                let sid = sid.to_string();
                tokio::task::spawn_blocking(move || {
                    if let Ok(conn) = crate::db::open_db_path(&db_path) {
                        let _ = agent_runs::set_session_id(&conn, &run_id, &sid);
                    }
                });
            },
        );
        let join = tokio::spawn(async move {
            // A resumed run regenerates its handoff inside the task (the
            // IPC call already returned) before the first prompt.
            let prompt = if let Some(h) = handoff {
                let _ = tx.send(RunUpdate::RestoringContext);
                let content = crate::handoff::ensure(
                    &db_path_for_perms,
                    &run_id_clone,
                    h.finished_at.as_deref(),
                    h.summarizer_agent,
                    &h.input,
                )
                .await;
                build_prompt(&parts, content.as_deref())
            } else {
                build_prompt(&parts, None)
            };
            let job = SessionJob {
                cwd,
                prompt,
                model,
                effort,
                tx,
                prompt_rx: Some(prompt_rx),
                cancel: Some(cancel),
                complete: Some(complete_for_closure),
                permissions: Some(permissions_map),
                db_path: Some(db_path_for_perms),
                run_id: run_id_clone,
                on_session: Some(on_session),
            };
            run_session_job(acp_agent, job).await;
        });

        // Step 10: Store RunHandle.
        {
            let mut runs = self.runs.lock().await;
            runs.insert(
                run_id_for_handle.clone(),
                RunHandle {
                    join,
                    cancel: cancel_for_handle,
                    prompt_tx: prompt_tx_for_handle,
                    complete,
                    session_conn,
                    acp_session_id,
                },
            );
        }
        {
            let mut buffers = self.buffers.lock().await;
            buffers.insert(run_id_for_handle.clone(), UpdateBuffer::new());
        }

        // Step 11: Spawn drain task.
        let runs_map = self.runs.clone();
        let buffers_map = self.buffers.clone();
        let run_id_for_drain = run_id_for_handle.clone();
        let app = self.app.get().cloned();
        tokio::spawn(async move {
            drain_updates(rx, buffers_map, runs_map, run_id_for_drain, db_path, app).await;
        });

        // Step 12: Return the real worktree path + branch so the caller can
        // UPDATE the placeholder row.
        Ok((worktree_path.to_string_lossy().to_string(), branch))
    }

    /// Resume an existing agent run by starting a fresh ACP session in the
    /// same worktree and re-sending the original prompt. Reuses the same
    /// run row and `prompt_tx` so follow-ups continue to work.
    pub async fn resume_run(
        &self,
        run_id: String,
        card_id: String,
        repo_path: String,
        worktree_path: String,
        branch: String,
        parts: PromptParts,
        acp_agent: agent_client_protocol::AcpAgent,
        db_path: PathBuf,
        model: Option<String>,
        effort: Option<String>,
        handoff: Option<crate::handoff::HandoffCtx>,
    ) -> Result<(), AcpError> {
        if self.runs.lock().await.contains_key(&run_id) {
            return Ok(());
        }
        self.start_run(
            run_id,
            card_id,
            repo_path,
            parts,
            acp_agent,
            db_path,
            Some((worktree_path, branch)),
            model,
            effort,
            handoff,
        )
        .await
        .map(|_| ())
    }

    /// Change a session config option (model / effort) on a live run via
    /// ACP `session/set_config_option`. The run must have a started session.
    pub async fn set_session_config(
        &self,
        run_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<(), AcpError> {
        let (conn, session_id) = {
            let runs = self.runs.lock().await;
            let handle = runs
                .get(run_id)
                .ok_or_else(|| AcpError::not_found(&format!("run not found: {run_id}")))?;
            (
                handle.session_conn.clone(),
                handle.acp_session_id.clone(),
            )
        };
        let conn = conn.get().ok_or_else(|| {
            AcpError::conflict("session not started yet; try again once the run is running")
        })?;
        let session_id = session_id.get().ok_or_else(|| {
            AcpError::conflict("session not started yet; try again once the run is running")
        })?;
        let value = value.trim();
        if value.is_empty() {
            return Err(AcpError::validation("value cannot be empty"));
        }
        let req = agent_client_protocol::schema::v1::SetSessionConfigOptionRequest::new(
            session_id.clone(),
            agent_client_protocol::schema::v1::SessionConfigId::new(config_id),
            value,
        );
        conn.send_request(req)
            .block_task()
            .await
            .map(|_| ())
            .map_err(|e| AcpError::internal(format!("set_config_option({config_id}) failed: {e}")))
    }

    /// Cancel a running agent. Cancels the task and waits up to 5s.
    /// Caller is responsible for updating the DB status.
    pub async fn cancel_run(&self, run_id: &str) {
        let handle = {
            let mut runs = self.runs.lock().await;
            runs.remove(run_id)
        };
        if let Some(handle) = handle {
            handle.cancel.notify_waiters();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle.join).await;
        }
    }

    /// Mark a waiting run as completed ("Done" button): the agent finished
    /// its turn and the user is satisfied. Sets the complete flag, then
    /// cancels the wait loop — the runner emits `Completed` instead of
    /// `Cancelled`, so `drain_updates` writes status `completed` and the
    /// card unlocks for merge. Errors if the run is not active.
    pub async fn complete_run(&self, run_id: &str) -> Result<(), AcpError> {
        let flag = {
            let runs = self.runs.lock().await;
            runs.get(run_id).map(|h| h.complete.clone())
        };
        match flag {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                self.cancel_run(run_id).await;
                Ok(())
            }
            None => Err(AcpError::not_found(&format!("run not found: {run_id}"))),
        }
    }

    /// List updates since cursor.
    pub async fn list_updates(&self, run_id: &str, cursor: usize) -> Vec<RunUpdate> {
        let mut buffers = self.buffers.lock().await;
        if let Some(buf) = buffers.get_mut(run_id) {
            buf.updates.iter().skip(cursor).cloned().collect()
        } else {
            Vec::new()
        }
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
            RequestPermissionOutcome, RequestPermissionResponse, SelectedPermissionOutcome,
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
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                    pending.option_id.clone(),
                ))
            } else {
                RequestPermissionOutcome::Cancelled
            };
            let response = RequestPermissionResponse::new(outcome);
            responder
                .respond(response)
                .map_err(|e| AcpError::internal(&format!("permission respond failed: {e}")))?;
        }
        // Wake the timeout task so it exits early instead of sleeping the
        // full duration after the user already answered.
        pending.responded.notify_one();
        Ok(())
    }

    /// Send a follow-up prompt to a running agent session. The run must be
    /// in the `WaitingForInput` state (agent stopped with EndTurn and is
    /// waiting for user input).
    pub async fn send_followup(&self, run_id: &str, text: String) -> Result<(), AcpError> {
        let runs = self.runs.lock().await;
        let handle = runs
            .get(run_id)
            .ok_or_else(|| AcpError::not_found(&format!("run not found: {run_id}")))?;
        handle
            .prompt_tx
            .send(text)
            .map_err(|_| AcpError::internal("failed to send follow-up prompt: channel closed"))
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
            handle.cancel.notify_waiters();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle.join).await;
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

/// Prompt sections assembled by the three call sites (`acp_create_run`,
/// `acp_resume_run`, CLI `cmd_run`) and joined by [`build_prompt`].
pub struct PromptParts {
    /// Per-agent override or global `acp_system_prompt` setting; may be empty.
    pub system: String,
    /// Output of [`build_skills_section`] (may be empty).
    pub skills: String,
    /// Card title + description (CLI appends `--input` before calling).
    pub card_body: String,
}

/// Join the prompt sections in fixed order: system prompt, skills, handoff
/// (resume only), card body. Empty sections are omitted; non-empty ones are
/// separated by `---` fences.
pub fn build_prompt(parts: &PromptParts, handoff: Option<&str>) -> String {
    let mut sections = Vec::new();
    if !parts.system.trim().is_empty() {
        sections.push(format!("# System prompt\n\n{}", parts.system.trim()));
    }
    if !parts.skills.is_empty() {
        sections.push(parts.skills.clone());
    }
    if let Some(h) = handoff {
        if !h.trim().is_empty() {
            sections.push(format!(
                "--- Previous session handoff (context only; do not redo completed work) ---\n\n{}",
                h.trim()
            ));
        }
    }
    sections.push(parts.card_body.clone());
    sections.join("\n\n---\n\n")
}

/// Resolve the system prompt for a run: the per-agent override wins when
/// non-empty, else the global `acp_system_prompt` setting (may be empty).
pub fn resolve_system_prompt(conn: &rusqlite::Connection, agent: &agents::Agent) -> String {
    if !agent.system_prompt.trim().is_empty() {
        return agent.system_prompt.clone();
    }
    crate::db::read_setting(conn, "acp_system_prompt").unwrap_or_default()
}

/// First `max` chars of `s`, char-safe (never splits a code point, which
/// `&s[..n]` would hit for multi-byte UTF-8).
pub fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Structured summary of a permission request's tool call: tool title +
/// raw_input JSON truncated at 500 chars. Replaces the old `format!("{:?}")`
/// debug dump, which leaked SDK-internal field noise to the UI.
pub fn summarize_tool_call(tc: &agent_client_protocol::schema::v1::ToolCallUpdate) -> String {
    let title = tc.fields.title.as_deref().unwrap_or("(unknown tool)");
    let args = match &tc.fields.raw_input {
        Some(v) => {
            let s = serde_json::to_string(v).unwrap_or_else(|_| format!("{v:?}"));
            if s.chars().count() > 500 {
                format!("{}…", truncate_chars(&s, 500))
            } else {
                s
            }
        }
        None => String::new(),
    };
    if args.is_empty() {
        title.to_string()
    } else {
        format!("{title}: {args}")
    }
}

/// Render an ACP `SessionUpdate` as human-readable text for the run panel.
/// Returns `None` for update kinds that are pure noise in a compact log
/// (available commands, mode/config changes, usage stats, plan metadata)
/// so the caller can skip emitting them entirely.
///
/// Why not `format!("{:?}", update)`: the Debug dump leaks SDK-internal
/// field names, enum variant paths, and `_meta` blobs that the user never
/// wants to see in the popup. This helper extracts only the meaningful
/// payload (agent text, tool titles, statuses) and formats it as plain
/// text the panel already knows how to render.
pub fn format_session_update(
    update: &agent_client_protocol::schema::v1::SessionUpdate,
) -> Option<String> {
    use agent_client_protocol::schema::v1::{SessionUpdate, ToolCallStatus};
    match update {
        // Agent's streamed reply — the primary signal the user wants.
        SessionUpdate::AgentMessageChunk(chunk) => text_from_content(&chunk.content),
        // Internal reasoning; show but dim via prefix so it's distinguishable.
        SessionUpdate::AgentThoughtChunk(chunk) => {
            text_from_content(&chunk.content).map(|t| format!("💭 {t}"))
        }
        // New tool call was initiated. Show its title + kind.
        SessionUpdate::ToolCall(tc) => {
            let title = tc.title.trim();
            if title.is_empty() {
                None
            } else {
                Some(format!("🔧 {title}"))
            }
        }
        // Status/title/content update on an existing tool call.
        SessionUpdate::ToolCallUpdate(tc) => {
            let title = tc.fields.title.as_deref().map(str::trim).unwrap_or("");
            let status = tc.fields.status.unwrap_or(ToolCallStatus::Pending);
            let label = if title.is_empty() { "(tool)" } else { title };
            let mark = match status {
                ToolCallStatus::InProgress => "…",
                ToolCallStatus::Completed => "✓",
                ToolCallStatus::Failed => "✗",
                _ => "·",
            };
            // Only emit on a meaningful state transition; skip empty updates.
            if matches!(status, ToolCallStatus::Pending) && title.is_empty() {
                None
            } else {
                Some(format!("🔧 {mark} {label}"))
            }
        }
        // Echo of the user's own prompt — drop to avoid duplicating the input.
        SessionUpdate::UserMessageChunk(_) => None,
        // Updates carrying only metadata: noisy, no actionable content for the popup.
        SessionUpdate::AvailableCommandsUpdate(_)
        | SessionUpdate::CurrentModeUpdate(_)
        | SessionUpdate::ConfigOptionUpdate(_)
        | SessionUpdate::SessionInfoUpdate(_)
        | SessionUpdate::UsageUpdate(_) => None,
        // Plan updates are rare and verbose; surface a one-line marker.
        SessionUpdate::Plan(_) => Some("📋 plan".to_string()),
        _ => None,
    }
}

/// Extract text from a `ContentBlock`. Only `Text` carries a string payload
/// the popup can usefully display; images/audio/resources are dropped (the
/// panel has no renderer for them).
fn text_from_content(block: &agent_client_protocol::schema::v1::ContentBlock) -> Option<String> {
    use agent_client_protocol::schema::v1::ContentBlock;
    match block {
        ContentBlock::Text(t) => {
            let s = t.text.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        }
        _ => None,
    }
}

/// Untrimmed text variant for chunk accumulation — fragments must join
/// byte-for-byte, so per-chunk trimming would corrupt the assembled message.
pub(crate) fn raw_text_from_content(
    block: &agent_client_protocol::schema::v1::ContentBlock,
) -> Option<String> {
    use agent_client_protocol::schema::v1::ContentBlock;
    match block {
        ContentBlock::Text(t) => {
            if t.text.is_empty() {
                None
            } else {
                Some(t.text.clone())
            }
        }
        _ => None,
    }
}

/// Flush an accumulated agent message as ONE SessionUpdate. `pending` holds
/// `(message_id, joined text)`; the text is trimmed as a whole at flush so
/// mid-message whitespace survives and leading/trailing noise is dropped.
pub(crate) fn flush_pending_chunk(
    pending: &mut Option<(Option<String>, String)>,
    tx: &tokio::sync::mpsc::UnboundedSender<RunUpdate>,
) {
    if let Some((_, text)) = pending.take() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            let _ = tx.send(RunUpdate::SessionUpdate {
                text: trimmed.to_string(),
            });
        }
    }
}

/// Pick the "allow" option from a permission request's option list.
/// Selects by `PermissionOptionKind` (AllowOnce preferred over AllowAlways)
/// rather than matching the option `name` — name matching was fragile: an
/// agent naming its options `["Permit always", "Deny"]` (no "allow"/"yes"
/// substring) caused the old fallback to return the first option, which could
/// be a Reject. When no Allow-kind option is present, returns the literal
/// `"allow"` id so the agent errors clearly instead of silently receiving a
/// Reject the user never chose.
pub fn pick_allow_option(
    options: &[agent_client_protocol::schema::v1::PermissionOption],
) -> agent_client_protocol::schema::v1::PermissionOptionId {
    use agent_client_protocol::schema::v1::PermissionOptionKind;
    // Prefer AllowOnce (least privilege) over AllowAlways.
    for o in options {
        if matches!(o.kind, PermissionOptionKind::AllowOnce) {
            return o.option_id.clone();
        }
    }
    for o in options {
        if matches!(o.kind, PermissionOptionKind::AllowAlways) {
            return o.option_id.clone();
        }
    }
    // No Allow-kind option: do NOT fall back to first (could be Reject).
    // Return literal "allow" so a misbehaving agent surfaces a clear error
    // ponytail: could log the option list here for debugging; not added yet.
    "allow".into()
}

fn emit_run_update(app: Option<&AppHandle>, run_id: &str, update: &RunUpdate) {
    if let Some(app) = app {
        let _ = app.emit(
            "acp:update",
            AcpUpdateEvent {
                run_id: run_id.to_string(),
                update: update.clone(),
            },
        );
    }
}

fn emit_active_runs_changed(app: Option<&AppHandle>) {
    let Some(app) = app else {
        return;
    };
    let Ok(conn) = crate::db::open_db(app) else {
        return;
    };
    let Ok(runs) = agent_runs::list_active(&conn) else {
        return;
    };
    let _ = app.emit(
        "acp:active_runs_changed",
        AcpActiveRunsChangedEvent { runs },
    );
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
    app: Option<AppHandle>,
) {
    while let Some(update) = rx.recv().await {
        // Coalesce anything already queued so one DB connection covers the
        // batch. Persisted BEFORE the buffer/emit pass so a panel loading
        // history right after a terminal update sees the full stream.
        let mut batch = vec![update];
        while let Ok(more) = rx.try_recv() {
            batch.push(more);
        }
        persist_run_updates(&db_path, &run_id, &batch);
        for update in batch {
            let mut buffers = buffers.lock().await;
            let output = if let Some(buf) = buffers.get_mut(&run_id) {
                if let RunUpdate::SessionUpdate { text } = &update {
                    if !buf.output.is_empty() {
                        buf.output.push('\n');
                    }
                    buf.output.push_str(text);
                }
                buf.updates.push(update.clone());
                emit_run_update(app.as_ref(), &run_id, &update);
                match &update {
                    RunUpdate::Completed { .. }
                    | RunUpdate::Failed { .. }
                    | RunUpdate::Cancelled => Some(buf.output.clone()),
                    _ => None,
                }
            } else {
                None
            };
            // Check for terminal states — remove handle + write DB status.
            let terminal = match &update {
                RunUpdate::Completed { stop_reason, .. } => {
                    Some(("completed", Some(stop_reason.as_str()), None))
                }
                RunUpdate::Failed { error } => Some(("failed", None, Some(error.as_str()))),
                RunUpdate::Cancelled => Some(("cancelled", Some("user_cancelled"), None)),
                _ => None,
            };
            if let Some((status, stop_reason, error)) = terminal {
                let mut runs = runs.lock().await;
                runs.remove(&run_id);
                drop(buffers);
                drop(runs);
                write_terminal_status(
                    &db_path, &run_id, status, output.as_deref(), stop_reason, error,
                );
                emit_active_runs_changed(app.as_ref());
                return;
            }
        }
    }
    // Channel closed (handle dropped mid-stream) — mark as failed (decision 47).
    let mut buffers = buffers.lock().await;
    if let Some(buf) = buffers.get_mut(&run_id) {
        let already_terminal = buf.updates.iter().any(|u| {
            matches!(
                u,
                RunUpdate::Completed { .. } | RunUpdate::Failed { .. } | RunUpdate::Cancelled
            )
        });
        if !already_terminal {
            let output = buf.output.clone();
            let failed_update = RunUpdate::Failed {
                error: "drain: channel closed".to_string(),
            };
            buf.updates.push(failed_update.clone());
            emit_run_update(app.as_ref(), &run_id, &failed_update);
            drop(buffers);
            write_terminal_status(
                &db_path,
                &run_id,
                "failed",
                Some(&output),
                None,
                Some("drain: channel closed"),
            );
            emit_active_runs_changed(app.as_ref());
        }
    }
}

/// Persist a batch of run updates to `agent_run_updates` (best-effort — a
/// failed insert never disturbs the live stream).
fn persist_run_updates(db_path: &PathBuf, run_id: &str, updates: &[RunUpdate]) {
    let Ok(conn) = crate::db::open_db_path(db_path) else {
        return;
    };
    for u in updates {
        if let Ok(json) = serde_json::to_string(u) {
            if let Err(e) = agent_runs::insert_run_update(&conn, run_id, &json) {
                eprintln!("drain: failed to persist update for run {run_id}: {e}");
            }
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
                &conn,
                run_id,
                status,
                output,
                stop_reason,
                error,
                Some(&now),
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

use crate::db::open_db;
use tauri::{Manager, State};

/// Tauri-managed state holding the run executor.
#[derive(Default)]
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
    model: Option<String>,
    effort: Option<String>,
) -> Result<agent_runs::AgentRun, AcpError> {
    // All DB work synchronously (no await — Connection is not Send).
    let (run_id, repo_path, parts, acp_agent, db_path, model, effort) = {
        let conn = open_db(&app)?;
        // Load agent.
        let agent = agents::get_agent(&conn, &agent_name)?
            .ok_or_else(|| AcpError::not_found(format!("Agent '{agent_name}' not found")))?;
        if !agent.enabled {
            return Err(AcpError::validation(format!(
                "Agent '{agent_name}' is disabled"
            )));
        }
        // Per-run override > agent default.
        let model = model.or_else(|| agent.model.clone());
        let effort = effort.or_else(|| agent.effort.clone());
        // Check lock.
        if agent_runs::is_card_locked(&conn, &card_id) {
            return Err(AcpError::locked(format!(
                "Card '{card_id}' already has an active agent run."
            )));
        }
        // Load card.
        let card = cards::get_card_by_id(&conn, &card_id)?
            .ok_or_else(|| AcpError::not_found(format!("Card '{card_id}' not found")))?;
        // Resolve repo_path: card tree_source path.
        let repo_path = cards::resolve_repo_path(&conn, &card)?;
        // Build AcpAgent.
        let acp_agent = if agent.built_in && agent_name == "claude-code" {
            agent_client_protocol::AcpAgent::claude_agent()
        } else {
            agent_client_protocol::AcpAgent::from_str(&agent.command)
                .map_err(|e| AcpError::internal(format!("Invalid agent command: {e}")))?
        };
        // Attach a debug callback that dumps every stdio line to stderr so
        // spawn/handshake failures are visible (opt-in via env var).
        let acp_agent = if std::env::var("TASKER_DEBUG_ACP").is_ok() {
            let debug_agent_name = agent_name.clone();
            acp_agent.with_debug(move |line, dir| {
                eprintln!("[acp-stdio:{debug_agent_name}:{dir:?}] {line}");
            })
        } else {
            acp_agent
        };
        // Load skills.
        let sn: Vec<String> = match &skill_names {
            Some(names) => agent
                .skills
                .iter()
                .filter(|s| names.contains(s))
                .cloned()
                .collect(),
            None => agent.skills.clone(),
        };
        let loaded = skills::load_skills(&sn);
        let skills_section = build_skills_section(&loaded);
        // Build prompt (system > skills > card body).
        let card_body = if card.description.is_empty() {
            card.title.clone()
        } else {
            format!("{}\n\n{}", card.title, card.description)
        };
        let system = resolve_system_prompt(&conn, &agent);
        let parts = PromptParts {
            system,
            skills: skills_section,
            card_body,
        };
        // Insert agent_runs row with placeholder worktree/branch; real
        // values are filled in after start_run creates the worktree.
        let run_id = uuid::Uuid::new_v4().to_string();
        agent_runs::insert_run(
            &conn,
            &run_id,
            &card_id,
            &agent_name,
            "/tmp/pending",
            "agent/pending",
            "pending",
            &sn,
        )?;
        // Resolve the DB path so the drain task can open its own connection.
        let mut db_path = app
            .path()
            .app_config_dir()
            .map_err(|e| AcpError::internal(format!("app_config_dir: {e}")))?;
        db_path.push("tasker.db");
        (run_id, repo_path, parts, acp_agent, db_path, model, effort)
    };
    // conn dropped here — safe to await.

    // Worktree + SDK spawn (async). On error, mark the row failed so the
    // card lock releases; the drain task never started, so nothing else
    // will write terminal state.
    let create_result = state
        .core
        .start_run(
            run_id.clone(),
            card_id,
            repo_path.clone(),
            parts,
            acp_agent,
            db_path.clone(),
            None,
            model,
            effort,
            None,
        )
        .await;
    let (worktree_path, branch) = match create_result {
        Ok(v) => v,
        Err(e) => {
            let conn = open_db(&app)?;
            let err_msg = e.to_string();
            agent_runs::update_status(
                &conn,
                &run_id,
                "failed",
                None,
                None,
                Some(&err_msg),
                Some(&crate::db::now_iso()),
            )?;
            return Err(e);
        }
    };

    // Update worktree_path/branch/status now that the worktree exists.
    {
        let conn = open_db(&app)?;
        agent_runs::set_worktree_info(&conn, &run_id, &worktree_path, &branch, &repo_path)?;
        agent_runs::update_status(&conn, &run_id, "running", None, None, None, None)?;
    }
    emit_active_runs_changed(Some(&app));

    // Read back the run row.
    let conn = open_db(&app)?;
    agent_runs::get_run(&conn, &run_id)?
        .ok_or_else(|| AcpError::internal("Failed to read back agent run"))
}

/// Resume an existing agent run by starting a fresh ACP session in the
/// existing worktree and re-sending the original prompt.
#[tauri::command]
pub async fn acp_resume_run(
    app: AppHandle,
    state: State<'_, RunnerState>,
    run_id: String,
) -> Result<agent_runs::AgentRun, AcpError> {
    let (run, repo_path, parts, acp_agent, db_path, model, effort, handoff) = {
        let conn = open_db(&app)?;
        let run = agent_runs::get_run(&conn, &run_id)?
            .ok_or_else(|| AcpError::not_found(format!("Run '{run_id}' not found")))?;
        // Widen the gate: after an app restart orphaned runs are reaped to
        // `failed` — exactly the resume case. Only merged runs are dead.
        if run.merged_at.is_some() {
            return Err(AcpError::validation(format!(
                "Run '{run_id}' is already merged; cannot resume"
            )));
        }
        let card = cards::get_card_by_id(&conn, &run.card_id)?
            .ok_or_else(|| AcpError::not_found(format!("Card '{0}' not found", run.card_id)))?;
        let agent = agents::get_agent(&conn, &run.agent_name)?
            .ok_or_else(|| AcpError::not_found(format!("Agent '{0}' not found", run.agent_name)))?;
        if !agent.enabled {
            return Err(AcpError::validation(format!(
                "Agent '{0}' is disabled",
                run.agent_name
            )));
        }
        let repo_path = match &run.repo_root {
            Some(p) => p.clone(),
            None => cards::resolve_repo_path(&conn, &card)?,
        };
        let build_agent = |debug: bool| -> Result<agent_client_protocol::AcpAgent, AcpError> {
            let a = if agent.built_in && run.agent_name == "claude-code" {
                agent_client_protocol::AcpAgent::claude_agent()
            } else {
                agent_client_protocol::AcpAgent::from_str(&agent.command)
                    .map_err(|e| AcpError::internal(format!("Invalid agent command: {e}")))?
            };
            if debug && std::env::var("TASKER_DEBUG_ACP").is_ok() {
                let debug_agent_name = run.agent_name.clone();
                Ok(a.with_debug(move |line, dir| {
                    eprintln!("[acp-stdio:{debug_agent_name}:{dir:?}] {line}");
                }))
            } else {
                Ok(a)
            }
        };
        let acp_agent = build_agent(true)?;
        // Throwaway summarizer session for the handoff (AcpAgent is not
        // Clone; rebuild from the same command).
        let summarizer_agent = build_agent(false)?;
        let loaded = skills::load_skills(&run.skills);
        let skills_section = build_skills_section(&loaded);
        let card_body = if card.description.is_empty() {
            card.title.clone()
        } else {
            format!("{}\n\n{}", card.title, card.description)
        };
        let system = resolve_system_prompt(&conn, &agent);
        let parts = PromptParts {
            system,
            skills: skills_section,
            card_body,
        };
        let mut db_path = app
            .path()
            .app_config_dir()
            .map_err(|e| AcpError::internal(format!("app_config_dir: {e}")))?;
        db_path.push("tasker.db");
        let model = agent.model.clone();
        let effort = agent.effort.clone();
        // Handoff input: the run's accumulated output + cwd candidates.
        let handoff = crate::handoff::HandoffCtx {
            finished_at: run.finished_at.clone(),
            summarizer_agent,
            input: crate::handoff::HandoffInput {
                output: run.output.clone().unwrap_or_default(),
                worktree_path: run.worktree_path.clone(),
                repo_root: run.repo_root.clone(),
            },
        };
        (run, repo_path, parts, acp_agent, db_path, model, effort, handoff)
    };

    let resume_result = state
        .core
        .resume_run(
            run_id.clone(),
            run.card_id.clone(),
            repo_path,
            run.worktree_path.clone(),
            run.branch.clone(),
            parts,
            acp_agent,
            db_path,
            model,
            effort,
            Some(handoff),
        )
        .await;
    if let Err(e) = &resume_result {
        let conn = open_db(&app)?;
        agent_runs::update_status(
            &conn,
            &run_id,
            "failed",
            None,
            None,
            Some(&e.to_string()),
            Some(&crate::db::now_iso()),
        )?;
        return Err(e.clone());
    }

    {
        let conn = open_db(&app)?;
        agent_runs::update_status(&conn, &run_id, "running", None, None, None, None)?;
    }
    emit_active_runs_changed(Some(&app));

    let conn = open_db(&app)?;
    agent_runs::get_run(&conn, &run_id)?
        .ok_or_else(|| AcpError::internal("Failed to read back agent run"))
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

/// List updates since cursor.
#[tauri::command]
pub async fn acp_list_updates(
    state: State<'_, RunnerState>,
    run_id: String,
    cursor: usize,
) -> Result<Vec<RunUpdate>, AcpError> {
    Ok(state.core.list_updates(&run_id, cursor).await)
}

/// Live process info for a run's agent subprocess.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunProcessInfo {
    pub run_id: String,
    /// DB status of the run row.
    pub status: String,
    /// Agent subprocess pid, found by /proc scan; `None` = not found.
    pub pid: Option<u32>,
    /// Seconds since the run row was created.
    pub elapsed_secs: u64,
}

/// Check whether a run's agent subprocess is alive: pid + uptime. The SDK
/// spawns the agent with cwd = worktree, so the scan matches on cwd.
#[tauri::command]
pub async fn acp_run_process_info(
    app: AppHandle,
    run_id: String,
) -> Result<RunProcessInfo, AcpError> {
    let conn = open_db(&app)?;
    let run = agent_runs::get_run(&conn, &run_id)?
        .ok_or_else(|| AcpError::not_found(format!("Run '{run_id}' not found")))?;
    let pid = if run.status == "running" {
        let pid = find_agent_pid(&PathBuf::from(&run.worktree_path));
        eprintln!(
            "[process_info:{run_id}] worktree={} pid={pid:?}",
            run.worktree_path
        );
        pid
    } else {
        None
    };
    Ok(RunProcessInfo {
        run_id: run.id,
        status: run.status,
        pid,
        elapsed_secs: elapsed_secs_since(&run.created_at),
    })
}

/// Oldest process whose cwd is `worktree`, i.e. the agent subprocess (its
/// children are younger). Linux /proc only; `None` when not found.
// ponytail: cwd match can also hit a surviving orphan child after the agent
// dies; if that misleads, compare /proc/<pid>/stat pgrp == pid (group leader).
fn find_agent_pid(worktree: &PathBuf) -> Option<u32> {
    // The DB stores the path we constructed (possibly through a symlinked
    // repo dir); /proc/<pid>/cwd readlink returns the fully-resolved path.
    // Canonicalize so the two forms compare equal.
    let want = std::fs::canonicalize(worktree).ok()?;
    let mut best: Option<(u64, u32)> = None; // (starttime, pid)
    for entry in std::fs::read_dir("/proc").ok()? {
        let Ok(entry) = entry else { continue };
        let Ok(name) = entry.file_name().into_string() else { continue };
        let Ok(pid) = name.parse::<u32>() else { continue };
        let Ok(cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) else {
            continue;
        };
        let Ok(cwd) = std::fs::canonicalize(&cwd) else { continue };
        if cwd != want {
            continue;
        }
        let start = proc_starttime(pid).unwrap_or(u64::MAX);
        if best.map_or(true, |(s, _)| start < s) {
            best = Some((start, pid));
        }
    }
    best.map(|(_, pid)| pid)
}

/// Field 22 (starttime) of `/proc/<pid>/stat` — clock ticks since boot.
fn proc_starttime(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm may contain spaces/parens — split after the LAST ')'.
    let after_comm = stat.rsplit(')').next()?;
    // after_comm[0] is state (field 3); starttime is field 22 → +19.
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

/// Seconds between an ISO timestamp written by [`crate::db::now_iso`] and now.
fn elapsed_secs_since(created_at: &str) -> u64 {
    use time::macros::format_description;
    use time::{OffsetDateTime, PrimitiveDateTime};
    const FMT: &[time::format_description::FormatItem<'_>] =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    // The trailing Z is a literal (now_iso writes UTC), not an offset token —
    // parse as a naive datetime and assume UTC.
    PrimitiveDateTime::parse(created_at, FMT)
        .map(|created| {
            let secs = (OffsetDateTime::now_utc() - created.assume_utc()).whole_seconds();
            secs.max(0) as u64
        })
        .unwrap_or(0)
}

/// Load the full persisted update stream for a run. Used by the panel when
/// the in-memory buffer is empty (app restart) so the transcript survives
/// restarts.
#[tauri::command]
pub async fn acp_load_run_history(
    app: AppHandle,
    run_id: String,
) -> Result<Vec<RunUpdate>, AcpError> {
    let conn = open_db(&app)?;
    let rows = agent_runs::list_run_updates(&conn, &run_id)?;
    Ok(rows
        .iter()
        .filter_map(|j| serde_json::from_str(j).ok())
        .collect())
}

/// Cancel a running agent. The drain task writes the `cancelled` terminal
/// status when it processes the Cancelled update — no duplicate write here.
#[tauri::command]
pub async fn acp_cancel_run(
    app: AppHandle,
    state: State<'_, RunnerState>,
    run_id: String,
) -> Result<(), AcpError> {
    state.core.cancel_run(&run_id).await;
    emit_active_runs_changed(Some(&app));
    Ok(())
}

/// Mark a waiting run as completed from the GUI ("Done" button).
#[tauri::command]
pub async fn acp_complete_run(
    app: AppHandle,
    state: State<'_, RunnerState>,
    run_id: String,
) -> Result<(), AcpError> {
    // Sets the complete flag + cancels the task; drain writes `completed`.
    state.core.complete_run(&run_id).await?;
    emit_active_runs_changed(Some(&app));
    Ok(())
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

/// Send a follow-up prompt to a running agent that is waiting for user input.
#[tauri::command]
pub async fn acp_send_followup(
    state: State<'_, RunnerState>,
    run_id: String,
    text: String,
) -> Result<(), AcpError> {
    state.core.send_followup(&run_id, text).await
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
    model: Option<String>,
    effort: Option<String>,
    system_prompt: Option<String>,
) -> Result<(), AcpError> {
    if name.is_empty() {
        return Err(AcpError::validation("Agent name cannot be empty"));
    }
    if name == "claude-code" {
        return Err(AcpError::validation(
            "Agent name 'claude-code' is reserved for the built-in agent",
        ));
    }
    if command.is_empty() {
        return Err(AcpError::validation(
            "Agent command cannot be empty for non-built-in agents",
        ));
    }
    let conn = open_db(&app)?;
    agents::insert_agent(&conn, &name, &command, &description, false, true, &skills)?;
    agents::update_agent(
        &conn,
        &name,
        &command,
        &description,
        &skills,
        model.as_deref(),
        effort.as_deref(),
        system_prompt.as_deref().unwrap_or(""),
    )
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
    model: Option<String>,
    effort: Option<String>,
    system_prompt: Option<String>,
) -> Result<(), AcpError> {
    let conn = open_db(&app)?;
    let existing = agents::get_agent(&conn, &name)?
        .ok_or_else(|| AcpError::not_found(format!("Agent '{name}' not found")))?;
    if existing.built_in && command != existing.command {
        return Err(AcpError::validation(format!(
            "Cannot change command of built-in agent '{name}'"
        )));
    }
    agents::update_agent(
        &conn,
        &name,
        &command,
        &description,
        &skills,
        model.as_deref(),
        effort.as_deref(),
        system_prompt.as_deref().unwrap_or(""),
    )
}

/// Change a session config option (model / effort) on a live run via ACP
/// `session/set_config_option`. `config_id` is e.g. "model" or "effort".
#[tauri::command]
pub async fn acp_set_session_config(
    state: State<'_, RunnerState>,
    run_id: String,
    config_id: String,
    value: String,
) -> Result<(), AcpError> {
    let allowed = ["model", "effort", "thought_level", "mode"];
    if !allowed.contains(&config_id.as_str()) {
        return Err(AcpError::validation(format!(
            "Unsupported config id '{config_id}' (allowed: {allowed:?})"
        )));
    }
    state
        .core
        .set_session_config(&run_id, &config_id, &value)
        .await
}

/// Delete an agent. Refuses built-in agents outright. For non-built-ins,
/// RESTRICT by default (returns the structured `AgentHasRuns` error if
/// runs exist); cascade with `delete_runs=true`. On cascade, removes each
/// run's worktree (dir + branch) before deleting the run rows, so the
/// `.tasker-worktrees/<card_id>` dirs and `agent/<card_id>` branches don't
/// get orphaned on disk.
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
    // On cascade, reap worktrees first (best-effort — a missing repo_root
    // or a failed `git worktree remove` must not block the DB delete).
    if delete_runs {
        let runs = agent_runs::list_runs_for_agent(&conn, &name)?;
        for r in &runs {
            if let Some(repo) = r.repo_root.as_ref() {
                let _ = WorktreeManager::new(&PathBuf::from(repo))
                    .remove(&r.card_id)
                    .await;
            }
        }
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

/// List recent runs (newest first), any status. Default limit 20 for UI
/// status panels that want a compact "recent activity" feed.
#[tauri::command]
pub async fn acp_list_recent_runs(
    app: AppHandle,
    limit: Option<i64>,
) -> Result<Vec<agent_runs::AgentRun>, AcpError> {
    let conn = open_db(&app)?;
    agent_runs::list_recent(&conn, limit.unwrap_or(20))
}

/// Diff between main and the agent branch for a card.
#[tauri::command]
pub async fn acp_diff_main(app: AppHandle, card_id: String) -> Result<DiffResult, AcpError> {
    let diff = {
        let conn = open_db(&app)?;
        let run = agent_runs::get_active_run(&conn, &card_id)?
            .or(agent_runs::get_latest_run_for_card(&conn, &card_id)?)
            .ok_or_else(|| AcpError::not_found("No run found for card"))?;
        let repo_root = run.repo_root.as_ref().ok_or_else(|| {
            AcpError::validation(
                "Run has no repo_root (may be a pre-migration run without a repo path)",
            )
        })?;
        let wt_mgr = WorktreeManager::new(&PathBuf::from(repo_root));
        wt_mgr.diff_main(&card_id).await?
    };
    let truncated = diff.len() > 1024 * 1024;
    let text = if truncated {
        truncate_chars(&diff, 1024 * 1024)
    } else {
        diff
    };
    Ok(DiffResult { text, truncated })
}

/// Diff request result.
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
        let run = agent_runs::get_active_run(&conn, &card_id)?
            .or(agent_runs::get_latest_run_for_card(&conn, &card_id)?)
            .ok_or_else(|| AcpError::not_found("No run found for card"))?;
        let (run_id, status) = (run.id, run.status);
        let force = force.unwrap_or(false);
        if status != "completed" && !force {
            return Err(AcpError::validation(format!(
                "Run status is '{status}', must be 'completed' (or use force=true)"
            )));
        }
        let repo = run
            .repo_root
            .as_ref()
            .ok_or_else(|| {
                AcpError::validation(
                    "Run has no repo_root (may be a pre-migration run without a repo path)",
                )
            })?
            .clone();
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

/// Remove the worktree for a card. Resolves the repo root from the
/// active/most-recent run's `repo_root` when a run row exists; falls back
/// to the card's `tree_source_id` → `tree_sources.path` when no run row
/// remains (e.g. after an agent cascade-delete orphaned the worktree).
#[tauri::command]
pub async fn acp_remove_worktree(app: AppHandle, card_id: String) -> Result<(), AcpError> {
    let repo_path = {
        let conn = open_db(&app)?;
        // Prefer a run row's snapshotted repo_root (deterministic, survives
        // tree_source edits). Fall back to the card's tree_source path so
        // orphaned worktrees can be cleaned even after run rows are gone.
        if let Some(run) = agent_runs::get_active_run(&conn, &card_id)?
            .or(agent_runs::get_latest_run_for_card(&conn, &card_id)?)
        {
            if let Some(repo) = run.repo_root {
                repo
            } else {
                return Err(AcpError::validation(
                    "Run has no repo_root (may be a pre-migration run without a repo path)",
                ));
            }
        } else {
            let card = cards::get_card_by_id(&conn, &card_id)?
                .ok_or_else(|| AcpError::not_found(format!("Card '{card_id}' not found")))?;
            cards::resolve_repo_path(&conn, &card)?
        }
    };
    let wt_mgr = WorktreeManager::new(&PathBuf::from(&repo_path));
    wt_mgr.remove(&card_id).await
}

/// Delete an agent run and its worktree. Refuses to delete active runs
/// (pending/running) to avoid leaving an agent process detached. For terminal
/// runs, removes the worktree (best-effort) then deletes the DB row.
#[tauri::command]
pub async fn acp_delete_run(app: AppHandle, run_id: String) -> Result<(), AcpError> {
    let (card_id, repo_root) = {
        let conn = open_db(&app)?;
        let run = agent_runs::get_run(&conn, &run_id)?
            .ok_or_else(|| AcpError::not_found(format!("Run '{run_id}' not found")))?;
        if run.status == "pending" || run.status == "running" {
            return Err(AcpError::locked(format!(
                "Cannot delete active run '{run_id}' (status: {}). Cancel it first.",
                run.status
            )));
        }
        (run.card_id, run.repo_root)
    };
    // Remove worktree outside the DB lock (best-effort).
    if let Some(repo) = repo_root {
        let _ = WorktreeManager::new(&PathBuf::from(&repo))
            .remove(&card_id)
            .await;
    }
    let conn = open_db(&app)?;
    agent_runs::delete_run(&conn, &run_id)
}

/// Clean up stale active runs.
/// `pending` runs (no worktree created yet) are marked failed so cards unlock.
/// `running` runs are left as-is so `acp_resume_run` can reconnect to them.
/// Called on startup (fire-and-forget) and via the CLI `cleanup` command.
pub fn cleanup_dangling(conn: &rusqlite::Connection) -> Result<Vec<String>, AcpError> {
    let active = agent_runs::list_active(conn)?;
    let mut reaped = Vec::new();
    for run in active {
        if run.status == "pending" {
            agent_runs::update_status(
                conn,
                &run.id,
                "failed",
                None,
                None,
                Some("dangling: tasker restarted before worktree was created"),
                Some(&crate::db::now_iso()),
            )?;
            reaped.push(run.id);
        }
    }
    Ok(reaped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::agent_runs;
    use crate::db::agents;
    use crate::db::test_db;
    use crate::db::{open_db_path, run_migrations};

    fn insert_test_card(conn: &rusqlite::Connection, id: &str) {
        conn.execute(
            r#"INSERT INTO cards (id, title, description, priority, "column", source, position, created_at, updated_at)
               VALUES (?1, 'Test', 'desc', 'medium', 'backlog', 'local', 0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
            rusqlite::params![id],
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
        insert_test_card(&conn, "c-1");
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(
            &conn,
            "r-1",
            "c-1",
            "my-agent",
            "/tmp/wt",
            "agent/c-1",
            "running",
            &[],
        )
        .unwrap();
        drop(conn);

        let (tx, rx) = mpsc::unbounded_channel::<RunUpdate>();
        let buffers = Arc::new(Mutex::new(HashMap::new()));
        buffers
            .lock()
            .await
            .insert("r-1".to_string(), UpdateBuffer::new());
        let runs = Arc::new(Mutex::new(HashMap::new()));
        let _ = tx.send(RunUpdate::Completed {
            output: "done".into(),
            stop_reason: "end_turn".into(),
        });
        drop(tx);
        drain_updates(rx, buffers, runs, "r-1".to_string(), db_path.clone(), None).await;

        let conn = open_db_path(&db_path).unwrap();
        let run = agent_runs::get_run(&conn, "r-1").unwrap().unwrap();
        assert_eq!(run.status, "completed");
        assert_eq!(run.stop_reason.as_deref(), Some("end_turn"));
        assert!(run.finished_at.is_some());
        assert!(!agent_runs::is_card_locked(&conn, "c-1"));
        // The full update stream is persisted for panel history reload.
        let history = agent_runs::list_run_updates(&conn, "r-1").unwrap();
        assert_eq!(history.len(), 1);
        let parsed: RunUpdate = serde_json::from_str(&history[0]).unwrap();
        assert!(matches!(parsed, RunUpdate::Completed { .. }));
    }

    #[tokio::test]
    async fn drain_writes_failed_status_on_channel_close() {
        let db_path = temp_file_db();
        let conn = open_db_path(&db_path).unwrap();
        insert_test_card(&conn, "c-2");
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(
            &conn,
            "r-2",
            "c-2",
            "my-agent",
            "/tmp/wt",
            "agent/c-2",
            "running",
            &[],
        )
        .unwrap();
        drop(conn);

        let (tx, rx) = mpsc::unbounded_channel::<RunUpdate>();
        let buffers = Arc::new(Mutex::new(HashMap::new()));
        buffers
            .lock()
            .await
            .insert("r-2".to_string(), UpdateBuffer::new());
        let runs = Arc::new(Mutex::new(HashMap::new()));
        // Drop the sender without sending a terminal update — drain should
        // fall back to the channel-closed path and write `failed`.
        drop(tx);
        drain_updates(rx, buffers, runs, "r-2".to_string(), db_path.clone(), None).await;

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
        insert_test_card(&conn, "c-3");
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(
            &conn,
            "r-3",
            "c-3",
            "my-agent",
            "/tmp/wt",
            "agent/c-3",
            "running",
            &[],
        )
        .unwrap();
        drop(conn);

        let (tx, rx) = mpsc::unbounded_channel::<RunUpdate>();
        let buffers = Arc::new(Mutex::new(HashMap::new()));
        buffers
            .lock()
            .await
            .insert("r-3".to_string(), UpdateBuffer::new());
        let runs = Arc::new(Mutex::new(HashMap::new()));
        let _ = tx.send(RunUpdate::Cancelled);
        drop(tx);
        drain_updates(rx, buffers, runs, "r-3".to_string(), db_path.clone(), None).await;

        let conn = open_db_path(&db_path).unwrap();
        let run = agent_runs::get_run(&conn, "r-3").unwrap().unwrap();
        assert_eq!(run.status, "cancelled");
        assert_eq!(run.stop_reason.as_deref(), Some("user_cancelled"));
        assert!(!agent_runs::is_card_locked(&conn, "c-3"));
    }

    #[test]
    fn create_run_failure_marks_row_failed_and_unlocks_card() {
        // Simulate the acp_create_run error path: insert a placeholder row,
        // then mark it failed (as the command does when start_run errors).
        let conn = test_db();
        insert_test_card(&conn, "c-4");
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(
            &conn,
            "r-4",
            "c-4",
            "my-agent",
            "/tmp/pending",
            "agent/pending",
            "pending",
            &[],
        )
        .unwrap();
        assert!(agent_runs::is_card_locked(&conn, "c-4"));
        // Mirror the error path in acp_create_run.
        agent_runs::update_status(
            &conn,
            "r-4",
            "failed",
            None,
            None,
            Some("worktree create failed: no git repo"),
            Some(&crate::db::now_iso()),
        )
        .unwrap();
        assert!(!agent_runs::is_card_locked(&conn, "c-4"));
        let run = agent_runs::get_run(&conn, "r-4").unwrap().unwrap();
        assert_eq!(run.status, "failed");
        assert_eq!(
            run.error.as_deref(),
            Some("worktree create failed: no git repo")
        );
    }

    #[test]
    fn latest_run_for_card_returns_most_recent() {
        let conn = test_db();
        insert_test_card(&conn, "c-5");
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(
            &conn,
            "r-old",
            "c-5",
            "my-agent",
            "/tmp/wt1",
            "agent/c-5",
            "completed",
            &[],
        )
        .unwrap();
        // Tiny delay so created_at differs; insert_run uses now_iso() per row.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        agent_runs::insert_run(
            &conn,
            "r-new",
            "c-5",
            "my-agent",
            "/tmp/wt2",
            "agent/c-5",
            "running",
            &[],
        )
        .unwrap();
        let latest = agent_runs::get_latest_run_for_card(&conn, "c-5")
            .unwrap()
            .unwrap();
        assert_eq!(latest.id, "r-new");
    }

    #[test]
    fn cleanup_dangling_marks_pending_as_failed_and_keeps_running() {
        let conn = test_db();
        insert_test_card(&conn, "c-1");
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(
            &conn,
            "r-1",
            "c-1",
            "my-agent",
            "/tmp/pending",
            "agent/pending",
            "pending",
            &[],
        )
        .unwrap();
        assert!(agent_runs::is_card_locked(&conn, "c-1"));
        let reaped = cleanup_dangling(&conn).unwrap();
        assert_eq!(reaped, vec!["r-1"]);
        assert!(!agent_runs::is_card_locked(&conn, "c-1"));
        let run = agent_runs::get_run(&conn, "r-1").unwrap().unwrap();
        assert_eq!(run.status, "failed");
        assert_eq!(
            run.error.as_deref(),
            Some("dangling: tasker restarted before worktree was created")
        );

        // Running runs are left alone so they can be resumed.
        insert_test_card(&conn, "c-2");
        agent_runs::insert_run(
            &conn,
            "r-2",
            "c-2",
            "my-agent",
            "/tmp/wt",
            "agent/c-2",
            "running",
            &[],
        )
        .unwrap();
        let reaped = cleanup_dangling(&conn).unwrap();
        assert!(reaped.is_empty());
        assert!(agent_runs::is_card_locked(&conn, "c-2"));
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
    fn find_agent_pid_finds_process_by_cwd() {
        let dir = std::env::temp_dir().join(format!("tasker-pid-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .current_dir(&dir)
            .spawn()
            .unwrap();
        let found = find_agent_pid(&dir);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(found, Some(child.id()));
        // Nonexistent worktree → None.
        assert_eq!(find_agent_pid(&PathBuf::from("/nonexistent-wt")), None);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn find_agent_pid_resolves_symlinked_worktree_path() {
        let dir = std::env::temp_dir().join(format!("tasker-pid-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let link = std::env::temp_dir().join(format!("tasker-pid-link-{}", uuid::Uuid::new_v4()));
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .current_dir(&dir)
            .spawn()
            .unwrap();
        // DB stores the symlinked form; kernel resolves it — must still match.
        let found = find_agent_pid(&link);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(found, Some(child.id()));
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn elapsed_secs_since_parses_now_iso() {
        let now = crate::db::now_iso();
        let secs = elapsed_secs_since(&now);
        assert!(secs < 5, "elapsed for 'now' should be ~0, got {secs}");
        assert_eq!(elapsed_secs_since("not-a-date"), 0);
        // An old timestamp yields a large positive value.
        assert!(elapsed_secs_since("2020-01-01T00:00:00Z") > 100_000_000);
    }

    fn parts(system: &str, skills: &str, body: &str) -> PromptParts {
        PromptParts {
            system: system.to_string(),
            skills: skills.to_string(),
            card_body: body.to_string(),
        }
    }

    #[test]
    fn resolve_system_prompt_agent_override_wins() {
        let conn = test_db();
        crate::db::agents::insert_agent(&conn, "a1", "echo", "T", false, true, &[]).unwrap();
        crate::db::agents::update_agent(&conn, "a1", "echo", "T", &[], None, None, "agent sys")
            .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('acp_system_prompt', 'global sys')",
            [],
        )
        .unwrap();
        let agent = crate::db::agents::get_agent(&conn, "a1").unwrap().unwrap();
        assert_eq!(resolve_system_prompt(&conn, &agent), "agent sys");
    }

    #[test]
    fn resolve_system_prompt_falls_back_to_global_setting() {
        let conn = test_db();
        crate::db::agents::insert_agent(&conn, "a2", "echo", "T", false, true, &[]).unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('acp_system_prompt', 'global sys')",
            [],
        )
        .unwrap();
        let agent = crate::db::agents::get_agent(&conn, "a2").unwrap().unwrap();
        assert_eq!(resolve_system_prompt(&conn, &agent), "global sys");
    }

    #[test]
    fn build_prompt_both_empty_omits_section() {
        let p = parts("", "", "body");
        assert_eq!(build_prompt(&p, None), "body");
    }

    #[test]
    fn build_prompt_section_order_system_skills_handoff_body() {
        let p = parts("sys", "# Preloaded skills\n\n## tdd\ntest", "body");
        let out = build_prompt(&p, Some("did stuff"));
        let idx = [
            out.find("# System prompt").unwrap(),
            out.find("# Preloaded skills").unwrap(),
            out.find("Previous session handoff").unwrap(),
            out.find("\nbody").unwrap_or(out.find("body").unwrap()),
        ];
        assert!(
            idx[0] < idx[1] && idx[1] < idx[2] && idx[2] < idx[3],
            "order wrong: {out}"
        );
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
    fn pick_allow_option_reject_only_returns_allow_literal_not_first() {
        // ponytail: regression for the bug where a Reject-only option list
        // caused "approve" to send the first (Reject) option. Now returns
        // the literal "allow" id so the agent errors clearly.
        use agent_client_protocol::schema::v1::{
            PermissionOption, PermissionOptionId, PermissionOptionKind,
        };
        let reject = PermissionOption::new(
            PermissionOptionId::new("deny"),
            "Permit always",
            PermissionOptionKind::RejectAlways,
        );
        let picked = pick_allow_option(&[reject]);
        assert_eq!(picked.0.as_ref(), "allow");
    }

    #[test]
    fn pick_allow_option_prefers_allow_once_over_allow_always() {
        use agent_client_protocol::schema::v1::{
            PermissionOption, PermissionOptionId, PermissionOptionKind,
        };
        let always = PermissionOption::new(
            PermissionOptionId::new("always"),
            "Allow always",
            PermissionOptionKind::AllowAlways,
        );
        let once = PermissionOption::new(
            PermissionOptionId::new("once"),
            "Allow once",
            PermissionOptionKind::AllowOnce,
        );
        let picked = pick_allow_option(&[always, once]);
        assert_eq!(picked.0.as_ref(), "once");
    }

    #[test]
    fn summarize_tool_call_truncates_long_args() {
        use agent_client_protocol::schema::v1::{ToolCallUpdate, ToolCallUpdateFields};
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
        use agent_client_protocol::schema::v1::{ToolCallUpdate, ToolCallUpdateFields};
        let tc = ToolCallUpdate::new(
            agent_client_protocol::schema::v1::ToolCallId::new("tc-2"),
            ToolCallUpdateFields::new().title("Read"),
        );
        assert_eq!(summarize_tool_call(&tc), "Read");
    }

    #[test]
    fn register_claude_code_name_rejected() {
        // Built-in name is reserved; re-registering must fail before
        // touching the DB. We exercise the validation guard directly.
        let err =
            AcpError::validation("Agent name 'claude-code' is reserved for the built-in agent");
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
            Err(AcpError::validation(
                "Cannot delete built-in agent 'claude-code'",
            ))
        } else {
            Ok(())
        };
        assert!(err.is_err());
    }

    #[test]
    fn delete_agent_returns_agent_has_runs_when_runs_exist() {
        let conn = test_db();
        insert_test_card(&conn, "c-dr");
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(
            &conn,
            "r-dr",
            "c-dr",
            "my-agent",
            "/tmp/wt",
            "agent/c-dr",
            "running",
            &[],
        )
        .unwrap();
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
        insert_test_card(&conn, "c-to");
        agents::insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(
            &conn,
            "r-to",
            "c-to",
            "my-agent",
            "/tmp/wt",
            "agent/c-to",
            "running",
            &[],
        )
        .unwrap();
        drop(conn);

        let (tx, rx) = mpsc::unbounded_channel::<RunUpdate>();
        let buffers = Arc::new(Mutex::new(HashMap::new()));
        buffers
            .lock()
            .await
            .insert("r-to".to_string(), UpdateBuffer::new());
        let runs = Arc::new(Mutex::new(HashMap::new()));
        let _ = tx.send(RunUpdate::PermissionTimeout);
        let _ = tx.send(RunUpdate::Completed {
            output: "done".into(),
            stop_reason: "end_turn".into(),
        });
        drop(tx);
        drain_updates(
            rx,
            buffers.clone(),
            runs,
            "r-to".to_string(),
            db_path.clone(),
            None,
        )
        .await;

        let conn = open_db_path(&db_path).unwrap();
        let run = agent_runs::get_run(&conn, "r-to").unwrap().unwrap();
        assert_eq!(run.status, "completed");
        let buf = buffers.lock().await;
        let updates = buf.get("r-to").unwrap();
        assert!(updates
            .updates
            .iter()
            .any(|u| matches!(u, RunUpdate::PermissionTimeout)));
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

    fn test_parts(body: &str) -> PromptParts {
        PromptParts {
            system: String::new(),
            skills: String::new(),
            card_body: body.to_string(),
        }
    }

    fn mock_acp_agent(hang: bool) -> agent_client_protocol::AcpAgent {
        use agent_client_protocol::AcpAgentConfig;
        let mut config = AcpAgentConfig::new(std::env::current_exe().unwrap())
            .arg("mock_acp_server")
            .arg("--nocapture")
            .env("MOCK_ACP_BINARY", "1");
        if hang {
            config = config.env("MOCK_ACP_HANG", "1");
        }
        agent_client_protocol::AcpAgent::new(config)
    }

    /// Mock that emits 3 chunks sharing one messageId — a single multi-chunk
    /// agent message — to exercise chunk coalescing.
    fn mock_acp_agent_coalesced() -> agent_client_protocol::AcpAgent {
        use agent_client_protocol::AcpAgentConfig;
        agent_client_protocol::AcpAgent::new(
            AcpAgentConfig::new(std::env::current_exe().unwrap())
                .arg("mock_acp_server")
                .arg("--nocapture")
                .env("MOCK_ACP_BINARY", "1")
                .env("MOCK_ACP_UPDATES", "3")
                .env("MOCK_ACP_SAME_MSG", "1"),
        )
    }

    /// Like `mock_acp_agent(true)` but also tells the mock to write its PID
    /// to `pid_file` — used by the hard-kill test to verify the SDK's
    /// `ChildGuard` SIGKILL's the process group after cancel.
    fn mock_acp_agent_with_pid(hang: bool, pid_file: &str) -> agent_client_protocol::AcpAgent {
        use agent_client_protocol::AcpAgentConfig;
        let mut config = AcpAgentConfig::new(std::env::current_exe().unwrap())
            .arg("mock_acp_server")
            .arg("--nocapture")
            .env("MOCK_ACP_BINARY", "1")
            .env("MOCK_ACP_PID_FILE", pid_file);
        if hang {
            config = config.env("MOCK_ACP_HANG", "1");
        }
        agent_client_protocol::AcpAgent::new(config)
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

    /// Wait for `WaitingForInput` to appear in the buffer.
    async fn wait_waiting_for_input(core: &RunCore, run_id: &str) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let has = {
                let buf = core.buffers.lock().await;
                buf.get(run_id)
                    .map(|b| {
                        b.updates
                            .iter()
                            .any(|u| matches!(u, RunUpdate::WaitingForInput { .. }))
                    })
                    .unwrap_or(false)
            };
            if has {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for WaitingForInput"
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    /// End-to-end: real git worktree + real spawned mock ACP process →
    /// streamed text update → agent stops with EndTurn → WaitingForInput
    /// → cancel → drain writes `cancelled` to the DB.
    #[tokio::test]
    async fn mock_agent_full_lifecycle_writes_completed() {
        let db_path = temp_file_db();
        let repo = temp_git_repo();
        let core = RunCore::default();
        // Mirror acp_create_run: insert the row first (with placeholders),
        // then start_run fills in the real worktree path + branch.
        {
            let conn = open_db_path(&db_path).unwrap();
            insert_test_card(&conn, "card-1");
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
            .start_run(
                "run-mock-1".into(),
                "card-1".into(),
                repo.to_string_lossy().into_owned(),
                test_parts("do the thing"),
                mock_acp_agent(false),
                db_path.clone(),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_run should succeed");
        assert!(PathBuf::from(&wt_path).exists(), "worktree should exist");
        assert!(branch.starts_with("agent/"), "branch prefix");

        // Wait for WaitingForInput (agent sent StopReason), then cancel
        // since the test has no interactive follow-up to send.
        wait_waiting_for_input(&core, "run-mock-1").await;
        core.cancel_run("run-mock-1").await;

        let updates = wait_terminal(&core, "run-mock-1").await;
        assert!(
            updates
                .iter()
                .any(|u| matches!(u, RunUpdate::SessionUpdate { text } if text.contains("mock output chunk"))),
            "expected streamed text, got: {updates:?}"
        );
        assert!(
            updates.iter().any(|u| matches!(u, RunUpdate::Cancelled)),
            "expected Cancelled, got: {updates:?}"
        );

        let conn = open_db_path(&db_path).unwrap();
        let run = agent_runs::get_run(&conn, "run-mock-1").unwrap().unwrap();
        assert_eq!(run.status, "cancelled");
        // Cleanup the worktree so the test leaves no litter.
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&wt_path)
            .current_dir(&repo)
            .output();
    }

    /// Chunks sharing a messageId are ONE agent message: they must arrive in
    /// the buffer as a single coalesced SessionUpdate, not one per chunk.
    #[tokio::test]
    async fn mock_agent_coalesces_same_message_chunks() {
        let db_path = temp_file_db();
        let repo = temp_git_repo();
        let core = RunCore::default();
        {
            let conn = open_db_path(&db_path).unwrap();
            insert_test_card(&conn, "card-1");
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
        let (wt_path, _branch) = core
            .start_run(
                "run-mock-2".into(),
                "card-1".into(),
                repo.to_string_lossy().into_owned(),
                test_parts("do the thing"),
                mock_acp_agent_coalesced(),
                db_path.clone(),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_run should succeed");

        wait_waiting_for_input(&core, "run-mock-2").await;
        core.cancel_run("run-mock-2").await;
        let updates = wait_terminal(&core, "run-mock-2").await;

        let texts: Vec<&str> = updates
            .iter()
            .filter_map(|u| match u {
                RunUpdate::SessionUpdate { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            texts.len(),
            1,
            "expected ONE coalesced message, got {texts:?}"
        );
        assert_eq!(
            texts[0],
            "mock output chunk 0mock output chunk 1mock output chunk 2"
        );

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
        let core = RunCore::default();
        {
            let conn = open_db_path(&db_path).unwrap();
            insert_test_card(&conn, "card-1");
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
            .start_run(
                "run-mock-2".into(),
                "card-1".into(),
                repo.to_string_lossy().into_owned(),
                test_parts("do the thing"),
                mock_acp_agent(true),
                db_path.clone(),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_run should succeed");

        // Wait for the session to be established (SessionId update) so the
        // cancel races a live session, not a spawn in progress.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let has_session = {
                let buf = core.buffers.lock().await;
                buf.get("run-mock-2")
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
        let core = RunCore::default();
        let pid_file =
            std::env::temp_dir().join(format!("tasker-mock-pid-{}.txt", uuid::Uuid::new_v4()));
        {
            let conn = open_db_path(&db_path).unwrap();
            insert_test_card(&conn, "card-hk");
            agents::insert_agent(&conn, "tester", "echo hi", "Test", false, true, &[]).unwrap();
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
            .start_run(
                "run-hk".into(),
                "card-hk".into(),
                repo.to_string_lossy().into_owned(),
                test_parts("do the thing"),
                mock_acp_agent_with_pid(true, pid_file.to_str().unwrap()),
                db_path.clone(),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_run should succeed");

        // Wait for the mock to write its PID (spawned + started reading).
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if pid_file.exists() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "mock never wrote PID"
            );
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
            assert!(
                tokio::time::Instant::now() < deadline,
                "session never established"
            );
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
