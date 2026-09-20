-- Per-agent default model + reasoning effort (ACP session config options).
-- NULL = unset; the agent's own default is used.
ALTER TABLE agents ADD COLUMN model TEXT;
ALTER TABLE agents ADD COLUMN effort TEXT;
