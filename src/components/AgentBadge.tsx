import { Show } from 'solid-js';
import type { AgentRun } from '../db.ts';

/** Status → badge label + color classes. */
const STATUS_BADGE: Record<string, { label: string; class: string }> = {
  pending: { label: 'queued', class: 'bg-p-low/20 text-p-low border-p-low/40' },
  running: { label: 'running', class: 'bg-p-med/20 text-p-med border-p-med/40 animate-pulse' },
  completed: { label: 'done', class: 'bg-p-high/20 text-p-high border-p-high/40' },
  failed: { label: 'failed', class: 'bg-p-urgent/20 text-p-urgent border-p-urgent/40' },
  cancelled: { label: 'cancelled', class: 'bg-base/40 text-ink-secondary border-border-subtle' },
};

export interface AgentBadgeProps {
  /** Active or most recent run for this card, or null. */
  run: AgentRun | null;
  /** Click handler — opens the run panel. */
  onClick?: () => void;
}

/** Small status pill shown on cards that have (or had) an agent run.
 * Board polls `acp_list_active_runs` once and distributes state to badges
 * (decision 8/40). */
export default function AgentBadge(props: AgentBadgeProps) {
  return (
    <Show when={props.run}>
      {(run) => {
        const badge = STATUS_BADGE[run().status] ?? STATUS_BADGE.pending;
        return (
          <button
            type="button"
            class={`text-[10px] font-semibold uppercase tracking-wide rounded border px-1.5 py-0.5 transition-colors hover:brightness-110 ${badge.class}`}
            onClick={(e) => {
              e.stopPropagation();
              props.onClick?.();
            }}
            title={`Agent: ${run().agentName} — ${run().status}`}
          >
            {badge.label}
          </button>
        );
      }}
    </Show>
  );
}
