import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from 'solid-js';
import { Portal } from 'solid-js/web';
import { DragDropContext, DragDropSensors, DragOverlay, useDragDropContext } from '@thisbeyond/solid-dnd';
import { micromark } from 'micromark';
import { Menu, useMenu } from '@ark-ui/solid/menu';
import { invoke } from '@tauri-apps/api/core';
import { toaster } from './ui/toaster.ts';
import type { ColumnId, KanbanCard, Priority, TreeSource } from '../types.ts';
import type { AgentRun, Agent, SkillManifest } from '../db.ts';
import { COLUMNS } from '../columns.ts';
import {
  createLocalCard,
  deleteCard as deleteCardDb,
  listCardsByColumn,
  listTreeSources,
  moveCard as moveCardDb,
  updateCard,
  acpErrorMessage,
  acpListActiveRuns,
  acpListAgents,
  acpListSkills,
  acpCreateRun,
  acpLatestRunForCard,
} from '../db.ts';
import Column from './Column.tsx';
import EditModal, { type EditModalResult } from './EditModal.tsx';
import AgentRunPanel from './AgentRunPanel.tsx';
import SkillPicker from './SkillPicker.tsx';

let reloadBoard: (() => Promise<void>) | null = null;

/** Re-seed the board from the database. Used by App, e.g. after a sync.
 * Triggers each column to re-fetch with a visible loading state, and
 * resolves only once every column fetch has settled. */
export function reload(): Promise<void> {
  return reloadBoard?.() ?? Promise.resolve();
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
  // Active agent runs keyed by card_id. Board polls acp_list_active_runs
  // every 2s while any run is active, then distributes to Card → AgentBadge.
  const [activeRuns, setActiveRuns] = createSignal<Record<string, AgentRun>>({});
  // AgentRunPanel (singleton): the run being inspected, or null when closed.
  const [panelRun, setPanelRun] = createSignal<AgentRun | null>(null);
  const [panelOpen, setPanelOpen] = createSignal(false);
  // Start-agent dialog state: target card + loaded agents/skills.
  const [startDialogOpen, setStartDialogOpen] = createSignal(false);
  const [startDialogCard, setStartDialogCard] = createSignal<KanbanCard | null>(null);
  const [startDialogAgents, setStartDialogAgents] = createSignal<Agent[]>([]);
  const [startDialogSkills, setStartDialogSkills] = createSignal<SkillManifest[]>([]);
  const [startAgentName, setStartAgentName] = createSignal('');
  const [startSkills, setStartSkills] = createSignal<string[]>([]);
  const [startBusy, setStartBusy] = createSignal(false);

  /** Poll active runs while any exists, then refresh badges + panel. */
  async function pollActiveRuns() {
    try {
      const runs = await acpListActiveRuns();
      const map: Record<string, AgentRun> = {};
      for (const r of runs) map[r.cardId] = r;
      setActiveRuns(map);
      // If the panel is open and the polled run is still active, refresh
      // panelRun so status/terminal fields stay live without a separate fetch.
      if (panelOpen()) {
        const pr = panelRun();
        if (pr) {
          const updated = map[pr.cardId];
          if (updated && updated.id === pr.id) setPanelRun(updated);
        }
      }
    } catch (e) {
      // Polling is best-effort; don't spam toasts on transient failures.
      console.error('acp_list_active_runs failed:', acpErrorMessage(e));
    }
  }

  createEffect(() => {
    const hasActive = Object.values(activeRuns()).some(
      (r) => r.status === 'pending' || r.status === 'running',
    );
    if (!hasActive) return;
    const id = setInterval(() => void pollActiveRuns(), 2000);
    onCleanup(() => clearInterval(id));
  });

  /** Open the run panel for a card's run. Falls back to the latest run
   * (any status) when no active run exists, so completed/failed runs can
   * still be inspected instead of disappearing from the badge. */
  async function openRunPanel(cardId: string) {
    const active = activeRuns()[cardId];
    if (active) {
      setPanelRun(active);
      setPanelOpen(true);
      return;
    }
    try {
      const latest = await acpLatestRunForCard(cardId);
      if (latest) {
        setPanelRun(latest);
        setPanelOpen(true);
      }
    } catch (e) {
      console.error('acp_latest_run_for_card failed:', acpErrorMessage(e));
    }
  }

  /** Right-click → "Start agent…": open the start dialog for a local card. */
  async function startAgentRun(card: KanbanCard) {
    setStartDialogCard(card);
    setStartDialogOpen(true);
    setStartBusy(false);
    try {
      const [agents, skills] = await Promise.all([acpListAgents(), acpListSkills()]);
      setStartDialogAgents(agents);
      setStartDialogSkills(skills);
      const first = agents.find((a) => a.enabled);
      setStartAgentName(first?.name ?? '');
      setStartSkills(first?.skills ?? []);
      if (!first) {
        toaster.warning({
          title: 'No agents registered',
          description: 'Add an agent in Settings → Agents',
        });
      }
    } catch (e) {
      toaster.error({ title: 'Could not load agents', description: acpErrorMessage(e) });
      setStartDialogOpen(false);
    }
  }

  /** Confirm the start dialog: create the run, then open its panel. */
  async function confirmStartAgentRun() {
    const card = startDialogCard();
    const agent = startDialogAgents().find((a) => a.name === startAgentName());
    if (!card || !agent) return;
    setStartBusy(true);
    try {
      const run = await acpCreateRun(card.id, agent.name, startSkills());
      setStartDialogOpen(false);
      await pollActiveRuns();
      setPanelRun(run);
      setPanelOpen(true);
    } catch (e) {
      toaster.error({ title: 'Could not start agent', description: acpErrorMessage(e) });
    } finally {
      setStartBusy(false);
    }
  }

  // Ark UI Menu machine. Controlled `open` so we can open programmatically
  // from Card's onContextMenu / Shift+F10 without a Menu.ContextTrigger
  // (decision 4: no asChild, no extra DOM node conflicting with use:draggable).
  // `anchorPoint` is bound reactively so the machine auto-repositions when the
  // cursor point changes (second right-click, Shift+F10 at card center). This
  // replaces a manual `reposition()` effect that raced the machine's own
  // CONTROLLED.OPEN reposition (which used the unset anchorPoint context and
  // fell back to the default position).
  const menuOpen = createMemo(() => currentlyMenuingCard() !== null);
  const menu = useMenu({
    onOpenChange: (details) => {
      if (!details.open) {
        // Restore focus to the card that owned the menu. The menu is
        // controlled + has no MenuTrigger, so Ark UI can't restore focus
        // automatically — without this, Escape sends focus to <body>.
        // Item-select handlers clear `currentlyMenuingCard` before this
        // callback runs, so we only refocus on dismiss (Escape / outside
        // click), not after selecting an item (where focus moves to the
        // edit modal / etc).
        const card = currentlyMenuingCard();
        setCurrentlyMenuingCard(null);
        if (card) {
          queueMicrotask(() => {
            document
              .querySelector<HTMLElement>(`[data-testid="card-${card.id}"]`)
              ?.focus();
          });
        }
      }
    },
    onPointerDownOutside: (e) => {
      // When the user right-clicks on a card while the menu is open,
      // don't close the menu — the card's onContextMenu handler will
      // update the anchor point and we reposition via the effect below.
      if (e.detail.contextmenu) {
        e.preventDefault();
      }
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
    // Reposition the menu at the new anchor point. On the first open, the
    // machine's CONTROLLED.OPEN transition handles positioning. On a
    // subsequent open (second right-click while menu is already open),
    // the machine stays in "open" state and doesn't reposition — so we
    // explicitly call reposition. Use setTimeout(0) to defer past the
    // machine's own open-transition reposition (which would overwrite
    // our position) and past any pending CLOSE microtasks from
    // pointerdown-outside.
    setTimeout(() => {
      menu.api().reposition({ getAnchorRect: () => ({ width: 0, height: 0, ...point }) });
    }, 0);
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

  // Repositioning on second right-click is handled in `openCardMenu`
  // above (setTimeout(0) reposition call). No separate effect needed.

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

  // Sync reload: re-fetch every column with skeletons visible. Awaits all
  // three fetches so callers (App.finishSync) know when the board is stable.
  reloadBoard = async () => {
    await Promise.all(COLUMNS.map((col) => fetchColumn(col.id, true)));
  };
  onMount(() => {
    void listTreeSources().then(setTreeSources).catch((e) =>
      toaster.error({ title: 'Could not load tree sources', description: acpErrorMessage(e) }),
    );
    for (const col of COLUMNS) {
      void fetchColumn(col.id, true).catch((e) =>
        toaster.error({ title: `Could not load column ${col.id}`, description: acpErrorMessage(e) }),
      );
    }
    void pollActiveRuns();
  });

  async function addCard(title: string, column: ColumnId) {
    // Backend returns the real card (UUID, position, timestamps) — append it
    // directly. No optimistic guess needed; the call is fast and gives us truth.
    try {
      const card = await createLocalCard(title, column);
      setCards((prev) => [...prev, card]);
    } catch (e) {
      toaster.error({ title: 'Add card failed', description: acpErrorMessage(e) });
    }
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
      if (card) void fetchColumn(card.column, false).catch((err) =>
        toaster.error({ title: 'Revert failed', description: acpErrorMessage(err) }),
      );
      toaster.error({ title: 'Edit failed', description: acpErrorMessage(e) });
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
      if (card) void fetchColumn(card.column, false).catch((err) =>
        toaster.error({ title: 'Revert failed', description: acpErrorMessage(err) }),
      );
      toaster.error({ title: 'Delete failed', description: acpErrorMessage(e) });
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
      void fetchColumn(oldColumn, false).catch((err) =>
        toaster.error({ title: 'Revert failed', description: acpErrorMessage(err) }),
      );
      void fetchColumn(column, false).catch((err) =>
        toaster.error({ title: 'Revert failed', description: acpErrorMessage(err) }),
      );
      toaster.error({ title: 'Move failed', description: acpErrorMessage(e) });
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
      toaster.error({ title: 'Open in editor failed', description: acpErrorMessage(e) });
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
                activeRuns={activeRuns}
                onAgentBadgeClick={(cardId) => void openRunPanel(cardId)}
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
            <Show when={currentlyMenuingCard()?.source === 'local'}>
              <Menu.Item
                value="agent"
                data-testid="menu-item-agent"
                onSelect={() => {
                  const card = currentlyMenuingCard();
                  if (card) void startAgentRun(card);
                  setCurrentlyMenuingCard(null);
                }}
                class="w-full text-left text-sm text-ink px-3 py-1.5 hover:bg-elevated transition-colors cursor-pointer"
              >
                Start agent…
              </Menu.Item>
            </Show>
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
      <AgentRunPanel
        open={panelOpen()}
        onOpenChange={setPanelOpen}
        run={panelRun()}
      />
      <Show when={startDialogOpen()}>
        <Portal>
          <div class="fixed inset-0 z-50 bg-black/50" />
          <div class="fixed inset-0 z-50 flex items-center justify-center px-4">
            <div class="relative w-full max-w-md bg-surface rounded-[var(--radius-card)] border border-border-subtle shadow-2xl p-5 flex flex-col gap-4">
              <div class="flex items-center justify-between">
                <h2 class="text-base font-bold text-ink">
                  Start Agent — {startDialogCard()?.title}
                </h2>
                <button
                  type="button"
                  class="text-xl text-ink-secondary hover:text-ink leading-none px-1"
                  aria-label="Close"
                  onClick={() => setStartDialogOpen(false)}
                >
                  ×
                </button>
              </div>
              <label class="block text-xs font-semibold text-ink-secondary" for="start-agent-picker">
                Agent
              </label>
              <select
                id="start-agent-picker"
                class="w-full text-sm rounded px-2 py-1.5 bg-base text-ink border border-border-subtle outline-none focus:border-accent"
                value={startAgentName()}
                onChange={(e) => {
                  const name = e.currentTarget.value;
                  setStartAgentName(name);
                  const agent = startDialogAgents().find((a) => a.name === name);
                  setStartSkills(agent?.skills ?? []);
                }}
              >
                <For each={startDialogAgents()}>
                  {(agent) => (
                    <option value={agent.name} disabled={!agent.enabled}>
                      {agent.name}{agent.enabled ? '' : ' (disabled)'}
                    </option>
                  )}
                </For>
              </select>
              <Show when={startDialogSkills().length > 0 && startAgentName()}>
                <SkillPicker
                  available={startDialogSkills()}
                  agentSkills={
                    startDialogAgents().find((a) => a.name === startAgentName())?.skills ?? []
                  }
                  selected={startSkills()}
                  onChange={setStartSkills}
                />
              </Show>
              <div class="flex gap-2 justify-end">
                <button
                  type="button"
                  class="px-3 py-1.5 text-sm font-medium rounded text-ink-secondary hover:bg-elevated transition-colors"
                  onClick={() => setStartDialogOpen(false)}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  class="px-3 py-1.5 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors disabled:opacity-50"
                  disabled={!startAgentName() || startBusy()}
                  onClick={() => void confirmStartAgentRun()}
                >
                  {startBusy() ? 'Starting…' : 'Run agent'}
                </button>
              </div>
            </div>
          </div>
        </Portal>
      </Show>
    </DragDropContext>
  );
}
