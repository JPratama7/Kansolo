// Card CRUD commands — filled in step n2

use super::*;
use crate::db::{max_position, now_iso, open_db, Card};
use crate::error::AcpError;

/// Map a single row from the `cards` table into a [`Card`].
/// Nullable columns are defaulted inline.
pub(crate) fn row_to_card(row: &rusqlite::Row) -> rusqlite::Result<Card> {
    Ok(Card {
        id: row.get("id")?,
        title: row.get("title")?,
        description: row.get::<_, Option<String>>("description")?.unwrap_or_default(),
        priority: row
            .get::<_, Option<String>>("priority")?
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "medium".to_string()),
        column: row.get("column")?,
        source: row.get("source")?,
        position: row.get("position")?,
        source_ref: row.get("source_ref")?,
        source_status: row.get("source_status")?,
        tree_source_id: row.get("tree_source_id")?,
        source_instance_id: row.get("source_instance_id")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// Canonical column list for SELECTs — uses the generalized columns.
pub(crate) const CARD_COLUMNS: &str = r#"id, title, description, priority, "column", source, position, source_ref, source_status, tree_source_id, source_instance_id, created_at, updated_at"#;

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
        .query_map(rusqlite::params![], row_to_card)
        .map_err(|e| e.to_string())?;
    let mut cards = Vec::new();
    for r in rows {
        cards.push(r.map_err(|e| e.to_string())?);
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
        .query_map(rusqlite::params![column], row_to_card)
        .map_err(|e| e.to_string())?;
    let mut cards = Vec::new();
    for r in rows {
        cards.push(r.map_err(|e| e.to_string())?);
    }
    Ok(cards)
}

/// Create a new local card with a fresh UUID and a position at the end of its column.
#[tauri::command]
pub async fn create_local_card(
    app: AppHandle,
    title: String,
    column: String,
) -> Result<Card, String> {
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
        source_instance_id: None,
        created_at: now.clone(),
        updated_at: now,
    })
}

/// Patch selected fields on a card. `None` leaves the existing value.
/// Empty `priority` is ignored; empty `tree_source_id` becomes NULL.
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
    if title.is_none()
        && description.is_none()
        && priority.is_none()
        && column.is_none()
        && source_status.is_none()
        && tree_source_id.is_none()
    {
        return Ok(());
    }

    if let Some(ref v) = column {
        crate::db::validate_column(v)?;
    }

    let conn = open_db(&app)?;
    conn.execute(
        r#"UPDATE cards SET
            title = COALESCE(?1, title),
            description = COALESCE(?2, description),
            priority = COALESCE(NULLIF(trim(?3), ''), priority),
            "column" = COALESCE(?4, "column"),
            source_status = COALESCE(?5, source_status),
            tree_source_id = CASE
                WHEN ?6 IS NULL THEN tree_source_id
                WHEN trim(?6) = '' THEN NULL
                ELSE ?6
            END,
            updated_at = ?7
          WHERE id = ?8"#,
        rusqlite::params![
            title,
            description,
            priority,
            column,
            source_status,
            tree_source_id,
            now_iso(),
            id,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Move a card to a new column/position. Always bumps updated_at. When
/// `position` is `None`, the card lands at the end of the target column
/// (max existing position + 1) — drag-drop always appends.
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

/// Delete a single card by id. Refuses if the card has an active agent run
/// (pending/running) — `agent_runs.card_id` is `ON DELETE CASCADE`, so
/// deleting would silently kill an in-progress run.
#[tauri::command]
pub async fn delete_card(app: AppHandle, id: String) -> Result<(), String> {
    let conn = open_db(&app)?;
    if crate::db::agent_runs::is_card_locked(&conn, &id) {
        return Err(format!("Card '{id}' has an active agent run and cannot be deleted"));
    }
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

/// Atomically remove every card + snapshot belonging to a source instance
/// (looked up by its `sources.id`). Wraps both deletes in a single
/// transaction so a crash between them can't leave orphaned snapshots
/// referencing deleted cards (fixes the TS non-atomicity bug).
///
/// Both `cards.source_instance_id` and `external_snapshots.source_instance_id`
/// are FKs to `sources.id`, so the delete is instance-scoped: clearing
/// instance A leaves instance B's cards + snapshots untouched even when both
/// share the same source *type*. Local cards (source = 'local') are never
/// touched. The frontend caller passes the selected source instance id.
#[tauri::command]
pub async fn delete_all_source_cards(app: AppHandle, source_id: String) -> Result<(), String> {
    let mut conn = open_db(&app)?;
    delete_all_source_cards_inner(&mut conn, &source_id)
}

/// Instance-scoped clear: delete every card + snapshot whose
/// `source_instance_id` matches `source_id`, in one transaction. Inner helper
/// so the SQL contract is unit-testable without an `AppHandle`.
pub fn delete_all_source_cards_inner(
    conn: &mut rusqlite::Connection,
    source_id: &str,
) -> Result<(), String> {
    // Refuse if any card under this source has an active agent run.
    // agent_runs.card_id is ON DELETE CASCADE — deleting would silently
    // kill in-progress runs.
    let locked: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_runs r
             JOIN cards c ON r.card_id = c.id
             WHERE c.source_instance_id = ?1 AND r.status IN ('pending', 'running')",
            rusqlite::params![source_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if locked > 0 {
        return Err(format!(
            "Source '{source_id}' has {locked} card(s) with active agent runs; cancel them first"
        ));
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM cards WHERE source_instance_id = ?1",
        rusqlite::params![source_id],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM external_snapshots WHERE source_instance_id = ?1",
        rusqlite::params![source_id],
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
        Ok(Some(row_to_card(row).map_err(|e| e.to_string())?))
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

/// Resolve a card's repo path via its `tree_source_id`. Used by the ACP
/// runner when no explicit `repo_path` is set on the card: looks up the
/// linked tree source's `path`. Returns `AcpError` so callers can use `?`
/// alongside other `AcpError` results.
pub fn resolve_repo_path(conn: &rusqlite::Connection, card: &Card) -> Result<String, AcpError> {
    let ts_id = card.tree_source_id.as_ref().ok_or_else(|| {
        AcpError::validation(format!("Card '{}' has no tree_source_id set.", card.id))
    })?;
    crate::db::settings::get_tree_source_path(conn, ts_id)?.ok_or_else(|| {
        AcpError::validation(format!(
            "Card '{}' links to missing tree_source '{}'.",
            card.id, ts_id
        ))
    })
}

/// Fetch a card by id. Sync helper used by the ACP runner; returns
/// `AcpError` so callers can use `?` alongside other `AcpError` results.
pub fn get_card_by_id(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<Option<Card>, crate::error::AcpError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {CARD_COLUMNS} FROM cards WHERE id = ?1 LIMIT 1"
        ))
        .map_err(crate::error::AcpError::internal)?;
    let mut rows = stmt
        .query(rusqlite::params![id])
        .map_err(crate::error::AcpError::internal)?;
    if let Some(row) = rows.next().map_err(crate::error::AcpError::internal)? {
        Ok(Some(
            row_to_card(row).map_err(crate::error::AcpError::internal)?,
        ))
    } else {
        Ok(None)
    }
}

/// Insert or overwrite an externally-sourced card, keyed by `(source, source_ref)`.
/// Preserves the existing id and per-column position on update; assigns a fresh
/// end-of-column position on insert. `updated_at` is remote-authoritative so a
/// sync never back-dates a card.
///
/// Runs on a borrowed connection so `sync_source` can use it inside one transaction.
pub fn upsert_card_from_sync_inner(conn: &rusqlite::Connection, card: &Card) -> Result<(), String> {
    let source_ref = card.source_ref.clone().unwrap_or_default();
    let instance_id = card.source_instance_id.as_deref().unwrap_or(&card.source);
    if find_existing_card_id(conn, instance_id, &source_ref)?.is_some() {
        update_synced_card(conn, card, &source_ref)
    } else {
        insert_synced_card(conn, card, &source_ref)
    }
}

fn find_existing_card_id(
    conn: &rusqlite::Connection,
    source_instance_id: &str,
    source_ref: &str,
) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT id FROM cards WHERE source_instance_id = ?1 AND source_ref = ?2",
        rusqlite::params![source_instance_id, source_ref],
        |row| row.get(0),
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
    .map_err(|e| e.to_string())
}

fn update_synced_card(
    conn: &rusqlite::Connection,
    card: &Card,
    source_ref: &str,
) -> Result<(), String> {
    conn.execute(
        r#"UPDATE cards SET
             title = ?1, description = ?2, priority = ?3, source_status = ?4,
             "column" = ?5, updated_at = ?6, source_instance_id = ?7
           WHERE source_instance_id = ?8 AND source_ref = ?9"#,
        rusqlite::params![
            card.title,
            card.description,
            card.priority,
            card.source_status,
            card.column,
            card.updated_at,
            card.source_instance_id,
            card.source_instance_id,
            source_ref,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn insert_synced_card(
    conn: &rusqlite::Connection,
    card: &Card,
    source_ref: &str,
) -> Result<(), String> {
    let position = max_position(conn, &card.column)? + 1;
    conn.execute(
        r#"INSERT INTO cards
             (id, title, description, priority, "column", source, position, source_ref, source_status, source_instance_id, created_at, updated_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"#,
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
            card.source_instance_id,
            card.created_at,
            card.updated_at,
        ],
    )
    .map_err(|e| e.to_string())?;
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

#[cfg(test)]
mod source_instance_tests {
    use super::delete_all_source_cards_inner;
    use crate::db::test_db;

    fn seed_source(conn: &rusqlite::Connection, id: &str, source_type: &str) {
        conn.execute(
            "INSERT INTO sources (id, source_type, label, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, source_type, source_type, "2026-01-01T00:00:00Z"],
        )
        .unwrap();
    }

    fn insert_card(
        conn: &rusqlite::Connection,
        id: &str,
        source: &str,
        source_ref: &str,
        instance: &str,
    ) {
        conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, source_ref, source_instance_id, created_at, updated_at)
               VALUES (?1, ?2, 'backlog', ?3, 0, ?4, ?5, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
            rusqlite::params![id, id, source, source_ref, instance],
        )
        .unwrap();
    }

    fn insert_snapshot(conn: &rusqlite::Connection, instance: &str, source_ref: &str) {
        conn.execute(
            r#"INSERT INTO external_snapshots (source_instance_id, source, source_ref, title, synced_at)
               VALUES (?1, 'test', ?2, 't', '2026-01-01T00:00:00Z')"#,
            rusqlite::params![instance, source_ref],
        )
        .unwrap();
    }

    fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn delete_clears_only_the_named_instance() {
        let mut conn = test_db();
        seed_source(&conn, "src-a", "test");
        seed_source(&conn, "src-b", "test");
        insert_card(&conn, "test-A1", "test", "A-1", "src-a");
        insert_card(&conn, "test-B1", "test", "B-1", "src-b");
        insert_snapshot(&conn, "src-a", "A-1");
        insert_snapshot(&conn, "src-b", "B-1");

        delete_all_source_cards_inner(&mut conn, "src-a").unwrap();

        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM cards WHERE source_instance_id = 'src-a'"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM cards WHERE source_instance_id = 'src-b'"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM external_snapshots WHERE source_instance_id = 'src-a'"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM external_snapshots WHERE source_instance_id = 'src-b'"
            ),
            1
        );
    }

    #[test]
    fn delete_leaves_local_cards_untouched() {
        let mut conn = test_db();
        seed_source(&conn, "src-a", "test");
        insert_card(&conn, "test-A1", "test", "A-1", "src-a");
        conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, created_at, updated_at)
               VALUES ('local-1', 'mine', 'backlog', 'local', 0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
            [],
        )
        .unwrap();

        delete_all_source_cards_inner(&mut conn, "src-a").unwrap();

        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM cards WHERE source = 'local'"),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM cards WHERE source_instance_id = 'src-a'"
            ),
            0
        );
    }

    #[test]
    fn card_fk_rejects_orphan_source_instance_id() {
        let conn = test_db();
        let result = conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, source_instance_id, created_at, updated_at)
               VALUES (?1, ?2, 'backlog', 'test', 0, ?3, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
            rusqlite::params!["c-1", "c-1", "no-such-instance"],
        );
        assert!(
            result.is_err(),
            "FK should reject insert with non-existent source_instance_id"
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
    fn snapshot_fk_rejects_orphan_source_instance_id() {
        let conn = test_db();
        let result = conn.execute(
            r#"INSERT INTO external_snapshots (source_instance_id, source, source_ref, title, synced_at)
               VALUES (?1, 'test', 'PROJ-1', 't', '2026-01-01T00:00:00Z')"#,
            rusqlite::params!["no-such-instance"],
        );
        assert!(
            result.is_err(),
            "FK should reject snapshot insert with non-existent source_instance_id"
        );
    }
}

#[cfg(test)]
mod resolve_repo_path_tests {
    use super::{get_card_by_id, resolve_repo_path};
    use crate::db::test_db;

    fn insert_tree_source(conn: &rusqlite::Connection, id: &str, path: &str) {
        conn.execute(
            "INSERT INTO tree_sources (id, label, path, editor_command, created_at)
             VALUES (?1, ?2, ?3, NULL, '2026-01-01T00:00:00Z')",
            rusqlite::params![id, id, path],
        )
        .unwrap();
    }

    fn insert_card(conn: &rusqlite::Connection, id: &str, tree_source_id: Option<&str>) {
        conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, tree_source_id, created_at, updated_at)
               VALUES (?1, ?2, 'backlog', 'local', 0, ?3, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
            rusqlite::params![id, id, tree_source_id],
        )
        .unwrap();
    }

    #[test]
    fn resolve_repo_path_none_tree_source_id() {
        let conn = test_db();
        insert_card(&conn, "c-1", None);
        let card = get_card_by_id(&conn, "c-1").unwrap().unwrap();

        let err = resolve_repo_path(&conn, &card).unwrap_err();
        assert!(
            err.to_string().contains("tree_source_id"),
            "error should mention tree_source_id, got: {err}"
        );
    }

    #[test]
    fn resolve_repo_path_missing_tree_source() {
        let conn = test_db();
        // Bypass FK to simulate a dangling tree_source_id (the row it
        // pointed to was deleted after the card was linked).
        conn.execute("PRAGMA foreign_keys=OFF", []).unwrap();
        insert_card(&conn, "c-1", Some("nonexistent-ts"));
        conn.execute("PRAGMA foreign_keys=ON", []).unwrap();
        let card = get_card_by_id(&conn, "c-1").unwrap().unwrap();

        let err = resolve_repo_path(&conn, &card).unwrap_err();
        assert!(
            err.to_string().contains("missing tree_source"),
            "error should mention missing tree_source, got: {err}"
        );
    }

    #[test]
    fn resolve_repo_path_happy_path() {
        let conn = test_db();
        insert_tree_source(&conn, "ts-1", "/tmp/myrepo");
        insert_card(&conn, "c-1", Some("ts-1"));
        let card = get_card_by_id(&conn, "c-1").unwrap().unwrap();

        let path = resolve_repo_path(&conn, &card).unwrap();
        assert_eq!(path, "/tmp/myrepo");
    }
}
