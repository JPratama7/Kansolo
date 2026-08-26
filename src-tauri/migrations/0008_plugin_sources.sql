-- 0008: Additive generalization for plugin-based sources.
-- New columns/tables alongside old Jira-named ones. Nothing reads the new schema yet.
-- config_json keys use snake_case: base_url, email, token, jql_parts.

ALTER TABLE cards ADD COLUMN source_ref TEXT;
ALTER TABLE cards ADD COLUMN source_status TEXT;

-- Copy existing Jira data into the new generalized columns.
UPDATE cards SET source_ref = jira_key, source_status = jira_status
WHERE jira_key IS NOT NULL;

-- One configured source instance = one row. Config stored as JSON text.
-- config_json keys are snake_case: base_url, email, token, jql_parts.
CREATE TABLE sources (
  id TEXT PRIMARY KEY,
  source_type TEXT NOT NULL,
  label TEXT NOT NULL,
  config_json TEXT NOT NULL DEFAULT '{}',
  status_mapping_json TEXT NOT NULL DEFAULT '{}',
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL
);

-- Generalized snapshot table (replaces jira_snapshots).
CREATE TABLE external_snapshots (
  source TEXT NOT NULL,
  source_ref TEXT NOT NULL,
  title TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  priority TEXT NOT NULL DEFAULT 'medium',
  source_status TEXT NOT NULL DEFAULT '',
  "column" TEXT NOT NULL DEFAULT 'backlog',
  synced_at TEXT NOT NULL,
  PRIMARY KEY (source, source_ref)
);

-- Persisted conflicts between sync_source and resolve_conflicts calls.
-- Survives app crash. conflict_json holds serialized SyncConflict.
CREATE TABLE pending_conflicts (
  source_id TEXT NOT NULL,
  source_ref TEXT NOT NULL,
  conflict_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (source_id, source_ref)
);

-- Backfill one sources row from existing jira_* settings (only if Jira was configured).
-- config_json keys are snake_case: base_url, email, token, jql_parts.
INSERT INTO sources (id, source_type, label, config_json, status_mapping_json, enabled, created_at)
SELECT 'migrated-jira', 'jira', 'Jira',
  json_object(
    'base_url', (SELECT value FROM settings WHERE key='jira_base_url'),
    'email', (SELECT value FROM settings WHERE key='jira_email'),
    'token', (SELECT value FROM settings WHERE key='jira_token'),
    'jql_parts', (SELECT value FROM settings WHERE key='jql_parts')
  ),
  COALESCE((SELECT value FROM settings WHERE key='status_mapping'), '{}'),
  1, '2026-08-25T00:00:00Z'
WHERE EXISTS (
  SELECT 1 FROM settings WHERE key='jira_base_url' AND value != ''
);

-- Backfill external_snapshots from jira_snapshots.
INSERT INTO external_snapshots
  (source, source_ref, title, description, priority, source_status, "column", synced_at)
SELECT 'jira', jira_key, title, description, priority, jira_status, "column", synced_at
FROM jira_snapshots;
