import { Show } from "solid-js";
import type { AgentRun } from "../db.ts";

/** Status → agent label + color. */
const STATUS_BADGE: Record<string, { label: string; class: string }> = {
  pending: { label: "queued", class: "text-ink-muted" },
  running: { label: "running", class: "text-accent" },
  completed: { label: "done", class: "text-ink-secondary" },
  failed: { label: "failed", class: "text-ink-secondary" },
  cancelled: { label: "cancelled", class: "text-ink-muted" },
};

export interface AgentBadgeProps {
  /** Active or most recent run for this card, or null. */
  run: AgentRun | null;
  /** Click handler — opens the run panel. */
  onClick?: () => void;
}

/** Small status pill shown on cards that have (or had) an agent run. */
export default function AgentBadge(props: AgentBadgeProps) {
  return (
    <Show when={props.run}>
      {(run) => {
        const badge = STATUS_BADGE[run().status] ?? STATUS_BADGE.pending;
        return (
          <button
            type="button"
            class={`text-[0.65rem] font-mono font-semibold ${badge.class} hover:text-accent-hover transition-colors`}
            onClick={(e) => {
              e.stopPropagation();
              props.onClick?.();
            }}
            title={`Agent: ${run().agentName} — ${run().status}`}
          >
            Agent {badge.label}
          </button>
        );
      }}
    </Show>
  );
}
