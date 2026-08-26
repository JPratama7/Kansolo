PRAGMA foreign_keys=OFF;
BEGIN;
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
  tree_source_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  FOREIGN KEY (tree_source_id) REFERENCES tree_sources(id) ON DELETE RESTRICT
);
INSERT INTO cards_new (id, title, description, "column", source, position, priority, source_ref, source_status, tree_source_id, created_at, updated_at)
SELECT id, title, description, "column", source, position, priority, source_ref, source_status, NULL, created_at, updated_at FROM cards;
DROP TABLE cards;
ALTER TABLE cards_new RENAME TO cards;
CREATE INDEX idx_cards_priority_created_column
  ON cards (priority, created_at, "column");
COMMIT;
PRAGMA foreign_keys=ON;
