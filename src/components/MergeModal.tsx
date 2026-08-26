import { For, createSignal, onCleanup, onMount } from 'solid-js';
import type { ConflictResolution, SyncConflict } from '../types.ts';

interface MergeModalProps {
  conflicts: SyncConflict[];
  onResolve: (resolutions: ConflictResolution[]) => void;
  onCancel: () => void;
}

type Choice = 'local' | 'remote';
type Choices = Record<string, Choice>;

const FIELD_LABELS: Record<string, string> = {
  title: 'Title',
  description: 'Description',
  priority: 'Priority',
  column: 'Column',
  sourceStatus: 'Source status',
};

export default function MergeModal(props: MergeModalProps) {
  // Per-field choice, keyed `${sourceRef}:${field}`.
  const [choices, setChoices] = createSignal<Choices>({});

  onMount(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') props.onCancel(); };
    window.addEventListener('keydown', onKey);
    onCleanup(() => window.removeEventListener('keydown', onKey));
  });

  function choiceFor(ref: string, field: string): Choice {
    return choices()[`${ref}:${field}`] ?? 'local';
  }

  function pick(ref: string, field: string, value: Choice) {
    setChoices((prev) => ({ ...prev, [`${ref}:${field}`]: value }));
  }

  function takeAll(ref: string, conflict: SyncConflict, value: Choice) {
    setChoices((prev) => {
      const next = { ...prev };
      for (const c of conflict.conflicts) {
        next[`${ref}:${c.field}`] = value;
      }
      return next;
    });
  }

  /** Package the per-field radio choices into ConflictResolution[] for Rust. */
  function submit() {
    const resolutions: ConflictResolution[] = props.conflicts.map((conflict) => {
      const fieldChoices: Record<string, 'local' | 'remote'> = {};
      for (const c of conflict.conflicts) {
        fieldChoices[c.field] = choiceFor(conflict.sourceRef, c.field);
      }
      return { sourceRef: conflict.sourceRef, choices: fieldChoices };
    });
    props.onResolve(resolutions);
  }

  return (
    <div
      class="fixed inset-0 z-50 flex items-start justify-center pt-12 px-4 bg-black/50"
      role="dialog"
      aria-modal="true"
      aria-label="Merge conflicts"
      onClick={props.onCancel}
    >
      <section
        class="w-full max-w-3xl max-h-[85vh] overflow-y-auto board-scroll bg-surface rounded-[var(--radius-card)] border border-border-subtle shadow-2xl"
        aria-label="Merge conflicts"
        onClick={(e) => e.stopPropagation()}
      >
        <div class="sticky top-0 flex items-center justify-between px-4 py-3 bg-surface border-b border-border-subtle">
          <h2 class="text-base font-bold text-ink">
            Merge conflicts <span class="tabular-nums">({props.conflicts.length})</span>
          </h2>
          <button
            type="button"
            class="text-xl text-ink-secondary hover:text-ink leading-none px-1"
            aria-label="Close"
            onClick={props.onCancel}
          >
            ×
          </button>
        </div>

        <div class="p-4 flex flex-col gap-4">
          <p class="text-xs text-ink-secondary">
            Local edits and remote changes diverged. Pick per field, or take all of one side per card.
          </p>

          <For each={props.conflicts}>
            {(conflict) => (
              <div class="rounded-[var(--radius-card)] border border-border-subtle p-3">
                <div class="flex items-center justify-between mb-2">
                  <span class="text-xs font-mono text-ink-secondary bg-base rounded px-1.5 py-0.5">
                    {conflict.sourceRef}
                  </span>
                  <div class="flex gap-1">
                    <button
                      type="button"
                      class="text-xs px-2 py-1 rounded border border-border-subtle text-ink-secondary hover:bg-elevated"
                      onClick={() => takeAll(conflict.sourceRef, conflict, 'local')}
                    >
                      All local
                    </button>
                    <button
                      type="button"
                      class="text-xs px-2 py-1 rounded border border-border-subtle text-ink-secondary hover:bg-elevated"
                      onClick={() => takeAll(conflict.sourceRef, conflict, 'remote')}
                    >
                      All remote
                    </button>
                  </div>
                </div>

                <For each={conflict.conflicts}>
                  {(c) => (
                    <div class="mb-3 last:mb-0">
                      <div class="text-xs font-semibold text-ink-secondary mb-1">
                        {FIELD_LABELS[c.field] ?? c.field}
                      </div>
                      <div class="grid grid-cols-2 gap-2">
                        <label
                          class="flex gap-2 items-start text-sm rounded px-2 py-1.5 border cursor-pointer"
                          classList={{
                            'border-accent bg-accent/10': choiceFor(conflict.sourceRef, c.field) === 'local',
                            'border-border-subtle': choiceFor(conflict.sourceRef, c.field) !== 'local',
                          }}
                        >
                          <input
                            type="radio"
                            name={`${conflict.sourceRef}:${c.field}`}
                            value="local"
                            checked={choiceFor(conflict.sourceRef, c.field) === 'local'}
                            onChange={() => pick(conflict.sourceRef, c.field, 'local')}
                          />
                          <span class="break-words text-ink">{c.local || '(empty)'}</span>
                        </label>
                        <label
                          class="flex gap-2 items-start text-sm rounded px-2 py-1.5 border cursor-pointer"
                          classList={{
                            'border-accent bg-accent/10': choiceFor(conflict.sourceRef, c.field) === 'remote',
                            'border-border-subtle': choiceFor(conflict.sourceRef, c.field) !== 'remote',
                          }}
                        >
                          <input
                            type="radio"
                            name={`${conflict.sourceRef}:${c.field}`}
                            value="remote"
                            checked={choiceFor(conflict.sourceRef, c.field) === 'remote'}
                            onChange={() => pick(conflict.sourceRef, c.field, 'remote')}
                          />
                          <span class="break-words text-ink">{c.remote || '(empty)'}</span>
                        </label>
                      </div>
                    </div>
                  )}
                </For>
              </div>
            )}
          </For>

          <div class="flex gap-2 justify-end sticky bottom-0 bg-surface pt-2">
            <button
              type="button"
              class="px-3 py-1.5 text-sm font-medium rounded text-ink-secondary hover:bg-elevated transition-colors"
              onClick={props.onCancel}
            >
              Cancel sync
            </button>
            <button
              type="button"
              class="px-3 py-1.5 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors"
              onClick={submit}
            >
              Apply merge
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}
