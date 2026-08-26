import { For, Show, createEffect, createSignal, onCleanup } from 'solid-js';
import { createDraggable, useDragDropContext } from '@thisbeyond/solid-dnd';
import { micromark } from 'micromark';
import { invoke } from '@tauri-apps/api/core';
import type { ColumnId, KanbanCard, Priority, TreeSource } from '../types.ts';
import { COLUMNS } from '../columns.ts';
import EditModal, { type EditModalResult } from './EditModal.tsx';

declare module 'solid-js' {
  namespace JSX {
    interface Directives {
      draggable: (el: HTMLElement, accessor: () => unknown) => void;
    }
  }
}

interface CardProps {
  card: KanbanCard;
  treeSources: () => TreeSource[];
  onEdit: (id: string, title: string, description: string, priority: Priority, treeSourceId: string) => void;
  onDelete: (id: string) => void;
  onMove: (id: string, column: ColumnId) => void;
}

const PRIORITY_STRIP: Record<Priority, string> = {
  low: 'bg-p-low',
  medium: 'bg-p-med',
  high: 'bg-p-high',
  urgent: 'bg-p-urgent',
};

export default function Card(props: CardProps) {
  const card = props.card;
  const draggable = createDraggable({ id: card.id, data: card });
  const [dndState] = useDragDropContext() as [
    { active: { draggable: string | null; droppable: string | null } },
  ];
  const isDragging = () => dndState.active.draggable === card.id;
  const [editing, setEditing] = createSignal(false);
  const [menu, setMenu] = createSignal<{ x: number; y: number } | null>(null);

  function save(result: EditModalResult) {
    props.onEdit(card.id, result.title, result.description, result.priority, result.treeSourceId);
    setEditing(false);
  }

  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    setMenu({ x: e.clientX, y: e.clientY });
  }

  /** Keyboard alternative to right-click: Shift+F10 opens the menu. */
  function onKeyDown(e: KeyboardEvent) {
    if (e.shiftKey && e.key === 'F10') {
      e.preventDefault();
      const el = e.currentTarget as HTMLElement;
      const r = el.getBoundingClientRect();
      setMenu({ x: r.left, y: r.bottom });
    }
  }

  function moveTo(col: ColumnId) {
    setMenu(null);
    if (col !== card.column) props.onMove(card.id, col);
  }

  // Close the context menu when the window loses focus (e.g. user clicks
  // another app, alt-tabs, or switches virtual desktop).
  createEffect(() => {
    if (!menu()) return;
    const close = () => setMenu(null);
    window.addEventListener('blur', close);
    onCleanup(() => window.removeEventListener('blur', close));
  });

  async function openInEditor() {
    setMenu(null);
    const id = card.treeSourceId;
    if (!id) return;
    const src = props.treeSources().find((s) => s.id === id)!;
    try {
      await invoke('open_in_editor', { path: src.path, command: src.editorCommand });
    } catch (e) {
      console.error('open_in_editor failed:', e);
    }
  }

  function editFromMenu() {
    setMenu(null);
    setEditing(true);
  }

  function closeMenu() {
    setMenu(null);
  }

  return (
    <>
      <article
        use:draggable={draggable}
        class="trello-card group bg-elevated rounded-[var(--radius-card)] border border-border-subtle cursor-grab active:cursor-grabbing"
        classList={{ 'opacity-0 is-dragging': isDragging() }}
        onContextMenu={onContextMenu}
        onKeyDown={onKeyDown}
        tabindex={0}
      >
        <div class={`priority-strip ${PRIORITY_STRIP[card.priority]}`} aria-hidden="true" />
        <div class="px-3 py-2">
          <p class="text-sm text-ink leading-snug">{card.title}</p>
          <Show when={card.description}>
            <div class="md-preview text-xs text-ink-secondary mt-1 line-clamp-2" innerHTML={micromark(card.description)} />
          </Show>
          <Show when={card.treeSourceId}>
            <p class="text-[10px] font-mono text-ink-muted mt-1 truncate" title={card.treeSourceId}>
              {props.treeSources().find((s) => s.id === card.treeSourceId)?.label ?? card.treeSourceId}
            </p>
          </Show>
          <div class="flex items-center justify-between gap-2 mt-2">
            <div class="flex items-center gap-1.5 min-w-0">
              <span class="text-[10px] font-semibold uppercase tracking-wide text-ink-secondary">
                {card.priority}
              </span>
              {card.source !== 'local' && card.sourceRef && (
                <span class="text-[10px] font-mono text-ink-secondary bg-base/60 rounded px-1 py-0.5 truncate">
                  {card.sourceRef}
                </span>
              )}
            </div>
            {card.source === 'local' && (
              <div class="flex gap-1 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-opacity">
                <button
                  type="button"
                  class="text-xs text-ink-secondary hover:text-ink px-1"
                  onClick={() => setEditing(true)}
                >
                  Edit
                </button>
                <button
                  type="button"
                  class="text-xs text-ink-secondary hover:text-p-urgent px-1"
                  onClick={() => props.onDelete(card.id)}
                >
                  Delete
                </button>
              </div>
            )}
            {card.source !== 'local' && (
              <div class="flex gap-1 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-opacity">
                <button
                  type="button"
                  class="text-xs text-ink-secondary hover:text-ink px-1"
                  onClick={() => setEditing(true)}
                >
                  Edit
                </button>
              </div>
            )}
          </div>
        </div>
      </article>
      <Show when={editing()}>
        <EditModal
          card={card}
          treeSources={props.treeSources}
          sourceRef={card.sourceRef}
          onCancel={() => setEditing(false)}
          onSave={save}
        />
      </Show>
      <Show when={menu()}>
        {(m) => (
          <>
            {/* Click-away overlay — closes the menu on any outside interaction. */}
            <div
              class="fixed inset-0 z-40"
              onClick={closeMenu}
              onContextMenu={(e) => { e.preventDefault(); closeMenu(); }}
              onKeyDown={(e) => { if (e.key === 'Escape') closeMenu(); }}
              tabindex={-1}
              aria-hidden="true"
            />
            <div
              class="fixed z-50 min-w-[10rem] bg-surface border border-border-subtle rounded-[var(--radius-card)] shadow-2xl py-1"
              style={{ left: `${m().x}px`, top: `${m().y}px` }}
              role="menu"
              aria-label="Card actions"
            >
              <button
                type="button"
                role="menuitem"
                class="w-full text-left text-sm text-ink px-3 py-1.5 hover:bg-elevated transition-colors"
                onClick={editFromMenu}
              >
                Edit
              </button>
              <Show when={card.treeSourceId}>
                <button
                  type="button"
                  role="menuitem"
                  class="w-full text-left text-sm text-ink px-3 py-1.5 hover:bg-elevated transition-colors"
                  onClick={openInEditor}
                >
                  Open in editor
                </button>
              </Show>
              <div class="my-1 border-t border-border-subtle" role="separator" />
              <For each={COLUMNS}>
                {(col) => (
                  <button
                    type="button"
                    role="menuitem"
                    class="w-full text-left text-sm text-ink px-3 py-1.5 hover:bg-elevated transition-colors disabled:opacity-40"
                    disabled={col.id === card.column}
                    onClick={() => moveTo(col.id)}
                  >
                    Move to {col.title}
                  </button>
                )}
              </For>
            </div>
          </>
        )}
      </Show>
    </>
  );
}
