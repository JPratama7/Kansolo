import { createEffect, createSignal, For, onCleanup, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { toaster } from "./ui/toaster.ts";
import AgentRunPanel from "./AgentRunPanel.tsx";
import type { AgentRun } from "../types.ts";
import {
  acpCancelRun,
  acpDeleteRun,
  acpErrorMessage,
  acpListRecentRuns,
  acpMerge,
  acpRemoveWorktree,
  listCards,
} from "../db.ts";

export interface AgentManagerProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

const STATUS_LABEL: Record<string, string> = {
  pending: "Queued",
  running: "Running",
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
};

const POLL_INTERVAL_MS = 3000;

/** Global panel listing every run (active + recent) via
 * `acp_list_recent_runs`. Columns: card title, agent name, status, started
 * at. Actions: open the run panel (reused as a drawer), cancel active runs,
 * merge/remove worktree for terminal runs. */
export default function AgentManager(props: AgentManagerProps) {
  const [runs, setRuns] = createSignal<AgentRun[]>([]);
  const [cardTitles, setCardTitles] = createSignal<Record<string, string>>({});
  const [busyId, setBusyId] = createSignal<string | null>(null);
  // Selected run for the nested AgentRunPanel "drawer".
  const [panelRun, setPanelRun] = createSignal<AgentRun | null>(null);
  const [panelOpen, setPanelOpen] = createSignal(false);

  async function refresh() {
    try {
      const [recent, cards] = await Promise.all([
        acpListRecentRuns(),
        listCards(),
      ]);
      setRuns(recent);
      const titles: Record<string, string> = {};
      for (const c of cards) titles[c.id] = c.title;
      setCardTitles(titles);
    } catch (e) {
      toaster.error({
        title: "Failed to load runs",
        description: acpErrorMessage(e),
      });
    }
  }

  // Refresh + poll whenever the dialog opens; clean up on close.
  createEffect(() => {
    if (props.open) {
      void refresh();
      const id = setInterval(() => void refresh(), POLL_INTERVAL_MS);
      onCleanup(() => clearInterval(id));
    }
  });

  function openPanel(run: AgentRun) {
    setPanelRun(run);
    setPanelOpen(true);
  }

  async function cancelRun(run: AgentRun) {
    setBusyId(run.id);
    try {
      await acpCancelRun(run.id);
      toaster.success({ title: "Run cancelled" });
      await refresh();
    } catch (e) {
      toaster.error({
        title: "Cancel failed",
        description: acpErrorMessage(e),
      });
    } finally {
      setBusyId(null);
    }
  }

  async function mergeRun(run: AgentRun) {
    setBusyId(run.id);
    try {
      const result = await acpMerge(run.cardId);
      if (result.success) toaster.success({ title: "Merged successfully" });
      else {toaster.warning({
          title: "Merge conflicts",
          description: `${result.conflicts.length} file(s)`,
        });}
      await refresh();
    } catch (e) {
      toaster.error({ title: "Merge failed", description: acpErrorMessage(e) });
    } finally {
      setBusyId(null);
    }
  }

  async function removeWorktree(run: AgentRun) {
    setBusyId(run.id);
    try {
      await acpRemoveWorktree(run.cardId);
      toaster.success({ title: "Worktree removed" });
      await refresh();
    } catch (e) {
      toaster.error({
        title: "Remove failed",
        description: acpErrorMessage(e),
      });
    } finally {
      setBusyId(null);
    }
  }

  async function deleteRun(run: AgentRun) {
    setBusyId(run.id);
    try {
      await acpDeleteRun(run.id);
      toaster.success({ title: "Run removed" });
      await refresh();
    } catch (e) {
      toaster.error({
        title: "Remove failed",
        description: acpErrorMessage(e),
      });
    } finally {
      setBusyId(null);
    }
  }

  const isActive = (r: AgentRun) =>
    r.status === "pending" || r.status === "running";
  const isTerminal = (r: AgentRun) =>
    r.status === "completed" || r.status === "failed" ||
    r.status === "cancelled";

  return (
    <>
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
            <Dialog.Content class="relative w-full max-w-3xl max-h-[80vh] flex flex-col bg-surface rounded-[var(--radius-card)] border border-border-subtle shadow-2xl overflow-hidden">
              <div class="flex items-center justify-between px-4 py-3 bg-surface border-b border-border-subtle">
                <h2 class="text-base font-bold text-ink">Agent Runs</h2>
                <button
                  type="button"
                  class="text-xl text-ink-secondary hover:text-ink leading-none px-1"
                  aria-label="Close"
                  onClick={() => props.onOpenChange(false)}
                >
                  ×
                </button>
              </div>
              <div class="flex-1 min-h-0 overflow-y-auto board-scroll">
                <Show
                  when={runs().length > 0}
                  fallback={
                    <p class="p-4 text-sm text-ink-secondary">No runs yet.</p>
                  }
                >
                  <table class="w-full text-sm">
                    <thead class="sticky top-0 bg-surface border-b border-border-subtle text-xs text-ink-secondary uppercase tracking-wide">
                      <tr>
                        <th class="text-left font-semibold px-4 py-2">Card</th>
                        <th class="text-left font-semibold px-4 py-2">Agent</th>
                        <th class="text-left font-semibold px-4 py-2">
                          Status
                        </th>
                        <th class="text-left font-semibold px-4 py-2">
                          Started
                        </th>
                        <th class="text-right font-semibold px-4 py-2">
                          Actions
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      <For each={runs()}>
                        {(run) => (
                          <tr class="border-b border-border-subtle last:border-0 hover:bg-hover/40">
                            <td class="px-4 py-2 text-ink truncate max-w-[180px]">
                              {cardTitles()[run.cardId] ?? run.cardId}
                            </td>
                            <td class="px-4 py-2 text-ink">{run.agentName}</td>
                            <td class="px-4 py-2 text-ink-secondary">
                              {STATUS_LABEL[run.status] ?? run.status}
                            </td>
                            <td class="px-4 py-2 text-ink-secondary text-xs">
                              {run.createdAt}
                            </td>
                            <td class="px-4 py-2 text-right whitespace-nowrap">
                              <button
                                type="button"
                                class="text-xs px-2 py-1 rounded border border-border-subtle text-ink hover:bg-elevated transition-colors mr-1"
                                onClick={() => openPanel(run)}
                              >
                                Open
                              </button>
                              <Show when={isActive(run)}>
                                <button
                                  type="button"
                                  class="text-xs px-2 py-1 rounded border border-p-urgent/40 text-p-urgent hover:bg-p-urgent/10 transition-colors disabled:opacity-50"
                                  disabled={busyId() === run.id}
                                  onClick={() => void cancelRun(run)}
                                >
                                  Cancel
                                </button>
                              </Show>
                              <Show when={isTerminal(run)}>
                                <button
                                  type="button"
                                  class="text-xs px-2 py-1 rounded bg-accent hover:bg-accent-hover text-base transition-colors disabled:opacity-50 mr-1"
                                  disabled={busyId() === run.id}
                                  onClick={() => void mergeRun(run)}
                                >
                                  Merge
                                </button>
                                <button
                                  type="button"
                                  class="text-xs px-2 py-1 rounded border border-border-subtle text-ink-secondary hover:text-ink transition-colors disabled:opacity-50 mr-1"
                                  disabled={busyId() === run.id}
                                  onClick={() => void removeWorktree(run)}
                                >
                                  Remove worktree
                                </button>
                                <button
                                  type="button"
                                  class="text-xs px-2 py-1 rounded border border-p-urgent/40 text-p-urgent hover:bg-p-urgent/10 transition-colors disabled:opacity-50"
                                  disabled={busyId() === run.id}
                                  onClick={() => void deleteRun(run)}
                                >
                                  Remove
                                </button>
                              </Show>
                            </td>
                          </tr>
                        )}
                      </For>
                    </tbody>
                  </table>
                </Show>
              </div>
            </Dialog.Content>
          </Dialog.Positioner>
        </Portal>
      </Dialog.Root>
      {/* Reuse AgentRunPanel as a drawer for the selected run. */}
      <AgentRunPanel
        open={panelOpen()}
        onOpenChange={setPanelOpen}
        run={panelRun()}
      />
    </>
  );
}
