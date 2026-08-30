// Settings/snapshot/tree/source commands — filled in step n3
use crate::db::{open_db, now_iso, ExternalSnapshot, SourceInstance, StatusMapping, TreeSource};
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::HashMap;
use tauri::AppHandle;

/// Read a single setting value by key, or `None` if absent.
#[tauri::command]
pub async fn get_setting(app: AppHandle, key: String) -> Result<Option<String>, String> {
    let conn = open_db(&app)?;
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other.to_string()),
    })
}

/// Upsert a single setting value.
#[tauri::command]
pub async fn set_setting(app: AppHandle, key: String, value: String) -> Result<(), String> {
    let conn = open_db(&app)?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = ?2",
        params![key, value],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Read all settings as a `HashMap<String, String>`.
#[tauri::command]
pub async fn get_all_settings(app: AppHandle) -> Result<HashMap<String, String>, String> {
    let conn = open_db(&app)?;
    let mut stmt = conn
        .prepare("SELECT key, value FROM settings")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut out = HashMap::new();
    for r in rows {
        let (k, v) = r.map_err(|e| e.to_string())?;
        out.insert(k, v);
    }
    Ok(out)
}

/// Atomically upsert a batch of settings inside a single transaction.
#[tauri::command]
pub async fn save_settings(
    app: AppHandle,
    settings: HashMap<String, String>,
) -> Result<(), String> {
    let mut conn = open_db(&app)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = ?2",
            )
            .map_err(|e| e.to_string())?;
        for (k, v) in &settings {
            stmt.execute(params![k, v]).map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// Read the last-synced snapshot for a `(source_instance_id, source_ref)`
/// pair, if any. Uses the generalized `external_snapshots` table.
#[tauri::command]
pub async fn get_snapshot(
    app: AppHandle,
    source_instance_id: String,
    source_ref: String,
) -> Result<Option<ExternalSnapshot>, String> {
    let conn = open_db(&app)?;
    conn.query_row(
        r#"SELECT source_instance_id, source, source_ref, title, description, priority, source_status, "column", synced_at
           FROM external_snapshots WHERE source_instance_id = ?1 AND source_ref = ?2 LIMIT 1"#,
        params![source_instance_id, source_ref],
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

/// Upsert the snapshot row for a `(source_instance_id, source_ref)` pair —
/// the external state at this sync instant. Mirrors src/db.ts lines 218-235.
#[tauri::command]
pub async fn save_snapshot(app: AppHandle, snap: ExternalSnapshot) -> Result<(), String> {
    let conn = open_db(&app)?;
    conn.execute(
        r#"INSERT INTO external_snapshots
             (source_instance_id, source, source_ref, title, description, priority, source_status, "column", synced_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
           ON CONFLICT(source_instance_id, source_ref) DO UPDATE SET
             source = ?2, title = ?4, description = ?5, priority = ?6, source_status = ?7,
             "column" = ?8, synced_at = ?9"#,
        params![
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

/// List all registered tree sources, ordered by label ascending.
#[tauri::command]
pub async fn list_tree_sources(app: AppHandle) -> Result<Vec<TreeSource>, String> {
    let conn = open_db(&app)?;
    let mut stmt = conn
        .prepare("SELECT id, label, path, editor_command FROM tree_sources ORDER BY label ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(TreeSource {
                id: row.get(0)?,
                label: row.get(1)?,
                path: row.get(2)?,
                editor_command: row.get::<_, Option<String>>(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Register a new tree source. Generates a UUID and trims/nulls empty editor_command.
#[tauri::command]
pub async fn add_tree_source(
    app: AppHandle,
    label: String,
    path: String,
    editor_command: Option<String>,
) -> Result<(), String> {
    let conn = open_db(&app)?;
    let id = uuid::Uuid::new_v4().to_string();
    let editor_command = editor_command
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    conn.execute(
        "INSERT INTO tree_sources (id, label, path, editor_command, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, label, path, editor_command, now_iso()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Update an existing tree source by id.
#[tauri::command]
pub async fn update_tree_source(
    app: AppHandle,
    id: String,
    label: String,
    path: String,
    editor_command: Option<String>,
) -> Result<(), String> {
    let conn = open_db(&app)?;
    let editor_command = editor_command
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    conn.execute(
        "UPDATE tree_sources SET label = ?1, path = ?2, editor_command = ?3 WHERE id = ?4",
        params![label, path, editor_command, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Resolve a tree source's path by id. Sync helper used by the ACP runner
/// to find a card's repo when no explicit `repo_path` is set.
pub fn get_tree_source_path(conn: &Connection, id: &str) -> Result<Option<String>, crate::error::AcpError> {
    conn.query_row(
        "SELECT path FROM tree_sources WHERE id = ?1",
        params![id],
        |r| r.get::<_, String>(0),
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(crate::error::AcpError::internal(other.to_string())),
    })
}

/// Count cards still linked to a tree source. Returns an error message
/// if any cards reference it, or `Ok(())` if deletion is safe.
pub(crate) fn ensure_tree_source_deletable(conn: &Connection, id: &str) -> Result<(), String> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM cards WHERE tree_source_id = ?1",
        params![id], |r| r.get(0),
    ).map_err(|e| e.to_string())?;
    if count > 0 {
        return Err(format!(
            "Cannot delete tree source: {count} card(s) still linked. Remove the link from those cards first."
        ));
    }
    Ok(())
}

/// Delete a tree source by id.
#[tauri::command]
pub async fn delete_tree_source(app: AppHandle, id: String) -> Result<(), String> {
    let conn = open_db(&app)?;
    ensure_tree_source_deletable(&conn, &id)?;
    conn.execute("DELETE FROM tree_sources WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---- Source instance CRUD ---------------------------------------------------
//
// `sources` rows store `config_json` and `status_mapping_json` as JSON text.
// The commands below deserialize those into `serde_json::Value` /
// `StatusMapping` for the API surface, and re-serialize on write.

/// Sentinel substituted for `config.token` on the read path. The TS settings
/// forms recognize this placeholder and preserve the stored token on save when
/// the field is unchanged (see the TS placeholder model). Never compare user
/// input against this to authenticate — it is a display-only mask.
pub const REDACTED_TOKEN: &str = "__REDACTED__";

/// Replace `config.token` with [`REDACTED_TOKEN`] when present, in place.
/// Only the read path (`list_sources` / `get_source`) goes through
/// [`row_to_source_instance`], so writes still round-trip the real token.
fn redact_config_token(config: &mut Value) {
    if let Some(obj) = config.as_object_mut() {
        if obj.contains_key("token") {
            obj.insert(
                "token".to_string(),
                Value::String(REDACTED_TOKEN.to_string()),
            );
        }
    }
}

/// Map a `sources` row into a [`SourceInstance`], parsing the JSON text columns.
/// A corrupt `config_json` or `status_mapping_json` value is surfaced as
/// `rusqlite::Error::FromSqlConversionFailure` rather than silently defaulting
/// to an empty config — silent defaults would mask a damaged row and let the
/// UI show a source that's secretly lost its config.
///
/// The `config.token` field is redacted (see [`redact_config_token`]) so the
/// plaintext never reaches the UI via the read path. The edit form relies on
/// the TS placeholder model to preserve the stored token when the field is
/// untouched.
fn row_to_source_instance(row: &rusqlite::Row) -> rusqlite::Result<SourceInstance> {
    let config_json: String = row.get("config_json")?;
    let status_mapping_json: String = row.get("status_mapping_json")?;
    let enabled: i64 = row.get("enabled")?;
    let mut config: Value = serde_json::from_str(&config_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(e),
        )
    })?;
    redact_config_token(&mut config);
    let status_mapping: StatusMapping = serde_json::from_str(&status_mapping_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(e),
        )
    })?;
    Ok(SourceInstance {
        id: row.get("id")?,
        source_type: row.get("source_type")?,
        label: row.get("label")?,
        config,
        status_mapping,
        enabled: enabled != 0,
        created_at: row.get("created_at")?,
    })
}

const SOURCE_COLUMNS: &str = "id, source_type, label, config_json, status_mapping_json, enabled, created_at";

/// List all registered source instances, ordered by label ascending.
#[tauri::command]
pub async fn list_sources(app: AppHandle) -> Result<Vec<SourceInstance>, String> {
    let conn = open_db(&app)?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SOURCE_COLUMNS} FROM sources ORDER BY label ASC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], row_to_source_instance)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Fetch a single source instance by id.
#[tauri::command]
pub async fn get_source(app: AppHandle, id: String) -> Result<Option<SourceInstance>, String> {
    let conn = open_db(&app)?;
    conn.query_row(
        &format!("SELECT {SOURCE_COLUMNS} FROM sources WHERE id = ?1 LIMIT 1"),
        params![id],
        row_to_source_instance,
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other.to_string()),
    })
}

/// Register a new source instance. Generates a UUID id and returns the
/// created row. `config` is stored as JSON text under `config_json`;
/// `status_mapping` under `status_mapping_json`.
#[tauri::command]
pub async fn add_source(
    app: AppHandle,
    source_type: String,
    label: String,
    config: Value,
    status_mapping: StatusMapping,
) -> Result<SourceInstance, String> {
    let conn = open_db(&app)?;
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = now_iso();
    let config_json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
    let status_mapping_json =
        serde_json::to_string(&status_mapping).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO sources (id, source_type, label, config_json, status_mapping_json, enabled, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
        params![id, source_type, label, config_json, status_mapping_json, created_at],
    )
    .map_err(|e| e.to_string())?;
    Ok(SourceInstance {
        id,
        source_type,
        label,
        config,
        status_mapping,
        enabled: true,
        created_at,
    })
}

/// Update an existing source instance by id. Rewrites label, config,
/// status_mapping, and the enabled flag.
#[tauri::command]
pub async fn update_source(
    app: AppHandle,
    id: String,
    label: String,
    config: Value,
    status_mapping: StatusMapping,
    enabled: bool,
) -> Result<(), String> {
    let conn = open_db(&app)?;
    let config_json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
    let status_mapping_json =
        serde_json::to_string(&status_mapping).map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE sources SET label = ?1, config_json = ?2, status_mapping_json = ?3, enabled = ?4
         WHERE id = ?5",
        params![label, config_json, status_mapping_json, enabled as i64, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a source instance by id.
///
/// This removes the `sources` row (config + status mapping) only. Cards
/// already imported from the source are kept — they retain their `source`
/// and `source_ref` fields, but will no longer be synced. Snapshots in
/// `external_snapshots` are also left in place (harmless stale rows).
#[tauri::command]
pub async fn delete_source(app: AppHandle, id: String) -> Result<(), String> {
    let conn = open_db(&app)?;
    conn.execute("DELETE FROM sources WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod source_instance_parse_tests {
    use super::row_to_source_instance;
    use crate::db::test_db;
    use rusqlite::params;

    /// Insert a `sources` row with the given `config_json` and a valid
    /// `status_mapping_json`, then run `row_to_source_instance` against it.
    fn map_row_with_config(config_json: &str) -> rusqlite::Result<()> {
        let conn = test_db();
        conn.execute(
            "INSERT INTO sources (id, source_type, label, config_json, status_mapping_json, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
            params!["src-1", "jira", "Jira", config_json, "{}", "2026-01-01T00:00:00Z"],
        )
        .unwrap();
        let mut stmt = conn
            .prepare("SELECT id, source_type, label, config_json, status_mapping_json, enabled, created_at FROM sources WHERE id = ?1")
            .unwrap();
        let mut rows = stmt.query(params!["src-1"]).unwrap();
        let row = rows.next().unwrap().unwrap();
        row_to_source_instance(row).map(|_| ())
    }

    #[test]
    fn valid_config_json_parses_cleanly() {
        assert!(map_row_with_config(r#"{"base_url":"https://x"}"#).is_ok());
    }

    #[test]
    fn corrupt_config_json_returns_err() {
        // Pre-fix this silently defaulted to an empty config object, masking
        // the damaged row. Post-fix it propagates FromSqlConversionFailure.
        let err = map_row_with_config("not valid json{").unwrap_err();
        assert!(
            matches!(err, rusqlite::Error::FromSqlConversionFailure(_, _, _)),
            "expected FromSqlConversionFailure, got {err:?}"
        );
    }

    #[test]
    fn corrupt_status_mapping_json_returns_err() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO sources (id, source_type, label, config_json, status_mapping_json, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
            params!["src-2", "jira", "Jira", "{}", "broken{", "2026-01-01T00:00:00Z"],
        )
        .unwrap();
        let mut stmt = conn
            .prepare("SELECT id, source_type, label, config_json, status_mapping_json, enabled, created_at FROM sources WHERE id = ?1")
            .unwrap();
        let mut rows = stmt.query(params!["src-2"]).unwrap();
        let row = rows.next().unwrap().unwrap();
        let err = row_to_source_instance(row).unwrap_err();
        assert!(
            matches!(err, rusqlite::Error::FromSqlConversionFailure(_, _, _)),
            "expected FromSqlConversionFailure, got {err:?}"
        );
    }

    /// The read path (`list_sources` / `get_source`) must never surface the
    /// plaintext Jira token. `row_to_source_instance` masks `config.token`
    /// with the [`REDACTED_TOKEN`] sentinel.
    #[test]
    fn read_path_redacts_config_token() {
        let conn = test_db();
        let config = r#"{"base_url":"https://x","email":"a@b","token":"super-secret"}"#;
        conn.execute(
            "INSERT INTO sources (id, source_type, label, config_json, status_mapping_json, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
            params!["src-tok", "jira", "Jira", config, "{}", "2026-01-01T00:00:00Z"],
        )
        .unwrap();
        let mut stmt = conn
            .prepare("SELECT id, source_type, label, config_json, status_mapping_json, enabled, created_at FROM sources WHERE id = ?1")
            .unwrap();
        let mut rows = stmt.query(params!["src-tok"]).unwrap();
        let row = rows.next().unwrap().unwrap();
        let inst = row_to_source_instance(row).unwrap();
        let token = inst.config.get("token").and_then(|v| v.as_str()).unwrap();
        assert_eq!(token, super::REDACTED_TOKEN);
        assert_ne!(token, "super-secret");
        // Non-secret fields are preserved.
        assert_eq!(inst.config.get("email").and_then(|v| v.as_str()), Some("a@b"));
    }
}

/// CRUD round-trip tests for the `sources` table.
///
/// The `add_source` / `update_source` / `delete_source` / `list_sources` /
/// `get_source` Tauri commands take an [`AppHandle`], which can't be
/// constructed in a unit test. These tests exercise the same SQL contract
/// those commands run against `test_db()`, then read back through
/// [`row_to_source_instance`] — the shared parse path the commands use on
/// the read side. If the SQL or the parser drifts, these fail.
#[cfg(test)]
mod source_crud_round_trip_tests {
    use super::row_to_source_instance;
    use crate::db::test_db;
    use crate::mapping::StatusMapping;
    use rusqlite::params;
    use serde_json::{json, Value};

    const COLS: &str = "id, source_type, label, config_json, status_mapping_json, enabled, created_at";

    /// Mirror of `add_source`'s INSERT. Returns the inserted id.
    fn add_source(
        conn: &rusqlite::Connection,
        source_type: &str,
        label: &str,
        config: &Value,
        status_mapping: &StatusMapping,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = crate::db::now_iso();
        let config_json = serde_json::to_string(config).unwrap();
        let status_mapping_json = serde_json::to_string(status_mapping).unwrap();
        conn.execute(
            "INSERT INTO sources (id, source_type, label, config_json, status_mapping_json, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
            params![id, source_type, label, config_json, status_mapping_json, created_at],
        )
        .unwrap();
        id
    }

    /// Mirror of `update_source`'s UPDATE.
    fn update_source(
        conn: &rusqlite::Connection,
        id: &str,
        label: &str,
        config: &Value,
        status_mapping: &StatusMapping,
        enabled: bool,
    ) {
        let config_json = serde_json::to_string(config).unwrap();
        let status_mapping_json = serde_json::to_string(status_mapping).unwrap();
        conn.execute(
            "UPDATE sources SET label = ?1, config_json = ?2, status_mapping_json = ?3, enabled = ?4
             WHERE id = ?5",
            params![label, config_json, status_mapping_json, enabled as i64, id],
        )
        .unwrap();
    }

    /// Mirror of `get_source`'s SELECT-by-id.
    fn get_source(conn: &rusqlite::Connection, id: &str) -> Option<crate::db::SourceInstance> {
        conn.query_row(
            &format!("SELECT {COLS} FROM sources WHERE id = ?1 LIMIT 1"),
            params![id],
            row_to_source_instance,
        )
        .ok()
    }

    /// Mirror of `list_sources`'s SELECT, ordered by label asc.
    fn list_sources(conn: &rusqlite::Connection) -> Vec<crate::db::SourceInstance> {
        let mut stmt = conn
            .prepare(&format!("SELECT {COLS} FROM sources ORDER BY label ASC"))
            .unwrap();
        let rows = stmt.query_map([], row_to_source_instance).unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    fn mapping() -> StatusMapping {
        StatusMapping {
            backlog: vec!["To Do".to_string()],
            ongoing: vec!["In Progress".to_string()],
            done: vec!["Done".to_string()],
        }
    }

    #[test]
    fn add_then_get_source_round_trips() {
        let conn = test_db();
        let config = json!({"base_url": "https://jira.x", "email": "a@b", "token": "secret"});
        let id = add_source(&conn, "jira", "Jira A", &config, &mapping());

        let inst = get_source(&conn, &id).expect("row should exist after add");
        assert_eq!(inst.id, id);
        assert_eq!(inst.source_type, "jira");
        assert_eq!(inst.label, "Jira A");
        assert_eq!(inst.config.get("base_url"), Some(&json!("https://jira.x")));
        assert_eq!(inst.status_mapping.done, vec!["Done".to_string()]);
        assert!(inst.enabled);
        // Read path redacts the token — see read_path_redacts_config_token.
        assert_eq!(
            inst.config.get("token").and_then(|v| v.as_str()),
            Some(super::REDACTED_TOKEN)
        );
    }

    #[test]
    fn update_source_persists_label_config_and_enabled() {
        let conn = test_db();
        let id = add_source(&conn, "jira", "Old", &json!({"base_url": "https://old"}), &mapping());

        let new_mapping = StatusMapping {
            backlog: vec!["Backlog".to_string()],
            ongoing: vec!["Active".to_string()],
            done: vec!["Closed".to_string()],
        };
        update_source(
            &conn,
            &id,
            "New Label",
            &json!({"base_url": "https://new", "jql_parts": {"project": "PROJ"}}),
            &new_mapping,
            false,
        );

        let inst = get_source(&conn, &id).expect("row should still exist after update");
        assert_eq!(inst.label, "New Label");
        assert!(!inst.enabled);
        assert_eq!(inst.config.get("base_url"), Some(&json!("https://new")));
        assert_eq!(
            inst.config.get("jql_parts").and_then(|v| v.get("project")),
            Some(&json!("PROJ"))
        );
        assert_eq!(inst.status_mapping.ongoing, vec!["Active".to_string()]);
    }

    #[test]
    fn delete_source_removes_row() {
        let conn = test_db();
        let id = add_source(&conn, "jira", "Doomed", &json!({}), &mapping());
        assert!(get_source(&conn, &id).is_some());

        conn.execute("DELETE FROM sources WHERE id = ?1", params![id])
            .unwrap();

        assert!(get_source(&conn, &id).is_none(), "row must be gone after delete");
    }

    #[test]
    fn list_sources_orders_by_label_and_redacts_tokens() {
        let conn = test_db();
        add_source(&conn, "jira", "Zeta", &json!({"token": "z-secret"}), &mapping());
        add_source(&conn, "jira", "Alpha", &json!({"token": "a-secret"}), &mapping());

        let list = list_sources(&conn);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].label, "Alpha");
        assert_eq!(list[1].label, "Zeta");

        // No plaintext token may leak through the list path.
        for inst in &list {
            let token = inst.config.get("token").and_then(|v| v.as_str());
            assert_eq!(token, Some(super::REDACTED_TOKEN));
            assert_ne!(token, Some("z-secret"));
            assert_ne!(token, Some("a-secret"));
        }
    }

    #[test]
    fn nested_json_config_round_trips() {
        let conn = test_db();
        let config = json!({
            "base_url": "https://jira.x",
            "jql_parts": {
                "project": "PROJ",
                "labels": ["a", "b"],
                "nested": {"k": 7}
            }
        });
        let id = add_source(&conn, "jira", "Nested", &config, &mapping());

        let inst = get_source(&conn, &id).unwrap();
        let parts = inst.config.get("jql_parts").unwrap();
        assert_eq!(parts.get("project"), Some(&json!("PROJ")));
        assert_eq!(
            parts.get("labels").and_then(|v| v.as_array()).map(|a| a.len()),
            Some(2)
        );
        assert_eq!(parts.get("nested").and_then(|v| v.get("k")), Some(&json!(7)));
    }
}

