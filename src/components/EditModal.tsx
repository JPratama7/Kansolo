import { Show, createEffect, createSignal, onCleanup, onMount } from 'solid-js';
import { Portal } from 'solid-js/web';
import { micromark } from 'micromark';
import { Dialog } from '@ark-ui/solid/dialog';
import { Tabs } from '@ark-ui/solid/tabs';
import type { KanbanCard, Priority, TreeSource } from '../types.ts';
import { PRIORITIES } from '../types.ts';
import { ArkSelect } from './ui/ArkSelect.tsx';
import { toaster } from './ui/toaster.ts';

export interface EditModalResult {
  title: string;
  description: string;
  priority: Priority;
  treeSourceId: string;
}

interface EditModalProps {
  card: KanbanCard | null;
  treeSources: () => TreeSource[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSave: (result: EditModalResult) => void;
  onDirtyChange?: (dirty: boolean) => void;
}

interface EditModalFormProps {
  card: KanbanCard;
  treeSources: () => TreeSource[];
  onClose: () => void;
  onSave: (result: EditModalResult) => void;
  onDirtyChange: (dirty: boolean) => void;
  onPreviewChange: (preview: boolean) => void;
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

/**
 * Inner form, mounted fresh per edited card via `<Show keyed>`. Owns the
 * editable field signals so they reset cleanly when the Board-level singleton
 * switches cards. Reports dirty/preview state upward so the Dialog.Root (which
 * lives in the parent) can guard Escape.
 */
function EditModalForm(props: EditModalFormProps) {
  const [title, setTitle] = createSignal(props.card.title);
  const [description, setDescription] = createSignal(props.card.description);
  const [priority, setPriority] = createSignal<Priority>(props.card.priority);
  const [treeSourceId, setTreeSourceId] = createSignal(props.card.treeSourceId ?? '');
  const [error, setError] = createSignal<string | null>(null);
  // Default to preview: opening a ticket is a reading action, editing is opt-in.
  const [preview, setPreview] = createSignal(true);

  const treeSourceLabel = () =>
    props.treeSources().find((s) => s.id === props.card.treeSourceId)?.label ?? props.card.treeSourceId ?? '';

  /** Track whether the user has unsaved edits (to warn on close). */
  const isDirty = () =>
    title() !== props.card.title ||
    description() !== props.card.description ||
    priority() !== props.card.priority ||
    treeSourceId() !== (props.card.treeSourceId ?? '');

  // Lift dirty/preview state to the wrapper so its close guard can read it.
  createEffect(() => props.onDirtyChange(isDirty()));
  createEffect(() => props.onPreviewChange(preview()));

  // Close request is routed through the wrapper's guarded `onClose` so the
  // unsaved-changes prompt lives in one place (shared by Escape/backdrop/×).
  function maybeClose() {
    props.onClose();
  }

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

  return (
    <form
      class="board-scroll bg-surface rounded-[var(--radius-card)] border border-border-subtle shadow-2xl ticket-panel overflow-y-auto"
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
            <Dialog.CloseTrigger
              class="shrink-0 text-xl text-ink-secondary hover:text-ink leading-none px-1"
              aria-label="Close"
            >
              ×
            </Dialog.CloseTrigger>
          </div>

          <div class="flex items-center justify-between gap-2 mb-3">
            <div class="flex items-center gap-1.5 min-w-0">
              <span class={`priority-pill ${PRIORITY_PILL[props.card.priority]}`}>{props.card.priority}</span>
              <Show when={props.card.sourceRef}>
                <span class="metadata-chip">{props.card.sourceRef}</span>
              </Show>
              <Show when={preview() && treeSourceLabel()}>
                <span class="metadata-chip">{treeSourceLabel()}</span>
              </Show>
            </div>
            <Tabs.Root
              value={preview() ? 'preview' : 'edit'}
              onValueChange={(e) => setPreview(e.value === 'preview')}
            >
              <Tabs.List class="segmented shrink-0" aria-label="View mode">
                <Tabs.Trigger
                  value="preview"
                  class="segmented-btn"
                  classList={{ 'segmented-active': preview() }}
                >
                  Preview
                </Tabs.Trigger>
                <Tabs.Trigger
                  value="edit"
                  class="segmented-btn"
                  classList={{ 'segmented-active': !preview() }}
                >
                  Edit
                </Tabs.Trigger>
              </Tabs.List>
            </Tabs.Root>
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
            <ArkSelect
              items={PRIORITIES.map((p) => ({ label: p, value: p }))}
              value={priority()}
              onValueChange={(v) => setPriority(v as Priority)}
              name="priority"
              class={FIELD}
            />

            <label class="block text-xs font-semibold text-ink-secondary mt-3 mb-1" for="edit-tree-source">
              Tree source <span class="font-normal text-ink-muted">(optional)</span>
            </label>
            <ArkSelect
              items={[
                { label: '(none)', value: '' },
                ...props.treeSources().map((s) => ({ label: s.label, value: s.id })),
              ]}
              value={treeSourceId()}
              onValueChange={setTreeSourceId}
              name="tree_source"
              class={FIELD}
            />
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
  );
}

// Column-rect snapshot for anchoring the modal to the edited card's column.
type ColumnRect = { left: number; top: number; width: number; height: number };

export default function EditModal(props: EditModalProps) {
  const [columnRect, setColumnRect] = createSignal<ColumnRect | null>(null);
  const [viewportH, setViewportH] = createSignal(typeof window !== 'undefined' ? window.innerHeight : 0);
  // Lifted from the form so the Dialog.Root-level close guard can read them.
  const [isDirty, setIsDirty] = createSignal(false);
  const [isPreview, setIsPreview] = createSignal(true);

  // Forward dirty state to Board so it can guard card-switching.
  createEffect(() => props.onDirtyChange?.(isDirty()));
  // Edge case 2B (dedup): at most one persistent confirmation toast per
  // modal instance. Holds the active toast id, or null when none is shown.
  const [pendingConfirmToastId, setPendingConfirmToastId] = createSignal<string | null>(null);

  function readColumnRect() {
    const card = props.card;
    if (!card) { setColumnRect(null); return; }
    const el = document.querySelector(`[data-column-id="${card.column}"]`);
    if (!el) { setColumnRect(null); return; }
    const r = (el as HTMLElement).getBoundingClientRect();
    setColumnRect({ left: r.left, top: r.top, width: r.width, height: r.height });
  }

  // Re-read the column rect whenever the dialog opens or the edited card changes.
  createEffect(() => {
    if (!props.open || !props.card) return;
    readColumnRect();
  });

  // When the modal closes (e.g. via Save), dismiss any lingering
  // confirmation toast so it doesn't outlive the editing session.
  createEffect((prevOpen: boolean | undefined) => {
    const open = props.open;
    if (prevOpen && !open) {
      const pendingId = pendingConfirmToastId();
      if (pendingId !== null) {
        toaster.dismiss(pendingId);
        setPendingConfirmToastId(null);
      }
    }
    return open;
  });

  onMount(() => {
    const onResize = () => { if (props.open) { readColumnRect(); setViewportH(window.innerHeight); } };
    window.addEventListener('resize', onResize);
    onCleanup(() => window.removeEventListener('resize', onResize));
  });

  const anchored = () => columnRect() !== null;
  const top = () => columnRect() ? Math.max(columnRect()!.top, 8) : 0;
  const maxHeight = () => columnRect() ? Math.min(columnRect()!.height, viewportH() - top() - 8) : 0;

  /**
   * Guarded close — replaces the old synchronous `window.confirm` with a
   * persistent confirmation toast (decision 2). The modal stays open until
   * the user clicks "Discard" in the toast. Implements all three edge cases
   * from decision 2B:
   *   - Dedup: one active confirmation toast per modal.
   *   - Second-Esc: while a toast is shown, Esc dismisses the toast (not the
   *     modal). First Esc on dirty modal shows the toast; subsequent Esc
   *     while toast visible dismisses the toast; Esc while clean closes.
   *   - Action-to-modal routing: the toast action captures `props.onOpenChange`
   *     in a closure. If the card was deleted/swapped while the toast was
   *     visible, `props.card` is null — dismiss silently, no crash.
   */
  function requestClose() {
    // Second-Esc / dedup: a confirmation toast is already shown. Dismiss it
    // and keep the modal open.
    const pendingId = pendingConfirmToastId();
    if (pendingId !== null) {
      toaster.dismiss(pendingId);
      setPendingConfirmToastId(null);
      return;
    }
    // Dirty: show a persistent confirmation toast, don't close yet.
    if (!isPreview() && isDirty()) {
      const id = toaster.create({
        title: 'Discard unsaved changes?',
        type: 'warning',
        duration: Infinity,
        action: {
          label: 'Discard',
          onClick: () => {
            // Action-to-modal routing guard: the edited card may have been
            // deleted/swapped while the toast was visible. Dismiss silently.
            if (props.card === null) {
              toaster.dismiss(id);
              setPendingConfirmToastId(null);
              return;
            }
            toaster.dismiss(id);
            setPendingConfirmToastId(null);
            props.onOpenChange(false);
          },
        },
      });
      setPendingConfirmToastId(id);
      return;
    }
    // Clean: close immediately.
    props.onOpenChange(false);
  }

  return (
    <Dialog.Root
      open={props.open}
      lazyMount
      unmountOnExit
      closeOnEscape
      closeOnInteractOutside
      aria-label="Edit card"
      onOpenChange={(e) => { if (!e.open) requestClose(); }}
      onEscapeKeyDown={(e) => {
        // Handle Escape ourselves so the dirty-guard prompt runs exactly once.
        e.preventDefault();
        requestClose();
      }}
      onInteractOutside={(e) => {
        // Handle backdrop/outside clicks ourselves for the same reason.
        e.preventDefault();
        requestClose();
      }}
    >
      <Portal>
        <Dialog.Backdrop class="fixed inset-0 z-50 bg-black/50 ticket-backdrop" />
        <Dialog.Positioner class="fixed z-50">
          <div
            class="fixed"
            classList={{ 'flex items-start justify-center pt-16 px-4': !anchored() }}
            style={anchored()
              ? { left: `${columnRect()!.left}px`, top: `${top()}px`, width: `${columnRect()!.width}px` }
              : { inset: '0' }}
          >
            <Dialog.Content
              class="board-scroll bg-surface rounded-[var(--radius-card)] border border-border-subtle shadow-2xl ticket-panel overflow-y-auto"
              classList={{ 'w-full max-w-2xl max-h-[85vh]': !anchored() }}
              style={anchored() ? { 'max-height': `${maxHeight()}px` } : undefined}
            >
              <Show when={props.card} keyed>
                {(card) => (
                  <EditModalForm
                    card={card}
                    treeSources={props.treeSources}
                    onClose={requestClose}
                    onSave={props.onSave}
                    onDirtyChange={setIsDirty}
                    onPreviewChange={setIsPreview}
                  />
                )}
              </Show>
            </Dialog.Content>
          </div>
        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
  );
}
