//! Pure 3-way merge logic for syncing external (Jira/GitHub/...) issues into
//! local cards. Ported from `src/sync.ts` -- no I/O, no Tauri commands, no DB.
//!
//! The TS module used `jiraKey` / `jiraStatus`; the generalized Rust `Card`
//! carries `source_ref` / `source_status` instead, and `ExternalSnapshot`
//! mirrors that shape. `MergeField::SourceStatus` is the renamed `jiraStatus`.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::db::{Card, ExternalSnapshot};

// ---------------------------------------------------------------------------
// MergeField
// ---------------------------------------------------------------------------

/// Fields that participate in the 3-way merge.
///
/// Serialized as the camelCase string the TS frontend sends
/// (`"title"`, `"description"`, `"priority"`, `"column"`, `"sourceStatus"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MergeField {
    Title,
    Description,
    Priority,
    Column,
    SourceStatus,
}

impl MergeField {
    /// All merge fields, in stable order (matches `MERGE_FIELDS` in sync.ts).
    pub const ALL: &'static [MergeField] = &[
        MergeField::Title,
        MergeField::Description,
        MergeField::Priority,
        MergeField::Column,
        MergeField::SourceStatus,
    ];

    fn as_str(self) -> &'static str {
        match self {
            MergeField::Title => "title",
            MergeField::Description => "description",
            MergeField::Priority => "priority",
            MergeField::Column => "column",
            MergeField::SourceStatus => "sourceStatus",
        }
    }
}

impl fmt::Display for MergeField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MergeField {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "title" => Ok(MergeField::Title),
            "description" => Ok(MergeField::Description),
            "priority" => Ok(MergeField::Priority),
            "column" => Ok(MergeField::Column),
            "sourceStatus" => Ok(MergeField::SourceStatus),
            other => Err(format!("unknown merge field: {other}")),
        }
    }
}

// Manual serde impls that round-trip through Display/FromStr so the wire
// format stays the plain string the TS frontend expects.
impl Serialize for MergeField {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for MergeField {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        MergeField::from_str(&s).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// FieldConflict / SyncDecision / Choice
// ---------------------------------------------------------------------------

/// One field where local and remote both diverged from the snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldConflict {
    pub field: MergeField,
    pub local: String,
    pub remote: String,
}

/// Per-card outcome of `plan_sync`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncDecisionType {
    Create,
    Update,
    Conflict,
    Noop,
}

/// Per-card outcome of `plan_sync`.
///
/// `card` is the card to persist for `create`/`update`, or the partially
/// merged card for `conflict`. `conflicts` is present only for `conflict`.
/// `remote` is the remote card that produced this decision (used to snapshot
/// after merge).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDecision {
    pub decision_type: SyncDecisionType,
    pub card: Option<Card>,
    pub conflicts: Option<Vec<FieldConflict>>,
    pub remote: Option<Card>,
}

/// User's per-field resolution choice for `apply_resolution`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Choice {
    Local,
    Remote,
}

// ---------------------------------------------------------------------------
// Field access helpers (mirror `readField` / `applyField` in sync.ts)
// ---------------------------------------------------------------------------

/// Read a merge field off a card or snapshot as a comparable string.
///
/// `Card.source_status` is optional (mirrors the TS `jiraStatus ?? ''`);
/// `ExternalSnapshot.source_status` is required.
trait MergeSource {
    fn read_merge_field(&self, field: MergeField) -> String;
}

impl MergeSource for Card {
    fn read_merge_field(&self, field: MergeField) -> String {
        match field {
            MergeField::Title => self.title.clone(),
            MergeField::Description => self.description.clone(),
            MergeField::Priority => self.priority.clone(),
            MergeField::Column => self.column.clone(),
            MergeField::SourceStatus => self.source_status.clone().unwrap_or_default(),
        }
    }
}

impl MergeSource for ExternalSnapshot {
    fn read_merge_field(&self, field: MergeField) -> String {
        match field {
            MergeField::Title => self.title.clone(),
            MergeField::Description => self.description.clone(),
            MergeField::Priority => self.priority.clone(),
            MergeField::Column => self.column.clone(),
            MergeField::SourceStatus => self.source_status.clone(),
        }
    }
}

/// Write a field value back onto a card (mirror `applyField` in sync.ts).
fn apply_field(card: &mut Card, field: MergeField, value: String) {
    match field {
        MergeField::Title => card.title = value,
        MergeField::Description => card.description = value,
        MergeField::Priority => card.priority = value,
        MergeField::Column => card.column = value,
        MergeField::SourceStatus => card.source_status = Some(value),
    }
}

// ---------------------------------------------------------------------------
// plan_sync
// ---------------------------------------------------------------------------

/// Pure 3-way merge for one external card.
///
/// - No local card -> `create` (insert remote).
/// - No snapshot (first sync after migration) -> `update` with remote; local
///   edits cannot be detected without a baseline and are overwritten this once.
/// - Otherwise: per field, compare local vs snapshot (local edit?) and remote
///   vs snapshot (remote change?). Both changed and differ -> conflict. Only
///   remote changed -> take remote. Only local changed -> keep local. Neither
///   -> keep local. If any conflict -> `conflict` with the partially-merged
///   card (non-conflicting remote fields applied, conflicting fields left as
///   local) plus the conflict list. Else `update` if anything changed, else
///   `noop`.
pub fn plan_sync(
    remote: &Card,
    local: Option<&Card>,
    snapshot: Option<&ExternalSnapshot>,
) -> SyncDecision {
    let Some(local) = local else {
        return create_decision(remote);
    };
    let Some(snapshot) = snapshot else {
        return first_sync_update(remote, local);
    };
    let (conflicts, merged) = merge_3way(local, snapshot, remote);
    if !conflicts.is_empty() {
        SyncDecision {
            decision_type: SyncDecisionType::Conflict,
            card: Some(merged),
            conflicts: Some(conflicts),
            remote: Some(remote.clone()),
        }
    } else {
        update_or_noop(local, &merged)
    }
}

fn create_decision(remote: &Card) -> SyncDecision {
    SyncDecision {
        decision_type: SyncDecisionType::Create,
        card: Some(remote.clone()),
        conflicts: None,
        remote: None,
    }
}

fn first_sync_update(remote: &Card, local: &Card) -> SyncDecision {
    // First migration sync: take remote content, preserve local id/position.
    let mut card = remote.clone();
    card.id = local.id.clone();
    card.position = local.position;
    SyncDecision {
        decision_type: SyncDecisionType::Update,
        card: Some(card),
        conflicts: None,
        remote: None,
    }
}

fn merge_3way(
    local: &Card,
    snapshot: &ExternalSnapshot,
    remote: &Card,
) -> (Vec<FieldConflict>, Card) {
    let mut conflicts = Vec::new();
    // Start from local (preserves id, position, createdAt, source).
    let mut merged = local.clone();

    for &field in MergeField::ALL {
        let local_val = local.read_merge_field(field);
        let snap_val = snapshot.read_merge_field(field);
        let remote_val = remote.read_merge_field(field);
        let local_changed = local_val != snap_val;
        let remote_changed = remote_val != snap_val;

        if local_changed && remote_changed && local_val != remote_val {
            conflicts.push(FieldConflict {
                field,
                local: local_val,
                remote: remote_val,
            });
            // Conflicting fields stay local; the UI resolves them.
        } else if remote_changed {
            apply_field(&mut merged, field, remote_val);
        }
        // else: keep local (either unchanged or locally edited).
    }

    (conflicts, merged)
}

fn update_or_noop(local: &Card, merged: &Card) -> SyncDecision {
    let changed = MergeField::ALL
        .iter()
        .any(|&f| merged.read_merge_field(f) != local.read_merge_field(f));
    SyncDecision {
        decision_type: if changed {
            SyncDecisionType::Update
        } else {
            SyncDecisionType::Noop
        },
        card: if changed { Some(merged.clone()) } else { None },
        conflicts: None,
        remote: None,
    }
}

// ---------------------------------------------------------------------------
// apply_resolution
// ---------------------------------------------------------------------------

/// Apply a user's merge resolution onto a conflict card. For each resolved
/// field, pick local or remote; unresolved fields keep the local value already
/// on the card. Returns the final card to persist.
///
/// `choices` keys are field name strings (`"title"`, `"sourceStatus"`, …)
/// matching the TS `ConflictResolution.choices` shape.
pub fn apply_resolution(
    conflict_card: &Card,
    conflicts: &[FieldConflict],
    choices: &HashMap<String, Choice>,
) -> Card {
    let mut resolved = conflict_card.clone();
    for c in conflicts {
        let pick = choices
            .get(c.field.as_str())
            .copied()
            .unwrap_or(Choice::Local);
        let value = match pick {
            Choice::Remote => c.remote.clone(),
            Choice::Local => c.local.clone(),
        };
        apply_field(&mut resolved, c.field, value);
    }
    resolved
}

// ---------------------------------------------------------------------------
// snapshot_from_card
// ---------------------------------------------------------------------------

/// Build a snapshot from a remote card (the external state at this sync
/// instant). `source_instance_id` is the owning `sources.id`; `source` is the
/// originating system type string (e.g. `"jira"`) kept for display.
pub fn snapshot_from_card(
    remote: &Card,
    source_instance_id: &str,
    source: &str,
    synced_at: &str,
) -> ExternalSnapshot {
    ExternalSnapshot {
        source_instance_id: source_instance_id.to_string(),
        source: source.to_string(),
        source_ref: remote.source_ref.clone().unwrap_or_default(),
        title: remote.title.clone(),
        description: remote.description.clone(),
        priority: remote.priority.clone(),
        source_status: remote.source_status.clone().unwrap_or_default(),
        column: remote.column.clone(),
        synced_at: synced_at.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests — mirror every case from `src/sync.test.ts`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: &str = "2026-08-24T00:00:00.000Z";

    /// Build a `Card` with the same defaults as `makeCard` in sync.test.ts.
    /// `jiraKey` → `source_ref`, `jiraStatus` → `source_status`.
    fn make_card(f: impl FnOnce(&mut Card)) -> Card {
        let mut card = Card {
            id: "jira-PROJ-1".to_string(),
            title: "t".to_string(),
            description: "d".to_string(),
            priority: "medium".to_string(),
            column: "backlog".to_string(),
            source: "jira".to_string(),
            position: 0,
            source_ref: Some("PROJ-1".to_string()),
            source_status: Some("To Do".to_string()),
            tree_source_id: None,
            source_instance_id: None,
            created_at: NOW.to_string(),
            updated_at: NOW.to_string(),
        };
        f(&mut card);
        card
    }

    /// Build an `ExternalSnapshot` with the same defaults as `makeSnap` in
    /// sync.test.ts. `jiraKey` → `source_ref`, `jiraStatus` → `source_status`.
    fn make_snap(f: impl FnOnce(&mut ExternalSnapshot)) -> ExternalSnapshot {
        let mut snap = ExternalSnapshot {
            source_instance_id: "src-1".to_string(),
            source: "jira".to_string(),
            source_ref: "PROJ-1".to_string(),
            title: "t".to_string(),
            description: "d".to_string(),
            priority: "medium".to_string(),
            source_status: "To Do".to_string(),
            column: "backlog".to_string(),
            synced_at: NOW.to_string(),
        };
        f(&mut snap);
        snap
    }

    #[test]
    fn no_local_card_creates_with_remote() {
        let remote = make_card(|c| c.title = "New".to_string());
        let d = plan_sync(&remote, None, None);
        assert_eq!(d.decision_type, SyncDecisionType::Create);
        assert_eq!(d.card.as_ref().unwrap(), &remote);
    }

    #[test]
    fn local_exists_no_snapshot_updates_with_remote() {
        let local = make_card(|c| {
            c.id = "local-1".to_string();
            c.position = 5;
            c.title = "local edit".to_string();
        });
        let remote = make_card(|c| c.title = "remote".to_string());
        let d = plan_sync(&remote, Some(&local), None);
        assert_eq!(d.decision_type, SyncDecisionType::Update);
        let card = d.card.as_ref().expect("card present");
        // Preserves local id + position; takes remote content.
        assert_eq!(card.id, "local-1");
        assert_eq!(card.position, 5);
        assert_eq!(card.title, "remote");
    }

    #[test]
    fn remote_only_changed_updates_take_remote() {
        let local = make_card(|_| {});
        let snap = make_snap(|_| {});
        let remote = make_card(|c| {
            c.title = "t-new".to_string();
            c.source_status = Some("In Progress".to_string());
            c.column = "ongoing".to_string();
        });
        let d = plan_sync(&remote, Some(&local), Some(&snap));
        assert_eq!(d.decision_type, SyncDecisionType::Update);
        let card = d.card.as_ref().unwrap();
        assert_eq!(card.title, "t-new");
        assert_eq!(card.source_status.as_deref(), Some("In Progress"));
        assert_eq!(card.column, "ongoing");
        // Untouched fields keep local.
        assert_eq!(card.description, "d");
    }

    #[test]
    fn local_only_changed_keeps_local_noop() {
        let local = make_card(|c| {
            c.title = "my edit".to_string();
            c.description = "my desc".to_string();
        });
        let snap = make_snap(|_| {});
        let remote = make_card(|_| {}); // same as snap
        let d = plan_sync(&remote, Some(&local), Some(&snap));
        assert_eq!(d.decision_type, SyncDecisionType::Noop);
        // No card to persist — local already correct.
        assert!(d.card.is_none(), "noop has no card");
    }

    #[test]
    fn both_changed_same_value_no_conflict_noop() {
        let local = make_card(|c| c.title = "same".to_string());
        let snap = make_snap(|c| c.title = "t".to_string());
        let remote = make_card(|c| c.title = "same".to_string());
        let d = plan_sync(&remote, Some(&local), Some(&snap));
        assert_eq!(d.decision_type, SyncDecisionType::Noop);
        assert!(d.conflicts.is_none(), "no conflicts when values agree");
    }

    #[test]
    fn both_changed_different_conflict() {
        let local = make_card(|c| {
            c.title = "local edit".to_string();
            c.description = "d".to_string();
        });
        let snap = make_snap(|_| {});
        let remote = make_card(|c| c.title = "remote edit".to_string());
        let d = plan_sync(&remote, Some(&local), Some(&snap));
        assert_eq!(d.decision_type, SyncDecisionType::Conflict);
        let conflicts = d.conflicts.as_ref().unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].field, MergeField::Title);
        assert_eq!(conflicts[0].local, "local edit");
        assert_eq!(conflicts[0].remote, "remote edit");
        // Partial merge: conflicting field keeps local.
        assert_eq!(d.card.as_ref().unwrap().title, "local edit");
    }

    #[test]
    fn multiple_conflicting_fields() {
        let local = make_card(|c| {
            c.title = "L".to_string();
            c.description = "LD".to_string();
            c.priority = "urgent".to_string();
            c.column = "done".to_string();
        });
        let snap = make_snap(|_| {});
        let remote = make_card(|c| {
            c.title = "R".to_string();
            c.description = "RD".to_string();
            c.priority = "low".to_string();
            c.column = "ongoing".to_string();
        });
        let d = plan_sync(&remote, Some(&local), Some(&snap));
        assert_eq!(d.decision_type, SyncDecisionType::Conflict);
        let conflicts = d.conflicts.as_ref().unwrap();
        assert_eq!(conflicts.len(), 4);
        // Mirror TS sync.test.ts: sort field *names* lexicographically.
        let mut fields: Vec<String> = conflicts.iter().map(|c| c.field.to_string()).collect();
        fields.sort();
        assert_eq!(
            fields,
            vec![
                "column".to_string(),
                "description".to_string(),
                "priority".to_string(),
                "title".to_string(),
            ]
        );
    }

    #[test]
    fn mixed_some_conflict_some_remote_some_local() {
        // title: both changed -> conflict
        // description: remote only -> take remote
        // priority: local only -> keep local
        let local = make_card(|c| {
            c.title = "L".to_string();
            c.description = "d".to_string();
            c.priority = "urgent".to_string();
        });
        let snap = make_snap(|_| {});
        let remote = make_card(|c| {
            c.title = "R".to_string();
            c.description = "RD".to_string();
            c.priority = "medium".to_string();
        });
        let d = plan_sync(&remote, Some(&local), Some(&snap));
        assert_eq!(d.decision_type, SyncDecisionType::Conflict);
        let conflicts = d.conflicts.as_ref().unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].field, MergeField::Title);
        // Partial merge applied remote description, kept local priority + title.
        let card = d.card.as_ref().unwrap();
        assert_eq!(card.description, "RD");
        assert_eq!(card.priority, "urgent");
        assert_eq!(card.title, "L");
    }

    #[test]
    fn apply_resolution_picks_remote() {
        let local = make_card(|c| c.title = "L".to_string());
        let snap = make_snap(|_| {});
        let remote = make_card(|c| c.title = "R".to_string());
        let d = plan_sync(&remote, Some(&local), Some(&snap));
        assert_eq!(d.decision_type, SyncDecisionType::Conflict);
        assert!(d.card.is_some());
        let mut choices = HashMap::new();
        choices.insert("title".to_string(), Choice::Remote);
        let resolved = apply_resolution(
            d.card.as_ref().unwrap(),
            d.conflicts.as_ref().unwrap(),
            &choices,
        );
        assert_eq!(resolved.title, "R");
    }

    #[test]
    fn apply_resolution_picks_local_default_when_omitted() {
        let local = make_card(|c| c.title = "L".to_string());
        let snap = make_snap(|_| {});
        let remote = make_card(|c| c.title = "R".to_string());
        let d = plan_sync(&remote, Some(&local), Some(&snap));
        assert_eq!(d.decision_type, SyncDecisionType::Conflict);
        assert!(d.card.is_some());
        let choices = HashMap::new();
        let resolved = apply_resolution(
            d.card.as_ref().unwrap(),
            d.conflicts.as_ref().unwrap(),
            &choices,
        );
        assert_eq!(resolved.title, "L");
    }

    #[test]
    fn snapshot_from_card_mirrors_remote() {
        let remote = make_card(|c| {
            c.title = "T".to_string();
            c.source_status = Some("In Progress".to_string());
            c.column = "ongoing".to_string();
        });
        let snap = snapshot_from_card(&remote, "src-1", "jira", NOW);
        assert_eq!(snap.source_ref, "PROJ-1");
        assert_eq!(snap.title, "T");
        assert_eq!(snap.source_status, "In Progress");
        assert_eq!(snap.column, "ongoing");
        assert_eq!(snap.synced_at, NOW);
    }

    #[test]
    fn local_move_only_keeps_local_no_conflict() {
        let local = make_card(|c| {
            c.column = "done".to_string();
            c.source_status = Some("To Do".to_string());
        });
        let snap = make_snap(|_| {});
        let remote = make_card(|c| {
            c.column = "backlog".to_string();
            c.source_status = Some("To Do".to_string());
        });
        let d = plan_sync(&remote, Some(&local), Some(&snap));
        // Remote unchanged; local moved. Noop — local already holds the move.
        assert_eq!(d.decision_type, SyncDecisionType::Noop);
    }

    #[test]
    fn local_move_plus_remote_status_change_conflict_on_column_only() {
        let local = make_card(|c| {
            c.column = "done".to_string();
            c.source_status = Some("To Do".to_string());
        });
        let snap = make_snap(|_| {});
        let remote = make_card(|c| {
            c.column = "ongoing".to_string();
            c.source_status = Some("In Progress".to_string());
        });
        let d = plan_sync(&remote, Some(&local), Some(&snap));
        assert_eq!(d.decision_type, SyncDecisionType::Conflict);
        // sourceStatus not conflicting (local unchanged) -> applied; column conflicting.
        assert_eq!(
            d.card.as_ref().unwrap().source_status.as_deref(),
            Some("In Progress")
        );
        let conflicts = d.conflicts.as_ref().unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].field, MergeField::Column);
    }
}
