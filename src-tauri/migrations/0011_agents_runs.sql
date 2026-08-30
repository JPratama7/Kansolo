-- 0011: Agent + agent_run tables for ACP integration.
-- Agents are CLI commands (built-in or user-registered) that run
-- against card-linked git worktrees via the ACP protocol.

CREATE TABLE IF NOT EXISTS agents (
    name         TEXT PRIMARY KEY,
    command      TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    built_in     INTEGER NOT NULL DEFAULT 0,
    enabled      INTEGER NOT NULL DEFAULT 1,
    skills_json  TEXT NOT NULL DEFAULT '[]',
    created_at   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_runs (
    id           TEXT PRIMARY KEY,
    card_id      TEXT NOT NULL,
    agent_name   TEXT NOT NULL,
    session_id   TEXT,
    worktree_path TEXT NOT NULL,
    branch       TEXT NOT NULL,
    status       TEXT NOT NULL,
    output       TEXT,
    stop_reason  TEXT,
    error        TEXT,
    pid          INTEGER,
    pgid         INTEGER,
    merged_at    TEXT,
    skills_json  TEXT NOT NULL DEFAULT '[]',
    created_at   TEXT NOT NULL,
    finished_at  TEXT,
    FOREIGN KEY (card_id) REFERENCES cards(id) ON DELETE CASCADE,
    FOREIGN KEY (agent_name) REFERENCES agents(name) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS idx_agent_runs_card ON agent_runs(card_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_runs_one_active
    ON agent_runs(card_id) WHERE status IN ('pending', 'running');

ALTER TABLE cards ADD COLUMN repo_path TEXT;

-- Seed the built-in claude-code agent (empty command sentinel;
-- the runner dispatches to AcpAgent::claude_agent() for built-ins).
INSERT OR IGNORE INTO agents (name, command, description, built_in, enabled, skills_json, created_at)
VALUES ('claude-code', '', 'Claude Code via ACP', 1, 1, '[]', '2026-01-01T00:00:00Z');
