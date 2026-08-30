import { For, Show, createEffect, createSignal, on, onCleanup } from 'solid-js';
import { Portal } from 'solid-js/web';
import { Dialog } from '@ark-ui/solid/dialog';
import { DiffView } from '@git-diff-view/solid';
import '@git-diff-view/solid/styles/diff-view.css';
import { toaster } from './ui/toaster.ts';
import type { AgentRun, RunUpdate, DiffResult, MergeResult } from '../db.ts';
import {
  acpListUpdates,
  acpCancelRun,
  acpDiffMain,
  acpMerge,
  acpRemoveWorktree,
  acpPermissionTimeoutMs,
  acpErrorMessage,
} from '../db.ts';
import { enqueuePermission, dequeuePermission, permissionHeadSignal } from './PermissionDialog.tsx';

export interface AgentRunPanelProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  run: AgentRun | null;
}

const STATUS_LABEL: Record<string, string> = {
  pending: 'Queued',
  running: 'Running',
  completed: 'Completed',
  failed: 'Failed',
  cancelled: 'Cancelled',
};

const POLL_INTERVAL_MS = 1000;

/** Parse raw unified diff text into hunk strings for DiffView.
 * Each hunk starts with `@@` and includes all lines until the next `@@`
 * or end of text. */
function parseHunks(diffText: string): string[] {
  const lines = diffText.split('\n');
  const hunks: string[] = [];
  let current: string[] = [];
  for (const line of lines) {
    if (line.startsWith('@@')) {
      if (current.length > 0) hunks.push(current.join('\n'));
      current = [line];
    } else if (current.length > 0) {
      current.push(line);
    }
  }
  if (current.length > 0) hunks.push(current.join('\n'));
  return hunks;
}

/** Run status + updates stream + diff view + merge button + cancel button +
 * remove worktree button. Opens when clicking an AgentBadge. */
export default function AgentRunPanel(props: AgentRunPanelProps) {
  const [updates, setUpdates] = createSignal<RunUpdate[]>([]);
  const [cursor, setCursor] = createSignal(0);
  const [diff, setDiff] = createSignal<DiffResult | null>(null);
  const [mergeResult, setMergeResult] = createSignal<MergeResult | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [showDiff, setShowDiff] = createSignal(false);
  const [diffMode, setDiffMode] = createSignal<'split' | 'unified'>('unified');
  // Permission requests are pushed to the module-level FIFO queue in
  // PermissionDialog.tsx; a single global dialog renders the queue head.
  // The panel keeps no local permission state.

  // Derived run identity + active flag, captured into their own signals so
  // the poll effect depends on stable booleans/ids rather than the whole
  // run object (which changes reference on every panelRun refresh).
  const [runId, setRunId] = createSignal<string | null>(null);
  const [hasActive, setHasActive] = createSignal(false);

  let pollTimer: ReturnType<typeof setInterval> | undefined;
  let updatesEl: HTMLDivElement | undefined;
  // In-flight guard: prevents overlapping pollUpdates calls from
  // double-appending the same updates when an interval fires before the
  // previous fetch resolves.
  let polling = false;

  // Mirror props.run identity/active into stable signals. Setting a signal
  // to an equal value is a no-op for downstream effects, so a panelRun
  // refresh with the same id+status does not retrigger the poll effect.
  createEffect(() => {
    setRunId(props.run?.id ?? null);
    setHasActive(props.run?.status === 'pending' || props.run?.status === 'running');
  });

  // Reset the stream/diff/merge state when switching to a different run.
  // `on` runs the effect whenever the id changes (including the first run).
  createEffect(
    on(
      () => props.run?.id,
      () => {
        setUpdates([]);
        setCursor(0);
        setDiff(null);
        setMergeResult(null);
      },
    ),
  );

  // Poll for updates while the run is active (pending/running). Depends on
  // the stable hasActive/runId signals, not the whole run object.
  createEffect(() => {
    const id = runId();
    const open = props.open;
    const active = hasActive();
    if (!open || !id || !active) return;
    pollTimer = setInterval(() => void pollUpdates(id), POLL_INTERVAL_MS);
    onCleanup(() => clearInterval(pollTimer));
  });

  async function pollUpdates(runId: string) {
    if (polling) return;
    polling = true;
    try {
      const newUpdates = await acpListUpdates(runId, cursor());
      if (newUpdates.length > 0) {
        setUpdates((prev) => [...prev, ...newUpdates]);
        setCursor((c) => c + newUpdates.length);
        // Auto-scroll to bottom.
        if (updatesEl) updatesEl.scrollTop = updatesEl.scrollHeight;
        // Forward permission requests to the global FIFO queue. The single
        // PermissionDialog (rendered at the App boundary) mediates the head.
        for (const u of newUpdates) {
          if (u.type === 'permissionRequest') {
            const timeoutMs = u.timeoutMs ?? (await acpPermissionTimeoutMs());
            enqueuePermission({
              runId,
              requestId: u.requestId,
              description: u.description,
              timeoutMs,
            });
          } else if (u.type === 'permissionTimeout') {
            // Rust auto-denied the oldest pending request (each permission
            // starts its own timeout timer on receipt, so timeouts fire in
            // enqueue order — i.e. the queue head). Drop the head so the
            // next request reveals immediately, mirroring the dialog's own
            // countdown dequeue.
            const head = permissionHeadSignal();
            if (head) dequeuePermission(head.requestId);
          }
        }
      }
    } catch {
      // Non-fatal — will retry on next poll.
    } finally {
      polling = false;
    }
  }

  async function handleCancel() {
    const run = props.run;
    if (!run) return;
    setBusy(true);
    try {
      await acpCancelRun(run.id);
      toaster.success({ title: 'Run cancelled' });
    } catch (e) {
      toaster.error({ title: 'Cancel failed', description: acpErrorMessage(e) });
    } finally {
      setBusy(false);
    }
  }

  async function handleDiff() {
    const run = props.run;
    if (!run) return;
    setBusy(true);
    try {
      const result = await acpDiffMain(run.cardId);
      setDiff(result);
      setShowDiff(true);
    } catch (e) {
      toaster.error({ title: 'Diff failed', description: acpErrorMessage(e) });
    } finally {
      setBusy(false);
    }
  }

  async function handleMerge() {
    const run = props.run;
    if (!run) return;
    setBusy(true);
    try {
      const result = await acpMerge(run.cardId);
      setMergeResult(result);
      if (result.success) {
        toaster.success({ title: 'Merged successfully' });
      } else {
        toaster.warning({
          title: 'Merge conflicts',
          description: `${result.conflicts.length} file(s) in conflict`,
        });
      }
    } catch (e) {
      toaster.error({ title: 'Merge failed', description: acpErrorMessage(e) });
    } finally {
      setBusy(false);
    }
  }

  async function handleRemoveWorktree() {
    const run = props.run;
    if (!run) return;
    setBusy(true);
    try {
      await acpRemoveWorktree(run.cardId);
      toaster.success({ title: 'Worktree removed' });
    } catch (e) {
      toaster.error({ title: 'Remove failed', description: acpErrorMessage(e) });
    } finally {
      setBusy(false);
    }
  }

  const isTerminal = () => {
    const s = props.run?.status;
    return s === 'completed' || s === 'failed' || s === 'cancelled';
  };

  const skillsUsed = () => props.run?.skills ?? [];

  return (
    <Dialog.Root
      open={props.open}
      lazyMount
      unmountOnExit
      closeOnEscape
      onOpenChange={(e) => props.onOpenChange(e.open)}
    >
      <Portal>
        <Dialog.Backdrop class="fixed inset-0 z-50 bg-black/50" />
        <Dialog.Positioner class="fixed inset-0 z-50 flex items-start justify-center pt-10 px-4">
          <Dialog.Content class="relative w-full max-w-2xl max-h-[80vh] flex flex-col bg-surface rounded-[var(--radius-card)] border border-border-subtle shadow-2xl overflow-hidden">
            {/* Header */}
            <div class="flex items-center justify-between px-4 py-3 bg-surface border-b border-border-subtle">
              <div class="min-w-0">
                <h2 class="text-base font-bold text-ink truncate">
                  Agent Run — {props.run?.agentName ?? '…'}
                </h2>
                <p class="text-xs text-ink-secondary">
                  {(props.run && STATUS_LABEL[props.run.status]) ?? '—'}
                  {' · '}
                  {props.run?.createdAt}
                </p>
              </div>
              <button
                type="button"
                class="text-xl text-ink-secondary hover:text-ink leading-none px-1"
                aria-label="Close"
                onClick={() => props.onOpenChange(false)}
              >
                ×
              </button>
            </div>

            {/* Body */}
            <div class="flex-1 min-h-0 overflow-y-auto board-scroll p-4 flex flex-col gap-4">
              {/* Skills used */}
              <Show when={skillsUsed().length > 0}>
                <div>
                  <p class="text-xs font-semibold text-ink-secondary mb-1">Skills</p>
                  <div class="flex flex-wrap gap-1">
                    <For each={skillsUsed()}>
                      {(name) => (
                        <span class="text-[10px] font-mono bg-base/60 rounded px-1.5 py-0.5 text-ink-secondary">
                          {name}
                        </span>
                      )}
                    </For>
                  </div>
                </div>
              </Show>

              {/* Update stream */}
              <div>
                <p class="text-xs font-semibold text-ink-secondary mb-1">Output</p>
                <div
                  ref={updatesEl}
                  class="h-48 overflow-y-auto board-scroll rounded border border-border-subtle bg-base/40 p-2 font-mono text-xs text-ink"
                >
                  <Show when={updates().length > 0} fallback={<p class="text-ink-secondary">No output yet.</p>}>
                    <For each={updates()}>
                      {(u) => {
                        switch (u.type) {
                          case 'sessionUpdate':
                            return <pre class="whitespace-pre-wrap break-words">{u.text}</pre>;
                          case 'sessionId':
                            return <p class="text-ink-secondary">Session: {u.sessionId}</p>;
                          case 'completed':
                            return <p class="text-p-high font-semibold">✓ Completed ({u.stopReason})</p>;
                          case 'failed':
                            return <p class="text-p-urgent font-semibold">✗ Failed: {u.error}</p>;
                          case 'cancelled':
                            return <p class="text-ink-secondary">— Cancelled —</p>;
                          case 'permissionRequest':
                            return <p class="text-p-med">Permission requested: {u.description}</p>;
                          case 'permissionTimeout':
                            return <p class="text-p-urgent">Permission timed out (auto-denied)</p>;
                          default:
                            return null;
                        }
                      }}
                    </For>
                  </Show>
                </div>
              </div>

              {/* Diff view */}
              <Show when={showDiff() && diff()}>
                {(d) => {
                  const hunks = parseHunks(d().text);
                  const hasHunks = hunks.length > 0 && d().text.trim().length > 0;
                  return (
                    <div>
                      <div class="flex items-center justify-between mb-1">
                        <div class="flex items-center gap-2">
                          <p class="text-xs font-semibold text-ink-secondary">Diff</p>
                          <Show when={hasHunks}>
                            <button
                              type="button"
                              class="text-[10px] px-1.5 py-0.5 rounded border border-border-subtle text-ink-secondary hover:text-ink transition-colors"
                              onClick={() => setDiffMode((m) => m === 'split' ? 'unified' : 'split')}
                            >
                              {diffMode() === 'split' ? 'Unified' : 'Split'}
                            </button>
                          </Show>
                        </div>
                        <Show when={d().truncated}>
                          <span class="text-[10px] text-p-urgent">truncated (1MB limit)</span>
                        </Show>
                      </div>
                      <Show
                        when={hasHunks}
                        fallback={
                          <pre class="h-40 overflow-auto board-scroll rounded border border-border-subtle bg-base/40 p-2 font-mono text-xs text-ink-secondary">
                            (no changes)
                          </pre>
                        }
                      >
                        <div class="h-64 overflow-auto board-scroll rounded border border-border-subtle bg-base/40">
                          <DiffView
                            data={{ hunks }}
                            diffViewMode={diffMode() === 'split' ? 1 : 4}
                            diffViewHighlight={true}
                            diffViewFontSize={12}
                            diffViewWrap={true}
                          />
                        </div>
                      </Show>
                    </div>
                  );
                }}
              </Show>

              {/* Merge result */}
              <Show when={mergeResult()}>
                {(r) => (
                  <div class={`rounded border p-3 ${r().success ? 'border-p-high/40 bg-p-high/10' : 'border-p-urgent/40 bg-p-urgent/10'}`}>
                    <p class={`text-sm font-semibold ${r().success ? 'text-p-high' : 'text-p-urgent'}`}>
                      {r().success ? 'Merge succeeded' : 'Merge conflicts'}
                    </p>
                    <Show when={!r().success}>
                      <ul class="mt-1 text-xs text-ink-secondary">
                        <For each={r().conflicts}>
                          {(c) => <li class="font-mono">{c}</li>}
                        </For>
                      </ul>
                      <Show when={r().repoBlocked}>
                        <p class="text-xs text-p-urgent mt-1">
                          Repository is blocked — resolve conflicts in terminal.
                        </p>
                      </Show>
                    </Show>
                  </div>
                )}
              </Show>

              {/* Error display */}
              <Show when={props.run?.error}>
                <div class="rounded border border-p-urgent/40 bg-p-urgent/10 p-3">
                  <p class="text-xs text-p-urgent font-semibold">Error</p>
                  <p class="text-xs text-ink mt-1">{props.run?.error}</p>
                </div>
              </Show>
            </div>

            {/* Footer — action buttons */}
            <div class="flex items-center justify-end gap-2 px-4 py-3 bg-surface border-t border-border-subtle">
              <Show when={!isTerminal()}>
                <button
                  type="button"
                  class="px-3 py-1.5 text-sm font-medium rounded border border-p-urgent/40 text-p-urgent hover:bg-p-urgent/10 transition-colors disabled:opacity-50"
                  disabled={busy()}
                  onClick={handleCancel}
                >
                  Cancel
                </button>
              </Show>
              <Show when={isTerminal()}>
                <button
                  type="button"
                  class="px-3 py-1.5 text-sm font-medium rounded border border-border-subtle text-ink hover:bg-elevated transition-colors disabled:opacity-50"
                  disabled={busy()}
                  onClick={handleDiff}
                >
                  View diff
                </button>
                <Show when={props.run?.status === 'completed'}>
                  <button
                    type="button"
                    class="px-3 py-1.5 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors disabled:opacity-50"
                    disabled={busy()}
                    onClick={handleMerge}
                  >
                    Merge
                  </button>
                </Show>
                <button
                  type="button"
                  class="px-3 py-1.5 text-sm font-medium rounded border border-border-subtle text-ink-secondary hover:text-ink transition-colors disabled:opacity-50"
                  disabled={busy()}
                  onClick={handleRemoveWorktree}
                >
                  Remove worktree
                </button>
              </Show>
            </div>
          </Dialog.Content>
        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
  );
}
