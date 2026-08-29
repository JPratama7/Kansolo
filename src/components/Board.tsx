import { For, Show, createEffect, createMemo, createSignal, onMount } from 'solid-js';
import { DragDropContext, DragDropSensors, DragOverlay, useDragDropContext } from '@thisbeyond/solid-dnd';
import { micromark } from 'micromark';
import { Menu } from '@ark-ui/solid/menu';
import { useMenu } from '@ark-ui/solid/menu';
import { invoke } from '@tauri-apps/api/core';
import { toaster } from './ui/toaster.ts';
import type { ColumnId, KanbanCard, Priority, TreeSource } from '../types.ts';
import { COLUMNS } from '../columns.ts';
import {
  createLocalCard,
  deleteCard as deleteCardDb,
  listCardsByColumn,
  listTreeSources,
  moveCard as moveCardDb,
  updateCard,
} from '../db.ts';
import Column from './Column.tsx';
import EditModal, { type EditModalResult } from './EditModal.tsx';

let reloadBoard: (() => void) | null = null;

/** Re-seed the board from the database. Used by App, e.g. after a sync.
 * Triggers each column to re-fetch with a visible loading state. */
export function reload(): Promise<void> {
  reloadBoard?.();
  return Promise.resolve();
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
function CardDragOverlay(props: { treeSources: () => TreeSource[] }) {
  const [state] = useDragDropContext() as [
    { active: { draggable: string | null }; draggables: Record<string, { data?: KanbanCard }> },
  ];
  const activeCard = () => {
    const id = state.active.draggable;
    return id ? state.draggables[id]?.data : undefined;
  };
  return (
    <DragOverlay class="trello-card is-drag-overlay bg-elevated rounded-[var(--radius-card)] border border-border-subtle shadow-2xl">
      <Show when={activeCard()}>
        {(card) => (
          <>
            <div class={`priority-strip ${PRIORITY_STRIP[card().priority]}`} aria-hidden="true" />
            <div class="px-3 py-2">
              <p class="text-sm text-ink leading-snug">{card().title}</p>
              <Show when={card().description}>
                <div class="md-preview text-xs text-ink-secondary mt-1 line-clamp-2" innerHTML={micromark(card().description)} />
              </Show>
              <Show when={card().treeSourceId}>
                <p class="text-[10px] font-mono text-ink-muted mt-1 truncate" title={card().treeSourceId}>
                  {props.treeSources().find((s) => s.id === card().treeSourceId)?.label ?? card().treeSourceId}
                </p>
              </Show>
              <div class="flex items-center justify-between gap-2 mt-2">
                <div class="flex items-center gap-1.5 min-w-0">
                  <span class="text-[10px] font-semibold uppercase tracking-wide text-ink-secondary">
                    {card().priority}
                  </span>
                  {card().source !== 'local' && card().sourceRef && (
                    <span class="text-[10px] font-mono text-ink-secondary bg-base/60 rounded px-1 py-0.5 truncate">
                      {card().sourceRef}
                    </span>
                  )}
                </div>
              </div>
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
  // Per-column loading state. True while a column's first fetch (or a sync
  // reload) is in flight — drives skeleton placeholders in Column.
  const [columnLoading, setColumnLoading] = createSignal<Record<ColumnId, boolean>>({
    backlog: true,
    ongoing: true,
    done: true,
  });
  // Singleton EditModal state: the card currently being edited, or null when
  // the modal is closed. Lifted out of Card so only one Dialog.Root exists.
  const [currentlyEditingCard, setCurrentlyEditingCard] = createSignal<KanbanCard | null>(null);
  // Dirty state lifted from EditModal so Board can guard card-switching.
  const [isEditingDirty, setIsEditingDirty] = createSignal(false);
  // Pending card switch: the card the user wants to edit, awaiting discard
  // confirmation for the current dirty card.
  const [pendingSwitchCard, setPendingSwitchCard] = createSignal<KanbanCard | null>(null);
  const [pendingSwitchToastId, setPendingSwitchToastId] = createSignal<string | null>(null);
  // Singleton context menu state: the card whose menu is open + the screen
  // point to anchor the menu at. Lifted out of Card so only one Menu.Root
  // machine exists (decision 12).
  const [currentlyMenuingCard, setCurrentlyMenuingCard] = createSignal<KanbanCard | null>(null);
  const [menuAnchorPoint, setMenuAnchorPoint] = createSignal<{ x: number; y: number }>({ x: 0, y: 0 });

  // Ark UI Menu machine. Controlled `open` so we can open programmatically
  // from Card's onContextMenu / Shift+F10 without a Menu.ContextTrigger
  // (decision 4: no asChild, no extra DOM node conflicting with use:draggable).
  const menuOpen = createMemo(() => currentlyMenuingCard() !== null);
  const menu = useMenu({
    onOpenChange: (details) => {
      if (!details.open) setCurrentlyMenuingCard(null);
    },
    positioning: { placement: 'bottom-start', gutter: 0 },
  });
  createEffect(() => {
    menu.api().setOpen(menuOpen());
  });

  /** Open the singleton menu for `card` anchored at `point`. Called from
   * Card's onContextMenu (right-click) and Shift+F10 handler. */
  function openCardMenu(card: KanbanCard, point: { x: number; y: number }) {
    setMenuAnchorPoint(point);
    setCurrentlyMenuingCard(card);
  }

  /** Guarded card-switch: if the current EditModal is dirty, show a
   * confirmation toast before switching. If clean (or same card), switch
   * immediately. Called from Card's onEdit and Menu's "Edit" item. */
  function requestEditCard(card: KanbanCard) {
    // Same card or no card open → switch directly.
    const current = currentlyEditingCard();
    if (!current || current.id === card.id || !isEditingDirty()) {
      setCurrentlyEditingCard(card);
      return;
    }
    // Dedup: dismiss any existing switch-confirmation toast first.
    const existingId = pendingSwitchToastId();
    if (existingId !== null) toaster.dismiss(existingId);
    setPendingSwitchCard(card);
    const id = toaster.create({
      title: 'Discard unsaved changes?',
      type: 'warning',
      duration: Infinity,
      action: {
        label: 'Discard',
        onClick: () => {
          toaster.dismiss(id);
          setPendingSwitchToastId(null);
          // Switch to the pending card. Guard: if it was cleared (e.g.
          // card deleted while toast shown), do nothing.
          const pending = pendingSwitchCard();
          if (pending) setCurrentlyEditingCard(pending);
          setPendingSwitchCard(null);
        },
      },
    });
    setPendingSwitchToastId(id);
  }

  // When the anchor point or target card changes while the menu is open,
  // reposition the menu at the new point. This handles the second
  // right-click case: the machine stays open (controlled), and we move it.
  createEffect(() => {
    const card = currentlyMenuingCard();
    if (!card) return;
    const point = menuAnchorPoint();
    // Reposition uses anchorPoint from the event point; setting it via
    // reposition() with getAnchorRect keeps the menu pinned to the cursor.
    menu.api().reposition({ getAnchorRect: () => ({ width: 0, height: 0, ...point }) });
  });

  /** Fetch one column's cards from the database and splice them into the
   * central cards signal. When `withSkeletons` is true, sets the column's
   * loading flag so Column shows placeholders during the fetch. Used for
   * initial load, sync reloads (skeletons), and error reverts (no skeletons). */
  async function fetchColumn(column: ColumnId, withSkeletons: boolean) {
    if (withSkeletons) setColumnLoading((prev) => ({ ...prev, [column]: true }));
    try {
      const fresh = await listCardsByColumn(column);
      setCards((prev) => [...prev.filter((c) => c.column !== column), ...fresh]);
    } finally {
      setColumnLoading((prev) => ({ ...prev, [column]: false }));
    }
  }

  // Sync reload: re-fetch every column with skeletons visible.
  reloadBoard = () => {
    for (const col of COLUMNS) void fetchColumn(col.id, true);
  };
  onMount(() => {
    void listTreeSources().then(setTreeSources);
    for (const col of COLUMNS) void fetchColumn(col.id, true);
  });

  async function addCard(title: string, column: ColumnId) {
    // Backend returns the real card (UUID, position, timestamps) — append it
    // directly. No optimistic guess needed; the call is fast and gives us truth.
    const card = await createLocalCard(title, column);
    setCards((prev) => [...prev, card]);
  }

  async function editCard(id: string, title: string, description: string, priority: Priority, treeSourceId: string) {
    // Optimistic: update the signal immediately so the card reflects the edit
    // without waiting on the backend round-trip.
    setCards((prev) =>
      prev.map((c) =>
        c.id === id
          ? { ...c, title, description, priority, treeSourceId: treeSourceId || undefined, updatedAt: new Date().toISOString() }
          : c,
      ),
    );
    try {
      await updateCard(id, { title, description, priority, treeSourceId });
    } catch (e) {
      // Revert: re-fetch the card's column to restore true state.
      const card = cards().find((c) => c.id === id);
      if (card) void fetchColumn(card.column, false);
      toaster.error({ title: 'Edit failed', description: e instanceof Error ? e.message : String(e) });
    }
  }

  function handleEditSave(result: EditModalResult) {
    const card = currentlyEditingCard();
    if (card) void editCard(card.id, result.title, result.description, result.priority, result.treeSourceId);
    setCurrentlyEditingCard(null);
  }

  async function deleteCard(id: string) {
    const card = cards().find((c) => c.id === id);
    // Optimistic: remove immediately.
    setCards((prev) => prev.filter((c) => c.id !== id));
    try {
      await deleteCardDb(id);
    } catch (e) {
      // Revert: re-fetch the card's column to restore it.
      if (card) void fetchColumn(card.column, false);
      toaster.error({ title: 'Delete failed', description: e instanceof Error ? e.message : String(e) });
    }
  }

  async function moveCardTo(id: string, column: ColumnId) {
    const card = cards().find((c) => c.id === id);
    if (!card || card.column === column) return;
    const oldColumn = card.column;
    // Optimistic: flip the card to the target column immediately so the drag
    // feels instant. Position will be corrected by the backend (appends to end).
    setCards((prev) =>
      prev.map((c) => (c.id === id ? { ...c, column, updatedAt: new Date().toISOString() } : c)),
    );
    try {
      await moveCardDb(id, column);
    } catch (e) {
      // Revert: re-fetch both affected columns.
      void fetchColumn(oldColumn, false);
      void fetchColumn(column, false);
      toaster.error({ title: 'Move failed', description: e instanceof Error ? e.message : String(e) });
    }
  }

  function handleDragEnd(event: DragEndEvent) {
    const card = event.draggable?.data;
    const column = event.droppable?.data;
    if (!card || !column || card.column === column) return;
    void moveCardTo(card.id, column);
  }

  async function openInEditor() {
    const card = currentlyMenuingCard();
    if (!card) return;
    const id = card.treeSourceId;
    if (!id) return;
    const src = treeSources().find((s) => s.id === id);
    if (!src) return;
    try {
      await invoke('open_in_editor', { path: src.path, command: src.editorCommand });
    } catch (e) {
      console.error('open_in_editor failed:', e);
    }
  }

  function moveMenuCardTo(column: ColumnId) {
    const card = currentlyMenuingCard();
    if (!card || card.column === column) return;
    void moveCardTo(card.id, column);
    setCurrentlyMenuingCard(null);
  }

  function editFromMenu() {
    const card = currentlyMenuingCard();
    if (!card) return;
    requestEditCard(card);
    setCurrentlyMenuingCard(null);
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
                loading={() => columnLoading()[column.id]}
                treeSources={treeSources}
                onAdd={(title) => void addCard(title, column.id)}
                onOpenEdit={requestEditCard}
                onDelete={deleteCard}
                onContextMenuOpen={openCardMenu}
              />
            )}
          </For>
        </main>
      </DragDropSensors>
      <CardDragOverlay treeSources={treeSources} />
      <EditModal
        card={currentlyEditingCard()}
        treeSources={treeSources}
        open={currentlyEditingCard() !== null}
        onOpenChange={(open) => { if (!open) setCurrentlyEditingCard(null); }}
        onSave={handleEditSave}
        onDirtyChange={setIsEditingDirty}
      />
      <Menu.RootProvider value={menu}>
        <Menu.Positioner>
          <Menu.Content data-testid="card-context-menu" class="min-w-[10rem] bg-surface border border-border-subtle rounded-[var(--radius-card)] shadow-2xl py-1">
            <Menu.Item value="edit" onSelect={editFromMenu} data-testid="menu-item-edit" class="w-full text-left text-sm text-ink px-3 py-1.5 hover:bg-elevated transition-colors cursor-pointer">
              Edit
            </Menu.Item>
            <Show when={currentlyMenuingCard()?.treeSourceId}>
              <Menu.Item value="editor" onSelect={openInEditor} data-testid="menu-item-editor" class="w-full text-left text-sm text-ink px-3 py-1.5 hover:bg-elevated transition-colors cursor-pointer">
                Open in editor
              </Menu.Item>
            </Show>
            <Menu.Separator class="my-1 border-t border-border-subtle" />
            <Menu.ItemGroup>
              <For each={COLUMNS}>
                {(col) => (
                  <Menu.Item
                    value={`move-${col.id}`}
                    data-testid={`menu-item-move-${col.id}`}
                    disabled={col.id === currentlyMenuingCard()?.column}
                    onSelect={() => moveMenuCardTo(col.id)}
                    class="w-full text-left text-sm text-ink px-3 py-1.5 hover:bg-elevated transition-colors cursor-pointer data-[disabled]:opacity-40"
                  >
                    Move to {col.title}
                  </Menu.Item>
                )}
              </For>
            </Menu.ItemGroup>
          </Menu.Content>
        </Menu.Positioner>
      </Menu.RootProvider>
    </DragDropContext>
  );
}
