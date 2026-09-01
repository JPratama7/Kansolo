use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

pub mod agent_runs;
pub mod agents;
pub mod cards;
pub mod settings;

/// Open a SQLite connection at `db_path` and apply WAL + busy_timeout pragmas.
pub fn open_db_path(db_path: &PathBuf) -> Result<Connection, String> {
    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "busy_timeout", 5000)
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Resolve `{app_config_dir}/tasker.db` and open it.
pub fn open_db<R: Runtime>(app: &AppHandle<R>) -> Result<Connection, String> {
    let mut db_path = app.path().app_config_dir().map_err(|e| e.to_string())?;
    db_path.push("tasker.db");
    open_db_path(&db_path)
}

/// ISO-8601 UTC timestamp. Uses the `time` crate, already in the dep tree.
pub fn now_iso() -> String {
    use time::macros::format_description;
    use time::OffsetDateTime;
    const FMT: &[time::format_description::FormatItem<'_>] =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    OffsetDateTime::now_utc()
        .format(FMT)
        .expect("format UTC timestamp")
}

/// Validate that `column` is one of the three board columns. Defense-in-depth
/// guard used at every entry point that accepts a column from the API/sync
/// layer, so a hostile or typo'd value is rejected before reaching SQL.
pub fn validate_column(column: &str) -> Result<(), String> {
    match column {
        "backlog" | "ongoing" | "done" => Ok(()),
        _ => Err("column must be backlog, ongoing, or done".to_string()),
    }
}

/// Highest `position` value among cards in `column`, or 0 if the column is empty.
/// `column` is bound as the sole SQL parameter — the literal `"column"` column
/// name is hardcoded in the SQL string, so there is no string interpolation to
/// inject. `validate_column` rejects anything outside backlog/ongoing/done.
pub fn max_position(conn: &Connection, column: &str) -> Result<i64, String> {
    validate_column(column)?;
    conn.query_row(
        r#"SELECT COALESCE(MAX(position), 0) FROM cards WHERE "column" = ?1"#,
        rusqlite::params![column],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

/// Card representation, shared between the DB row and the API.
/// Nullable columns are defaulted on read so callers always get a `String`.
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
    pub source_instance_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Snapshot of an external issue (Jira, GitHub, etc.) mirrored into the
/// cards table. `source` identifies the originating system, while
/// `source_ref`/`source_status` carry the system-native id and status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalSnapshot {
    pub source_instance_id: String,
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

/// Read the last-synced snapshot for a `(source, source_ref)` pair, if any.
/// Inner helper that runs on a borrowed connection (works inside a
/// `Transaction` via deref coercion) so `sync_source` can do all its
/// reads/writes on one atomic transaction. Mirrors the `get_snapshot`
/// Tauri command in `db::settings`.
pub fn get_snapshot_inner(
    conn: &Connection,
    source_instance_id: &str,
    source_ref: &str,
) -> Result<Option<ExternalSnapshot>, String> {
    conn.query_row(
        r#"SELECT source_instance_id, source, source_ref, title, description, priority, source_status, "column", synced_at
           FROM external_snapshots WHERE source_instance_id = ?1 AND source_ref = ?2 LIMIT 1"#,
        rusqlite::params![source_instance_id, source_ref],
        |row| {
            Ok(ExternalSnapshot {
                source_instance_id: row.get(0)?,
                source: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                source_ref: row.get(2)?,
                title: row.get(3)?,
                description: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                priority: row
                    .get::<_, Option<String>>(5)?
                    .filter(|p| !p.is_empty())
                    .unwrap_or_else(|| "medium".to_string()),
                source_status: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                column: row.get(7)?,
                synced_at: row.get(8)?,
            })
        },
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other.to_string()),
    })
}

/// Upsert the snapshot row for a `(source, source_ref)` pair — the external
/// state at this sync instant. Inner helper that runs on a borrowed
/// connection (works inside a `Transaction` via deref coercion) so
/// `sync_source` can do all its reads/writes on one atomic transaction.
/// Mirrors the `save_snapshot` Tauri command in `db::settings`.
pub fn save_snapshot_inner(conn: &Connection, snap: &ExternalSnapshot) -> Result<(), String> {
    conn.execute(
        r#"INSERT INTO external_snapshots
             (source_instance_id, source, source_ref, title, description, priority, source_status, "column", synced_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
           ON CONFLICT(source_instance_id, source_ref) DO UPDATE SET
             source = ?2, title = ?4, description = ?5, priority = ?6, source_status = ?7,
             "column" = ?8, synced_at = ?9"#,
        rusqlite::params![
            snap.source_instance_id,
            snap.source,
            snap.source_ref,
            snap.title,
            snap.description,
            snap.priority,
            snap.source_status,
            snap.column,
            snap.synced_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

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
    (
        6,
        include_str!("../../migrations/0006_tree_source_editor.sql"),
    ),
    (
        7,
        include_str!("../../migrations/0007_cards_priority_index.sql"),
    ),
    (8, include_str!("../../migrations/0008_plugin_sources.sql")),
    (
        9,
        include_str!("../../migrations/0009_drop_legacy_jira.sql"),
    ),
    (
        10,
        include_str!("../../migrations/0010_tree_source_id_fk.sql"),
    ),
    (11, include_str!("../../migrations/0011_agents_runs.sql")),
    (12, include_str!("../../migrations/0012_drop_pid_pgid.sql")),
    (
        13,
        include_str!("../../migrations/0013_source_instance_id.sql"),
    ),
    (14, include_str!("../../migrations/0014_drop_repo_path.sql")),
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
            // Migrations manage their own transactions (some contain
            // BEGIN/COMMIT for multi-statement DDL). execute_batch runs
            // all statements in the batch; if one fails, the error is
            // surfaced and the migration is not recorded as applied.
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
pub fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.pragma_update(None, "busy_timeout", 5000).unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    run_migrations(&conn).unwrap();
    conn
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        assert!(
            result.is_err(),
            "FK should reject insert with non-existent tree_source_id"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("FOREIGN KEY")
                || err.contains("foreign key")
                || err.contains("constraint"),
            "error should mention FK constraint, got: {err}"
        );
    }

    #[test]
    fn delete_pre_check_blocks_when_cards_linked() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO tree_sources (id, label, path, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["ts-1", "My Source", "/some/path", "2026-01-01T00:00:00Z"],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, tree_source_id, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
            rusqlite::params!["c-1", "Test card", "backlog", "local", 1, "ts-1", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"],
        ).unwrap();
        let result = crate::db::settings::ensure_tree_source_deletable(&conn, "ts-1");
        assert!(result.is_err(), "should block deletion when card is linked");
        let err = result.unwrap_err();
        assert!(
            err.contains("Cannot delete tree source"),
            "error should be user-friendly, got: {err}"
        );
        assert!(
            err.contains("1 card(s)"),
            "error should mention count, got: {err}"
        );
    }

    #[test]
    fn delete_pre_check_allows_when_no_cards_linked() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO tree_sources (id, label, path, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["ts-1", "My Source", "/some/path", "2026-01-01T00:00:00Z"],
        )
        .unwrap();
        let result = crate::db::settings::ensure_tree_source_deletable(&conn, "ts-1");
        assert!(result.is_ok(), "should allow deletion when no cards linked");
    }

    #[test]
    fn clean_delete_succeeds_after_unlink() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO tree_sources (id, label, path, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["ts-1", "My Source", "/some/path", "2026-01-01T00:00:00Z"],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, tree_source_id, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
            rusqlite::params!["c-1", "Test card", "backlog", "local", 1, "ts-1", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"],
        ).unwrap();
        // Unlink the card
        conn.execute(
            "UPDATE cards SET tree_source_id = NULL WHERE id = ?1",
            rusqlite::params!["c-1"],
        )
        .unwrap();
        // Pre-check now passes
        crate::db::settings::ensure_tree_source_deletable(&conn, "ts-1").unwrap();
        // Delete succeeds
        conn.execute(
            "DELETE FROM tree_sources WHERE id = ?1",
            rusqlite::params!["ts-1"],
        )
        .unwrap();
    }
}

#[cfg(test)]
mod column_validation_tests {
    use super::{max_position, test_db, validate_column};

    #[test]
    fn validate_column_rejects_invalid_value() {
        let err = validate_column("pending").unwrap_err();
        assert_eq!(err, "column must be backlog, ongoing, or done");
    }

    #[test]
    fn validate_column_accepts_board_columns() {
        validate_column("backlog").unwrap();
        validate_column("ongoing").unwrap();
        validate_column("done").unwrap();
    }

    #[test]
    fn max_position_rejects_hostile_column_string() {
        // A pre-fix build interpolated `column` into the SQL, so this string
        // would have been an injection vector. Post-fix it is bound as a plain
        // parameter value AND rejected up-front by validate_column, so the
        // cards table is untouched and we get the friendly error.
        let conn = test_db();
        let hostile = r#"backlog"; DROP TABLE cards; --"#;
        let err = max_position(&conn, hostile).unwrap_err();
        assert_eq!(err, "column must be backlog, ongoing, or done");
        // Table still exists — no injection happened.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM cards", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn max_position_returns_zero_for_empty_column() {
        let conn = test_db();
        assert_eq!(max_position(&conn, "backlog").unwrap(), 0);
    }
}

#[cfg(test)]
mod migration_tests {
    use rusqlite::Connection;

    /// Build an in-memory DB with the PRE-0014 schema: cards WITH repo_path,
    /// agent_runs WITHOUT repo_root, plus tree_sources / sources / agents so
    /// every FK the migration touches is satisfiable. Mirrors the schema
    /// state immediately after migration 0013 (so running 0014 on this DB
    /// exercises the same path a real upgrading user would hit).
    fn pre_0014_db() -> Connection {
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
               CREATE TABLE sources (
                 id TEXT PRIMARY KEY,
                 source_type TEXT NOT NULL,
                 label TEXT NOT NULL,
                 config_json TEXT NOT NULL DEFAULT '{}',
                 status_mapping_json TEXT NOT NULL DEFAULT '{}',
                 enabled INTEGER NOT NULL DEFAULT 1,
                 created_at TEXT NOT NULL
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
                 repo_path TEXT,
                 created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 FOREIGN KEY (tree_source_id) REFERENCES tree_sources(id) ON DELETE RESTRICT,
                 FOREIGN KEY (source_instance_id) REFERENCES sources(id) ON DELETE SET NULL
               );
               CREATE INDEX idx_cards_priority_created_column
                 ON cards (priority, created_at, "column");
               CREATE TABLE agent_runs (
                 id TEXT PRIMARY KEY,
                 card_id TEXT NOT NULL,
                 agent_name TEXT NOT NULL,
                 session_id TEXT,
                 worktree_path TEXT NOT NULL,
                 branch TEXT NOT NULL,
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
                 ON agent_runs(card_id) WHERE status IN ('pending', 'running');"#,
        )
        .unwrap();
        conn
    }

    const MIGRATION_0014: &str = include_str!("../../migrations/0014_drop_repo_path.sql");

    #[test]
    fn migration_0014_backfill_handles_all_edge_cases() {
        let conn = pre_0014_db();

        // Existing tree source that card1 will match by path.
        conn.execute(
            "INSERT INTO tree_sources (id, label, path, editor_command, created_at)
             VALUES (?1, ?2, ?3, NULL, ?4)",
            rusqlite::params![
                "ts-existing",
                "Existing",
                "/existing/repo",
                "2026-01-01T00:00:00Z"
            ],
        )
        .unwrap();

        // Built-in agent so agent_runs FK is satisfiable.
        conn.execute(
            "INSERT INTO agents (name, command, description, built_in, enabled, skills_json, created_at)
             VALUES (?1, ?2, ?3, 1, 1, '[]', ?4)",
            rusqlite::params!["claude-code", "", "Claude Code via ACP", "2026-01-01T00:00:00Z"],
        )
        .unwrap();

        // Four cards covering every backfill branch:
        //   card1: repo_path matches existing tree_source, tree_source_id NULL  -> linked by path
        //   card2: repo_path matches no tree_source, tree_source_id NULL        -> new tree_source created + linked
        //   card3: repo_path set AND tree_source_id already set                  -> tree_source_id preserved
        //   card4: repo_path NULL, tree_source_id NULL                           -> stays NULL
        conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, priority,
                                  repo_path, tree_source_id, created_at, updated_at)
               VALUES (?1, ?2, ?3, 'local', 0, 'medium', ?4, NULL, ?5, ?5)"#,
            rusqlite::params![
                "card1",
                "Card 1",
                "backlog",
                "/existing/repo",
                "2026-01-01T00:00:00Z"
            ],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, priority,
                                  repo_path, tree_source_id, created_at, updated_at)
               VALUES (?1, ?2, ?3, 'local', 1, 'medium', ?4, NULL, ?5, ?5)"#,
            rusqlite::params![
                "card2",
                "Card 2",
                "backlog",
                "/orphan/repo",
                "2026-01-01T00:00:00Z"
            ],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, priority,
                                  repo_path, tree_source_id, created_at, updated_at)
               VALUES (?1, ?2, ?3, 'local', 2, 'medium', ?4, ?5, ?6, ?6)"#,
            rusqlite::params![
                "card3",
                "Card 3",
                "ongoing",
                "/some/repo",
                "ts-existing",
                "2026-01-01T00:00:00Z"
            ],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, priority,
                                  repo_path, tree_source_id, created_at, updated_at)
               VALUES (?1, ?2, ?3, 'local', 3, 'medium', NULL, NULL, ?4, ?4)"#,
            rusqlite::params!["card4", "Card 4", "done", "2026-01-01T00:00:00Z"],
        )
        .unwrap();

        // Agent runs: one for card1 (had repo_path), one for card4 (NULL repo_path).
        // repo_root must be backfilled from the card's repo_path for card1's run
        // and stay NULL for card4's run.
        conn.execute(
            r#"INSERT INTO agent_runs (id, card_id, agent_name, worktree_path, branch,
                                        status, skills_json, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, 'done', '[]', ?6)"#,
            rusqlite::params![
                "run-1",
                "card1",
                "claude-code",
                "/wt/card1",
                "feat-1",
                "2026-01-01T00:00:00Z"
            ],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO agent_runs (id, card_id, agent_name, worktree_path, branch,
                                        status, skills_json, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, 'done', '[]', ?6)"#,
            rusqlite::params![
                "run-4",
                "card4",
                "claude-code",
                "/wt/card4",
                "feat-4",
                "2026-01-01T00:00:00Z"
            ],
        )
        .unwrap();

        // Run migration 0014 exactly as run_migrations would.
        conn.execute_batch(MIGRATION_0014)
            .expect("migration 0014 should execute cleanly");

        // card1: linked to the existing tree_source by path match.
        let card1_ts: Option<String> = conn
            .query_row(
                "SELECT tree_source_id FROM cards WHERE id = ?1",
                rusqlite::params!["card1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(card1_ts.as_deref(), Some("ts-existing"));

        // card2: a new tree_source was created for the orphan path and card2 linked to it.
        let card2_ts: Option<String> = conn
            .query_row(
                "SELECT tree_source_id FROM cards WHERE id = ?1",
                rusqlite::params!["card2"],
                |r| r.get(0),
            )
            .unwrap();
        let orphan_ts: Option<String> = conn
            .query_row(
                "SELECT id FROM tree_sources WHERE path = ?1",
                rusqlite::params!["/orphan/repo"],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            card2_ts.is_some(),
            "card2 should be linked to a tree_source"
        );
        assert_eq!(
            card2_ts, orphan_ts,
            "card2 should be linked to the newly-created orphan tree_source"
        );
        assert!(
            orphan_ts
                .as_deref()
                .map_or(false, |s| s.starts_with("migrated-")),
            "orphan tree_source id should be prefixed with 'migrated-', got {orphan_ts:?}"
        );

        // card3: tree_source_id was already set and must be preserved (not overwritten by path match).
        let card3_ts: Option<String> = conn
            .query_row(
                "SELECT tree_source_id FROM cards WHERE id = ?1",
                rusqlite::params!["card3"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            card3_ts.as_deref(),
            Some("ts-existing"),
            "card3's pre-existing tree_source_id must be preserved"
        );

        // card4: no repo_path and no tree_source_id -> stays NULL.
        let card4_ts: Option<String> = conn
            .query_row(
                "SELECT tree_source_id FROM cards WHERE id = ?1",
                rusqlite::params!["card4"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(card4_ts, None, "card4 should remain unlinked");

        // The cards table no longer has a repo_path column.
        let mut stmt = conn.prepare("PRAGMA table_info(cards)").unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            !cols.iter().any(|c| c == "repo_path"),
            "repo_path column should be dropped, got columns: {cols:?}"
        );
        // Sanity: the columns we expect to keep are still there.
        for expected in [
            "id",
            "title",
            "tree_source_id",
            "source_instance_id",
            "created_at",
        ] {
            assert!(
                cols.iter().any(|c| c == expected),
                "missing column {expected}: {cols:?}"
            );
        }

        // agent_runs.repo_root backfilled from the card's repo_path at run-create time.
        let run1_root: Option<String> = conn
            .query_row(
                "SELECT repo_root FROM agent_runs WHERE id = ?1",
                rusqlite::params!["run-1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            run1_root.as_deref(),
            Some("/existing/repo"),
            "run-1 repo_root should be backfilled from card1's repo_path"
        );

        let run4_root: Option<String> = conn
            .query_row(
                "SELECT repo_root FROM agent_runs WHERE id = ?1",
                rusqlite::params!["run-4"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            run4_root, None,
            "run-4 repo_root should stay NULL because card4 had no repo_path"
        );

        // FKs still enforced post-migration (PRAGMA foreign_keys was toggled OFF/ON by the migration).
        let fk_on: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk_on, 1, "foreign_keys pragma should be ON after migration");
    }
}
