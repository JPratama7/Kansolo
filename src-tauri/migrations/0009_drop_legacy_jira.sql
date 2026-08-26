-- 0009: Drop legacy Jira schema. All code now uses source_ref/source_status/external_snapshots/sources.
-- Destructive: drops columns, a table, and settings keys. Backup tasker.db before running manually.

-- Recreate cards without the source CHECK constraint (free-text source type for plugins)
-- and without the legacy jira_key/jira_status columns. priority is preserved (needed by
-- idx_cards_priority_created_column and external_snapshots).
CREATE TABLE cards_new (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  description TEXT DEFAULT '',
  "column" TEXT NOT NULL CHECK ("column" IN ('backlog','ongoing','done')),
  source TEXT NOT NULL DEFAULT 'local',
  position INTEGER NOT NULL DEFAULT 0,
  priority TEXT NOT NULL DEFAULT 'medium',
  source_ref TEXT,
  source_status TEXT,
  source_path TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

INSERT INTO cards_new (id, title, description, "column", source, position, priority, source_ref, source_status, source_path, created_at, updated_at)
SELECT id, title, description, "column", source, position, priority, source_ref, source_status, source_path, created_at, updated_at FROM cards;

DROP TABLE cards;
ALTER TABLE cards_new RENAME TO cards;

-- Recreate indexes that were on the old cards table (from 0007_cards_priority_index.sql).
CREATE INDEX idx_cards_priority_created_column
  ON cards (priority, created_at, "column");

-- Drop the old Jira snapshot table.
DROP TABLE jira_snapshots;

-- Delete old Jira-specific settings keys.
DELETE FROM settings WHERE key IN
  ('jira_base_url', 'jira_email', 'jira_token', 'jira_jql', 'jql_parts', 'status_mapping');
