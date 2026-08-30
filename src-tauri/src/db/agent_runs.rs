use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use crate::db::now_iso;
use crate::error::AcpError;
use crate::db::agents::{parse_skills_json, serialize_skills};

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
    pub status: String,
    pub output: Option<String>,
    pub stop_reason: Option<String>,
    pub error: Option<String>,
    pub pid: Option<i64>,
    pub pgid: Option<i64>,
    pub merged_at: Option<String>,
    pub skills: Vec<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

/// Insert a new agent run.
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
        "INSERT INTO agent_runs (id, card_id, agent_name, worktree_path, branch, status, skills_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, card_id, agent_name, worktree_path, branch, status, serialize_skills(skills), now_iso()],
    ).map_err(AcpError::internal)?;
    Ok(())
}

/// Get the active run for a card, if any.
pub fn get_active_run(conn: &Connection, card_id: &str) -> Result<Option<AgentRun>, AcpError> {
    let row = conn.query_row(
        "SELECT id, card_id, agent_name, session_id, worktree_path, branch, status,
                output, stop_reason, error, pid, pgid, merged_at, skills_json, created_at, finished_at
         FROM agent_runs WHERE card_id = ?1 AND status IN ('pending', 'running') LIMIT 1",
        params![card_id],
        row_to_run,
    );
    match row {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(AcpError::internal(e.to_string())),
    }
}

/// Get a single run by id.
pub fn get_run(conn: &Connection, id: &str) -> Result<Option<AgentRun>, AcpError> {
    let row = conn.query_row(
        "SELECT id, card_id, agent_name, session_id, worktree_path, branch, status,
                output, stop_reason, error, pid, pgid, merged_at, skills_json, created_at, finished_at
         FROM agent_runs WHERE id = ?1",
        params![id],
        row_to_run,
    );
    match row {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(AcpError::internal(e.to_string())),
    }
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
    ).map_err(AcpError::internal)?;
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
            "SELECT id, card_id, agent_name, session_id, worktree_path, branch, status,
                    output, stop_reason, error, pid, pgid, merged_at, skills_json, created_at, finished_at
             FROM agent_runs ORDER BY created_at DESC LIMIT ?1"
        ))
        .map_err(AcpError::internal)?;
    let rows = stmt.query_map(params![limit], row_to_run).map_err(AcpError::internal)?;
    let mut runs = Vec::new();
    for r in rows {
        runs.push(r.map_err(AcpError::internal)?);
    }
    Ok(runs)
}

/// Check if a card has an active (locked) run.
pub fn is_card_locked(conn: &Connection, card_id: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_runs WHERE card_id = ?1 AND status IN ('pending', 'running'))",
        params![card_id],
        |r| r.get::<_, i64>(0),
    ).map(|v| v != 0).unwrap_or(false)
}

fn list_runs_with_filter(conn: &Connection, filter: &str) -> Result<Vec<AgentRun>, AcpError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT id, card_id, agent_name, session_id, worktree_path, branch, status,
                    output, stop_reason, error, pid, pgid, merged_at, skills_json, created_at, finished_at
             FROM agent_runs WHERE {filter}"
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
    let skills_json: String = r.get(13)?;
    Ok(AgentRun {
        id: r.get(0)?,
        card_id: r.get(1)?,
        agent_name: r.get(2)?,
        session_id: r.get(3)?,
        worktree_path: r.get(4)?,
        branch: r.get(5)?,
        status: r.get(6)?,
        output: r.get(7)?,
        stop_reason: r.get(8)?,
        error: r.get(9)?,
        pid: r.get(10)?,
        pgid: r.get(11)?,
        merged_at: r.get(12)?,
        skills: parse_skills_json(&skills_json),
        created_at: r.get(14)?,
        finished_at: r.get(15)?,
    })
}
