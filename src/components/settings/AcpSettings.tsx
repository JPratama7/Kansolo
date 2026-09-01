import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { getAllSettings, setSetting, invalidatePermissionTimeoutCache } from "../../db.ts";

const INPUT =
  "w-full text-sm rounded px-2 py-1.5 bg-base text-ink placeholder:text-ink-secondary border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent";

/** ACP settings section: default agent, auto-cleanup, skills directory,
 * permission timeout, prune-orphans. Stored in the existing settings KV
 * table. */
export default function AcpSettings() {
  const [defaultAgent, setDefaultAgent] = createSignal("claude-code");
  const [autoCleanup, setAutoCleanup] = createSignal(true);
  const [skillsDir, setSkillsDir] = createSignal("");
  const [permissionTimeout, setPermissionTimeout] = createSignal(300);
  const [pruneOrphans, setPruneOrphans] = createSignal(false);
  const [saved, setSaved] = createSignal(false);
  let savedTimer: ReturnType<typeof setTimeout> | null = null;

  // Load settings on mount — values come from getAllSettings in parent.
  // Component receives initial values via props-free signal init.
  onMount(() => {
    void (async () => {
      const s = await getAllSettings();
      setDefaultAgent(s["acp_default_agent"] ?? "claude-code");
      setAutoCleanup(s["acp_auto_cleanup"] !== "false");
      setSkillsDir(s["acp_skills_dir"] ?? "");
      setPermissionTimeout(
        parseInt(s["acp_permission_timeout"] ?? "300", 10) || 300,
      );
      setPruneOrphans(s["acp_prune_orphans"] === "true");
    })();
  });

  // Clear the "Saved!" indicator timer on unmount so it can't fire after teardown.
  onCleanup(() => {
    if (savedTimer) clearTimeout(savedTimer);
  });

  async function save() {
    try {
      await setSetting("acp_default_agent", defaultAgent());
      await setSetting("acp_auto_cleanup", autoCleanup() ? "true" : "false");
      await setSetting("acp_skills_dir", skillsDir());
      await setSetting("acp_permission_timeout", String(permissionTimeout()));
      invalidatePermissionTimeoutCache();
      await setSetting("acp_prune_orphans", pruneOrphans() ? "true" : "false");
      setSaved(true);
      if (savedTimer) clearTimeout(savedTimer);
      savedTimer = setTimeout(() => setSaved(false), 2000);
    } catch {
      // Non-fatal.
    }
  }

  return (
    <fieldset class="border border-border-subtle rounded-[var(--radius-card)] p-3">
      <legend class="text-xs font-semibold text-ink-secondary px-1">
        Agent Settings
      </legend>
      <div class="flex flex-col gap-3">
        <div>
          <label
            class="block text-xs font-semibold text-ink-secondary mb-1"
            for="acp-default-agent"
          >
            Default agent
          </label>
          <input
            id="acp-default-agent"
            type="text"
            class={INPUT}
            value={defaultAgent()}
            onInput={(e) => setDefaultAgent(e.currentTarget.value)}
            placeholder="claude-code"
          />
        </div>
        <label class="flex items-center gap-2 text-sm text-ink">
          <input
            type="checkbox"
            class="accent-accent"
            checked={autoCleanup()}
            onChange={(e) => setAutoCleanup(e.currentTarget.checked)}
          />
          Auto-cleanup dangling runs on startup
        </label>
        <div>
          <label
            class="block text-xs font-semibold text-ink-secondary mb-1"
            for="acp-skills-dir"
          >
            Skills directory (empty = ~/.agents/skills/)
          </label>
          <input
            id="acp-skills-dir"
            type="text"
            class={INPUT}
            value={skillsDir()}
            onInput={(e) => setSkillsDir(e.currentTarget.value)}
            placeholder="~/.agents/skills/"
          />
        </div>
        <div>
          <label
            class="block text-xs font-semibold text-ink-secondary mb-1"
            for="acp-perm-timeout"
          >
            Permission timeout (seconds)
          </label>
          <input
            id="acp-perm-timeout"
            type="number"
            class={INPUT}
            value={permissionTimeout()}
            min={30}
            max={3600}
            onInput={(e) =>
              setPermissionTimeout(parseInt(e.currentTarget.value, 10) || 300)}
          />
        </div>
        <label class="flex items-center gap-2 text-sm text-ink">
          <input
            type="checkbox"
            class="accent-accent"
            checked={pruneOrphans()}
            onChange={(e) => setPruneOrphans(e.currentTarget.checked)}
          />
          Prune orphan worktrees on cleanup
        </label>
        <div class="flex items-center gap-3">
          <button
            type="button"
            class="px-3 py-1.5 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors"
            onClick={() => void save()}
          >
            Save
          </button>
          <Show when={saved()}>
            <span class="text-xs text-p-high">Saved!</span>
          </Show>
        </div>
      </div>
    </fieldset>
  );
}
