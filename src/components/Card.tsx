import { Show } from "solid-js";
import { createDraggable, useDragDropContext } from "@thisbeyond/solid-dnd";
import Markdown from "./Markdown.tsx";
import type { KanbanCard, Priority, TreeSource } from "../types.ts";
import type { AgentRun } from "../db.ts";
import AgentBadge from "./AgentBadge.tsx";

declare module "solid-js" {
  namespace JSX {
    interface Directives {
      draggable: (el: HTMLElement, accessor: () => unknown) => void;
    }
  }
}

interface CardProps {
  card: KanbanCard;
  treeSources: () => TreeSource[];
  onOpenEdit: (card: KanbanCard) => void;
  onDelete: (id: string) => void;
  /** Open the singleton context menu at the given screen point. */
  onContextMenuOpen: (
    card: KanbanCard,
    point: { x: number; y: number },
  ) => void;
  /** Active or most recent agent run for this card, or null. */
  agentRun?: () => AgentRun | null;
  /** Called when the agent badge is clicked. */
  onAgentBadgeClick?: (cardId: string) => void;
}

const PRIORITY_STRIP: Record<Priority, string> = {
  low: "bg-p-low",
  medium: "bg-p-med",
  high: "bg-p-high",
  urgent: "bg-p-urgent",
};

export default function Card(props: CardProps) {
  const card = props.card;
  const draggable = createDraggable({ id: card.id, data: card });
  const [dndState] = useDragDropContext() as [
    { active: { draggable: string | null; droppable: string | null } },
  ];
  const isDragging = () => dndState.active.draggable === card.id;

  /** Right-click: forward to the Board-level singleton menu. */
  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    props.onContextMenuOpen(card, { x: e.clientX, y: e.clientY });
  }

  /** Keyboard alternative to right-click: Shift+F10 opens the menu at the
   * card's center (closest analog to a right-click on the element). */
  function onKeyDown(e: KeyboardEvent) {
    if (e.shiftKey && e.key === "F10") {
      e.preventDefault();
      const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
      props.onContextMenuOpen(card, {
        x: r.left + r.width / 2,
        y: r.top + r.height / 2,
      });
    }
  }

  return (
    <article
      use:draggable={draggable}
      data-testid={`card-${card.id}`}
      class="trello-card group relative bg-surface rounded-[var(--radius-card)] border border-card-border cursor-grab active:cursor-grabbing"
      classList={{ "is-dragging": isDragging() }}
      onContextMenu={onContextMenu}
      onKeyDown={onKeyDown}
      tabindex={0}
    >
      <div
        class={`priority-bar ${PRIORITY_STRIP[card.priority]}`}
        aria-hidden="true"
      />
      <div class="p-4">
        <p class="text-[0.95rem] font-semibold text-ink leading-snug">
          {card.title}
        </p>
        <Show when={card.description}>
          <Markdown
            content={card.description}
            class="md-preview text-[0.82rem] text-ink-secondary mt-1 line-clamp-2 leading-snug"
          />
        </Show>
        <Show when={card.treeSourceId}>
          <p
            class="text-[0.65rem] font-mono text-ink-muted mt-2 truncate"
            title={card.treeSourceId}
          >
            {props.treeSources().find((s) => s.id === card.treeSourceId)
              ?.label ?? card.treeSourceId}
          </p>
        </Show>
        <div class="flex items-center justify-between gap-2 mt-3">
          <div class="flex items-center gap-2 min-w-0 font-mono text-[0.65rem] text-ink-secondary">
            {card.source !== "local" && card.sourceRef && (
              <span class="metadata-chip">{card.sourceRef}</span>
            )}
            <Show when={props.agentRun?.()}>
              {(run) => (
                <AgentBadge
                  run={run()}
                  onClick={() => props.onAgentBadgeClick?.(card.id)}
                />
              )}
            </Show>
          </div>
          {card.source === "local" && (
            <div class="flex gap-1 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-opacity">
              <button
                type="button"
                class="text-[0.8rem] font-sans text-ink-secondary hover:text-ink px-1"
                onClick={() => props.onOpenEdit(card)}
              >
                Edit
              </button>
              <button
                type="button"
                class="text-[0.8rem] font-sans text-ink-secondary hover:text-p-urgent px-1"
                onClick={() => props.onDelete(card.id)}
              >
                Delete
              </button>
            </div>
          )}
          {card.source !== "local" && (
            <div class="flex gap-1 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-opacity">
              <button
                type="button"
                class="text-[0.8rem] font-sans text-ink-secondary hover:text-ink px-1"
                onClick={() => props.onOpenEdit(card)}
              >
                Edit
              </button>
            </div>
          )}
        </div>
      </div>
    </article>
  );
}
