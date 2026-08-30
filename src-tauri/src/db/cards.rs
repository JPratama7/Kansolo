// Card CRUD commands — filled in step n2

use super::*;
use crate::db::{Card, CardRow, open_db, now_iso, max_position};

/// Map a single row from the `cards` table into a [`CardRow`].
/// Uses only the generalized `source_ref`/`source_status` columns.
fn row_to_card_row(row: &rusqlite::Row) -> rusqlite::Result<CardRow> {
    Ok(CardRow {
        id: row.get("id")?,
        title: row.get("title")?,
        description: row.get("description")?,
        priority: row.get("priority")?,
        column: row.get("column")?,
        source: row.get("source")?,
        position: row.get("position")?,
        source_ref: row.get("source_ref")?,
        source_status: row.get("source_status")?,
        tree_source_id: row.get("tree_source_id")?,
        repo_path: row.get("repo_path")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// Canonical column list for SELECTs — uses the generalized columns.
const CARD_COLUMNS: &str = r#"id, title, description, priority, "column", source, position, source_ref, source_status, tree_source_id, repo_path, created_at, updated_at"#;

/// SELECT all cards ordered by column then position.
#[tauri::command]
pub async fn list_cards(app: AppHandle) -> Result<Vec<Card>, String> {
    let conn = open_db(&app)?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {CARD_COLUMNS} FROM cards ORDER BY \"column\", position ASC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![], row_to_card_row)
        .map_err(|e| e.to_string())?;
    let mut cards = Vec::new();
    for r in rows {
        let row = r.map_err(|e| e.to_string())?;
        cards.push(Card::from(row));
    }
    Ok(cards)
}

/// SELECT cards for a single column, ordered by position. Used by the
/// per-column lazy fetch so each column loads (and shows its loading state)
/// independently rather than waiting on one whole-board query.
#[tauri::command]
pub async fn list_cards_by_column(app: AppHandle, column: String) -> Result<Vec<Card>, String> {
    let conn = open_db(&app)?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {CARD_COLUMNS} FROM cards WHERE \"column\" = ?1 ORDER BY position ASC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![column], row_to_card_row)
        .map_err(|e| e.to_string())?;
    let mut cards = Vec::new();
    for r in rows {
        let row = r.map_err(|e| e.to_string())?;
        cards.push(Card::from(row));
    }
    Ok(cards)
}

/// Create a new local card with a fresh UUID and a position at the end of its column.
#[tauri::command]
pub async fn create_local_card(app: AppHandle, title: String, column: String) -> Result<Card, String> {
    crate::db::validate_column(&column)?;
    let conn = open_db(&app)?;
    let now = now_iso();
    let id = uuid::Uuid::new_v4().to_string();
    let position = max_position(&conn, &column)? + 1;
    conn.execute(
        r#"INSERT INTO cards (id, title, description, priority, "column", source, position, created_at, updated_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
        rusqlite::params![
            id,
            title,
            "",          // description
            "medium",    // priority
            column,
            "local",     // source
            position,
            now,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(Card {
        id,
        title,
        description: String::new(),
        priority: "medium".to_string(),
        column,
        source: "local".to_string(),
        position,
        source_ref: None,
        source_status: None,
        tree_source_id: None,
        repo_path: None,
        created_at: now.clone(),
        updated_at: now,
    })
}

/// Patch selected fields on a card. Mirrors src/db.ts updateCard (lines 104-139):
/// only non-None fields go into the SET clause, and updated_at is always bumped.
#[tauri::command]
pub async fn update_card(
    app: AppHandle,
    id: String,
    title: Option<String>,
    description: Option<String>,
    priority: Option<String>,
    column: Option<String>,
    source_status: Option<String>,
    tree_source_id: Option<String>,
) -> Result<(), String> {
    let conn = open_db(&app)?;
    let mut sets: Vec<String> = Vec::new();
    // Boxed values keep the borrow alive for the dynamic params![] call below.
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(v) = title {
        sets.push(format!("title = ?{}", binds.len() + 1));
        binds.push(Box::new(v));
    }
    if let Some(v) = description {
        sets.push(format!("description = ?{}", binds.len() + 1));
        binds.push(Box::new(v));
    }
    if let Some(v) = priority {
        sets.push(format!("priority = ?{}", binds.len() + 1));
        binds.push(Box::new(v));
    }
    if let Some(v) = column {
        sets.push(format!(r#""column" = ?{}"#, binds.len() + 1));
        binds.push(Box::new(v));
    }
    if let Some(v) = source_status {
        sets.push(format!("source_status = ?{}", binds.len() + 1));
        binds.push(Box::new(v));
    }
    if let Some(v) = tree_source_id {
        // Empty string normalizes to NULL, matching the TS `patch.treeSourceId || null` behavior.
        let to_store: Option<String> = if v.is_empty() { None } else { Some(v) };
        sets.push(format!("tree_source_id = ?{}", binds.len() + 1));
        binds.push(Box::new(to_store));
    }

    if sets.is_empty() {
        // Nothing to do; mirror the TS early-return.
        return Ok(());
    }

    sets.push(format!("updated_at = ?{}", binds.len() + 1));
    binds.push(Box::new(now_iso()));

    let id_idx = binds.len() + 1;
    binds.push(Box::new(id));

    let sql = format!("UPDATE cards SET {} WHERE id = ?{}", sets.join(", "), id_idx);
    let params: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    conn.execute(&sql, params.as_slice())
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Move a card to a new column/position. Always bumps updated_at. When
/// `position` is `None`, the card lands at the end of the target column
/// (max existing position + 1) — used by drag-drop, which always appends.
#[tauri::command]
pub async fn move_card(
    app: AppHandle,
    id: String,
    column: String,
    position: Option<i64>,
) -> Result<(), String> {
    crate::db::validate_column(&column)?;
    let conn = open_db(&app)?;
    let final_pos = match position {
        Some(p) => p,
        None => max_position(&conn, &column)? + 1,
    };
    conn.execute(
        r#"UPDATE cards SET "column" = ?1, position = ?2, updated_at = ?3 WHERE id = ?4"#,
        rusqlite::params![column, final_pos, now_iso(), id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a single card by id.
#[tauri::command]
pub async fn delete_card(app: AppHandle, id: String) -> Result<(), String> {
    let conn = open_db(&app)?;
    conn.execute("DELETE FROM cards WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Check whether a card has an active (pending/running) agent run.
#[tauri::command]
pub async fn is_card_locked_cmd(app: AppHandle, id: String) -> Result<bool, String> {
    let conn = open_db(&app)?;
    Ok(crate::db::agent_runs::is_card_locked(&conn, &id))
}

/// Atomically remove every card sourced from a source instance (looked up by
/// its `sources.id`) and its sync snapshots. Wraps both deletes in a single
/// transaction so a crash between them can't leave orphaned snapshots
/// referencing deleted cards (fixes the TS non-atomicity bug).
///
/// The `cards`/`external_snapshots` tables key off the source *type* string
/// (e.g. "jira"), not the instance id — there is no `source_id` FK column — so
/// we resolve the instance id → source_type inside the same transaction and
/// delete by type. The frontend caller passes the selected source instance id.
#[tauri::command]
pub async fn delete_all_source_cards(app: AppHandle, source_id: String) -> Result<(), String> {
    let mut conn = open_db(&app)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let source_type: String = tx
        .query_row(
            "SELECT source_type FROM sources WHERE id = ?1",
            rusqlite::params![source_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM cards WHERE source = ?1", rusqlite::params![source_type])
        .map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM external_snapshots WHERE source = ?1",
        rusqlite::params![source_type],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// Find the local card linked to a `(source, source_ref)` pair, if any.
/// Inner helper that runs on a borrowed connection (works inside a
/// `Transaction` via deref coercion) so `sync_source` can do all its
/// reads/writes on one atomic transaction.
pub fn get_card_by_source_ref_inner(
    conn: &rusqlite::Connection,
    source: &str,
    source_ref: &str,
) -> Result<Option<Card>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {CARD_COLUMNS} FROM cards WHERE source = ?1 AND source_ref = ?2 LIMIT 1"
        ))
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(rusqlite::params![source, source_ref])
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let card_row = row_to_card_row(row).map_err(|e| e.to_string())?;
        Ok(Some(Card::from(card_row)))
    } else {
        Ok(None)
    }
}

/// Tauri command wrapper — opens a fresh connection and delegates to
/// [`get_card_by_source_ref_inner`].
#[tauri::command]
pub async fn get_card_by_source_ref(
    app: AppHandle,
    source: String,
    source_ref: String,
) -> Result<Option<Card>, String> {
    let conn = open_db(&app)?;
    get_card_by_source_ref_inner(&conn, &source, &source_ref)
}

/// Fetch a card by id. Sync helper used by the ACP runner; returns
/// `AcpError` so callers can use `?` alongside other `AcpError` results.
pub fn get_card_by_id(conn: &rusqlite::Connection, id: &str) -> Result<Option<Card>, crate::error::AcpError> {
    let mut stmt = conn
        .prepare(&format!("SELECT {CARD_COLUMNS} FROM cards WHERE id = ?1 LIMIT 1"))
        .map_err(crate::error::AcpError::internal)?;
    let mut rows = stmt
        .query(rusqlite::params![id])
        .map_err(crate::error::AcpError::internal)?;
    if let Some(row) = rows.next().map_err(crate::error::AcpError::internal)? {
        let card_row = row_to_card_row(row).map_err(crate::error::AcpError::internal)?;
        Ok(Some(Card::from(card_row)))
    } else {
        Ok(None)
    }
}

/// Insert a new externally-sourced card or fully overwrite an existing one
/// (matched by `(source, source_ref)`). Mirrors src/db.ts upsertCardFromSync
/// (lines 242-284): preserves the existing id and per-column position when
/// updating; assigns a fresh end-of-column position when inserting.
/// Writes only the generalized `source_ref`/`source_status` columns.
///
/// `updated_at` is remote-authoritative: on both insert and update it is set
/// from `card.updated_at` (the upstream snapshot timestamp), never from local
/// clock time, so a sync never back-dates a card or clobbers the remote's
/// notion of "last modified" with a local now() stamp.
///
/// Inner helper that runs on a borrowed connection (works inside a
/// `Transaction` via deref coercion) so `sync_source` can do all its
/// reads/writes on one atomic transaction.
pub fn upsert_card_from_sync_inner(
    conn: &rusqlite::Connection,
    card: &Card,
) -> Result<(), String> {
    let source_ref = card.source_ref.clone().unwrap_or_default();
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM cards WHERE source = ?1 AND source_ref = ?2",
            rusqlite::params![card.source, source_ref],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .map_err(|e| e.to_string())?;

    if existing.is_some() {
        // Preserve id + position; refresh mutable fields from the remote.
        conn.execute(
            r#"UPDATE cards SET
                 title = ?1, description = ?2, priority = ?3, source_status = ?4,
                 "column" = ?5, updated_at = ?6
               WHERE source = ?7 AND source_ref = ?8"#,
            rusqlite::params![
                card.title,
                card.description,
                card.priority,
                card.source_status,
                card.column,
                card.updated_at,
                card.source,
                source_ref,
            ],
        )
        .map_err(|e| e.to_string())?;
    } else {
        let position = max_position(conn, &card.column)? + 1;
        conn.execute(
            r#"INSERT INTO cards
                 (id, title, description, priority, "column", source, position, source_ref, source_status, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"#,
            rusqlite::params![
                card.id,
                card.title,
                card.description,
                card.priority,
                card.column,
                card.source,
                position,
                source_ref,
                card.source_status,
                card.created_at,
                card.updated_at,
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Tauri command wrapper — opens a fresh connection and delegates to
/// [`upsert_card_from_sync_inner`]. Registered as a Tauri command for
/// completeness; the lib.rs invoke_handler registration is owned by
/// another node.
#[tauri::command]
pub async fn upsert_card_from_sync(app: AppHandle, card: Card) -> Result<(), String> {
    let conn = open_db(&app)?;
    upsert_card_from_sync_inner(&conn, &card)
}

#[cfg(test)]
mod column_guard_tests {
    use crate::db::validate_column;

    // `create_local_card` and `move_card` are Tauri commands whose first line
    // is `validate_column(&column)?`, so this guard is the contract those
    // commands enforce before touching the DB. They require an AppHandle to
    // drive end-to-end; the guard itself is pure and unit-tested here.

    #[test]
    fn create_local_card_column_guard_rejects_invalid() {
        let err = validate_column("in_progress").unwrap_err();
        assert_eq!(err, "column must be backlog, ongoing, or done");
    }

    #[test]
    fn move_card_column_guard_rejects_invalid() {
        let err = validate_column("archived").unwrap_err();
        assert_eq!(err, "column must be backlog, ongoing, or done");
    }
}
