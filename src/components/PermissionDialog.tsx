import { createEffect, createSignal, onCleanup, onMount, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { acpCancelRun, acpErrorMessage, acpPermissionTimeoutMs, acpRespondPermission } from "../db.ts";
import type { AcpUpdateEvent } from "../types.ts";
import { safeListen } from "../event.ts";
import { toaster } from "./ui/toaster.ts";

/** Queued permission request awaiting user mediation. `timeoutMs` comes
 * from the Rust payload when present, else from the
 * `acp_permission_timeout` setting (see `acpPermissionTimeoutMs`). */
export interface PermissionQueueItem {
  runId: string;
  requestId: string;
  description: string;
  timeoutMs: number;
}

// Module-level FIFO for permission requests. Run panels enqueue; the dialog
// renders only the head. The Rust per-run pending map is left untouched.
const permissionQueue: PermissionQueueItem[] = [];
const [permissionHead, setPermissionHead] = createSignal<
  PermissionQueueItem | null
>(null);

function syncHead() {
  setPermissionHead(permissionQueue[0] ?? null);
}

/** Enqueue a permission request from a run panel. Duplicate requestIds
 * (e.g. a late poll re-delivering the same request) are ignored. */
export function enqueuePermission(item: PermissionQueueItem): void {
  if (permissionQueue.some((q) => q.requestId === item.requestId)) return;
  permissionQueue.push(item);
  syncHead();
}

/** Remove a request from the queue by requestId (idempotent). Reveals the
 * next queued request as the new head. Called on user respond and on
 * timeout. */
export function dequeuePermission(requestId: string): void {
  const i = permissionQueue.findIndex((q) => q.requestId === requestId);
  if (i >= 0) permissionQueue.splice(i, 1);
  syncHead();
}

/** Drop every queued request belonging to one run. Called when the user
 * stops a run from the permission dialog — without this, remaining
 * requests from the same run would keep popping up one after another. */
export function dequeuePermissionsForRun(runId: string): void {
  for (let i = permissionQueue.length - 1; i >= 0; i--) {
    if (permissionQueue[i].runId === runId) permissionQueue.splice(i, 1);
  }
  syncHead();
}

/** Current queue head (the single request being mediated). */
export function permissionHeadSignal(): PermissionQueueItem | null {
  return permissionHead();
}

/** Clear the whole queue (test helper / teardown). */
export function clearPermissionQueue(): void {
  permissionQueue.length = 0;
  syncHead();
}

/** Permission mediation UI. Renders only the queue head — concurrent
 * requests from multiple runs are serialized by the module-level FIFO
 * store. The description is the structured tool summary (tool name +
 * truncated args) produced on the Rust side; rendered as plain text
 * under an "untrusted agent content" label. The countdown reads
 * `timeoutMs` from the queue item (Rust payload or setting), not a
 * hardcoded constant. */
export default function PermissionDialog() {
  const head = permissionHead;
  const [remaining, setRemaining] = createSignal(0);
  const [stopping, setStopping] = createSignal(false);

  onMount(() => {
    let cancelled = false;
    const unlistenPromise = safeListen<AcpUpdateEvent>("acp:update", async (event) => {
      if (cancelled) return;
      const u = event.payload.update;
      if (u.type === "permissionRequest") {
        const timeoutMs = u.timeoutMs ?? (await acpPermissionTimeoutMs());
        enqueuePermission({
          runId: event.payload.runId,
          requestId: u.requestId,
          description: u.description,
          timeoutMs,
        });
      } else if (u.type === "permissionTimeout") {
        const headItem = permissionHeadSignal();
        if (headItem) dequeuePermission(headItem.requestId);
      }
    });

    onCleanup(() => {
      cancelled = true;
      unlistenPromise.then((fn) => fn());
    });
  });

  let timer: ReturnType<typeof setInterval> | undefined;
  createEffect(() => {
    const item = head();
    // Clear the previous timer before starting a new one — without this,
    // switching from one permission request to another leaks the old interval.
    clearInterval(timer);
    timer = undefined;
    if (item) {
      setRemaining(item.timeoutMs);
      const start = Date.now();
      timer = setInterval(() => {
        const left = Math.max(0, item.timeoutMs - (Date.now() - start));
        setRemaining(left);
        if (left === 0) {
          // Rust already auto-denied via PermissionTimeout; just drop the
          // queue entry so the next request (if any) reveals.
          dequeuePermission(item.requestId);
        }
      }, 1000);
    } else {
      clearInterval(timer);
    }
  });

  onCleanup(() => clearInterval(timer));

  const secondsLeft = () => Math.ceil(remaining() / 1000);

  async function respond(approved: boolean) {
    const item = head();
    if (!item) return;
    const reqId = item.requestId;
    // Dequeue first so the next request reveals immediately, even if the
    // Tauri call is slow.
    dequeuePermission(reqId);
    try {
      await acpRespondPermission(reqId, approved);
    } catch (e) {
      toaster.error({
        title: "Permission response failed",
        description: acpErrorMessage(e),
      });
    }
  }

  /** Escape hatch for permission loops: an agent that keeps requesting
   * permissions (e.g. denied tool calls retried in a loop) would otherwise
   * re-open this dialog forever. Stop the run and drop queued requests. The
   * modal backdrop blocks the panel's own stop button, so the dialog must
   * offer this itself. */
  async function stopRun() {
    const item = head();
    if (!item || stopping()) return;
    setStopping(true);
    const runId = item.runId;
    dequeuePermissionsForRun(runId);
    try {
      await acpCancelRun(runId);
      toaster.success({ title: "Run stopped" });
    } catch (e) {
      toaster.error({
        title: "Stop failed",
        description: acpErrorMessage(e),
      });
    } finally {
      setStopping(false);
    }
  }

  return (
    <Dialog.Root
      open={head() !== null}
      lazyMount
      unmountOnExit
      closeOnEscape
      closeOnInteractOutside={false}
      onOpenChange={(e) => {
        if (!e.open) {
          const item = head();
          if (item) dequeuePermission(item.requestId);
        }
      }}
    >
      <Show when={head()}>
        {(item) => (
          <Portal>
            <Dialog.Backdrop class="fixed inset-0 z-[60] bg-black/50" />
            <Dialog.Positioner class="fixed inset-0 z-[60] flex items-center justify-center px-4">
              <Dialog.Content class="relative w-full max-w-md bg-surface rounded-[var(--radius-card)] border border-border-subtle shadow-2xl p-5">
                <Dialog.Title class="text-base font-bold text-ink mb-2">
                  Permission Request
                </Dialog.Title>
                {
                  /* Untrusted agent content: rendered as plain text (no HTML
                    interpolation) — the description is a structured tool
                    summary (tool name + truncated args) from the Rust side. */
                }
                <div class="rounded border border-border-subtle bg-base/40 p-3 mb-3">
                  <p class="text-[10px] font-semibold uppercase tracking-wide text-ink-secondary mb-1">
                    Untrusted agent content
                  </p>
                  <p class="text-sm text-ink whitespace-pre-wrap break-words font-mono">
                    {item().description}
                  </p>
                </div>
                <div class="rounded border border-p-urgent/40 bg-p-urgent/10 p-3 mb-3">
                  <p class="text-xs text-p-urgent font-medium">
                    Warning: The agent has full filesystem and network access
                    inside its worktree directory. Approve only if you trust
                    this action.
                  </p>
                </div>
                <button
                  type="button"
                  class="w-full mb-4 px-3 py-1.5 text-xs font-medium rounded border border-border-subtle text-ink-secondary hover:text-p-urgent hover:border-p-urgent/50 transition-colors disabled:opacity-50"
                  disabled={stopping()}
                  onClick={() => void stopRun()}
                >
                  {stopping() ? "Stopping run…" : "Stop run"}
                </button>
                <div class="flex items-center justify-between gap-3">
                  <span class="text-xs text-ink-secondary">
                    Auto-deny in {secondsLeft()}s
                  </span>
                  <div class="flex gap-2">
                    <button
                      type="button"
                      class="px-3 py-1.5 text-sm font-medium rounded border border-border-subtle text-ink hover:bg-elevated transition-colors disabled:opacity-50"
                      disabled={stopping()}
                      onClick={() => void respond(false)}
                    >
                      Deny
                    </button>
                    <button
                      type="button"
                      class="px-3 py-1.5 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors disabled:opacity-50"
                      disabled={stopping()}
                      onClick={() => void respond(true)}
                    >
                      Approve
                    </button>
                  </div>
                </div>
              </Dialog.Content>
            </Dialog.Positioner>
          </Portal>
        )}
      </Show>
    </Dialog.Root>
  );
}
