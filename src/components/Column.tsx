import { For, Show, createSignal } from 'solid-js';
import { createDroppable, useDragDropContext } from '@thisbeyond/solid-dnd';
import type { ColumnId, KanbanCard, TreeSource } from '../types.ts';
import Card from './Card.tsx';

declare module 'solid-js' {
  namespace JSX {
    interface Directives {
      droppable: (el: HTMLElement, accessor: () => unknown) => void;
    }
  }
}

interface ColumnProps {
  column: { id: ColumnId; title: string };
  cards: () => KanbanCard[];
  treeSources: () => TreeSource[];
  onAdd: (title: string) => void;
  onOpenEdit: (card: KanbanCard) => void;
  onDelete: (id: string) => void;
  onContextMenuOpen: (card: KanbanCard, point: { x: number; y: number }) => void;
}

import { PRIORITIES } from '../types.ts';

const COL_ACCENT: Record<ColumnId, string> = {
  backlog: 'bg-col-backlog',
  ongoing: 'bg-col-ongoing',
  done: 'bg-col-done',
};

export default function Column(props: ColumnProps) {
  const droppable = createDroppable({
    id: props.column.id,
    data: props.column.id,
  });
  const [dndState] = useDragDropContext() as [
    { active: { draggable: string | null; droppable: string | null } },
  ];
  const isOver = () => dndState.active.droppable === props.column.id;

  const columnCards = () => props.cards().filter((c) => c.column === props.column.id);
  // Sort by priority (urgent → low), then by position within same priority.
  const sortedCards = () =>
    [...columnCards()].sort((a, b) => {
      const pa = PRIORITIES.indexOf(a.priority);
      const pb = PRIORITIES.indexOf(b.priority);
      if (pb !== pa) return pb - pa;
      return a.position - b.position;
    });

  const [adding, setAdding] = createSignal(false);
  const [newTitle, setNewTitle] = createSignal('');

  function submitAdd(e: Event) {
    e.preventDefault();
    const title = newTitle().trim();
    if (title) {
      props.onAdd(title);
      setNewTitle('');
    }
  }

  return (
    <section
      use:droppable={droppable}
      data-column-id={props.column.id}
      class="@container flex flex-col flex-1 min-w-0 max-h-full bg-surface rounded-[var(--radius-list)] border border-border-subtle"
      classList={{ 'ring-2 ring-accent': isOver() }}
    >
      <div class={`col-accent ${COL_ACCENT[props.column.id]}`} aria-hidden="true" />

      <header class="flex items-center justify-between px-3 pt-2 pb-1">
        <h2 class="text-sm font-bold text-ink">{props.column.title}</h2>
        <span class="text-xs font-semibold text-ink-secondary bg-elevated rounded px-1.5 py-0.5 tabular-nums">
          {sortedCards().length}
        </span>
      </header>

      <div class="card-grid px-2 py-1 board-scroll overflow-y-auto">
        <Show
          when={sortedCards().length > 0}
          fallback={
            <p class="text-ink-muted text-xs py-3 text-center">Drag cards here</p>
          }
        >
          <For each={sortedCards()}>
            {(card) => <Card card={card} treeSources={props.treeSources} onOpenEdit={props.onOpenEdit} onDelete={props.onDelete} onContextMenuOpen={props.onContextMenuOpen} />}
          </For>
        </Show>
      </div>

      <div class="px-2 py-1.5">
        <Show
          when={adding()}
          fallback={
            <button
              type="button"
              class="add-link w-full text-left text-sm rounded px-2 py-1.5 transition-colors"
              onClick={() => setAdding(true)}
            >
              + Add a card
            </button>
          }
        >
          <form onSubmit={submitAdd}>
            <label for={`add-card-${props.column.id}`} class="sr-only">Card title</label>
            <textarea
              id={`add-card-${props.column.id}`}
              name="card_title"
              autocomplete="off"
              class="w-full text-sm rounded-[var(--radius-card)] p-2 bg-elevated text-ink placeholder:text-ink-muted resize-none outline-none focus:ring-2 focus:ring-accent border border-border-subtle"
              value={newTitle()}
              onInput={(e) => setNewTitle(e.currentTarget.value)}
              placeholder="Enter a title for this card…"
              rows={3}
              autofocus
            />
            <div class="flex items-center gap-2 mt-1.5">
              <button
                type="submit"
                class="px-3 py-1.5 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors"
              >
                Add card
              </button>
              <button
                type="button"
                class="px-2 py-1.5 text-xl text-ink-secondary hover:text-ink leading-none"
                aria-label="Close"
                onClick={() => {
                  setAdding(false);
                  setNewTitle('');
                }}
              >
                ×
              </button>
            </div>
          </form>
        </Show>
      </div>
    </section>
  );
}
