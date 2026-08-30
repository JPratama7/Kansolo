import { For, Show, createMemo } from 'solid-js';
import type { SkillManifest } from '../db.ts';

export interface SkillPickerProps {
  /** All available skills from disk (acp_list_skills). */
  available: SkillManifest[];
  /** The agent's configured skill names. */
  agentSkills: string[];
  /** Currently selected skill names (controlled). */
  selected: string[];
  /** Called when the selection changes. */
  onChange: (selected: string[]) => void;
}

/** Per-run skill subset selection. Lists the selected agent's skills with
 * checkboxes (pre-checked = all). Selected names passed to `acp_create_run`
 * as `skill_names`. If agent has no skills, picker is skipped. */
export default function SkillPicker(props: SkillPickerProps) {
  /** Filter available skills to only those in the agent's skill list. */
  const agentSkillManifests = createMemo(() => {
    const map = new Map(props.available.map((s) => [s.name, s]));
    return props.agentSkills
      .map((name) => map.get(name))
      .filter((s): s is SkillManifest => s !== undefined);
  });

  function toggle(name: string) {
    const current = new Set(props.selected);
    if (current.has(name)) current.delete(name);
    else current.add(name);
    props.onChange([...current]);
  }

  return (
    <Show when={agentSkillManifests().length > 0}>
      <div class="flex flex-col gap-1.5">
        <p class="text-xs font-semibold text-ink-secondary">Skills</p>
        <For each={agentSkillManifests()}>
          {(skill) => {
            const checked = () => props.selected.includes(skill.name);
            return (
              <label class="flex items-start gap-2 text-sm text-ink cursor-pointer">
                <input
                  type="checkbox"
                  class="accent-accent mt-0.5"
                  checked={checked()}
                  onChange={() => toggle(skill.name)}
                />
                <div class="min-w-0">
                  <p class="font-medium leading-tight">{skill.name}</p>
                  <Show when={skill.description}>
                    <p class="text-xs text-ink-secondary line-clamp-2">{skill.description}</p>
                  </Show>
                </div>
              </label>
            );
          }}
        </For>
      </div>
    </Show>
  );
}
