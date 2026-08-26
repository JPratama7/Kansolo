use rusqlite::Connection;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

pub mod cards;
pub mod settings;

/// Open a SQLite connection at `db_path` and apply WAL + busy_timeout pragmas.
pub fn open_db_path(db_path: &PathBuf) -> Result<Connection, String> {
    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
    conn.pragma_update(None, "journal_mode", "WAL").map_err(|e| e.to_string())?;
    conn.pragma_update(None, "busy_timeout", 5000).map_err(|e| e.to_string())?;
    conn.pragma_update(None, "foreign_keys", "ON").map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Resolve `{app_config_dir}/tasker.db` and open it.
pub fn open_db<R: Runtime>(app: &AppHandle<R>) -> Result<Connection, String> {
    let mut db_path = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    db_path.push("tasker.db");
    open_db_path(&db_path)
}

/// ISO-8601 UTC timestamp. Avoids pulling in chrono for one call site —
/// formats the timestamp manually from `SystemTime` using the
/// civil-from-days algorithm (Howard Hinnant).
pub fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Good enough for a timestamp column; not calendar-correct across leap
    // seconds, which SQLite does not care about anyway.
    let days = secs / 86400;
    let rem = secs % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    // Civil-from-days algorithm (Howard Hinnant). days since 1970-01-01.
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Highest `position` value among cards in `column`, or 0 if the column is empty.
pub fn max_position(conn: &Connection, column: &str) -> Result<i64, String> {
    conn.query_row(
        &format!(
            "SELECT COALESCE(MAX(position), 0) FROM cards WHERE \"{}\" = ?1",
            column
        ),
        rusqlite::params![column],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

/// Raw row from the `cards` table — mirrors the schema 1:1.
pub struct CardRow {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub column: String,
    pub source: String,
    pub position: i64,
    pub source_ref: Option<String>,
    pub source_status: Option<String>,
    pub tree_source_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// API-facing card representation. `description` and `priority` are
/// promoted to plain `String` (with sensible defaults) and `tree_source_id`
/// serializes as `treeSourceId`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub id: String,
    pub title: String,
    pub description: String,
    pub priority: String,
    pub column: String,
    pub source: String,
    pub position: i64,
    pub source_ref: Option<String>,
    pub source_status: Option<String>,
    pub tree_source_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<CardRow> for Card {
    fn from(r: CardRow) -> Self {
        Card {
            id: r.id,
            title: r.title,
            description: r.description.unwrap_or_default(),
            priority: r
                .priority
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| "medium".to_string()),
            column: r.column,
            source: r.source,
            position: r.position,
            source_ref: r.source_ref,
            source_status: r.source_status,
            tree_source_id: r.tree_source_id,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

/// Snapshot of an external issue (Jira, GitHub, etc.) mirrored into the
/// cards table. `source` identifies the originating system, while
/// `source_ref`/`source_status` carry the system-native id and status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalSnapshot {
    pub source: String,
    pub source_ref: String,
    pub title: String,
    pub description: String,
    pub priority: String,
    pub source_status: String,
    pub column: String,
    pub synced_at: String,
}

/// Re-export StatusMapping from the mapping module so callers that import
/// `db::StatusMapping` keep working.
pub use crate::mapping::StatusMapping;

/// A user-registered external source instance (Jira project, GitHub repo,
/// etc.) with its config and status-to-column mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInstance {
    pub id: String,
    pub source_type: String,
    pub label: String,
    pub config: serde_json::Value,
    pub status_mapping: StatusMapping,
    pub enabled: bool,
    pub created_at: String,
}

/// A user-registered file/tree source for card creation.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeSource {
    pub id: String,
    pub label: String,
    pub path: String,
    pub editor_command: Option<String>,
}

/// Ordered migration SQL embedded at compile time.
const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../../migrations/0001_init.sql")),
    (2, include_str!("../../migrations/0002_add_priority.sql")),
    (3, include_str!("../../migrations/0003_jira_snapshot.sql")),
    (4, include_str!("../../migrations/0004_add_source_path.sql")),
    (5, include_str!("../../migrations/0005_tree_sources.sql")),
    (6, include_str!("../../migrations/0006_tree_source_editor.sql")),
    (7, include_str!("../../migrations/0007_cards_priority_index.sql")),
    (8, include_str!("../../migrations/0008_plugin_sources.sql")),
    (9, include_str!("../../migrations/0009_drop_legacy_jira.sql")),
    (10, include_str!("../../migrations/0010_tree_source_id_fk.sql")),
];

/// Apply pending DB migrations via rusqlite. Idempotent — skips already-applied
/// versions. Imports legacy `_sqlx_migrations` records on first run so existing
/// DBs from the tauri-plugin-sql era don't re-apply migrations 1-7.
pub fn run_migrations(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );",
    )
    .map_err(|e| e.to_string())?;

    // Import from the legacy sqlx table if present (tauri-plugin-sql era).
    let _ = conn.execute_batch(
        "INSERT OR IGNORE INTO _migrations (version, applied_at)
         SELECT version, installed_on
         FROM _sqlx_migrations
         WHERE success = 1;",
    );

    for (version, sql) in MIGRATIONS {
        let applied: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM _migrations WHERE version = ?1)",
                [version],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !applied {
            conn.execute_batch(sql)
                .map_err(|e| format!("migration {version} failed: {e}"))?;
            conn.execute(
                "INSERT INTO _migrations (version, applied_at) VALUES (?1, ?2)",
                rusqlite::params![version, now_iso()],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod fk_tests {
    use rusqlite::Connection;

    /// Build a fresh in-memory DB with the tree_sources + cards tables
    /// matching migration 0010's schema (FK + CHECK + defaults).
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
                 created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 FOREIGN KEY (tree_source_id) REFERENCES tree_sources(id) ON DELETE RESTRICT
               );"#,
        ).unwrap();
        conn
    }

    #[test]
    fn foreign_keys_pragma_is_on() {
        let conn = test_db();
        let fk_on: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk_on, 1, "PRAGMA foreign_keys must be ON");
    }

    #[test]
    fn valid_tree_source_id_insert_succeeds() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO tree_sources (id, label, path, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["ts-1", "My Source", "/some/path", "2026-01-01T00:00:00Z"],
        ).unwrap();
        conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, tree_source_id, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
            rusqlite::params!["c-1", "Test card", "backlog", "local", 1, "ts-1", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"],
        ).unwrap();
    }

    #[test]
    fn orphan_tree_source_id_insert_rejected_by_fk() {
        let conn = test_db();
        let result = conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, tree_source_id, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
            rusqlite::params!["c-1", "Test card", "backlog", "local", 1, "nonexistent-id", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"],
        );
        assert!(result.is_err(), "FK should reject insert with non-existent tree_source_id");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("FOREIGN KEY") || err.contains("foreign key") || err.contains("constraint"),
            "error should mention FK constraint, got: {err}");
    }

    #[test]
    fn delete_pre_check_blocks_when_cards_linked() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO tree_sources (id, label, path, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["ts-1", "My Source", "/some/path", "2026-01-01T00:00:00Z"],
        ).unwrap();
        conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, tree_source_id, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
            rusqlite::params!["c-1", "Test card", "backlog", "local", 1, "ts-1", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"],
        ).unwrap();
        let result = crate::db::settings::ensure_tree_source_deletable(&conn, "ts-1");
        assert!(result.is_err(), "should block deletion when card is linked");
        let err = result.unwrap_err();
        assert!(err.contains("Cannot delete tree source"), "error should be user-friendly, got: {err}");
        assert!(err.contains("1 card(s)"), "error should mention count, got: {err}");
    }

    #[test]
    fn delete_pre_check_allows_when_no_cards_linked() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO tree_sources (id, label, path, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["ts-1", "My Source", "/some/path", "2026-01-01T00:00:00Z"],
        ).unwrap();
        let result = crate::db::settings::ensure_tree_source_deletable(&conn, "ts-1");
        assert!(result.is_ok(), "should allow deletion when no cards linked");
    }

    #[test]
    fn clean_delete_succeeds_after_unlink() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO tree_sources (id, label, path, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["ts-1", "My Source", "/some/path", "2026-01-01T00:00:00Z"],
        ).unwrap();
        conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, tree_source_id, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
            rusqlite::params!["c-1", "Test card", "backlog", "local", 1, "ts-1", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"],
        ).unwrap();
        // Unlink the card
        conn.execute("UPDATE cards SET tree_source_id = NULL WHERE id = ?1", rusqlite::params!["c-1"]).unwrap();
        // Pre-check now passes
        crate::db::settings::ensure_tree_source_deletable(&conn, "ts-1").unwrap();
        // Delete succeeds
        conn.execute("DELETE FROM tree_sources WHERE id = ?1", rusqlite::params!["ts-1"]).unwrap();
    }
}
