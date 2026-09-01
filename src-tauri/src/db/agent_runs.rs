use crate::db::agents::{parse_skills_json, serialize_skills};
use crate::db::now_iso;
use crate::error::AcpError;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

/// Canonical column list for SELECTs on `agent_runs`, in the order
/// [`row_to_run`] reads them.
const RUN_COLUMNS: &str = "id, card_id, agent_name, session_id, worktree_path, branch, repo_root, status, output, stop_reason, error, merged_at, skills_json, created_at, finished_at";

/// API-facing agent run representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRun {
    pub id: String,
    pub card_id: String,
    pub agent_name: String,
    pub session_id: Option<String>,
    pub worktree_path: String,
    pub branch: String,
    pub repo_root: Option<String>,
    pub status: String,
    pub output: Option<String>,
    pub stop_reason: Option<String>,
    pub error: Option<String>,
    pub merged_at: Option<String>,
    pub skills: Vec<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

/// Insert a new agent run. `repo_root` is resolved up front from the
/// card's `tree_source_id` → `tree_sources.path`, so runs that fail before
/// the worktree step still carry a non-null repo root (the post-worktree
/// `set_worktree_info` call remains the source of truth when it runs).
pub fn insert_run(
    conn: &Connection,
    id: &str,
    card_id: &str,
    agent_name: &str,
    worktree_path: &str,
    branch: &str,
    status: &str,
    skills: &[String],
) -> Result<(), AcpError> {
    conn.execute(
        "INSERT INTO agent_runs (id, card_id, agent_name, worktree_path, branch, status, skills_json, created_at, repo_root)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                 (SELECT ts.path FROM cards c
                  JOIN tree_sources ts ON ts.id = c.tree_source_id
                  WHERE c.id = ?2))",
        params![id, card_id, agent_name, worktree_path, branch, status, serialize_skills(skills), now_iso()],
    ).map_err(AcpError::internal)?;
    Ok(())
}

/// Get the active run for a card, if any.
pub fn get_active_run(conn: &Connection, card_id: &str) -> Result<Option<AgentRun>, AcpError> {
    let row = conn.query_row(
        &format!(
            "SELECT {RUN_COLUMNS} FROM agent_runs WHERE card_id = ?1 AND status IN ('pending', 'running') LIMIT 1"
        ),
        params![card_id],
        row_to_run,
    );
    match row {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(AcpError::internal(e.to_string())),
    }
}

/// Get the most recent run for a card, regardless of status.
pub fn get_latest_run_for_card(
    conn: &Connection,
    card_id: &str,
) -> Result<Option<AgentRun>, AcpError> {
    let row = conn.query_row(
        &format!(
            "SELECT {RUN_COLUMNS} FROM agent_runs WHERE card_id = ?1 ORDER BY created_at DESC LIMIT 1"
        ),
        params![card_id],
        row_to_run,
    );
    match row {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(AcpError::internal(e.to_string())),
    }
}

/// Update the worktree_path, branch, and status for a run (used after the
/// worktree is created, to replace the placeholder values from insert_run).
pub fn update_worktree_branch(
    conn: &Connection,
    id: &str,
    worktree_path: &str,
    branch: &str,
    status: &str,
) -> Result<(), AcpError> {
    conn.execute(
        "UPDATE agent_runs SET worktree_path = ?1, branch = ?2, status = ?3 WHERE id = ?4",
        params![worktree_path, branch, status, id],
    )
    .map_err(AcpError::internal)?;
    Ok(())
}

/// Get a single run by id.
pub fn get_run(conn: &Connection, id: &str) -> Result<Option<AgentRun>, AcpError> {
    let row = conn.query_row(
        &format!("SELECT {RUN_COLUMNS} FROM agent_runs WHERE id = ?1"),
        params![id],
        row_to_run,
    );
    match row {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(AcpError::internal(e.to_string())),
    }
}

/// Delete a run by id. Callers must remove the worktree first if desired;
/// this only deletes the DB row.
pub fn delete_run(conn: &Connection, id: &str) -> Result<(), AcpError> {
    conn.execute("DELETE FROM agent_runs WHERE id = ?1", params![id])
        .map_err(AcpError::internal)?;
    Ok(())
}

/// Update run status and terminal fields.
pub fn update_status(
    conn: &Connection,
    id: &str,
    status: &str,
    output: Option<&str>,
    stop_reason: Option<&str>,
    error: Option<&str>,
    finished_at: Option<&str>,
) -> Result<(), AcpError> {
    conn.execute(
        "UPDATE agent_runs SET status = ?1, output = ?2, stop_reason = ?3, error = ?4, finished_at = ?5
         WHERE id = ?6",
        params![status, output, stop_reason, error, finished_at, id],
    ).map_err(AcpError::internal)?;
    Ok(())
}

/// Mark a run as merged (sets `merged_at` timestamp).
pub fn set_merged(conn: &Connection, id: &str, merged_at: &str) -> Result<(), AcpError> {
    conn.execute(
        "UPDATE agent_runs SET merged_at = ?1 WHERE id = ?2",
        params![merged_at, id],
    )
    .map_err(AcpError::internal)?;
    Ok(())
}

/// List all active runs.
pub fn list_active(conn: &Connection) -> Result<Vec<AgentRun>, AcpError> {
    list_runs_with_filter(conn, "status IN ('pending', 'running')")
}

/// List recent runs (newest first).
pub fn list_recent(conn: &Connection, limit: i64) -> Result<Vec<AgentRun>, AcpError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {RUN_COLUMNS} FROM agent_runs ORDER BY created_at DESC LIMIT ?1"
        ))
        .map_err(AcpError::internal)?;
    let rows = stmt
        .query_map(params![limit], row_to_run)
        .map_err(AcpError::internal)?;
    let mut runs = Vec::new();
    for r in rows {
        runs.push(r.map_err(AcpError::internal)?);
    }
    Ok(runs)
}

/// Check if a card has an active (locked) run.
/// Fail-closed: on query error, returns `true` (locked) so a transient
/// DB failure can't allow a second concurrent run on the same card.
pub fn is_card_locked(conn: &Connection, card_id: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_runs WHERE card_id = ?1 AND status IN ('pending', 'running'))",
        params![card_id],
        |r| r.get::<_, i64>(0),
    ).map(|v| v != 0).unwrap_or(true)
}

/// Count all runs for an agent (any status). Used by `acp_delete_agent`
/// to decide between the structured `AgentHasRuns` error and a clean
/// delete (or cascade when `delete_runs=true`).
pub fn count_runs_for_agent(conn: &Connection, agent_name: &str) -> Result<i64, AcpError> {
    conn.query_row(
        "SELECT COUNT(*) FROM agent_runs WHERE agent_name = ?1",
        params![agent_name],
        |r| r.get::<_, i64>(0),
    )
    .map_err(AcpError::internal)
}

/// List all runs for an agent (any status). Used by `acp_delete_agent`
/// cascade to remove each run's worktree before deleting the run rows.
pub fn list_runs_for_agent(conn: &Connection, agent_name: &str) -> Result<Vec<AgentRun>, AcpError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {RUN_COLUMNS} FROM agent_runs WHERE agent_name = ?1"
        ))
        .map_err(AcpError::internal)?;
    let rows = stmt
        .query_map(params![agent_name], row_to_run)
        .map_err(AcpError::internal)?;
    let mut runs = Vec::new();
    for r in rows {
        runs.push(r.map_err(AcpError::internal)?);
    }
    Ok(runs)
}

fn list_runs_with_filter(conn: &Connection, filter: &str) -> Result<Vec<AgentRun>, AcpError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {RUN_COLUMNS} FROM agent_runs WHERE {filter}"
        ))
        .map_err(AcpError::internal)?;
    let rows = stmt.query_map([], row_to_run).map_err(AcpError::internal)?;
    let mut runs = Vec::new();
    for r in rows {
        runs.push(r.map_err(AcpError::internal)?);
    }
    Ok(runs)
}

fn row_to_run(r: &rusqlite::Row) -> rusqlite::Result<AgentRun> {
    let skills_json: String = r.get(12)?;
    Ok(AgentRun {
        id: r.get(0)?,
        card_id: r.get(1)?,
        agent_name: r.get(2)?,
        session_id: r.get(3)?,
        worktree_path: r.get(4)?,
        branch: r.get(5)?,
        repo_root: r.get(6)?,
        status: r.get(7)?,
        output: r.get(8)?,
        stop_reason: r.get(9)?,
        error: r.get(10)?,
        merged_at: r.get(11)?,
        skills: parse_skills_json(&skills_json),
        created_at: r.get(13)?,
        finished_at: r.get(14)?,
    })
}

/// Persist the worktree path, branch, and repo root for a run (used after
/// the worktree is created, to replace the placeholder values from
/// `insert_run`). Distinct from `update_worktree_branch`, which also flips
/// `status` and predates `repo_root`.
pub fn set_worktree_info(
    conn: &Connection,
    id: &str,
    worktree_path: &str,
    branch: &str,
    repo_root: &str,
) -> Result<(), AcpError> {
    conn.execute(
        "UPDATE agent_runs SET worktree_path = ?1, branch = ?2, repo_root = ?3
         WHERE id = ?4",
        params![worktree_path, branch, repo_root, id],
    )
    .map_err(AcpError::internal)?;
    Ok(())
}

/// Check whether any card linked to `tree_source_id` has an active
/// (pending/running) agent run. Used to block tree-source deletion while a
/// run is in flight, mirroring the per-card lock in [`is_card_locked`].
pub fn is_tree_source_locked(conn: &Connection, tree_source_id: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM cards c
            JOIN agent_runs r ON r.card_id = c.id
            WHERE c.tree_source_id = ?1
              AND r.status IN ('pending', 'running')
        )",
        params![tree_source_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|v| v != 0)
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{get_run, insert_run, is_tree_source_locked, set_worktree_info};
    use crate::db::agents;
    use crate::db::test_db;
    use rusqlite::{params, Connection};

    fn insert_tree_source(conn: &Connection, id: &str, path: &str) {
        conn.execute(
            "INSERT INTO tree_sources (id, label, path, editor_command, created_at)
             VALUES (?1, ?2, ?3, NULL, '2026-01-01T00:00:00Z')",
            params![id, id, path],
        )
        .unwrap();
    }

    fn insert_card(conn: &Connection, id: &str, tree_source_id: Option<&str>) {
        conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, tree_source_id, created_at, updated_at)
               VALUES (?1, ?2, 'backlog', 'local', 0, ?3, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
            params![id, id, tree_source_id],
        )
        .unwrap();
    }

    fn insert_agent(conn: &Connection) {
        agents::insert_agent(conn, "claude", "echo", "Test", false, true, &[]).unwrap();
    }

    #[test]
    fn insert_run_sets_repo_root_from_tree_source() {
        let conn = test_db();
        insert_tree_source(&conn, "ts-1", "/tmp/myrepo");
        insert_card(&conn, "c-1", Some("ts-1"));
        insert_agent(&conn);
        insert_run(
            &conn,
            "r-1",
            "c-1",
            "claude",
            "placeholder",
            "placeholder",
            "pending",
            &[],
        )
        .unwrap();

        let run = get_run(&conn, "r-1").unwrap().unwrap();
        assert_eq!(run.repo_root, Some("/tmp/myrepo".to_string()));
    }

    #[test]
    fn insert_run_repo_root_null_when_no_tree_source() {
        let conn = test_db();
        insert_card(&conn, "c-1", None);
        insert_agent(&conn);
        insert_run(
            &conn,
            "r-1",
            "c-1",
            "claude",
            "placeholder",
            "placeholder",
            "pending",
            &[],
        )
        .unwrap();

        let run = get_run(&conn, "r-1").unwrap().unwrap();
        assert_eq!(run.repo_root, None);
    }

    #[test]
    fn set_worktree_info_writes_fields() {
        let conn = test_db();
        insert_card(&conn, "c-1", None);
        insert_agent(&conn);
        insert_run(
            &conn,
            "r-1",
            "c-1",
            "claude",
            "placeholder",
            "placeholder",
            "pending",
            &[],
        )
        .unwrap();

        set_worktree_info(&conn, "r-1", "/actual/wt", "feature-branch", "/repo/root").unwrap();

        let run = get_run(&conn, "r-1").unwrap().unwrap();
        assert_eq!(run.worktree_path, "/actual/wt");
        assert_eq!(run.branch, "feature-branch");
        assert_eq!(run.repo_root, Some("/repo/root".to_string()));
    }

    #[test]
    fn is_tree_source_locked_true() {
        let conn = test_db();
        insert_tree_source(&conn, "ts-1", "/tmp/myrepo");
        insert_card(&conn, "c-1", Some("ts-1"));
        insert_agent(&conn);
        insert_run(
            &conn,
            "r-1",
            "c-1",
            "claude",
            "placeholder",
            "placeholder",
            "running",
            &[],
        )
        .unwrap();

        assert!(is_tree_source_locked(&conn, "ts-1"));
    }

    #[test]
    fn is_tree_source_locked_false() {
        let conn = test_db();
        insert_tree_source(&conn, "ts-1", "/tmp/myrepo");
        insert_card(&conn, "c-1", Some("ts-1"));
        insert_agent(&conn);
        insert_run(
            &conn,
            "r-1",
            "c-1",
            "claude",
            "placeholder",
            "placeholder",
            "completed",
            &[],
        )
        .unwrap();

        assert!(!is_tree_source_locked(&conn, "ts-1"));
    }
}
