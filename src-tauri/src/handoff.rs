//! Resume handoff: lazily generate a summary of a run's previous output so
//! a resumed session starts with context. The handoff document is owned by
//! the app (stored in the tasker DB), never written into the worktree.
//! Generation spawns a throwaway ACP session (same agent definition, new
//! id); failure or timeout falls back to tail truncation. Handoff failure
//! never fails the run.

use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::db::agent_runs;
use crate::runner::RunUpdate;
use crate::session::{run_session_job, SessionJob};

/// Summarizer budget: give up and fall back to tail truncation.
const SUMMARIZE_TIMEOUT_SECS: u64 = 120;
/// Cap on the run output fed to the summarizer (chars, from the tail).
const SUMMARIZER_INPUT_CAP: usize = 100_000;
/// Fallback handoff size: last N chars of the raw run output.
const FALLBACK_TAIL_CHARS: usize = 8_000;

/// Data needed to (re)generate a run's handoff.
#[derive(Clone)]
pub struct HandoffInput {
    /// Accumulated `SessionUpdate` text from the run's previous life.
    pub output: String,
    pub worktree_path: String,
    pub repo_root: Option<String>,
}

/// Everything `start_run` needs to regenerate context before the first
/// prompt of a resumed session.
pub struct HandoffCtx {
    /// The run's `finished_at` (staleness reference for an existing handoff).
    pub finished_at: Option<String>,
    /// Separate AcpAgent value for the throwaway summarizer session
    /// (`AcpAgent` is not `Clone`; rebuild from the agent command).
    pub summarizer_agent: agent_client_protocol::AcpAgent,
    pub input: HandoffInput,
}

/// Look up the handoff for a run; generate it on miss or staleness.
/// Staleness rule: an existing handoff is reused only when
/// `updated_at >= finished_at` (i.e. generated after the run's last
/// terminal transition). Returns `None` when there is nothing useful
/// (empty output, no usable cwd). Never fails the caller.
pub async fn ensure(
    db_path: &PathBuf,
    run_id: &str,
    finished_at: Option<&str>,
    summarizer_agent: agent_client_protocol::AcpAgent,
    input: &HandoffInput,
) -> Option<String> {
    {
        let conn = crate::db::open_db_path(db_path).ok()?;
        match agent_runs::get_handoff(&conn, run_id) {
            Ok(Some(h)) => {
                let fresh = match finished_at {
                    Some(f) => h.updated_at.as_str() >= f,
                    None => true,
                };
                if fresh {
                    return Some(h.content);
                }
            }
            Ok(None) | Err(_) => {}
        }
    }
    if input.output.trim().is_empty() {
        return None;
    }
    // cwd: worktree if it still exists, else repo root, else skip.
    let cwd = {
        let wt = PathBuf::from(&input.worktree_path);
        if wt.exists() {
            wt
        } else if let Some(repo) = &input.repo_root {
            let p = PathBuf::from(repo);
            if p.exists() {
                p
            } else {
                eprintln!("[handoff:{run_id}] no usable cwd; skipping handoff");
                return None;
            }
        } else {
            eprintln!("[handoff:{run_id}] no usable cwd; skipping handoff");
            return None;
        }
    };
    let content = summarize(db_path, run_id, summarizer_agent, &cwd, &input.output).await;
    let (content, source) = match content {
        Some(c) if !c.trim().is_empty() => (c, "summary"),
        _ => (tail_chars(&input.output, FALLBACK_TAIL_CHARS), "truncated"),
    };
    let conn = crate::db::open_db_path(db_path).ok()?;
    agent_runs::upsert_handoff(&conn, run_id, &content, source).ok()?;
    Some(content)
}

/// Spawn a throwaway summarizer session over the run's output (tail-capped)
/// and collect the agent's reply. `None` on spawn failure, timeout, or
/// empty reply.
async fn summarize(
    db_path: &PathBuf,
    run_id: &str,
    summarizer_agent: agent_client_protocol::AcpAgent,
    cwd: &PathBuf,
    output: &str,
) -> Option<String> {
    let prompt = format!(
        "Summarize the following transcript of a previous work session on this task. \
         Produce a compact handoff document: what was attempted, what was done, \
         the current state, and what remains. Do not redo any work; this is context only.\n\n\
         --- Previous session output (tail) ---\n\n{}",
        tail_chars(output, SUMMARIZER_INPUT_CAP)
    );
    let (tx, mut rx) = mpsc::unbounded_channel::<RunUpdate>();
    let job = SessionJob {
        cwd: cwd.clone(),
        prompt,
        model: None,
        effort: None,
        tx,
        prompt_rx: None,
        cancel: None,
        complete: None,
        permissions: None,
        db_path: Some(db_path.clone()),
        run_id: format!("{run_id}:handoff"),
        on_session: None,
    };
    tokio::spawn(run_session_job(summarizer_agent, job));
    let mut text = String::new();
    let mut ok = false;
    let collected = tokio::time::timeout(Duration::from_secs(SUMMARIZE_TIMEOUT_SECS), async {
        while let Some(update) = rx.recv().await {
            match update {
                RunUpdate::SessionUpdate { text: t } => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&t);
                }
                RunUpdate::Completed { .. } => {
                    ok = true;
                    return;
                }
                RunUpdate::Failed { error } => {
                    eprintln!("[handoff:{run_id}] summarizer failed: {error}");
                    return;
                }
                RunUpdate::Cancelled => return,
                _ => {}
            }
        }
    })
    .await;
    if collected.is_err() {
        eprintln!("[handoff:{run_id}] summarizer timed out after {SUMMARIZE_TIMEOUT_SECS}s");
        return None;
    }
    if ok && !text.trim().is_empty() {
        Some(text)
    } else {
        None
    }
}

/// Last `max` chars of `s` on a char boundary. Reuses the shared
/// char-safe truncator by reversing twice.
fn tail_chars(s: &str, max: usize) -> String {
    let rev: String = s.chars().rev().collect();
    crate::runner::truncate_chars(&rev, max).chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{agent_runs, agents, open_db_path, run_migrations};
    use std::path::PathBuf;

    fn temp_file_db() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tasker-handoff-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("tasker.db");
        let conn = open_db_path(&db_path).unwrap();
        run_migrations(&conn).unwrap();
        conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, created_at, updated_at)
               VALUES ('c-1', 'Test', 'backlog', 'local', 0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
            [],
        )
        .unwrap();
        agents::insert_agent(&conn, "tester", "echo hi", "Test", false, true, &[]).unwrap();
        agent_runs::insert_run(&conn, "r-1", "c-1", "tester", "/tmp/wt", "agent/c-1", "completed", &[])
            .unwrap();
        drop(conn);
        db_path
    }

    /// Mock ACP agent (spawns the test binary itself). `fail` sets
    /// MOCK_ACP_FAIL so the prompt turn errors.
    fn mock_agent(fail: bool) -> agent_client_protocol::AcpAgent {
        use agent_client_protocol::AcpAgentConfig;
        let mut config = AcpAgentConfig::new(std::env::current_exe().unwrap())
            .arg("mock_acp_server")
            .arg("--nocapture")
            .env("MOCK_ACP_BINARY", "1");
        if fail {
            config = config.env("MOCK_ACP_FAIL", "1");
        }
        agent_client_protocol::AcpAgent::new(config)
    }

    fn input(output: &str, worktree: &str) -> HandoffInput {
        HandoffInput {
            output: output.to_string(),
            worktree_path: worktree.to_string(),
            repo_root: None,
        }
    }

    #[tokio::test]
    async fn ensure_generates_summary_via_mock_agent() {
        let db_path = temp_file_db();
        // Worktree must exist for the summarizer cwd; use the db's dir.
        let wt = db_path.parent().unwrap().to_path_buf();
        let content = ensure(
            &db_path,
            "r-1",
            None,
            mock_agent(false),
            &input("did stuff", wt.to_str().unwrap()),
        )
        .await
        .expect("handoff should be generated");
        assert!(content.contains("mock output chunk"), "got: {content}");
        let conn = open_db_path(&db_path).unwrap();
        let h = agent_runs::get_handoff(&conn, "r-1").unwrap().unwrap();
        assert_eq!(h.source, "summary");
    }

    #[tokio::test]
    async fn ensure_spawn_failure_falls_back_to_truncated() {
        let db_path = temp_file_db();
        let wt = db_path.parent().unwrap().to_path_buf();
        let content = ensure(
            &db_path,
            "r-1",
            None,
            mock_agent(true),
            &input("previous output tail", wt.to_str().unwrap()),
        )
        .await
        .expect("fallback handoff should exist");
        assert_eq!(content, "previous output tail");
        let conn = open_db_path(&db_path).unwrap();
        let h = agent_runs::get_handoff(&conn, "r-1").unwrap().unwrap();
        assert_eq!(h.source, "truncated");
    }

    #[tokio::test]
    async fn ensure_reuses_fresh_handoff_without_spawning() {
        let db_path = temp_file_db();
        {
            let conn = open_db_path(&db_path).unwrap();
            agent_runs::upsert_handoff(&conn, "r-1", "stored summary", "summary").unwrap();
        }
        // finished_at older than the handoff's updated_at → fresh.
        let content = ensure(
            &db_path,
            "r-1",
            Some("2026-01-01T00:00:00Z"),
            mock_agent(false),
            &input("did stuff", "/nonexistent-wt"),
        )
        .await
        .expect("fresh handoff should be reused");
        assert_eq!(content, "stored summary");
    }

    #[tokio::test]
    async fn ensure_regenerates_stale_handoff() {
        let db_path = temp_file_db();
        {
            let conn = open_db_path(&db_path).unwrap();
            agent_runs::upsert_handoff(&conn, "r-1", "stale summary", "summary").unwrap();
        }
        let wt = db_path.parent().unwrap().to_path_buf();
        // finished_at NEWER than the stored handoff → stale → regenerate.
        let content = ensure(
            &db_path,
            "r-1",
            Some("2099-01-01T00:00:00Z"),
            mock_agent(false),
            &input("did stuff", wt.to_str().unwrap()),
        )
        .await
        .expect("stale handoff should regenerate");
        assert!(content.contains("mock output chunk"), "got: {content}");
    }

    #[tokio::test]
    async fn ensure_empty_output_returns_none_without_row() {
        let db_path = temp_file_db();
        let wt = db_path.parent().unwrap().to_path_buf();
        let content = ensure(
            &db_path,
            "r-1",
            None,
            mock_agent(false),
            &input("   ", wt.to_str().unwrap()),
        )
        .await;
        assert!(content.is_none());
        let conn = open_db_path(&db_path).unwrap();
        assert!(agent_runs::get_handoff(&conn, "r-1").unwrap().is_none());
    }

    #[tokio::test]
    async fn ensure_missing_cwd_skips_generation() {
        let db_path = temp_file_db();
        // Worktree and repo_root both nonexistent → no handoff, no row.
        let content = ensure(
            &db_path,
            "r-1",
            None,
            mock_agent(false),
            &input("did stuff", "/nonexistent-wt"),
        )
        .await;
        assert!(content.is_none());
        let conn = open_db_path(&db_path).unwrap();
        assert!(agent_runs::get_handoff(&conn, "r-1").unwrap().is_none());
    }
}
