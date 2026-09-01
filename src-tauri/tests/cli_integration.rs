//! Integration tests for the tasker-agent CLI's library dependencies.
//!
//! These tests verify the DB/skill/runner operations that the CLI binary
//! relies on, using the same in-memory test DB pattern as the inline tests.
//! The CLI binary itself is a thin wrapper over `kansolo_lib` functions.

use kansolo_lib::db::{agent_runs, agents, cards, now_iso};
use kansolo_lib::error::AcpError;
use kansolo_lib::runner;
use kansolo_lib::skills;
use rusqlite::Connection;

/// Build a fresh in-memory DB with all tables (mirrors db::test_db).
fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    conn.execute_batch(
        r#"CREATE TABLE tree_sources (
             id TEXT PRIMARY KEY,
             label TEXT NOT NULL,
             path TEXT NOT NULL,
             editor_command TEXT,
             created_at TEXT NOT NULL
           );
           CREATE TABLE cards (
             id TEXT PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT DEFAULT '',
             "column" TEXT NOT NULL CHECK ("column" IN ('backlog','ongoing','done')),
             source TEXT NOT NULL DEFAULT 'local',
             position INTEGER NOT NULL DEFAULT 0,
             priority TEXT NOT NULL DEFAULT 'medium',
             source_ref TEXT,
             source_status TEXT,
             tree_source_id TEXT,
            source_instance_id TEXT,
             created_at TEXT NOT NULL,
             updated_at TEXT NOT NULL,
             FOREIGN KEY (tree_source_id) REFERENCES tree_sources(id) ON DELETE RESTRICT
           );
           CREATE TABLE agents (
             name TEXT PRIMARY KEY,
             command TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             built_in INTEGER NOT NULL DEFAULT 0,
             enabled INTEGER NOT NULL DEFAULT 1,
             skills_json TEXT NOT NULL DEFAULT '[]',
             created_at TEXT NOT NULL
           );
           CREATE TABLE agent_runs (
             id TEXT PRIMARY KEY,
             card_id TEXT NOT NULL,
             agent_name TEXT NOT NULL,
             session_id TEXT,
             worktree_path TEXT NOT NULL,
             branch TEXT NOT NULL,
             repo_root TEXT,
             status TEXT NOT NULL,
             output TEXT,
             stop_reason TEXT,
             error TEXT,
             merged_at TEXT,
             skills_json TEXT NOT NULL DEFAULT '[]',
             created_at TEXT NOT NULL,
             finished_at TEXT,
             FOREIGN KEY (card_id) REFERENCES cards(id) ON DELETE CASCADE,
             FOREIGN KEY (agent_name) REFERENCES agents(name) ON DELETE RESTRICT
           );
           CREATE INDEX idx_agent_runs_card ON agent_runs(card_id);
           CREATE UNIQUE INDEX idx_agent_runs_one_active
             ON agent_runs(card_id)
             WHERE status IN ('pending','running');
           CREATE TABLE settings (
             key TEXT PRIMARY KEY,
             value TEXT NOT NULL
           );
           CREATE TABLE external_snapshots (
             source TEXT NOT NULL,
             source_ref TEXT NOT NULL,
             title TEXT NOT NULL,
             description TEXT NOT NULL,
             priority TEXT NOT NULL,
             source_status TEXT NOT NULL,
             "column" TEXT NOT NULL,
             synced_at TEXT NOT NULL,
             PRIMARY KEY (source, source_ref)
           );
           CREATE TABLE sources (
             id TEXT PRIMARY KEY,
             source_type TEXT NOT NULL,
             label TEXT NOT NULL,
             config_json TEXT NOT NULL,
             status_mapping_json TEXT NOT NULL,
             enabled INTEGER NOT NULL DEFAULT 1,
             created_at TEXT NOT NULL
           );"#,
    )
    .unwrap();
    conn
}

/// Insert a local card (mirrors CLI's cmd_run). The repo path is resolved
/// from the card's `tree_source_id` at run time, not stored on the card.
fn insert_card(conn: &rusqlite::Connection, id: &str) {
    conn.execute(
        r#"INSERT INTO cards (id, title, description, priority, "column", source, position, created_at, updated_at)
           VALUES (?1, 'Test Card', 'desc', 'medium', 'backlog', 'local', 0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
        rusqlite::params![id],
    ).unwrap();
}

// ── Agent CRUD (CLI `agents add/edit/remove/list`) ──

#[test]
fn cli_agents_add_and_list() {
    let conn = test_db();
    agents::insert_agent(
        &conn,
        "my-agent",
        "/usr/bin/echo",
        "Test agent",
        false,
        true,
        &["tdd".to_string()],
    )
    .unwrap();
    let list = agents::list_agents(&conn).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "my-agent");
    assert_eq!(list[0].command, "/usr/bin/echo");
    assert_eq!(list[0].description, "Test agent");
    assert!(!list[0].built_in);
    assert!(list[0].enabled);
    assert_eq!(list[0].skills, vec!["tdd"]);
}

#[test]
fn cli_agents_add_built_in_claude_code() {
    let conn = test_db();
    // Built-in claude-code has empty command — allowed.
    agents::insert_agent(&conn, "claude-code", "", "Claude Code", true, true, &[]).unwrap();
    let agent = agents::get_agent(&conn, "claude-code").unwrap().unwrap();
    assert!(agent.built_in);
    assert_eq!(agent.command, "");
}

#[test]
fn cli_agents_edit_updates_command_and_skills() {
    let conn = test_db();
    agents::insert_agent(
        &conn,
        "dev",
        "cmd1",
        "desc1",
        false,
        true,
        &["s1".to_string()],
    )
    .unwrap();
    agents::update_agent(
        &conn,
        "dev",
        "cmd2",
        "desc2",
        &["s2".to_string(), "s3".to_string()],
    )
    .unwrap();
    let agent = agents::get_agent(&conn, "dev").unwrap().unwrap();
    assert_eq!(agent.command, "cmd2");
    assert_eq!(agent.description, "desc2");
    assert_eq!(agent.skills, vec!["s2", "s3"]);
}

#[test]
fn cli_agents_remove() {
    let conn = test_db();
    agents::insert_agent(&conn, "temp", "cmd", "", false, true, &[]).unwrap();
    agents::delete_agent(&conn, "temp", false).unwrap();
    assert!(agents::get_agent(&conn, "temp").unwrap().is_none());
}

#[test]
fn cli_agents_remove_blocked_if_runs_exist() {
    let conn = test_db();
    insert_card(&conn, "c-1");
    agents::insert_agent(&conn, "a", "cmd", "", false, true, &[]).unwrap();
    agent_runs::insert_run(
        &conn,
        "r-1",
        "c-1",
        "a",
        "/tmp/wt",
        "agent/c-1",
        "running",
        &[],
    )
    .unwrap();
    // delete_agent with delete_runs=false should fail.
    let result = agents::delete_agent(&conn, "a", false);
    assert!(result.is_err());
}

// ── Card validation (CLI cmd_run checks) ──

#[test]
fn cli_run_accepts_imported_card_with_tree_source() {
    let conn = test_db();
    // Imported (jira) card linked to a tree_source — the repo path is
    // resolved from tree_sources.path via tree_source_id at run time.
    conn.execute(
        r#"INSERT INTO tree_sources (id, label, path, editor_command, created_at)
           VALUES ('ts-1', 'My Repo', '/tmp/repo', NULL, '2026-01-01T00:00:00Z')"#,
        [],
    )
    .unwrap();
    conn.execute(
        r#"INSERT INTO cards (id, title, description, priority, "column", source, position, tree_source_id, created_at, updated_at)
           VALUES ('jira-1', 'Jira', 'do thing', 'medium', 'backlog', 'jira', 0, 'ts-1', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
        [],
    ).unwrap();
    let card = cards::get_card_by_id(&conn, "jira-1").unwrap().unwrap();
    assert_eq!(card.source, "jira");
    assert_eq!(card.tree_source_id.as_deref(), Some("ts-1"));
    // Runner resolves the repo path from the linked tree source.
    let path = cards::resolve_repo_path(&conn, &card).unwrap();
    assert_eq!(path, "/tmp/repo");
}

#[test]
fn cli_run_rejects_card_without_repo_or_tree_source() {
    let conn = test_db();
    insert_card(&conn, "c-norepo");
    let card = cards::get_card_by_id(&conn, "c-norepo").unwrap().unwrap();
    assert!(card.tree_source_id.is_none());
    // No tree_source_id => resolve_repo_path returns a validation error.
    let err = cards::resolve_repo_path(&conn, &card).unwrap_err();
    assert!(err.message.contains("tree_source_id"));
}

#[test]
fn cli_run_accepts_card_with_tree_source() {
    let conn = test_db();
    conn.execute(
        r#"INSERT INTO tree_sources (id, label, path, editor_command, created_at)
           VALUES ('ts-ok', 'My Repo', '/tmp/myrepo', NULL, '2026-01-01T00:00:00Z')"#,
        [],
    )
    .unwrap();
    conn.execute(
        r#"INSERT INTO cards (id, title, description, priority, "column", source, position, tree_source_id, created_at, updated_at)
           VALUES ('c-ok', 'Card', 'desc', 'medium', 'backlog', 'local', 0, 'ts-ok', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
        [],
    ).unwrap();
    let card = cards::get_card_by_id(&conn, "c-ok").unwrap().unwrap();
    assert_eq!(card.tree_source_id.as_deref(), Some("ts-ok"));
    let path = cards::resolve_repo_path(&conn, &card).unwrap();
    assert_eq!(path, "/tmp/myrepo");
}

#[test]
fn cli_run_resolves_repo_from_tree_source() {
    let conn = test_db();
    // Card with tree_source_id (no repo_path column anymore).
    conn.execute(
        r#"INSERT INTO tree_sources (id, label, path, editor_command, created_at)
           VALUES ('ts-1', 'My Repo', '/tmp/myrepo', NULL, '2026-01-01T00:00:00Z')"#,
        [],
    )
    .unwrap();
    conn.execute(
        r#"INSERT INTO cards (id, title, description, priority, "column", source, position, tree_source_id, created_at, updated_at)
           VALUES ('c-ts', 'Card', 'desc', 'medium', 'backlog', 'jira', 0, 'ts-1', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
        [],
    ).unwrap();
    let card = cards::get_card_by_id(&conn, "c-ts").unwrap().unwrap();
    assert_eq!(card.tree_source_id.as_deref(), Some("ts-1"));
    // Runner resolves repo_path from tree_source.
    let path = cards::resolve_repo_path(&conn, &card).unwrap();
    assert_eq!(path, "/tmp/myrepo");
}

// ── Run lifecycle (CLI run/status/cancel) ──

#[test]
fn cli_run_creates_run_and_locks_card() {
    let conn = test_db();
    insert_card(&conn, "c-1");
    agents::insert_agent(&conn, "a", "cmd", "", false, true, &[]).unwrap();
    agent_runs::insert_run(
        &conn,
        "r-1",
        "c-1",
        "a",
        "/tmp/wt",
        "agent/c-1",
        "running",
        &[],
    )
    .unwrap();
    assert!(agent_runs::is_card_locked(&conn, "c-1"));
}

#[test]
fn cli_run_card_lock_prevents_second_run() {
    let conn = test_db();
    insert_card(&conn, "c-1");
    agents::insert_agent(&conn, "a", "cmd", "", false, true, &[]).unwrap();
    agent_runs::insert_run(
        &conn,
        "r-1",
        "c-1",
        "a",
        "/tmp/wt",
        "agent/c-1",
        "running",
        &[],
    )
    .unwrap();
    assert!(agent_runs::is_card_locked(&conn, "c-1"));
    // CLI checks is_card_locked before creating a second run.
}

#[test]
fn cli_cancel_updates_status() {
    let conn = test_db();
    insert_card(&conn, "c-1");
    agents::insert_agent(&conn, "a", "cmd", "", false, true, &[]).unwrap();
    agent_runs::insert_run(
        &conn,
        "r-1",
        "c-1",
        "a",
        "/tmp/wt",
        "agent/c-1",
        "running",
        &[],
    )
    .unwrap();
    agent_runs::update_status(
        &conn,
        "r-1",
        "cancelled",
        None,
        Some("user_cancelled"),
        None,
        Some(&now_iso()),
    )
    .unwrap();
    let run = agent_runs::get_run(&conn, "r-1").unwrap().unwrap();
    assert_eq!(run.status, "cancelled");
    assert_eq!(run.stop_reason.as_deref(), Some("user_cancelled"));
    assert!(!agent_runs::is_card_locked(&conn, "c-1"));
}

#[test]
fn cli_status_lists_recent_runs() {
    let conn = test_db();
    insert_card(&conn, "c-1");
    agents::insert_agent(&conn, "a", "cmd", "", false, true, &[]).unwrap();
    agent_runs::insert_run(
        &conn,
        "r-1",
        "c-1",
        "a",
        "/tmp/wt",
        "agent/c-1",
        "completed",
        &[],
    )
    .unwrap();
    agent_runs::insert_run(
        &conn,
        "r-2",
        "c-1",
        "a",
        "/tmp/wt",
        "agent/c-1",
        "running",
        &[],
    )
    .unwrap();
    let runs = agent_runs::list_recent(&conn, 20).unwrap();
    assert_eq!(runs.len(), 2);
}

#[test]
fn cli_status_get_active_run() {
    let conn = test_db();
    insert_card(&conn, "c-1");
    agents::insert_agent(&conn, "a", "cmd", "", false, true, &[]).unwrap();
    agent_runs::insert_run(
        &conn,
        "r-1",
        "c-1",
        "a",
        "/tmp/wt",
        "agent/c-1",
        "running",
        &[],
    )
    .unwrap();
    let run = agent_runs::get_active_run(&conn, "c-1").unwrap().unwrap();
    assert_eq!(run.id, "r-1");
    assert_eq!(run.status, "running");
}

// ── Cleanup (CLI cleanup command) ──

#[test]
fn cli_cleanup_reaps_dangling_runs() {
    let conn = test_db();
    insert_card(&conn, "c-1");
    agents::insert_agent(&conn, "a", "cmd", "", false, true, &[]).unwrap();
    // Pending runs (no worktree yet) are reaped.
    agent_runs::insert_run(
        &conn,
        "r-1",
        "c-1",
        "a",
        "/tmp/pending",
        "agent/pending",
        "pending",
        &[],
    )
    .unwrap();
    let reaped = runner::cleanup_dangling(&conn).unwrap();
    assert_eq!(reaped, vec!["r-1"]);
    let run = agent_runs::get_run(&conn, "r-1").unwrap().unwrap();
    assert_eq!(run.status, "failed");

    // Running runs are left alone so they can be resumed.
    agent_runs::insert_run(
        &conn,
        "r-2",
        "c-1",
        "a",
        "/tmp/wt",
        "agent/c-1",
        "running",
        &[],
    )
    .unwrap();
    let reaped = runner::cleanup_dangling(&conn).unwrap();
    assert!(reaped.is_empty());
    let run = agent_runs::get_run(&conn, "r-2").unwrap().unwrap();
    assert_eq!(run.status, "running");
}

#[test]
fn cli_cleanup_noop_when_no_active() {
    let conn = test_db();
    let reaped = runner::cleanup_dangling(&conn).unwrap();
    assert!(reaped.is_empty());
}

// ── Skills (CLI skills command) ──

#[test]
fn cli_skills_list_returns_manifest() {
    let skills = skills::list_skills();
    // May be empty in test env (no ~/.config/devin/skills), but should not panic.
    for s in &skills {
        assert!(!s.name.is_empty());
    }
}

#[test]
fn cli_skills_load_filters_unknown() {
    // Loading unknown skill names returns empty entries (graceful).
    let loaded = skills::load_skills(&["nonexistent-skill-xyz".to_string()]);
    assert!(loaded.is_empty());
}

#[test]
fn cli_build_skills_section_empty_for_no_skills() {
    assert_eq!(runner::build_skills_section(&[]), "");
}

#[test]
fn cli_build_skills_section_formats_known_skills() {
    let skills = vec![("tdd".to_string(), "Write tests first".to_string())];
    let section = runner::build_skills_section(&skills);
    assert!(section.starts_with("# Preloaded skills"));
    assert!(section.contains("## tdd"));
    assert!(section.contains("Write tests first"));
}

// ── Merge (CLI merge command) ──

#[test]
fn cli_merge_sets_merged_at() {
    let conn = test_db();
    insert_card(&conn, "c-1");
    agents::insert_agent(&conn, "a", "cmd", "", false, true, &[]).unwrap();
    agent_runs::insert_run(
        &conn,
        "r-1",
        "c-1",
        "a",
        "/tmp/wt",
        "agent/c-1",
        "completed",
        &[],
    )
    .unwrap();
    agent_runs::set_merged(&conn, "r-1", &now_iso()).unwrap();
    let run = agent_runs::get_run(&conn, "r-1").unwrap().unwrap();
    assert!(run.merged_at.is_some());
}

// ── Error types (CLI error display) ──

#[test]
fn cli_error_validation_message() {
    let e = AcpError::validation("bad input");
    assert_eq!(e.message, "bad input");
}

#[test]
fn cli_error_not_found_message() {
    let e = AcpError::not_found("missing thing");
    assert_eq!(e.message, "missing thing");
}

#[test]
fn cli_error_locked_message() {
    let e = AcpError::locked("card is locked");
    assert_eq!(e.message, "card is locked");
}
