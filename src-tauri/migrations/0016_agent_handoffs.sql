-- 0016: Resume handoff + persisted run update stream + per-agent system prompt.
-- agent_handoffs: one summary document per run (owned by the app, never
-- written into the worktree). Regenerated lazily at resume time when stale.
-- agent_run_updates: full serialized RunUpdate stream so the panel can
-- reload the transcript after an app restart (in-memory buffer is lost).

ALTER TABLE agents ADD COLUMN system_prompt TEXT NOT NULL DEFAULT '';

CREATE TABLE IF NOT EXISTS agent_handoffs (
    run_id     TEXT PRIMARY KEY,
    content    TEXT NOT NULL,
    source     TEXT NOT NULL DEFAULT 'summary', -- 'summary' | 'truncated'
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES agent_runs(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS agent_run_updates (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id      TEXT NOT NULL,
    update_json TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES agent_runs(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_agent_run_updates_run
    ON agent_run_updates(run_id, seq);
