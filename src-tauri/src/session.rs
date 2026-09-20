//! One-shot ACP session runner: connect → session/new → prompt → read
//! until terminal. Shared skeleton for the GUI run task (`start_run`),
//! the CLI `run` command (stdout sink), and the handoff summarizer
//! (collector + timeout). All outcomes are reported through `tx` as
//! `RunUpdate`s — spawn failures arrive as `RunUpdate::Failed`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex, Notify};

use crate::db::agent_runs;
use crate::runner::{
    flush_pending_chunk, format_session_update, pick_allow_option, raw_text_from_content,
    summarize_tool_call, PendingPermission, RunUpdate,
};

/// Everything one ACP session needs. `start_run` fills every field (full
/// hooks); the CLI and the handoff summarizer leave the interactive ones
/// `None` (auto-approve permissions, first StopReason completes the run).
#[derive(Clone)]
pub struct SessionJob {
    pub cwd: PathBuf,
    pub prompt: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub tx: mpsc::UnboundedSender<RunUpdate>,
    /// Follow-up prompt receiver (interactive runs only). Wrapped in
    /// `Arc<Mutex<Option<_>>>` because the connect closure may be polled
    /// more than once; the receiver is taken on the single actual call.
    pub prompt_rx: Option<
        Arc<Mutex<Option<mpsc::UnboundedReceiver<String>>>>,
    >,
    /// Cancel token (`None` = not cancellable).
    pub cancel: Option<Arc<Notify>>,
    /// When set, the cancel branch emits `Completed` instead of `Cancelled`.
    pub complete: Option<Arc<AtomicBool>>,
    /// GUI permission flow: register pending requests + arm the timeout
    /// task. `None` = auto-approve the allow option (CLI, summarizer).
    pub permissions: Option<Arc<Mutex<HashMap<String, Arc<PendingPermission>>>>>,
    /// DB path: permission-timeout setting + session_id persistence.
    /// `None` skips both (CLI keeps its own terminal-status write).
    pub db_path: Option<PathBuf>,
    /// Run id: permission request ids + session_id persistence.
    pub run_id: String,
    /// Called once the session starts (publish live conn + session id for
    /// on-the-fly config changes). `None` for one-shot sessions.
    pub on_session: Option<
        Arc<
            dyn Fn(
                    &agent_client_protocol::ConnectionTo<agent_client_protocol::Agent>,
                    &str,
                ) + Send
                + Sync,
        >,
    >,
}

/// Run one ACP session to completion. All outcomes are reported via `tx`;
/// connect failures arrive as `RunUpdate::Failed`.
pub async fn run_session_job(
    agent: agent_client_protocol::AcpAgent,
    job: SessionJob,
) {
    let tx_for_err = job.tx.clone();
    let result = agent_client_protocol::Client
        .connect_with(agent, async move |cx| session_loop(cx, job).await)
        .await;
    match result {
        Ok(()) => eprintln!("[session] connect_with returned Ok"),
        Err(e) => {
            eprintln!("[session] connect_with returned Err: {e}");
            let _ = tx_for_err.send(RunUpdate::Failed {
                error: e.to_string(),
            });
        }
    }
}

/// Apply session config options (model / effort) via ACP
/// `session/set_config_option`. Failures are logged and skipped — agents
/// that don't support a config id return a JSON-RPC error, which must
/// not kill the run.
pub async fn apply_session_config(
    conn: &agent_client_protocol::ConnectionTo<agent_client_protocol::Agent>,
    session_id: &str,
    model: Option<&str>,
    effort: Option<&str>,
) {
    use agent_client_protocol::schema::v1::SetSessionConfigOptionRequest;
    for (config_id, value) in [("model", model), ("effort", effort)] {
        let Some(value) = value else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let req = SetSessionConfigOptionRequest::new(
            agent_client_protocol::schema::v1::SessionId::new(session_id),
            agent_client_protocol::schema::v1::SessionConfigId::new(config_id),
            value,
        );
        let what = format!("session/set_config_option({config_id}={value})");
        match conn.send_request(req).block_task().await {
            Ok(_) => eprintln!("[session_config] applied {what}"),
            Err(e) => eprintln!("[session_config] {what} skipped: {e}"),
        }
    }
}

/// The connect closure body: build the session, send the prompt, and read
/// updates until a stop reason, cancellation, or error.
async fn session_loop(
    cx: agent_client_protocol::ConnectionTo<agent_client_protocol::Agent>,
    job: SessionJob,
) -> Result<(), agent_client_protocol::Error> {
    let tx = job.tx.clone();
    let run_id = job.run_id.clone();
    let permissions = job.permissions.clone();
    let db_path = job.db_path.clone();
    let cancel = job
        .cancel
        .clone()
        .unwrap_or_else(|| Arc::new(Notify::new()));
    let complete = job
        .complete
        .clone()
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    // Take the receiver from the Arc<Mutex<Option<_>>>.
    // Only the first call to this closure gets it.
    let mut prompt_rx = match &job.prompt_rx {
        Some(rx) => rx.lock().await.take(),
        None => None,
    };
    eprintln!(
        "[session:{run_id}] connect closure entered, building session cwd={}",
        job.cwd.display()
    );
    let mut session = cx.build_session(&job.cwd).block_task().start_session().await?;
    let session_id = session.session_id().0.clone();
    eprintln!("[session:{run_id}] session started: id={session_id}");
    if let Some(cb) = &job.on_session {
        cb(session.connection(), &session_id);
    }
    apply_session_config(
        session.connection(),
        &session_id,
        job.model.as_deref(),
        job.effort.as_deref(),
    )
    .await;
    let _ = tx.send(RunUpdate::SessionId {
        session_id: session_id.to_string(),
    });
    // Persist the session id so a later resume can spawn a fresh agent
    // session in the same worktree.
    if let Some(db_path) = &db_path {
        let db_path = db_path.clone();
        let run_id2 = run_id.clone();
        let session_id2 = session_id.to_string();
        tokio::task::spawn_blocking(move || {
            if let Ok(conn) = crate::db::open_db_path(&db_path) {
                let _ = agent_runs::set_session_id(&conn, &run_id2, &session_id2);
            }
        });
    }
    // Clone the connection so the cancel branch can send the
    // `session/cancel` notification without borrowing `session` while
    // `read_update()`'s future is live.
    let conn_for_cancel = session.connection().clone();
    eprintln!("[session:{run_id}] sending prompt ({} chars)", job.prompt.len());
    session.send_prompt(&job.prompt)?;
    eprintln!("[session:{run_id}] prompt sent, reading updates");
    // Read updates until stop reason, or until cancelled. On cancel, send
    // the ACP `session/cancel` notification (cooperative — the agent may
    // ignore it) and emit a Cancelled update so the drain task records
    // terminal state. When this closure returns, the SDK drops the
    // connection's `ChildGuard`, which SIGKILL's the entire process group
    // after a 1s grace period — so even an agent that ignores
    // `session/cancel` is hard-killed.
    //
    // `followup` is set when the agent stops with EndTurn and the user
    // sends a reply. It's checked AFTER the select block so the `read`
    // future (which mutably borrows `session`) is dropped before
    // `send_prompt`.
    //
    // `pending_chunk` accumulates `agent_message_chunk` fragments of ONE
    // message (same messageId) and flushes them as a single SessionUpdate
    // on any boundary — otherwise every chunk becomes its own bubble in
    // the panel.
    let mut pending_chunk: Option<(Option<String>, String)> = None;
    loop {
        let mut followup: Option<String> = None;
        let mut cancelled = false;
        let mut failed = false;
        let mut terminal = false;
        {
            let read = session.read_update();
            tokio::pin!(read);
            tokio::select! {
                biased;
                _ = cancel.notified() => {
                    let _ = conn_for_cancel.send_notification(
                        agent_client_protocol::schema::v1::CancelNotification::new(session_id.clone()),
                    );
                    flush_pending_chunk(&mut pending_chunk, &tx);
                    if complete.load(Ordering::SeqCst) {
                        let _ = tx.send(RunUpdate::Completed {
                            output: String::new(),
                            stop_reason: "done".to_string(),
                        });
                    } else {
                        let _ = tx.send(RunUpdate::Cancelled);
                    }
                    cancelled = true;
                }
                msg = read => {
                    match msg {
                        Ok(msg) => {
                            use agent_client_protocol::SessionMessage;
                            match msg {
                                SessionMessage::SessionMessage(dispatch) => {
                                    let method = dispatch.method().to_string();
                                    if method == "session/request_permission" {
                                        // Permission request interrupts any in-flight agent
                                        // message — flush it first so the reply tail isn't lost.
                                        flush_pending_chunk(&mut pending_chunk, &tx);
                                        use agent_client_protocol::schema::v1::RequestPermissionRequest;
                                        match dispatch.into_request::<RequestPermissionRequest>() {
                                            Ok(Ok((req, responder))) => match &permissions {
                                                Some(perms_map) => {
                                                    handle_gui_permission(
                                                        perms_map, &db_path, &run_id, &tx,
                                                        &req, responder,
                                                    ).await;
                                                }
                                                None => {
                                                    // One-shot session (CLI, summarizer):
                                                    // auto-approve the allow option.
                                                    let option_id = pick_allow_option(&req.options);
                                                    let description = summarize_tool_call(&req.tool_call);
                                                    let _ = tx.send(RunUpdate::PermissionRequest {
                                                        request_id: req.tool_call.tool_call_id.0.to_string(),
                                                        description,
                                                    });
                                                    use agent_client_protocol::schema::v1::{
                                                        RequestPermissionOutcome,
                                                        RequestPermissionResponse,
                                                        SelectedPermissionOutcome,
                                                    };
                                                    let _ = responder.respond(
                                                        RequestPermissionResponse::new(
                                                            RequestPermissionOutcome::Selected(
                                                                SelectedPermissionOutcome::new(option_id),
                                                            ),
                                                        ),
                                                    );
                                                }
                                            },
                                            Ok(Err(_)) | Err(_) => {
                                                flush_pending_chunk(&mut pending_chunk, &tx);
                                                let _ = tx.send(RunUpdate::SessionUpdate {
                                                    text: "permission request: failed to parse".to_string(),
                                                });
                                            }
                                        }
                                    } else if method == "session/update" {
                                        use agent_client_protocol::schema::v1::SessionNotification;
                                        match dispatch.into_notification::<SessionNotification>() {
                                            Ok(Ok(notif)) => {
                                                use agent_client_protocol::schema::v1::SessionUpdate;
                                                match &notif.update {
                                                    // Accumulate message fragments; flush as one
                                                    // update on any other update type or a messageId
                                                    // change.
                                                    SessionUpdate::AgentMessageChunk(chunk) => {
                                                        if let Some(t) = raw_text_from_content(&chunk.content) {
                                                            let id = chunk
                                                                .message_id
                                                                .clone()
                                                                .map(|m| m.0.to_string());
                                                            match &mut pending_chunk {
                                                                Some((pid, buf)) if *pid == id => buf.push_str(&t),
                                                                _ => {
                                                                    flush_pending_chunk(&mut pending_chunk, &tx);
                                                                    pending_chunk = Some((id, t));
                                                                }
                                                            }
                                                        }
                                                    }
                                                    other => {
                                                        flush_pending_chunk(&mut pending_chunk, &tx);
                                                        if let Some(text) = format_session_update(other) {
                                                            let _ = tx.send(RunUpdate::SessionUpdate { text });
                                                        }
                                                    }
                                                }
                                            }
                                            Ok(Err(_)) => {}
                                            Err(e) => {
                                                flush_pending_chunk(&mut pending_chunk, &tx);
                                                let _ = tx.send(RunUpdate::SessionUpdate {
                                                    text: format!("parse error: {e}"),
                                                });
                                            }
                                        }
                                    } else {
                                        flush_pending_chunk(&mut pending_chunk, &tx);
                                        let _ = tx.send(RunUpdate::SessionUpdate {
                                            text: format!("(unhandled: {method})"),
                                        });
                                    }
                                }
                                SessionMessage::StopReason(reason) => {
                                    eprintln!("[session:{run_id}] stop reason: {reason:?}");
                                    let stop_str = format!("{reason:?}");
                                    // Agent finished replying — flush the last message tail
                                    // before signalling the wait state.
                                    flush_pending_chunk(&mut pending_chunk, &tx);
                                    if prompt_rx.is_none() {
                                        // One-shot session: first stop reason completes it.
                                        let _ = tx.send(RunUpdate::Completed {
                                            output: String::new(),
                                            stop_reason: stop_str.clone(),
                                        });
                                        terminal = true;
                                    } else {
                                        let _ = tx.send(RunUpdate::WaitingForInput {
                                            stop_reason: stop_str.clone(),
                                        });
                                        // Wait for follow-up prompt or cancel.
                                        loop {
                                            tokio::select! {
                                                biased;
                                                _ = cancel.notified() => {
                                                    flush_pending_chunk(&mut pending_chunk, &tx);
                                                    if complete.load(Ordering::SeqCst) {
                                                        let _ = tx.send(RunUpdate::Completed {
                                                            output: String::new(),
                                                            stop_reason: "done".to_string(),
                                                        });
                                                    } else {
                                                        let _ = tx.send(RunUpdate::Cancelled);
                                                    }
                                                    terminal = true;
                                                    break;
                                                }
                                                msg = prompt_rx.as_mut().unwrap().recv() => {
                                                    if let Some(p) = msg {
                                                        followup = Some(p);
                                                        break;
                                                    } else {
                                                        flush_pending_chunk(&mut pending_chunk, &tx);
                                                        let _ = tx.send(RunUpdate::Completed {
                                                            output: String::new(),
                                                            stop_reason: stop_str.clone(),
                                                        });
                                                        terminal = true;
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        Err(e) => {
                            eprintln!("[session:{run_id}] read_update error: {e}");
                            flush_pending_chunk(&mut pending_chunk, &tx);
                            let _ = tx.send(RunUpdate::Failed {
                                error: e.to_string(),
                            });
                            failed = true;
                        }
                    }
                }
            }
        } // `read` future dropped here — `session` is free again.
        if cancelled || failed || terminal {
            break;
        }
        if let Some(p) = followup {
            eprintln!("[session:{run_id}] sending follow-up prompt ({} chars)", p.len());
            let _ = tx.send(RunUpdate::SessionUpdate {
                text: format!("— user: {p}"),
            });
            session.send_prompt(&p)?;
            continue;
        }
        // Normal SessionMessage update — keep reading.
        continue;
    }
    Ok(())
}

/// GUI permission flow: register the pending request so the UI can respond
/// via `acp_respond_permission`, and arm the auto-deny timeout task.
#[allow(clippy::too_many_arguments)]
async fn handle_gui_permission(
    perms_map: &Arc<Mutex<HashMap<String, Arc<PendingPermission>>>>,
    db_path: &Option<PathBuf>,
    run_id: &str,
    tx: &mpsc::UnboundedSender<RunUpdate>,
    req: &agent_client_protocol::schema::v1::RequestPermissionRequest,
    responder: agent_client_protocol::Responder<
        agent_client_protocol::schema::v1::RequestPermissionResponse,
    >,
) {
    let req_id = format!("{run_id}:{}", req.tool_call.tool_call_id.0);
    let description = summarize_tool_call(&req.tool_call);
    let option_id = pick_allow_option(&req.options);
    let responded = Arc::new(Notify::new());
    let pending = Arc::new(PendingPermission {
        responder: tokio::sync::Mutex::new(Some(responder)),
        option_id,
        responded: responded.clone(),
    });
    {
        let mut perms = perms_map.lock().await;
        perms.insert(req_id.clone(), pending.clone());
    }
    let _ = tx.send(RunUpdate::PermissionRequest {
        request_id: req_id.clone(),
        description,
    });
    let tx_t = tx.clone();
    let perms_t = perms_map.clone();
    let req_id_t = req_id;
    let db_path_t = db_path.clone();
    tokio::spawn(async move {
        let timeout = read_permission_timeout(db_path_t.as_ref());
        tokio::select! {
            biased;
            _ = responded.notified() => {}
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
                        RequestPermissionOutcome, RequestPermissionResponse,
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

/// Read the `acp_permission_timeout` setting (seconds, default 300) from
/// the DB by path. Opens a short-lived connection — the permission timeout
/// task runs outside the run's main connection (Connection is !Send).
fn read_permission_timeout(db_path: Option<&PathBuf>) -> u64 {
    let Some(db_path) = db_path else {
        return 300;
    };
    let Ok(conn) = crate::db::open_db_path(db_path) else {
        return 300;
    };
    crate::db::read_setting(&conn, "acp_permission_timeout")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(300)
}
