use crate::db::now_iso;
use crate::error::AcpError;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

/// Raw row from the `agents` table.
pub struct AgentRow {
    pub name: String,
    pub command: String,
    pub description: String,
    pub built_in: bool,
    pub enabled: bool,
    pub skills_json: String,
    pub created_at: String,
}

/// API-facing agent representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub name: String,
    pub command: String,
    pub description: String,
    pub built_in: bool,
    pub enabled: bool,
    pub skills: Vec<String>,
    pub created_at: String,
}

impl From<AgentRow> for Agent {
    fn from(r: AgentRow) -> Self {
        let skills = parse_skills_json(&r.skills_json);
        Agent {
            name: r.name,
            command: r.command,
            description: r.description,
            built_in: r.built_in,
            enabled: r.enabled,
            skills,
            created_at: r.created_at,
        }
    }
}

/// Parse skills_json string into Vec<String>.
pub fn parse_skills_json(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

/// Serialize Vec<String> into skills_json string.
pub fn serialize_skills(skills: &[String]) -> String {
    serde_json::to_string(skills).unwrap_or_else(|_| "[]".to_string())
}

/// Insert a new agent.
pub fn insert_agent(
    conn: &Connection,
    name: &str,
    command: &str,
    description: &str,
    built_in: bool,
    enabled: bool,
    skills: &[String],
) -> Result<(), AcpError> {
    conn.execute(
        "INSERT INTO agents (name, command, description, built_in, enabled, skills_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![name, command, description, built_in as i64, enabled as i64, serialize_skills(skills), now_iso()],
    ).map_err(AcpError::internal)?;
    Ok(())
}

/// Get a single agent by name.
pub fn get_agent(conn: &Connection, name: &str) -> Result<Option<Agent>, AcpError> {
    let row = conn.query_row(
        "SELECT name, command, description, built_in, enabled, skills_json, created_at
         FROM agents WHERE name = ?1",
        params![name],
        |r| {
            Ok(AgentRow {
                name: r.get(0)?,
                command: r.get(1)?,
                description: r.get(2)?,
                built_in: r.get::<_, i64>(3)? != 0,
                enabled: r.get::<_, i64>(4)? != 0,
                skills_json: r.get(5)?,
                created_at: r.get(6)?,
            })
        },
    );
    match row {
        Ok(r) => Ok(Some(Agent::from(r))),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(AcpError::internal(e.to_string())),
    }
}

/// List all agents ordered by name.
pub fn list_agents(conn: &Connection) -> Result<Vec<Agent>, AcpError> {
    let mut stmt = conn
        .prepare("SELECT name, command, description, built_in, enabled, skills_json, created_at FROM agents ORDER BY name ASC")
        .map_err(AcpError::internal)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(AgentRow {
                name: r.get(0)?,
                command: r.get(1)?,
                description: r.get(2)?,
                built_in: r.get::<_, i64>(3)? != 0,
                enabled: r.get::<_, i64>(4)? != 0,
                skills_json: r.get(5)?,
                created_at: r.get(6)?,
            })
        })
        .map_err(AcpError::internal)?;
    let mut agents = Vec::new();
    for r in rows {
        agents.push(Agent::from(r.map_err(AcpError::internal)?));
    }
    Ok(agents)
}

/// Update an agent's command, description, and skills.
pub fn update_agent(
    conn: &Connection,
    name: &str,
    command: &str,
    description: &str,
    skills: &[String],
) -> Result<(), AcpError> {
    conn.execute(
        "UPDATE agents SET command = ?1, description = ?2, skills_json = ?3 WHERE name = ?4",
        params![command, description, serialize_skills(skills), name],
    )
    .map_err(AcpError::internal)?;
    Ok(())
}

/// Delete an agent. RESTRICT by default (fails if runs exist).
/// With `delete_runs=true`, cascades by deleting runs first.
pub fn delete_agent(conn: &Connection, name: &str, delete_runs: bool) -> Result<(), AcpError> {
    if delete_runs {
        conn.execute(
            "DELETE FROM agent_runs WHERE agent_name = ?1",
            params![name],
        )
        .map_err(AcpError::internal)?;
    }
    conn.execute("DELETE FROM agents WHERE name = ?1", params![name])
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("FOREIGN KEY") || msg.contains("constraint") {
                AcpError::conflict(format!("Cannot delete agent '{name}': agent_runs still exist. Pass delete_runs=true to cascade."))
            } else {
                AcpError::internal(msg)
            }
        })?;
    Ok(())
}

/// Upsert an agent (insert or replace).
pub fn upsert_agent(
    conn: &Connection,
    name: &str,
    command: &str,
    description: &str,
    built_in: bool,
    enabled: bool,
    skills: &[String],
) -> Result<(), AcpError> {
    conn.execute(
        "INSERT INTO agents (name, command, description, built_in, enabled, skills_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(name) DO UPDATE SET
           command = ?2, description = ?3, built_in = ?4, enabled = ?5, skills_json = ?6",
        params![name, command, description, built_in as i64, enabled as i64, serialize_skills(skills), now_iso()],
    ).map_err(AcpError::internal)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_db;

    fn insert_test_card(conn: &Connection, id: &str) {
        conn.execute(
            r#"INSERT INTO cards (id, title, description, priority, "column", source, position, created_at, updated_at)
               VALUES (?1, 'Test', 'desc', 'medium', 'backlog', 'local', 0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
            params![id],
        ).unwrap();
    }

    #[test]
    fn agent_skills_round_trip() {
        let conn = test_db();
        insert_agent(
            &conn,
            "my-agent",
            "echo hi",
            "Test agent",
            false,
            true,
            &["tdd".to_string(), "code-review".to_string()],
        )
        .unwrap();
        let agent = get_agent(&conn, "my-agent").unwrap().unwrap();
        assert_eq!(agent.skills, vec!["tdd", "code-review"]);
        update_agent(
            &conn,
            "my-agent",
            "echo bye",
            "Updated",
            &["tdd".to_string()],
        )
        .unwrap();
        let agent = get_agent(&conn, "my-agent").unwrap().unwrap();
        assert_eq!(agent.skills, vec!["tdd"]);
        assert_eq!(agent.command, "echo bye");
    }

    #[test]
    fn delete_agent_restrict_without_runs_flag() {
        let conn = test_db();
        insert_test_card(&conn, "c-1");
        insert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        crate::db::agent_runs::insert_run(
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
        let result = delete_agent(&conn, "my-agent", false);
        assert!(
            result.is_err(),
            "delete should be restricted while runs exist"
        );
        delete_agent(&conn, "my-agent", true).unwrap();
        assert!(get_agent(&conn, "my-agent").unwrap().is_none());
    }

    #[test]
    fn upsert_agent_inserts_and_updates() {
        let conn = test_db();
        upsert_agent(&conn, "my-agent", "echo hi", "Test", false, true, &[]).unwrap();
        let agent = get_agent(&conn, "my-agent").unwrap().unwrap();
        assert_eq!(agent.command, "echo hi");
        upsert_agent(
            &conn,
            "my-agent",
            "echo bye",
            "Updated",
            false,
            true,
            &["tdd".to_string()],
        )
        .unwrap();
        let agent = get_agent(&conn, "my-agent").unwrap().unwrap();
        assert_eq!(agent.command, "echo bye");
        assert_eq!(agent.description, "Updated");
        assert_eq!(agent.skills, vec!["tdd"]);
    }

    #[test]
    fn list_agents_ordered_by_name() {
        let conn = test_db();
        insert_agent(&conn, "zebra", "echo z", "Z", false, true, &[]).unwrap();
        insert_agent(&conn, "alpha", "echo a", "A", false, true, &[]).unwrap();
        let agents = list_agents(&conn).unwrap();
        // Migration 0011 seeds the built-in claude-code agent.
        let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "claude-code", "zebra"]);
    }
}
