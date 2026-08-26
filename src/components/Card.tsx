import { Show, createSignal } from 'solid-js';
import { createDraggable, useDragDropContext } from '@thisbeyond/solid-dnd';
import { micromark } from 'micromark';
import { invoke } from '@tauri-apps/api/core';
import type { KanbanCard, Priority, TreeSource } from '../types.ts';
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
  sourceLabels: () => Record<string, string>;
  sourceEditors: () => Record<string, string | undefined>;
  onEdit: (id: string, title: string, description: string, priority: Priority, sourcePath: string) => void;
  onDelete: (id: string) => void;
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
    props.onEdit(card.id, result.title, result.description, result.priority, result.sourcePath);
    setEditing(false);
  }

  function onContextMenu(e: MouseEvent) {
    // No source path and no tree sources registered → nothing to open or edit.
    if (!card.sourcePath && props.treeSources().length === 0) return;
    e.preventDefault();
    setMenu({ x: e.clientX, y: e.clientY });
  }

  async function openInEditor() {
    setMenu(null);
    const path = card.sourcePath;
    if (!path) return;
    const command = props.sourceEditors()[path];
    try {
      await invoke('open_in_editor', { path, command });
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
        classList={{ 'opacity-50 rotate-1 z-50 relative': isDragging() }}
        onContextMenu={onContextMenu}
      >
        <div class={`priority-strip ${PRIORITY_STRIP[card.priority]}`} aria-hidden="true" />
        <div class="px-3 py-2">
          <p class="text-sm text-ink leading-snug">{card.title}</p>
          <Show when={card.description}>
            <div class="md-preview text-xs text-ink-secondary mt-1 line-clamp-2" innerHTML={micromark(card.description)} />
          </Show>
          <Show when={card.sourcePath}>
            <p class="text-[10px] font-mono text-ink-muted mt-1 truncate" title={card.sourcePath}>
              {props.sourceLabels()[card.sourcePath!] ?? card.sourcePath}
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
              <div class="flex gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
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
              <div class="flex gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
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
            <div class="fixed inset-0 z-40" onClick={closeMenu} onContextMenu={(e) => { e.preventDefault(); closeMenu(); }} />
            <div
              class="fixed z-50 min-w-[10rem] bg-surface border border-border-subtle rounded-[var(--radius-card)] shadow-2xl py-1"
              style={{ left: `${m().x}px`, top: `${m().y}px` }}
            >
              <Show when={card.sourcePath}>
                <button
                  type="button"
                  class="w-full text-left text-sm text-ink px-3 py-1.5 hover:bg-elevated transition-colors"
                  onClick={openInEditor}
                >
                  Open in editor
                </button>
              </Show>
              <Show when={!card.sourcePath}>
                <button
                  type="button"
                  class="w-full text-left text-sm text-ink px-3 py-1.5 hover:bg-elevated transition-colors"
                  onClick={editFromMenu}
                >
                  Edit current card
                </button>
              </Show>
            </div>
          </>
        )}
      </Show>
    </>
  );
}
