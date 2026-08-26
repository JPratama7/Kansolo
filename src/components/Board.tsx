import { For, Show, createSignal, onMount } from 'solid-js';
import { DragDropContext, DragDropSensors, DragOverlay, useDragDropContext } from '@thisbeyond/solid-dnd';
import type { ColumnId, KanbanCard, Priority, TreeSource } from '../types.ts';
import { COLUMNS } from '../columns.ts';
import {
  createLocalCard,
  deleteCard as deleteCardDb,
  listCards,
  listTreeSources,
  moveCard as moveCardDb,
  updateCard,
} from '../db.ts';
import Column from './Column.tsx';

let loadCards: (() => Promise<void>) | null = null;

/** Re-seed the board from the database. Used by App, e.g. after a sync. */
export function reload(): Promise<void> {
  return loadCards ? loadCards() : Promise.resolve();
}

interface DragEndEvent {
  draggable?: { data?: KanbanCard };
  droppable?: { data?: ColumnId } | null;
}

const PRIORITY_STRIP: Record<Priority, string> = {
  low: 'bg-p-low',
  medium: 'bg-p-med',
  high: 'bg-p-high',
  urgent: 'bg-p-urgent',
};

/** Portals the dragged card to document.body so it paints above all columns. */
function CardDragOverlay() {
  const [state] = useDragDropContext() as [
    { active: { draggable: string | null }; draggables: Record<string, { data?: KanbanCard }> },
  ];
  const activeCard = () => {
    const id = state.active.draggable;
    return id ? state.draggables[id]?.data : undefined;
  };
  return (
    <DragOverlay class="trello-card is-dragging bg-elevated rounded-[var(--radius-card)] border border-border-subtle shadow-2xl">
      <Show when={activeCard()}>
        {(card) => (
          <>
            <div class={`priority-strip ${PRIORITY_STRIP[card().priority]}`} aria-hidden="true" />
            <div class="px-3 py-2">
              <p class="text-sm text-ink leading-snug">{card().title}</p>
            </div>
          </>
        )}
      </Show>
    </DragOverlay>
  );
}

export default function Board() {
  const [cards, setCards] = createSignal<KanbanCard[]>([]);
  const [treeSources, setTreeSources] = createSignal<TreeSource[]>([]);

  loadCards = async () => {
    setCards(await listCards());
    const sources = await listTreeSources();
    setTreeSources(sources);
  };
  onMount(() => {
    void loadCards?.();
  });

  async function addCard(title: string, column: ColumnId) {
    const card = await createLocalCard(title, column); // db first
    setCards((prev) => [...prev, card]);
  }

  async function editCard(id: string, title: string, description: string, priority: Priority, treeSourceId: string) {
    await updateCard(id, { title, description, priority, treeSourceId });
    setCards((prev) =>
      prev.map((c) =>
        c.id === id
          ? { ...c, title, description, priority, treeSourceId: treeSourceId || undefined, updatedAt: new Date().toISOString() }
          : c,
      ),
    );
  }

  async function deleteCard(id: string) {
    await deleteCardDb(id); // db first
    setCards((prev) => prev.filter((c) => c.id !== id));
  }

  async function moveCardTo(id: string, column: ColumnId, position: number) {
    await moveCardDb(id, column, position); // db first
    setCards((prev) =>
      prev.map((c) =>
        c.id === id ? { ...c, column, position, updatedAt: new Date().toISOString() } : c,
      ),
    );
  }

  /** MVP: a dropped card lands at the end of the target column. */
  function endPosition(column: ColumnId): number {
    const inColumn = cards().filter((c) => c.column === column);
    return inColumn.length === 0 ? 1 : Math.max(...inColumn.map((c) => c.position)) + 1;
  }

  function handleDragEnd(event: DragEndEvent) {
    const card = event.draggable?.data;
    const column = event.droppable?.data;
    if (!card || !column || card.column === column) return;
    void moveCardTo(card.id, column, endPosition(column));
  }

  return (
    <DragDropContext onDragEnd={handleDragEnd}>
      <DragDropSensors>
        <main id="main-board" class="board-scroll flex-1 flex gap-3 p-3 overflow-y-auto bg-base">
          <For each={COLUMNS}>
            {(column) => (
              <Column
                column={column}
                cards={cards}
                treeSources={treeSources}
                onAdd={(title) => void addCard(title, column.id)}
                onEdit={editCard}
                onDelete={deleteCard}
                onMove={(id, col) => void moveCardTo(id, col, endPosition(col))}
              />
            )}
          </For>
        </main>
      </DragDropSensors>
      <CardDragOverlay />
    </DragDropContext>
  );
}
