import { createSignal, For, Show } from "solid-js";
import { createDroppable, useDragDropContext } from "@thisbeyond/solid-dnd";
import type { ColumnId, KanbanCard, TreeSource } from "../types.ts";
import { PRIORITIES } from "../types.ts";
import type { AgentRun } from "../db.ts";
import Card from "./Card.tsx";

declare module "solid-js" {
  namespace JSX {
    interface Directives {
      droppable: (el: HTMLElement, accessor: () => unknown) => void;
    }
  }
}

interface ColumnProps {
  column: { id: ColumnId; title: string };
  cards: () => KanbanCard[];
  /** True while this column's cards are being fetched — shows skeletons. */
  loading: () => boolean;
  treeSources: () => TreeSource[];
  onAdd: (title: string) => void;
  onOpenEdit: (card: KanbanCard) => void;
  onDelete: (id: string) => void;
  onContextMenuOpen: (
    card: KanbanCard,
    point: { x: number; y: number },
  ) => void;
  /** Active agent runs keyed by card_id. */
  activeRuns?: () => Record<string, AgentRun>;
  /** Called when an agent badge is clicked. */
  onAgentBadgeClick?: (cardId: string) => void;
}

const COL_RULE: Record<ColumnId, string> = {
  backlog: "border-col-backlog",
  ongoing: "border-col-ongoing",
  done: "border-col-done",
};

/** Skeleton count shown while a column loads. Enough to signal "cards are
 * coming" without filling the column with noise. */
const SKELETON_COUNT = 3;

export default function Column(props: ColumnProps) {
  const droppable = createDroppable({
    id: props.column.id,
    data: props.column.id,
  });
  const [dndState] = useDragDropContext() as [
    { active: { draggable: string | null; droppable: string | null } },
  ];
  const isOver = () => dndState.active.droppable === props.column.id;

  const columnCards = () =>
    props.cards().filter((c) => c.column === props.column.id);
  // Sort by priority (urgent → low), then by position within same priority.
  const sortedCards = () =>
    [...columnCards()].sort((a, b) => {
      const pa = PRIORITIES.indexOf(a.priority);
      const pb = PRIORITIES.indexOf(b.priority);
      if (pb !== pa) return pb - pa;
      return a.position - b.position;
    });

  const [adding, setAdding] = createSignal(false);
  const [newTitle, setNewTitle] = createSignal("");

  function submitAdd(e: Event) {
    e.preventDefault();
    const title = newTitle().trim();
    if (title) {
      props.onAdd(title);
      setNewTitle("");
    }
  }

  return (
    <section
      use:droppable={droppable}
      data-column-id={props.column.id}
      class="flex flex-col min-w-0 h-full"
      classList={{ "ring-2 ring-accent": isOver() }}
    >
      <header
        class={`flex items-baseline justify-between py-2 border-b ${
          COL_RULE[props.column.id]
        }`}
      >
        <h2 class="font-serif text-[1.4rem] text-ink">{props.column.title}</h2>
        <span class="text-[0.75rem] font-mono text-ink-muted tabular-nums">
          {props.loading() ? "…" : sortedCards().length}
        </span>
      </header>

      <div class="flex-1 flex flex-col gap-4 overflow-y-auto board-scroll min-h-0">
        <Show
          when={props.loading()}
          fallback={
            <Show
              when={sortedCards().length > 0}
              fallback={
                <p class="text-ink-muted text-xs py-3 text-center">
                  Drag cards here
                </p>
              }
            >
              <For each={sortedCards()}>
                {(card) => (
                  <Card
                    card={card}
                    treeSources={props.treeSources}
                    onOpenEdit={props.onOpenEdit}
                    onDelete={props.onDelete}
                    onContextMenuOpen={props.onContextMenuOpen}
                    agentRun={props.activeRuns
                      ? () => props.activeRuns!()[card.id] ?? null
                      : undefined}
                    onAgentBadgeClick={props.onAgentBadgeClick}
                  />
                )}
              </For>
            </Show>
          }
        >
          <For each={Array.from({ length: SKELETON_COUNT })}>
            {() => <CardSkeleton />}
          </For>
        </Show>
      </div>

      <div class="px-2 py-1.5">
        <Show
          when={adding()}
          fallback={
            <button
              type="button"
              class="add-link w-full text-left text-[0.8rem] rounded px-2 py-1.5 transition-colors"
              onClick={() => setAdding(true)}
            >
              + Add a card
            </button>
          }
        >
          <form onSubmit={submitAdd}>
            <label for={`add-card-${props.column.id}`} class="sr-only">
              Card title
            </label>
            <textarea
              id={`add-card-${props.column.id}`}
              name="card_title"
              autocomplete="off"
              class="w-full text-sm rounded-[var(--radius-card)] p-2 bg-surface text-ink placeholder:text-ink-muted resize-none outline-none focus:ring-2 focus:ring-accent border border-border-subtle"
              value={newTitle()}
              onInput={(e) => setNewTitle(e.currentTarget.value)}
              placeholder="Enter a title for this card…"
              rows={3}
              autofocus
            />
            <div class="flex items-center gap-2 mt-1.5">
              <button
                type="submit"
                class="px-3 py-2 text-[0.8rem] font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors"
              >
                Add card
              </button>
              <button
                type="button"
                class="px-2 py-1.5 text-xl text-ink-secondary hover:text-ink leading-none"
                aria-label="Close"
                onClick={() => {
                  setAdding(false);
                  setNewTitle("");
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

/** Placeholder card shown while a column's cards load. Mirrors the real
 * card silhouette (priority strip + title + description + footer) so the
 * layout doesn't shift when real cards arrive. The shimmer is a single
 * quiet ambient cue; reduced-motion users see a static block. */
function CardSkeleton() {
  return (
    <div
      class="card-skeleton relative flex rounded-[var(--radius-card)] border border-card-border"
      aria-hidden="true"
    >
      <div class="w-1 shrink-0 self-stretch bg-elevated rounded-l-[var(--radius-card)]" />
      <div class="flex-1 p-4">
        <div class="card-skeleton-line" style={{ width: "85%" }} />
        <div
          class="card-skeleton-line card-skeleton-line--sm mt-1.5"
          style={{ width: "60%" }}
        />
        <div class="flex items-center gap-1.5 mt-3">
          <div
            class="card-skeleton-line card-skeleton-line--xs"
            style={{ width: "3rem" }}
          />
          <div
            class="card-skeleton-line card-skeleton-line--xs"
            style={{ width: "4.5rem" }}
          />
        </div>
      </div>
    </div>
  );
}
