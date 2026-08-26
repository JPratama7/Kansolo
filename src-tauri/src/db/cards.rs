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
        source_path: row.get("source_path")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// Canonical column list for SELECTs — uses the generalized columns.
const CARD_COLUMNS: &str = r#"id, title, description, priority, "column", source, position, source_ref, source_status, source_path, created_at, updated_at"#;

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

/// Create a new local card with a fresh UUID and a position at the end of its column.
#[tauri::command]
pub async fn create_local_card(app: AppHandle, title: String, column: String) -> Result<Card, String> {
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
        source_path: None,
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
    source_path: Option<String>,
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
    if let Some(v) = source_path {
        // Empty string normalizes to NULL, matching the TS `patch.sourcePath || null` behavior.
        let to_store: Option<String> = if v.is_empty() { None } else { Some(v) };
        sets.push(format!("source_path = ?{}", binds.len() + 1));
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

/// Move a card to a new column/position. Always bumps updated_at.
#[tauri::command]
pub async fn move_card(app: AppHandle, id: String, column: String, position: i64) -> Result<(), String> {
    let conn = open_db(&app)?;
    conn.execute(
        r#"UPDATE cards SET "column" = ?1, position = ?2, updated_at = ?3 WHERE id = ?4"#,
        rusqlite::params![column, position, now_iso(), id],
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

/// Atomically remove every card sourced from `source` and its sync snapshots.
/// Wraps both deletes in a single transaction so a crash between them can't
/// leave orphaned snapshots referencing deleted cards (fixes the TS non-atomicity bug).
#[tauri::command]
pub async fn delete_all_source_cards(app: AppHandle, source: String) -> Result<(), String> {
    let mut conn = open_db(&app)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM cards WHERE source = ?1", rusqlite::params![source])
        .map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM external_snapshots WHERE source = ?1",
        rusqlite::params![source],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// Find the local card linked to a `(source, source_ref)` pair, if any.
#[tauri::command]
pub async fn get_card_by_source_ref(
    app: AppHandle,
    source: String,
    source_ref: String,
) -> Result<Option<Card>, String> {
    let conn = open_db(&app)?;
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

/// Insert a new externally-sourced card or fully overwrite an existing one
/// (matched by `(source, source_ref)`). Mirrors src/db.ts upsertCardFromSync
/// (lines 242-284): preserves the existing id and per-column position when
/// updating; assigns a fresh end-of-column position when inserting.
/// Writes only the generalized `source_ref`/`source_status` columns.
#[tauri::command]
pub async fn upsert_card_from_sync(app: AppHandle, card: Card) -> Result<(), String> {
    let conn = open_db(&app)?;

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
        let position = max_position(&conn, &card.column)? + 1;
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
