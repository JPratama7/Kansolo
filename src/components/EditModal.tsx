import { For, Show, createSignal } from 'solid-js';
import { micromark } from 'micromark';
import type { KanbanCard, Priority, TreeSource } from '../types.ts';
import { PRIORITIES } from '../types.ts';

export interface EditModalResult {
  title: string;
  description: string;
  priority: Priority;
  sourcePath: string;
}

interface EditModalProps {
  card: KanbanCard;
  treeSources: () => TreeSource[];
  sourceRef?: string;
  onCancel: () => void;
  onSave: (result: EditModalResult) => void;
}

const FIELD =
  'w-full text-sm rounded px-2 py-1.5 bg-base text-ink placeholder:text-ink-muted border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent';

export default function EditModal(props: EditModalProps) {
  const [title, setTitle] = createSignal(props.card.title);
  const [description, setDescription] = createSignal(props.card.description);
  const [priority, setPriority] = createSignal<Priority>(props.card.priority);
  const [sourcePath, setSourcePath] = createSignal(props.card.sourcePath ?? '');
  const [error, setError] = createSignal<string | null>(null);
  const [preview, setPreview] = createSignal(false);

  function submit(e: Event) {
    e.preventDefault();
    const trimmed = title().trim();
    if (trimmed === '') {
      setError('Title cannot be empty.');
      return;
    }
    props.onSave({
      title: trimmed,
      description: description().trim(),
      priority: priority(),
      sourcePath: sourcePath().trim(),
    });
  }

  return (
    <div
      class="fixed inset-0 z-50 flex items-start justify-center pt-16 px-4 bg-black/50"
      role="dialog"
      aria-modal="true"
      onClick={props.onCancel}
    >
      <form
        class="w-full max-w-2xl max-h-[85vh] overflow-y-auto board-scroll bg-surface rounded-[var(--radius-card)] border border-border-subtle shadow-2xl p-4"
        onClick={(e) => e.stopPropagation()}
        onSubmit={submit}
      >
        <div class="flex items-start justify-between mb-3">
          <h2 class="text-base font-bold text-ink">Edit card</h2>
          <button
            type="button"
            class="text-xl text-ink-secondary hover:text-ink leading-none px-1"
            aria-label="Close"
            onClick={props.onCancel}
          >
            ×
          </button>
        </div>
        <Show when={props.sourceRef}>
          <span class="inline-block text-xs font-mono text-ink-secondary bg-base rounded px-1.5 py-0.5 mb-3">
            {props.sourceRef}
          </span>
        </Show>

        <label class="block text-xs font-semibold text-ink-secondary mb-1">Title</label>
        <input
          class={FIELD}
          value={title()}
          onInput={(e) => setTitle(e.currentTarget.value)}
          placeholder="Title"
          autofocus
        />

        <div class="flex items-center justify-between mt-3 mb-1">
          <label class="block text-xs font-semibold text-ink-secondary">
            Description
          </label>
          <button
            type="button"
            class="text-xs text-ink-secondary hover:text-ink"
            onClick={() => setPreview(!preview())}
          >
            {preview() ? 'Edit' : 'Preview'}
          </button>
        </div>
        <Show
          when={!preview()}
          fallback={
            <div class="md-preview w-full min-h-[6rem] text-sm rounded px-2 py-1.5 bg-base text-ink border border-border-subtle">
              <Show
                when={description().trim()}
                fallback={
                  <p class="text-ink-muted">No description.</p>
                }
              >
                <div class="md-preview w-full min-h-[6rem] text-sm rounded px-2 py-1.5 bg-base text-ink border border-border-subtle" innerHTML={micromark(description())} />
              </Show>
            </div>
          }
        >
          <textarea
            class={`${FIELD} resize-y`}
            value={description()}
            onInput={(e) => setDescription(e.currentTarget.value)}
            placeholder="Add a more detailed description…"
            rows={8}
          />
        </Show>

        <label class="block text-xs font-semibold text-ink-secondary mt-3 mb-1">Priority</label>
        <select
          class={FIELD}
          value={priority()}
          onChange={(e) => setPriority(e.currentTarget.value as Priority)}
        >
          <For each={PRIORITIES}>{(p) => <option value={p}>{p}</option>}</For>
        </select>

        <label class="block text-xs font-semibold text-ink-secondary mt-3 mb-1">
          Source path <span class="font-normal text-ink-muted">(optional)</span>
        </label>
        <select
          class={FIELD}
          value={sourcePath()}
          onChange={(e) => setSourcePath(e.currentTarget.value)}
        >
          <option value="">(none)</option>
          <For each={props.treeSources()}>
            {(src) => <option value={src.path}>{src.label}</option>}
          </For>
          {/* Preserve a sourcePath that isn't a registered tree source; without
              this the select can't match it, falls back to (none), and save
              would wipe the value. */}
          <Show
            when={sourcePath() && !props.treeSources().some((s) => s.path === sourcePath())}
          >
            <option value={sourcePath()}>{sourcePath()}</option>
          </Show>
        </select>

        <Show when={error()}>
          <p class="mt-3 text-sm text-p-urgent" role="alert">{error()}</p>
        </Show>

        <div class="flex gap-2 justify-end mt-4">
          <button
            type="button"
            class="px-3 py-1.5 text-sm font-medium rounded text-ink-secondary hover:bg-elevated transition-colors"
            onClick={props.onCancel}
          >
            Cancel
          </button>
          <button
            type="submit"
            class="px-3 py-1.5 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors"
          >
            Save
          </button>
        </div>
      </form>
    </div>
  );
}
