import { For, Show, createEffect, createSignal } from 'solid-js';
import { toaster } from '../ui/toaster.ts';
import type { Agent, SkillManifest } from '../../db.ts';
import {
  acpListAgents,
  acpListSkills,
  acpListActiveRuns,
  acpRegisterAgent,
  acpUpdateAgent,
  acpDeleteAgent,
  acpErrorMessage,
} from '../../db.ts';

const INPUT =
  'w-full text-sm rounded px-2 py-1.5 bg-base text-ink placeholder:text-ink-secondary border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent';

/** Settings tab for managing registered agents. Lists agents with add/edit
 * form (name, command, description) and a skill multi-select from
 * `acp_list_skills` (scans `~/.agents/skills/` on disk). */
export default function AgentRegistry() {
  const [agents, setAgents] = createSignal<Agent[]>([]);
  const [skills, setSkills] = createSignal<SkillManifest[]>([]);
  const [activeByAgent, setActiveByAgent] = createSignal<Record<string, number>>({});
  const [editing, setEditing] = createSignal<Agent | null>(null);
  const [name, setName] = createSignal('');
  const [command, setCommand] = createSignal('');
  const [description, setDescription] = createSignal('');
  const [selectedSkills, setSelectedSkills] = createSignal<string[]>([]);
  const [adding, setAdding] = createSignal(false);

  async function refresh() {
    try {
      const [agentList, skillList, active] = await Promise.all([
        acpListAgents(),
        acpListSkills(),
        acpListActiveRuns(),
      ]);
      setAgents(agentList);
      setSkills(skillList);
      const counts: Record<string, number> = {};
      for (const r of active) counts[r.agentName] = (counts[r.agentName] ?? 0) + 1;
      setActiveByAgent(counts);
    } catch (e) {
      toaster.error({ title: 'Failed to load agents', description: acpErrorMessage(e) });
    }
  }

  createEffect(() => { void refresh(); });

  function startAdd() {
    setEditing(null);
    setAdding(true);
    setName('');
    setCommand('');
    setDescription('');
    setSelectedSkills([]);
  }

  function startEdit(agent: Agent) {
    setAdding(false);
    setEditing(agent);
    setName(agent.name);
    setCommand(agent.command);
    setDescription(agent.description);
    setSelectedSkills(agent.skills);
  }

  function cancelForm() {
    setEditing(null);
    setAdding(false);
  }

  async function save() {
    const n = name().trim();
    const cmd = command().trim();
    const desc = description().trim();
    if (!n) return;
    // Reject empty command for non-built-in (decision 21).
    if (!cmd && n !== 'claude-code') {
      toaster.error({ title: 'Validation error', description: 'Command cannot be empty for non-built-in agents' });
      return;
    }
    try {
      if (editing()) {
        await acpUpdateAgent(n, cmd, desc, selectedSkills());
        toaster.success({ title: 'Agent updated', description: n });
      } else {
        await acpRegisterAgent(n, cmd, desc, selectedSkills());
        toaster.success({ title: 'Agent registered', description: n });
      }
      cancelForm();
      await refresh();
    } catch (e) {
      toaster.error({ title: 'Save failed', description: acpErrorMessage(e) });
    }
  }

  async function remove(name: string) {
    try {
      await acpDeleteAgent(name, false);
      toaster.success({ title: 'Agent removed', description: name });
      await refresh();
    } catch (e) {
      // If runs exist, prompt for cascade delete.
      const msg = acpErrorMessage(e);
      if (msg.includes('locked') || msg.includes('runs') || msg.includes('active')) {
        const id = toaster.create({
          title: `Delete agent "${name}" and all its runs?`,
          type: 'warning',
          duration: Infinity,
          action: {
            label: 'Delete all',
            onClick: () => {
              toaster.dismiss(id);
              void (async () => {
                try {
                  await acpDeleteAgent(name, true);
                  toaster.success({ title: 'Agent + runs removed', description: name });
                  await refresh();
                } catch (e2) {
                  toaster.error({ title: 'Delete failed', description: acpErrorMessage(e2) });
                }
              })();
            },
          },
        });
      } else {
        toaster.error({ title: 'Delete failed', description: msg });
      }
    }
  }

  function toggleSkill(name: string) {
    setSelectedSkills((prev) =>
      prev.includes(name) ? prev.filter((s) => s !== name) : [...prev, name],
    );
  }

  const isFormOpen = () => adding() || editing() !== null;

  return (
    <fieldset class="border border-border-subtle rounded-[var(--radius-card)] p-3">
      <legend class="text-xs font-semibold text-ink-secondary px-1">Agent Registry</legend>
      <div class="flex flex-col gap-3">
        {/* Agent list */}
        <Show when={agents().length > 0}>
          <ul class="flex flex-col gap-1">
            <For each={agents()}>
              {(agent) => (
                <li class="flex items-center justify-between gap-2 text-sm text-ink">
                  <div class="min-w-0">
                    <span class="font-semibold truncate">{agent.name}</span>
                    {agent.builtIn && (
                      <span class="text-[10px] font-mono text-ink-secondary bg-base/60 rounded px-1 py-0.5 ml-1">
                        built-in
                      </span>
                    )}
                    <Show when={(activeByAgent()[agent.name] ?? 0) > 0}>
                      <span class="text-[10px] font-semibold text-p-high bg-p-high/10 rounded px-1 py-0.5 ml-1">
                        {activeByAgent()[agent.name]} running
                      </span>
                    </Show>
                    <Show when={agent.description}>
                      <p class="text-xs text-ink-secondary truncate">{agent.description}</p>
                    </Show>
                  </div>
                  <div class="flex items-center gap-2 shrink-0">
                    <Show when={agent.skills.length > 0}>
                      <span class="text-[10px] text-ink-secondary">
                        {agent.skills.length} skill{agent.skills.length !== 1 ? 's' : ''}
                      </span>
                    </Show>
                    <span class={`text-[10px] ${agent.enabled ? 'text-p-high' : 'text-ink-secondary'}`}>
                      {agent.enabled ? 'enabled' : 'disabled'}
                    </span>
                    <button
                      type="button"
                      class="text-xs text-ink-secondary hover:text-ink"
                      onClick={() => startEdit(agent)}
                    >
                      Edit
                    </button>
                    <Show when={!agent.builtIn}>
                      <button
                        type="button"
                        class="text-xs text-ink-secondary hover:text-p-urgent"
                        onClick={() => void remove(agent.name)}
                      >
                        Delete
                      </button>
                    </Show>
                  </div>
                </li>
              )}
            </For>
          </ul>
        </Show>

        {/* Add/Edit form */}
        <Show when={isFormOpen()}>
          <div class="flex flex-col gap-2 rounded border border-border-subtle p-3 bg-base/40">
            <div>
              <label class="block text-xs font-semibold text-ink-secondary mb-1" for="agent-name">
                Name
              </label>
              <input
                id="agent-name"
                type="text"
                class={INPUT}
                value={name()}
                onInput={(e) => setName(e.currentTarget.value)}
                disabled={!!editing()}
                placeholder="e.g. my-agent"
              />
            </div>
            <div>
              <label class="block text-xs font-semibold text-ink-secondary mb-1" for="agent-command">
                Command (empty for built-in)
              </label>
              <input
                id="agent-command"
                type="text"
                class={INPUT}
                value={command()}
                onInput={(e) => setCommand(e.currentTarget.value)}
                placeholder="e.g. claude-code or /path/to/agent"
              />
            </div>
            <div>
              <label class="block text-xs font-semibold text-ink-secondary mb-1" for="agent-desc">
                Description
              </label>
              <input
                id="agent-desc"
                type="text"
                class={INPUT}
                value={description()}
                onInput={(e) => setDescription(e.currentTarget.value)}
                placeholder="What this agent does"
              />
            </div>
            {/* Skill multi-select */}
            <Show when={skills().length > 0}>
              <div>
                <p class="text-xs font-semibold text-ink-secondary mb-1">Skills</p>
                <div class="flex flex-wrap gap-1.5">
                  <For each={skills()}>
                    {(skill) => {
                      const checked = () => selectedSkills().includes(skill.name);
                      return (
                        <label class="flex items-center gap-1 text-xs text-ink cursor-pointer">
                          <input
                            type="checkbox"
                            class="accent-accent"
                            checked={checked()}
                            onChange={() => toggleSkill(skill.name)}
                          />
                          {skill.name}
                        </label>
                      );
                    }}
                  </For>
                </div>
              </div>
            </Show>
            <div class="flex gap-2 justify-end">
              <button
                type="button"
                class="text-xs text-ink-secondary hover:text-ink"
                onClick={cancelForm}
              >
                Cancel
              </button>
              <button
                type="button"
                class="px-3 py-1 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors disabled:opacity-50"
                disabled={!name().trim()}
                onClick={() => void save()}
              >
                {editing() ? 'Update' : 'Add'}
              </button>
            </div>
          </div>
        </Show>

        <Show when={!isFormOpen()}>
          <button
            type="button"
            class="self-start text-sm text-accent hover:text-accent-hover underline-offset-2 hover:underline"
            onClick={startAdd}
          >
            + Add agent
          </button>
        </Show>
      </div>
    </fieldset>
  );
}
