-- 0012: Drop the never-populated pid/pgid columns from agent_runs.
--
-- These columns were added in 0011 as placeholders for child-process
-- tracking, but the runner never writes them (the ACP SDK does not expose
-- the child pid in its public API — see acp_spike.rs spike-gate-17), so
-- they are always NULL. Drop them to keep the schema honest.
ALTER TABLE agent_runs DROP COLUMN pid;
ALTER TABLE agent_runs DROP COLUMN pgid;
