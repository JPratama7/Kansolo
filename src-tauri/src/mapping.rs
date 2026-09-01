//! Status and priority mapping between upstream names and the local kanban
//! board. No I/O, no Tauri commands, no DB access.

use serde::{Deserialize, Serialize};

/// Kanban column contract shared with the TypeScript side (camelCase JSON).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StatusMapping {
    pub backlog: Vec<String>,
    pub ongoing: Vec<String>,
    pub done: Vec<String>,
}

impl StatusMapping {
    /// Yields the three column lists in canonical order: backlog, ongoing, done.
    fn columns(&self) -> [&[String]; 3] {
        [&self.backlog, &self.ongoing, &self.done]
    }
}

/// Unmatched statuses land here (catch-all rule).
pub const CATCH_ALL_COLUMN: &str = "backlog";

/// Canonical column ids in priority order.
pub const ALL_COLUMNS: [&str; 3] = ["backlog", "ongoing", "done"];

/// Default when nothing matches and the input is empty/unknown.
pub const DEFAULT_PRIORITY: &str = "medium";

/// Synonyms for each priority, matched case-insensitively against the Jira
/// `priorityName`. First match wins; unmatched names fall back to 'medium'.
///
/// Order mirrors the canonical priority order so iteration is deterministic.
pub const PRIORITY_SYNONYMS: [(&str, &[&str]); 4] = [
    ("low", &["low", "lowest", "minor", "trivial"]),
    ("medium", &["medium", "normal", "default"]),
    ("high", &["high", "higher", "major"]),
    (
        "urgent",
        &["urgent", "highest", "critical", "blocker", "emergency"],
    ),
];

/// Resolve a Jira status name to its kanban column via a case-insensitive
/// lookup in each column's status list. Unmatched statuses fall back to
/// `'backlog'`.
pub fn resolve_column(status_name: &str, mapping: &StatusMapping) -> &'static str {
    let needle = status_name.trim().to_lowercase();
    let columns = mapping.columns();
    for (column, statuses) in ALL_COLUMNS.iter().zip(columns.iter()) {
        for status in *statuses {
            if status.trim().to_lowercase() == needle {
                return column;
            }
        }
    }
    CATCH_ALL_COLUMN
}

/// True when `status_name` appears in any column's status list
/// (case-insensitive, trimmed). Empty input returns `false`.
pub fn is_status_mapped(status_name: &str, mapping: &StatusMapping) -> bool {
    let needle = status_name.trim().to_lowercase();
    if needle.is_empty() {
        return false;
    }
    for statuses in mapping.columns() {
        for status in statuses {
            if status.trim().to_lowercase() == needle {
                return true;
            }
        }
    }
    false
}

/// Map a Jira priority name to a canonical priority literal
/// (`'low'` | `'medium'` | `'high'` | `'urgent'`). Case-insensitive, trims
/// whitespace, falls back to `'medium'` for `None`/empty/unknown names.
pub fn resolve_priority(priority_name: Option<&str>) -> &'static str {
    let Some(name) = priority_name else {
        return DEFAULT_PRIORITY;
    };
    let needle = name.trim().to_lowercase();
    if needle.is_empty() {
        return DEFAULT_PRIORITY;
    }
    for (priority, synonyms) in PRIORITY_SYNONYMS.iter() {
        if synonyms.iter().any(|s| *s == needle) {
            return priority;
        }
    }
    DEFAULT_PRIORITY
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(backlog: &[&str], ongoing: &[&str], done: &[&str]) -> StatusMapping {
        StatusMapping {
            backlog: backlog.iter().map(|s| s.to_string()).collect(),
            ongoing: ongoing.iter().map(|s| s.to_string()).collect(),
            done: done.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn maps_known_status_to_its_column() {
        let m = mapping(
            &["To Do", "Backlog", "Open"],
            &["In Progress", "In Review"],
            &["Done", "Closed", "Resolved"],
        );
        assert_eq!(resolve_column("In Progress", &m), "ongoing");
        assert_eq!(resolve_column("Done", &m), "done");
        assert_eq!(resolve_column("Backlog", &m), "backlog");
    }

    #[test]
    fn is_case_insensitive() {
        let m = mapping(&["To Do"], &["IN PROGRESS", "in review"], &["done"]);
        assert_eq!(resolve_column("in progress", &m), "ongoing");
        assert_eq!(resolve_column("DONE", &m), "done");
        assert_eq!(resolve_column("tO dO", &m), "backlog");
    }

    #[test]
    fn falls_back_to_backlog_for_unknown_status() {
        let m = mapping(&["To Do"], &["In Progress"], &["Done"]);
        assert_eq!(resolve_column("Blocked Pending QA", &m), "backlog");
    }

    #[test]
    fn empty_mapping_falls_back_to_backlog() {
        let m = mapping(&[], &[], &[]);
        assert_eq!(resolve_column("Anything At All", &m), "backlog");
    }

    #[test]
    fn resolve_priority_maps_canonical_names_case_insensitively() {
        assert_eq!(resolve_priority(Some("High")), "high");
        assert_eq!(resolve_priority(Some("HIGH")), "high");
        assert_eq!(resolve_priority(Some("  urgent  ")), "urgent");
        assert_eq!(resolve_priority(Some("Low")), "low");
        assert_eq!(resolve_priority(Some("Medium")), "medium");
    }

    #[test]
    fn resolve_priority_maps_common_jira_synonyms() {
        assert_eq!(resolve_priority(Some("Highest")), "urgent");
        assert_eq!(resolve_priority(Some("Critical")), "urgent");
        assert_eq!(resolve_priority(Some("Blocker")), "urgent");
        assert_eq!(resolve_priority(Some("Major")), "high");
        assert_eq!(resolve_priority(Some("Normal")), "medium");
        assert_eq!(resolve_priority(Some("Trivial")), "low");
    }

    #[test]
    fn resolve_priority_falls_back_to_medium_for_unknown_or_empty() {
        assert_eq!(resolve_priority(None), "medium");
        assert_eq!(resolve_priority(Some("")), "medium");
        assert_eq!(resolve_priority(Some("   ")), "medium");
        assert_eq!(resolve_priority(Some("WTFBBQ")), "medium");
    }

    #[test]
    fn is_status_mapped_true_for_known_status_in_any_column() {
        let m = mapping(&["To Do", "Backlog"], &["In Progress"], &["Done"]);
        assert!(is_status_mapped("To Do", &m));
        assert!(is_status_mapped("in progress", &m));
        assert!(is_status_mapped("DONE", &m));
    }

    #[test]
    fn is_status_mapped_false_for_unknown_status() {
        let m = mapping(&["To Do"], &["In Progress"], &["Done"]);
        assert!(!is_status_mapped("Blocked Pending QA", &m));
        assert!(!is_status_mapped("In Triage", &m));
    }

    #[test]
    fn is_status_mapped_false_for_empty_status_name() {
        let m = mapping(&["To Do"], &[], &[]);
        assert!(!is_status_mapped("", &m));
        assert!(!is_status_mapped("   ", &m));
    }
}
