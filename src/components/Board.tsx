import { For, createSignal, onMount } from 'solid-js';
import { DragDropContext, DragDropSensors } from '@thisbeyond/solid-dnd';
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

export default function Board() {
  const [cards, setCards] = createSignal<KanbanCard[]>([]);
  const [treeSources, setTreeSources] = createSignal<TreeSource[]>([]);
  const [sourceLabels, setSourceLabels] = createSignal<Record<string, string>>({});
  const [sourceEditors, setSourceEditors] = createSignal<Record<string, string | undefined>>({});

  loadCards = async () => {
    setCards(await listCards());
    const sources = await listTreeSources();
    setTreeSources(sources);
    setSourceLabels(Object.fromEntries(sources.map((s) => [s.path, s.label])));
    setSourceEditors(Object.fromEntries(sources.map((s) => [s.path, s.editorCommand])));
  };
  onMount(() => {
    void loadCards?.();
  });

  async function addCard(title: string, column: ColumnId) {
    const card = await createLocalCard(title, column); // db first
    setCards((prev) => [...prev, card]);
  }

  async function editCard(id: string, title: string, description: string, priority: Priority, sourcePath: string) {
    await updateCard(id, { title, description, priority, sourcePath }); // db first
    setCards((prev) =>
      prev.map((c) =>
        c.id === id
          ? { ...c, title, description, priority, sourcePath: sourcePath || undefined, updatedAt: new Date().toISOString() }
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
        <main class="board-scroll flex-1 flex gap-3 p-3 overflow-y-auto bg-base">
          <For each={COLUMNS}>
            {(column) => (
              <Column
                column={column}
                cards={cards}
                treeSources={treeSources}
                sourceLabels={sourceLabels}
                sourceEditors={sourceEditors}
                onAdd={(title) => void addCard(title, column.id)}
                onEdit={editCard}
                onDelete={deleteCard}
              />
            )}
          </For>
        </main>
      </DragDropSensors>
    </DragDropContext>
  );
}
