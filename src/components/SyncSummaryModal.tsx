import { For, Show, createMemo, createSignal, onCleanup, onMount } from 'solid-js';

export interface SyncSummaryEntry {
  label: string;
  sourceType: string;
  count: number;
}

interface SyncSummaryModalProps {
  entries: SyncSummaryEntry[];
  /** ISO timestamp of the sync run; shown as a receipt-style timestamp. */
  syncedAt?: string;
  onClose: () => void;
}

/** Per-source-type segment color. Falls back to a neutral for unknown types. */
const SOURCE_COLORS: Record<string, string> = {
  jira: 'var(--color-accent)',
  github: 'var(--color-col-ongoing)',
  gitlab: 'var(--color-col-done)',
};
const DEFAULT_SOURCE_COLOR = 'var(--color-elevated)';

function sourceColor(sourceType: string): string {
  return SOURCE_COLORS[sourceType] ?? DEFAULT_SOURCE_COLOR;
}

/** Animate an integer from 0 to target over `duration` ms, ease-out.
 *  Respects prefers-reduced-motion (jumps to target immediately). */
function useCountUp(target: number, duration = 500) {
  const [current, setCurrent] = createSignal(0);
  onMount(() => {
    if (target <= 0) return;
    const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    if (reduce) { setCurrent(target); return; }
    const start = performance.now();
    let raf = 0;
    const tick = (now: number) => {
      const t = Math.min(1, (now - start) / duration);
      // ease-out cubic
      const eased = 1 - Math.pow(1 - t, 3);
      setCurrent(Math.round(target * eased));
      if (t < 1) raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    onCleanup(() => cancelAnimationFrame(raf));
  });
  return current;
}

export default function SyncSummaryModal(props: SyncSummaryModalProps) {
  const total = createMemo(() => props.entries.reduce((sum, e) => sum + e.count, 0));
  const displayedTotal = useCountUp(total());

  // Mount flag flips true next frame so the proportion bar transitions from 0.
  const [mounted, setMounted] = createSignal(false);
  onMount(() => {
    const id = requestAnimationFrame(() => setMounted(true));
    onCleanup(() => cancelAnimationFrame(id));
  });

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      // Bail if another handler already consumed this Escape (e.g. an open
      // menu inside the modal) so we don't double-close.
      if (e.defaultPrevented) return;
      if (e.key === 'Escape') props.onClose();
    };
    window.addEventListener('keydown', onKey);
    onCleanup(() => window.removeEventListener('keydown', onKey));
  });

  const timestamp = createMemo(() => {
    const v = props.syncedAt;
    if (!v) return '';
    const d = new Date(v);
    return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  });

  const hasImports = () => total() > 0;

  return (
    <div
      class="sync-backdrop fixed inset-0 z-50 flex items-start justify-center pt-20 px-4 bg-black/60 backdrop-blur-[2px]"
      role="dialog"
      aria-modal="true"
      aria-label="Sync summary"
      data-testid="sync-summary-modal"
      onClick={props.onClose}
    >
      <section
        class="sync-panel w-full max-w-md max-h-[85vh] overflow-y-auto board-scroll bg-surface rounded-[var(--radius-card)] border border-border-subtle shadow-2xl"
        aria-label="Sync summary"
        onClick={(e) => e.stopPropagation()}
      >
        <div class="flex items-center justify-between px-5 pt-4 pb-2">
          <span class="text-[11px] font-semibold uppercase tracking-[0.18em] text-ink-secondary">
            Sync complete
          </span>
          <Show when={timestamp()}>
            <span class="text-[11px] font-mono text-ink-muted tabular-nums">{timestamp()}</span>
          </Show>
        </div>

        <div class="px-5 pb-5 flex flex-col gap-4">
          <div class="flex items-baseline gap-2">
            <span
              class="text-6xl font-extrabold tabular-nums leading-none"
              classList={{ 'text-accent': hasImports(), 'text-ink-muted': !hasImports() }}
            >
              {displayedTotal()}
            </span>
            <span class="text-sm font-medium text-ink-secondary">
              ticket{total() === 1 ? '' : 's'} imported
            </span>
          </div>

          <Show when={hasImports()}>
            <div class="sync-bar" classList={{ 'is-mounted': mounted() }}>
              <For each={props.entries}>
                {(e) => (
                  <div
                    class="sync-bar-seg"
                    style={{
                      '--seg-width': `${(e.count / total()) * 100}%`,
                      'background': sourceColor(e.sourceType),
                    }}
                  />
                )}
              </For>
            </div>
          </Show>

          <Show
            when={props.entries.length > 0}
            fallback={<p class="text-sm text-ink-secondary">No enabled sources.</p>}
          >
            <ul class="flex flex-col gap-1.5">
              <For each={props.entries}>
                {(e, i) => (
                  <li
                    class="sync-row flex items-center gap-2.5 text-sm rounded-md px-2.5 py-2 bg-elevated/40 border border-border-subtle"
                    style={{ 'animation-delay': `${120 + i() * 40}ms` }}
                  >
                    <span
                      class="w-2 h-2 rounded-full shrink-0"
                      style={{ 'background': sourceColor(e.sourceType) }}
                      aria-hidden="true"
                    />
                    <span class="min-w-0 truncate font-semibold text-ink">{e.label}</span>
                    <span class="metadata-chip shrink-0">{e.sourceType}</span>
                    <span class="ml-auto font-mono tabular-nums text-ink-secondary shrink-0">
                      {e.count}
                    </span>
                  </li>
                )}
              </For>
            </ul>
          </Show>

          <Show when={!hasImports()}>
            <div class="rounded-md border border-border-subtle bg-elevated/30 px-3 py-2.5">
              <p class="text-sm text-ink-secondary leading-snug">
                No tickets matched this run.
              </p>
              <p class="text-xs text-ink-muted mt-1 leading-snug">
                Open Settings and check each source's JQL or filters, credentials, and project key.
              </p>
            </div>
          </Show>

          <div class="flex gap-2 justify-end pt-1">
            <button
              type="button"
              class="px-4 py-2 text-sm font-semibold rounded-md bg-accent hover:bg-accent-hover text-base transition-colors focus-visible:outline-2 focus-visible:outline-offset-2"
              onClick={props.onClose}
            >
              Done
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}
