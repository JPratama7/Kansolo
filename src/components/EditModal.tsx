import { For, Show, createSignal, onCleanup, onMount } from 'solid-js';
import { micromark } from 'micromark';
import type { KanbanCard, Priority, TreeSource } from '../types.ts';
import { PRIORITIES } from '../types.ts';

export interface EditModalResult {
  title: string;
  description: string;
  priority: Priority;
  treeSourceId: string;
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

const PRIORITY_STRIP: Record<Priority, string> = {
  low: 'bg-p-low',
  medium: 'bg-p-med',
  high: 'bg-p-high',
  urgent: 'bg-p-urgent',
};

const PRIORITY_PILL: Record<Priority, string> = {
  low: 'priority-pill--low',
  medium: 'priority-pill--medium',
  high: 'priority-pill--high',
  urgent: 'priority-pill--urgent',
};

export default function EditModal(props: EditModalProps) {
  const [title, setTitle] = createSignal(props.card.title);
  const [description, setDescription] = createSignal(props.card.description);
  const [priority, setPriority] = createSignal<Priority>(props.card.priority);
  const [treeSourceId, setTreeSourceId] = createSignal(props.card.treeSourceId ?? '');
  const [error, setError] = createSignal<string | null>(null);
  // Default to preview: opening a ticket is a reading action, editing is opt-in.
  const [preview, setPreview] = createSignal(true);

  const treeSourceLabel = () =>
    props.treeSources().find((s) => s.id === props.card.treeSourceId)?.label ?? props.card.treeSourceId ?? '';

  // Rect of the column the edited card belongs to. When present, the modal
  // overlays that column (left/top/width match). Null falls back to a centered
  // layout so the modal still works if the column DOM can't be found.
  const [columnRect, setColumnRect] = createSignal<{ left: number; top: number; width: number; height: number } | null>(null);
  const [viewportH, setViewportH] = createSignal(typeof window !== 'undefined' ? window.innerHeight : 0);

  function readColumnRect() {
    const el = document.querySelector(`[data-column-id="${props.card.column}"]`);
    if (!el) { setColumnRect(null); return; }
    const r = (el as HTMLElement).getBoundingClientRect();
    setColumnRect({ left: r.left, top: r.top, width: r.width, height: r.height });
  }

  /** Track whether the user has unsaved edits (to warn on close). */
  const isDirty = () =>
    title() !== props.card.title ||
    description() !== props.card.description ||
    priority() !== props.card.priority ||
    treeSourceId() !== (props.card.treeSourceId ?? '');

  function maybeClose() {
    if (!preview() && isDirty()) {
      if (!window.confirm('Discard unsaved changes?')) return;
    }
    props.onCancel();
  }

  onMount(() => {
    readColumnRect();
    const onResize = () => { readColumnRect(); setViewportH(window.innerHeight); };
    window.addEventListener('resize', onResize);
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') maybeClose(); };
    window.addEventListener('keydown', onKey);
    onCleanup(() => {
      window.removeEventListener('resize', onResize);
      window.removeEventListener('keydown', onKey);
    });
  });

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
      treeSourceId: treeSourceId().trim(),
    });
  }

  const rect = columnRect();
  const anchored = rect !== null;
  const top = rect ? Math.max(rect.top, 8) : 0;
  const maxHeight = rect ? Math.min(rect.height, viewportH() - top - 8) : 0;

  return (
    <div
      class="fixed inset-0 z-50 bg-black/50 ticket-backdrop"
      classList={{
        'flex items-start justify-center pt-16 px-4': !anchored,
      }}
      role="dialog"
      aria-modal="true"
      aria-label="Edit card"
      onClick={maybeClose}
    >
      <form
        class="board-scroll bg-surface rounded-[var(--radius-card)] border border-border-subtle shadow-2xl ticket-panel overflow-y-auto"
        classList={{
          'fixed': anchored,
          'w-full max-w-2xl max-h-[85vh]': !anchored,
          'max-h-[85vh]': !anchored,
        }}
        style={anchored
          ? { left: `${rect!.left}px`, top: `${top}px`, width: `${rect!.width}px`, 'max-height': `${maxHeight}px` }
          : undefined}
        onClick={(e) => e.stopPropagation()}
        onSubmit={submit}
      >
        <div class={`priority-strip ${PRIORITY_STRIP[props.card.priority]}`} aria-hidden="true" />
        <div class="p-4">
          <div class="flex items-start justify-between gap-3 mb-3">
            <Show
              when={!preview()}
              fallback={
                <h2 class="text-lg font-semibold text-ink leading-snug break-words">{title()}</h2>
              }
            >
              <label for="edit-title" class="sr-only">Title</label>
              <input
                id="edit-title"
                name="title"
                autocomplete="off"
                class="w-full text-lg font-semibold bg-transparent text-ink outline-none border-b border-transparent focus:border-accent pb-0.5"
                value={title()}
                onInput={(e) => setTitle(e.currentTarget.value)}
                placeholder="Title…"
                autofocus
              />
            </Show>
            <button
              type="button"
              class="shrink-0 text-xl text-ink-secondary hover:text-ink leading-none px-1"
              aria-label="Close"
              onClick={maybeClose}
            >
              ×
            </button>
          </div>

          <div class="flex items-center justify-between gap-2 mb-3">
            <div class="flex items-center gap-1.5 min-w-0">
              <span class={`priority-pill ${PRIORITY_PILL[props.card.priority]}`}>{props.card.priority}</span>
              <Show when={props.sourceRef}>
                <span class="metadata-chip">{props.sourceRef}</span>
              </Show>
              <Show when={preview() && treeSourceLabel()}>
                <span class="metadata-chip">{treeSourceLabel()}</span>
              </Show>
            </div>
            <div class="segmented shrink-0" role="tablist" aria-label="View mode" onKeyDown={(e) => {
              if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
                e.preventDefault();
                setPreview(!preview());
                // Focus the newly-active tab.
                queueMicrotask(() => {
                  const tabs = (e.currentTarget as HTMLElement).querySelectorAll('[role="tab"]');
                  const idx = preview() ? 0 : 1;
                  (tabs[idx] as HTMLElement)?.focus();
                });
              }
            }}>
              <button
                type="button"
                role="tab"
                aria-selected={preview()}
                class="segmented-btn"
                classList={{ 'segmented-active': preview() }}
                onClick={() => setPreview(true)}
              >
                Preview
              </button>
              <button
                type="button"
                role="tab"
                aria-selected={!preview()}
                class="segmented-btn"
                classList={{ 'segmented-active': !preview() }}
                onClick={() => setPreview(false)}
              >
                Edit
              </button>
            </div>
          </div>

          <div class="h-px bg-border-subtle mb-3" />

          <Show
            when={!preview()}
            fallback={
              <div class="md-preview w-full min-h-[8rem] text-sm text-ink">
                <Show
                  when={description().trim()}
                  fallback={<p class="text-ink-muted">No description.</p>}
                >
                  <div innerHTML={micromark(description())} />
                </Show>
              </div>
            }
          >
            <label class="block text-xs font-semibold text-ink-secondary mb-1" for="edit-description">Description</label>
            <textarea
              id="edit-description"
              name="description"
              autocomplete="off"
              class={`${FIELD} resize-y`}
              value={description()}
              onInput={(e) => setDescription(e.currentTarget.value)}
              placeholder="Add a more detailed description…"
              rows={8}
            />
          </Show>

          <Show when={!preview()}>
            <label class="block text-xs font-semibold text-ink-secondary mt-3 mb-1" for="edit-priority">Priority</label>
            <select
              id="edit-priority"
              name="priority"
              class={FIELD}
              value={priority()}
              onChange={(e) => setPriority(e.currentTarget.value as Priority)}
            >
              <For each={PRIORITIES}>{(p) => <option value={p}>{p}</option>}</For>
            </select>

            <label class="block text-xs font-semibold text-ink-secondary mt-3 mb-1" for="edit-tree-source">
              Tree source <span class="font-normal text-ink-muted">(optional)</span>
            </label>
            <select
              id="edit-tree-source"
              name="tree_source"
              class={FIELD}
              value={treeSourceId()}
              onChange={(e) => setTreeSourceId(e.currentTarget.value)}
            >
              <option value="">(none)</option>
              <For each={props.treeSources()}>
                {(src) => <option value={src.id}>{src.label}</option>}
              </For>
            </select>
          </Show>

          <Show when={error()}>
            <p class="mt-3 text-sm text-p-urgent" role="alert">{error()}</p>
          </Show>

          <div class="flex gap-2 justify-end mt-4">
            <button
              type="button"
              class="px-3 py-1.5 text-sm font-medium rounded text-ink-secondary hover:bg-elevated transition-colors"
              onClick={maybeClose}
            >
              {preview() ? 'Close' : 'Cancel'}
            </button>
            <Show when={!preview()}>
              <button
                type="submit"
                class="px-3 py-1.5 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors"
              >
                Save
              </button>
            </Show>
          </div>
        </div>
      </form>
    </div>
  );
}
