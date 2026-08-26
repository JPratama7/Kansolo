// Settings/snapshot/tree/source commands — filled in step n3
use crate::db::{open_db, now_iso, ExternalSnapshot, SourceInstance, StatusMapping, TreeSource};
use rusqlite::params;
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

/// Read the last-synced snapshot for a `(source, source_ref)` pair, if any.
/// Uses the generalized `external_snapshots` table.
#[tauri::command]
pub async fn get_snapshot(
    app: AppHandle,
    source: String,
    source_ref: String,
) -> Result<Option<ExternalSnapshot>, String> {
    let conn = open_db(&app)?;
    conn.query_row(
        r#"SELECT source, source_ref, title, description, priority, source_status, "column", synced_at
           FROM external_snapshots WHERE source = ?1 AND source_ref = ?2 LIMIT 1"#,
        params![source, source_ref],
        |row| {
            Ok(ExternalSnapshot {
                source: row.get(0)?,
                source_ref: row.get(1)?,
                title: row.get(2)?,
                description: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                priority: row
                    .get::<_, Option<String>>(4)?
                    .filter(|p| !p.is_empty())
                    .unwrap_or_else(|| "medium".to_string()),
                source_status: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                column: row.get(6)?,
                synced_at: row.get(7)?,
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
/// state at this sync instant. Mirrors src/db.ts lines 218-235.
#[tauri::command]
pub async fn save_snapshot(app: AppHandle, snap: ExternalSnapshot) -> Result<(), String> {
    let conn = open_db(&app)?;
    conn.execute(
        r#"INSERT INTO external_snapshots
             (source, source_ref, title, description, priority, source_status, "column", synced_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
           ON CONFLICT(source, source_ref) DO UPDATE SET
             title = ?3, description = ?4, priority = ?5, source_status = ?6,
             "column" = ?7, synced_at = ?8"#,
        params![
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

/// Delete a tree source by id.
#[tauri::command]
pub async fn delete_tree_source(app: AppHandle, id: String) -> Result<(), String> {
    let conn = open_db(&app)?;
    conn.execute("DELETE FROM tree_sources WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---- Source instance CRUD ---------------------------------------------------
//
// `sources` rows store `config_json` and `status_mapping_json` as JSON text.
// The commands below deserialize those into `serde_json::Value` /
// `StatusMapping` for the API surface, and re-serialize on write.

/// Map a `sources` row into a [`SourceInstance`], parsing the JSON text columns.
fn row_to_source_instance(row: &rusqlite::Row) -> rusqlite::Result<SourceInstance> {
    let config_json: String = row.get("config_json")?;
    let status_mapping_json: String = row.get("status_mapping_json")?;
    let enabled: i64 = row.get("enabled")?;
    let config: Value = serde_json::from_str(&config_json)
        .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
    let status_mapping: StatusMapping = serde_json::from_str(&status_mapping_json)
        .unwrap_or_else(|_| StatusMapping {
            backlog: Vec::new(),
            ongoing: Vec::new(),
            done: Vec::new(),
        });
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
#[tauri::command]
pub async fn delete_source(app: AppHandle, id: String) -> Result<(), String> {
    let conn = open_db(&app)?;
    conn.execute("DELETE FROM sources WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

