CREATE TABLE cards (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  description TEXT DEFAULT '',
  "column" TEXT NOT NULL CHECK ("column" IN ('backlog','ongoing','done')),
  source TEXT NOT NULL DEFAULT 'local' CHECK (source IN ('local','jira')),
  position INTEGER NOT NULL DEFAULT 0,
  jira_key TEXT,
  jira_status TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
