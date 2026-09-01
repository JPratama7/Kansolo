-- 0014: Drop cards.repo_path, migrate to tree_source_id.
-- Also adds agent_runs.repo_root (snapshot of resolved path at create time)
-- and backfills it from cards.repo_path before dropping that column.

-- Step 1: Add repo_root to agent_runs (additive, no rebuild).
ALTER TABLE agent_runs ADD COLUMN repo_root TEXT;

-- Step 2: Backfill repo_root from cards.repo_path (MUST precede step 4).
UPDATE agent_runs
SET repo_root = (SELECT repo_path FROM cards WHERE cards.id = agent_runs.card_id)
WHERE repo_root IS NULL;

-- Step 3a: Create tree sources for orphan repo_paths (no matching tree_source.path).
INSERT OR IGNORE INTO tree_sources (id, label, path, editor_command, created_at)
SELECT 'migrated-' || lower(hex(randomblob(8))),
       repo_path, repo_path, NULL, '2026-01-01T00:00:00Z'
FROM (SELECT DISTINCT repo_path FROM cards
      WHERE repo_path IS NOT NULL
        AND tree_source_id IS NULL
        AND repo_path NOT IN (SELECT path FROM tree_sources));

-- Step 3b: Link cards to tree sources by path match.
UPDATE cards
SET tree_source_id = (SELECT ts.id FROM tree_sources ts WHERE ts.path = cards.repo_path)
WHERE repo_path IS NOT NULL AND tree_source_id IS NULL;

-- Step 4: Rebuild cards without repo_path (keep source_instance_id from 0013).
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
  source_instance_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  FOREIGN KEY (tree_source_id) REFERENCES tree_sources(id) ON DELETE RESTRICT,
  FOREIGN KEY (source_instance_id) REFERENCES sources(id) ON DELETE SET NULL
);
INSERT INTO cards_new (id, title, description, "column", source, position, priority,
                       source_ref, source_status, tree_source_id, source_instance_id,
                       created_at, updated_at)
SELECT id, title, description, "column", source, position, priority,
       source_ref, source_status, tree_source_id, source_instance_id,
       created_at, updated_at FROM cards;
DROP TABLE cards;
ALTER TABLE cards_new RENAME TO cards;
CREATE INDEX idx_cards_priority_created_column
  ON cards (priority, created_at, "column");
COMMIT;
PRAGMA foreign_keys=ON;
