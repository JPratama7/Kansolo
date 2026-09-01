-- 0013: Instance-scoped linking for cards + external_snapshots.
-- Previously both tables keyed off the source *type* string (e.g. "jira"),
-- so delete_all_source_cards(instance_id) could only clear type-wide and
-- two same-type instances shared snapshot rows. This migration adds a
-- source_instance_id FK to both tables and rekeys external_snapshots on
-- (source_instance_id, source_ref).

-- cards: add nullable FK column. ADD COLUMN with a REFERENCES clause is
-- allowed by SQLite only when the default is NULL, which is what we want
-- (ON DELETE SET NULL keeps cards when a source instance is removed).
ALTER TABLE cards ADD COLUMN source_instance_id TEXT
  REFERENCES sources(id) ON DELETE SET NULL;

-- Backfill: only safe when exactly one sources row exists for a given
-- source_type (ambiguous multi-instance rows stay NULL and get relinked
-- on the next sync of their owning instance).
UPDATE cards
SET source_instance_id = (
  SELECT s.id FROM sources s
  WHERE s.source_type = cards.source
  AND s.enabled = 1
  AND (SELECT COUNT(*) FROM sources s2
       WHERE s2.source_type = cards.source AND s2.enabled = 1) = 1
)
WHERE cards.source != 'local'
  AND (SELECT COUNT(*) FROM sources s2
       WHERE s2.source_type = cards.source AND s2.enabled = 1) = 1;

-- external_snapshots: rekey from (source, source_ref) to
-- (source_instance_id, source_ref). SQLite can't alter a PK in place, so
-- rebuild the table (same pattern as migration 0010).
CREATE TABLE external_snapshots_new (
  source_instance_id TEXT NOT NULL
    REFERENCES sources(id) ON DELETE CASCADE,
  source_ref TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT '',
  title TEXT NOT NULL DEFAULT '',
  description TEXT NOT NULL DEFAULT '',
  priority TEXT NOT NULL DEFAULT 'medium',
  source_status TEXT NOT NULL DEFAULT '',
  "column" TEXT NOT NULL DEFAULT 'backlog',
  synced_at TEXT NOT NULL,
  PRIMARY KEY (source_instance_id, source_ref)
);

-- Only backfill rows whose owning instance is unambiguous (exactly one
-- enabled sources row of that type). Ambiguous rows are dropped — they
-- get recreated on the next sync of their owning instance.
INSERT INTO external_snapshots_new
  (source_instance_id, source_ref, source, title, description,
   priority, source_status, "column", synced_at)
SELECT
  (SELECT s.id FROM sources s
   WHERE s.source_type = external_snapshots.source AND s.enabled = 1),
  source_ref, source, title, description,
  priority, source_status, "column", synced_at
FROM external_snapshots
WHERE external_snapshots.source != ''
  AND (SELECT COUNT(*) FROM sources s2
       WHERE s2.source_type = external_snapshots.source AND s2.enabled = 1) = 1;

DROP TABLE external_snapshots;
ALTER TABLE external_snapshots_new RENAME TO external_snapshots;
