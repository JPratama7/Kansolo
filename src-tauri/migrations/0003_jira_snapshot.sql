CREATE TABLE jira_snapshots (
  jira_key TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  priority TEXT NOT NULL DEFAULT 'medium',
  jira_status TEXT NOT NULL DEFAULT '',
  "column" TEXT NOT NULL DEFAULT 'backlog',
  synced_at TEXT NOT NULL
);
