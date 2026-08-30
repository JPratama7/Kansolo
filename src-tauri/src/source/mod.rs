pub mod jira;

use std::collections::HashMap;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::db::Card;
use crate::sync::FieldConflict;

/// A card as returned by a `SourceProvider` before status/priority mapping
/// is applied. `source_ref` is the provider-native id (Jira key, GitHub
/// issue number, etc.); `status_name`/`priority_name` are the raw,
/// unmapped strings reported by the upstream system.
#[derive(Debug, Clone)]
pub struct RawCard {
    pub source_ref: String,
    pub title: String,
    pub description: String,
    pub status_name: String,
    pub priority_name: Option<String>,
}

/// Plugin seam for an external card source (Jira, GitHub, etc.).
///
/// Each provider implements this trait and registers itself in `registry()`.
/// The frontend supplies a per-source settings UI component, so the trait
/// intentionally exposes no `config_schema` method — only the data needed to
/// list, fetch, and (optionally) discover selectable options for a source.
#[async_trait]
pub trait SourceProvider: Send + Sync {
    /// Stable identifier persisted on `SourceInstance.source_type`
    /// (e.g. `"jira"`, `"github"`).
    fn source_type(&self) -> &'static str;

    /// Human-readable label shown in the "Add source" picker.
    fn display_label(&self) -> &'static str;

    /// Fetch cards from the upstream system in their raw, unmapped form.
    /// `config` is the opaque JSON blob stored on the `SourceInstance`.
    async fn fetch_raw(&self, config: &serde_json::Value) -> Result<Vec<RawCard>, String>;

    /// Fetch selectable options (projects, repos, boards, …) for the
    /// settings UI given partial config. Default returns an empty object
    /// for providers that don't need a discovery step.
    async fn fetch_options(
        &self,
        _config: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({}))
    }
}

/// Metadata for one registered source type, returned to the frontend so it
/// can render the "Add source" picker without knowing the providers ahead
/// of time.
#[derive(Debug, Clone, Serialize)]
pub struct SourceTypeMeta {
    pub source_type: String,
    pub label: String,
}

/// A single card whose local copy and remote snapshot disagree on one or
/// more fields. `conflicts` lists the divergent fields; the frontend asks
/// the user to resolve each via a `ConflictResolution`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConflict {
    pub source_ref: String,
    pub card: Card,
    pub conflicts: Vec<FieldConflict>,
    pub remote: Card,
}

/// Outcome of a sync run: conflicts needing user resolution, statuses the
/// user's mapping doesn't cover (so the UI can prompt to extend it), and
/// the ISO-8601 timestamp the sync completed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub conflicts: Vec<SyncConflict>,
    pub unmapped_statuses: Vec<String>,
    pub synced_at: String,
    /// Number of cards created or updated this run (excludes noops/conflicts).
    pub imported_count: usize,
}

/// User's per-field resolution for one conflicting card, sent from the
/// frontend. `choices` maps each conflicting field name to either
/// `"local"` or `"remote"`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictResolution {
    pub source_ref: String,
    pub choices: HashMap<String, String>,
}

/// Result of fetching + mapping cards from a source: the mapped cards plus
/// any upstream statuses the user's mapping doesn't cover.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchResult {
    pub cards: Vec<Card>,
    pub unmapped_statuses: Vec<String>,
}

/// All registered source providers, keyed by `source_type`.
///
/// Built once and cached for the process lifetime via a `OnceLock` — the
/// provider set is static and the HashMap is only ever read after init, so
/// there's no need to rebuild it on every call. Returns a `&'static`
/// reference; callers borrow rather than take ownership.
pub fn registry() -> &'static HashMap<&'static str, Box<dyn SourceProvider>> {
    static REGISTRY: OnceLock<HashMap<&'static str, Box<dyn SourceProvider>>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut map: HashMap<&'static str, Box<dyn SourceProvider>> = HashMap::new();
        map.insert("jira", Box::new(jira::JiraSource) as Box<dyn SourceProvider>);
        map
    })
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

use std::collections::HashSet;

use rusqlite::params;
use tauri::AppHandle;

use crate::db::{get_snapshot_inner, now_iso, open_db, save_snapshot_inner, SourceInstance};
use crate::db::cards::{
    get_card_by_source_ref_inner, upsert_card_from_sync, upsert_card_from_sync_inner,
};
use crate::db::settings::{get_source, save_snapshot};
use crate::mapping::{
    is_status_mapped, resolve_column, resolve_priority, StatusMapping,
};
use crate::sync::{apply_resolution, plan_sync, snapshot_from_card, Choice, SyncDecisionType};

/// Map a raw upstream card into a local [`Card`] using the source's
/// status→column and priority mappings. Returns the card plus `false` when
/// the upstream status isn't covered by the mapping (so the caller can
/// collect it into `unmapped_statuses`).
fn map_raw_card(raw: &RawCard, source_type: &str, mapping: &StatusMapping) -> (Card, bool) {
    let column = resolve_column(&raw.status_name, mapping).to_string();
    let priority = resolve_priority(raw.priority_name.as_deref()).to_string();
    let mapped = is_status_mapped(&raw.status_name, mapping);
    let now = now_iso();
    let card = Card {
        id: format!("{}-{}", source_type, raw.source_ref),
        title: raw.title.clone(),
        description: raw.description.clone(),
        priority,
        column,
        source: source_type.to_string(),
        position: 0,
        source_ref: Some(raw.source_ref.clone()),
        source_status: Some(raw.status_name.clone()),
        tree_source_id: None,
        repo_path: None,
        created_at: now.clone(),
        updated_at: now,
    };
    (card, mapped)
}

/// Load a `SourceInstance` row by id, returning a descriptive error if it's
/// missing or its provider isn't registered.
async fn load_instance(
    app: &AppHandle,
    source_id: &str,
) -> Result<(SourceInstance, &'static str, &'static Box<dyn SourceProvider>), String> {
    let instance = get_source(app.clone(), source_id.to_string())
        .await?
        .ok_or_else(|| format!("No source instance found for id `{source_id}`"))?;
    let reg = registry();
    let provider = reg
        .get(instance.source_type.as_str())
        .ok_or_else(|| format!("No provider registered for source type `{}`", instance.source_type))?;
    let source_type = provider.source_type();
    Ok((instance, source_type, provider))
}

/// Fetch cards from a source and apply status/priority mapping. Returns the
/// mapped cards plus any upstream statuses the user's mapping doesn't cover.
#[tauri::command]
pub async fn fetch_source_cards(
    app: AppHandle,
    source_id: String,
) -> Result<FetchResult, String> {
    let (instance, source_type, provider) = load_instance(&app, &source_id).await?;
    let raw_cards = provider.fetch_raw(&instance.config).await?;

    let mut cards = Vec::with_capacity(raw_cards.len());
    let mut unmapped: HashSet<String> = HashSet::new();
    for raw in &raw_cards {
        let (card, mapped) = map_raw_card(raw, source_type, &instance.status_mapping);
        if !mapped {
            unmapped.insert(raw.status_name.clone());
        }
        cards.push(card);
    }
    Ok(FetchResult {
        cards,
        unmapped_statuses: unmapped.into_iter().collect(),
    })
}

/// Fetch selectable options (projects, repos, boards, …) for a source's
/// settings UI given its current config.
#[tauri::command]
pub async fn fetch_source_options(
    app: AppHandle,
    source_id: String,
) -> Result<serde_json::Value, String> {
    let (instance, _source_type, provider) = load_instance(&app, &source_id).await?;
    provider.fetch_options(&instance.config).await
}

/// List every registered source type with its display label, so the frontend
/// can render the "Add source" picker without hard-coding provider ids.
#[tauri::command]
pub async fn list_source_types() -> Result<Vec<SourceTypeMeta>, String> {
    let reg = registry();
    let mut out: Vec<SourceTypeMeta> = reg
        .iter()
        .map(|(source_type, provider)| SourceTypeMeta {
            source_type: source_type.to_string(),
            label: provider.display_label().to_string(),
        })
        .collect();
    // Stable order so the picker doesn't reshuffle between calls.
    out.sort_by(|a, b| a.source_type.cmp(&b.source_type));
    Ok(out)
}

/// Sync a source: fetch remote cards, 3-way merge against local + snapshot,
/// persist creates/updates, and stash conflicts in `pending_conflicts` for
/// the frontend to resolve via [`resolve_conflicts`].
///
/// Thin Tauri wrapper — opens one connection and delegates to
/// [`sync_source_inner`], which runs the whole pipeline inside a single
/// transaction so a mid-loop failure rolls back every upsert / snapshot /
/// pending-conflict write from this run.
#[tauri::command]
pub async fn sync_source(app: AppHandle, source_id: String) -> Result<SyncResult, String> {
    let (instance, source_type, provider) = load_instance(&app, &source_id).await?;
    let mut conn = open_db(&app)?;
    sync_source_inner(
        &mut conn,
        &source_id,
        source_type,
        &instance.status_mapping,
        provider.as_ref(),
        &instance.config,
    )
    .await
}

/// Atomic sync pipeline. Fetches raw cards from `provider`, then runs the
/// 3-way merge + persist loop inside one transaction on `conn`:
///
/// - All card upserts, snapshot saves, and `pending_conflicts` inserts share
///   the same transaction, so either the entire run commits or nothing does.
/// - Any error returned from an inner helper (or the conflict insert) bubbles
///   up via `?`; the `Transaction` is then dropped without `commit()`, which
///   rolls it back — no partial upserts or conflicts survive.
///
/// Takes a `&dyn SourceProvider` so tests can substitute a fake double
/// without going through the Tauri command / `AppHandle` seam.
pub async fn sync_source_inner(
    conn: &mut rusqlite::Connection,
    source_id: &str,
    source_type: &str,
    status_mapping: &StatusMapping,
    provider: &dyn SourceProvider,
    config: &serde_json::Value,
) -> Result<SyncResult, String> {
    let raw_cards = provider.fetch_raw(config).await?;
    let synced_at = now_iso();

    let mut conflicts: Vec<SyncConflict> = Vec::new();
    let mut unmapped: HashSet<String> = HashSet::new();
    let mut imported: usize = 0;

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for raw in &raw_cards {
        let (remote, mapped) = map_raw_card(raw, source_type, status_mapping);
        if !mapped {
            unmapped.insert(raw.status_name.clone());
        }

        let source_ref = raw.source_ref.clone();
        let local = get_card_by_source_ref_inner(&tx, source_type, &source_ref)?;
        let snapshot = get_snapshot_inner(&tx, source_type, &source_ref)?;

        let decision = plan_sync(&remote, local.as_ref(), snapshot.as_ref());
        match decision.decision_type {
            SyncDecisionType::Create | SyncDecisionType::Update => {
                if let Some(card) = decision.card {
                    upsert_card_from_sync_inner(&tx, &card)?;
                    let snap = snapshot_from_card(&card, source_type, &synced_at);
                    save_snapshot_inner(&tx, &snap)?;
                    imported += 1;
                }
            }
            SyncDecisionType::Conflict => {
                let conflict = SyncConflict {
                    source_ref: source_ref.clone(),
                    card: decision.card.unwrap_or(remote.clone()),
                    conflicts: decision.conflicts.unwrap_or_default(),
                    remote: decision.remote.unwrap_or(remote),
                };
                let conflict_json =
                    serde_json::to_string(&conflict).map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT INTO pending_conflicts (source_id, source_ref, conflict_json, created_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(source_id, source_ref) DO UPDATE SET
                       conflict_json = ?3, created_at = ?4",
                    params![source_id, source_ref, conflict_json, synced_at],
                )
                .map_err(|e| e.to_string())?;
                conflicts.push(conflict);
            }
            SyncDecisionType::Noop => {}
        }
    }
    tx.commit().map_err(|e| e.to_string())?;

    Ok(SyncResult {
        conflicts,
        unmapped_statuses: unmapped.into_iter().collect(),
        synced_at,
        imported_count: imported,
    })
}

/// Resolve previously-persisted conflicts. For each resolution: load the
/// `pending_conflicts` row, deserialize the `SyncConflict`, apply the user's
/// per-field choices, persist the resolved card + fresh snapshot, then delete
/// the row. Errors on one resolution abort the batch (frontend re-submits).
#[tauri::command]
pub async fn resolve_conflicts(
    app: AppHandle,
    source_id: String,
    resolutions: Vec<ConflictResolution>,
) -> Result<(), String> {
    let conn = open_db(&app)?;
    // Look up the source instance once to get the source_type for snapshots.
    let instance = get_source(app.clone(), source_id.clone())
        .await?
        .ok_or_else(|| format!("No source instance found for id `{source_id}`"))?;
    let source_type = instance.source_type.clone();
    let now = now_iso();

    for resolution in resolutions {
        let row = conn
            .query_row(
                "SELECT conflict_json FROM pending_conflicts
                 WHERE source_id = ?1 AND source_ref = ?2 LIMIT 1",
                params![source_id, resolution.source_ref],
                |r| r.get::<_, String>(0),
            )
            .map_err(|e| e.to_string())?;
        let conflict: SyncConflict =
            serde_json::from_str(&row).map_err(|e| e.to_string())?;

        // Translate the string choices ("local"/"remote") into `Choice`.
        let mut choices: HashMap<String, Choice> = HashMap::new();
        for (field, value) in &resolution.choices {
            let choice = match value.as_str() {
                "remote" => Choice::Remote,
                _ => Choice::Local,
            };
            choices.insert(field.clone(), choice);
        }

        let resolved = apply_resolution(&conflict.card, &conflict.conflicts, &choices);
        upsert_card_from_sync(app.clone(), resolved.clone()).await?;
        let snap = snapshot_from_card(&resolved, &source_type, &now);
        save_snapshot(app.clone(), snap).await?;

        conn.execute(
            "DELETE FROM pending_conflicts WHERE source_id = ?1 AND source_ref = ?2",
            params![source_id, resolution.source_ref],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Build a JQL string from builder parts (frontend preview before saving).
/// Deserializes the camelCase `JqlParts` shape, runs `build_jql`, returns
/// the composed query (possibly empty).
#[tauri::command]
pub async fn preview_jql(jql_parts: serde_json::Value) -> Result<String, String> {
    let parts: jira::JqlParts =
        serde_json::from_value(jql_parts).map_err(|e| format!("Invalid jql_parts: {e}"))?;
    Ok(jira::build_jql(&parts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_db;
    use async_trait::async_trait;

    /// Fake `SourceProvider` test double. Returns a fixed batch of raw cards
    /// (or an error) without any network I/O. Confirms the trait is
    /// object-safe and mockable through the `&dyn SourceProvider` seam.
    struct FakeProvider {
        cards: Vec<RawCard>,
        fail: bool,
    }

    #[async_trait]
    impl SourceProvider for FakeProvider {
        fn source_type(&self) -> &'static str {
            "test"
        }
        fn display_label(&self) -> &'static str {
            "Test"
        }
        async fn fetch_raw(&self, _config: &serde_json::Value) -> Result<Vec<RawCard>, String> {
            if self.fail {
                Err("fetch failed".to_string())
            } else {
                Ok(self.cards.clone())
            }
        }
    }

    fn raw(src_ref: &str, title: &str, status: &str) -> RawCard {
        RawCard {
            source_ref: src_ref.to_string(),
            title: title.to_string(),
            description: String::new(),
            status_name: status.to_string(),
            priority_name: None,
        }
    }

    fn mapping() -> StatusMapping {
        StatusMapping {
            backlog: vec!["To Do".to_string()],
            ongoing: vec!["In Progress".to_string()],
            done: vec!["Done".to_string()],
        }
    }

    fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[tokio::test]
    async fn happy_path_persists_cards_and_snapshots() {
        let mut conn = test_db();
        let provider = FakeProvider {
            cards: vec![raw("PROJ-1", "First", "To Do"), raw("PROJ-2", "Second", "Done")],
            fail: false,
        };
        let result = sync_source_inner(
            &mut conn,
            "src-1",
            "test",
            &mapping(),
            &provider,
            &serde_json::json!({}),
        )
        .await
        .expect("happy path sync should succeed");

        assert_eq!(result.imported_count, 2);
        assert!(result.conflicts.is_empty());
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM cards WHERE source = 'test'"), 2);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM external_snapshots WHERE source = 'test'"),
            2
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM pending_conflicts"), 0);
    }

    #[tokio::test]
    async fn mid_loop_failure_rolls_back_transaction() {
        let mut conn = test_db();
        // Pre-insert a card whose id collides with the id `map_raw_card` will
        // build for PROJ-2 (`{source_type}-{source_ref}` = "test-PROJ-2"), but
        // with a different source_ref so the upsert takes the INSERT path and
        // hits the PRIMARY KEY violation. PROJ-1 upserts first; PROJ-2 fails
        // mid-loop; the whole transaction must roll back.
        conn.execute(
            r#"INSERT INTO cards (id, title, "column", source, position, source_ref, created_at, updated_at)
               VALUES ('test-PROJ-2', 'blocker', 'backlog', 'test', 99, 'OTHER', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
            [],
        )
        .unwrap();

        let provider = FakeProvider {
            cards: vec![raw("PROJ-1", "First", "To Do"), raw("PROJ-2", "Second", "Done")],
            fail: false,
        };
        let err = sync_source_inner(
            &mut conn,
            "src-1",
            "test",
            &mapping(),
            &provider,
            &serde_json::json!({}),
        )
        .await
        .expect_err("mid-loop failure should surface an error");
        assert!(err.contains("UNIQUE") || err.contains("constraint") || err.contains("PRIMARY"),
            "error should mention the constraint violation, got: {err}");

        // PROJ-1 was upserted inside the transaction before the failure — it
        // must have been rolled back.
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM cards WHERE source = 'test' AND source_ref = 'PROJ-1'"),
            0
        );
        // No snapshots persisted this run.
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM external_snapshots WHERE source = 'test'"),
            0
        );
        // No pending conflicts persisted.
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM pending_conflicts"), 0);
        // The pre-existing row (inserted outside the tx) survives.
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM cards WHERE source = 'test' AND source_ref = 'OTHER'"),
            1
        );
    }

    #[tokio::test]
    async fn fetch_failure_persists_nothing() {
        let mut conn = test_db();
        let provider = FakeProvider {
            cards: vec![raw("PROJ-1", "First", "To Do")],
            fail: true,
        };
        let err = sync_source_inner(
            &mut conn,
            "src-1",
            "test",
            &mapping(),
            &provider,
            &serde_json::json!({}),
        )
        .await
        .expect_err("fetch failure should surface an error");
        assert!(err.contains("fetch failed"), "got: {err}");
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM cards WHERE source = 'test'"), 0);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM pending_conflicts"), 0);
    }
}
