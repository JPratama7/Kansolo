import { createSignal, onMount, Show } from "solid-js";
import { initTheme } from "./theme.ts";
import { Portal } from "solid-js/web";
import { Toast, Toaster } from "@ark-ui/solid/toast";
import Board, { reload } from "./components/Board.tsx";
import Settings from "./components/Settings.tsx";
import MergeModal from "./components/MergeModal.tsx";
import ClearSourceModal from "./components/ClearSourceModal.tsx";
import SyncSummaryModal, {
  type SyncSummaryEntry,
} from "./components/SyncSummaryModal.tsx";
import AgentManager from "./components/AgentManager.tsx";
import PermissionDialog from "./components/PermissionDialog.tsx";
import { toaster } from "./components/ui/toaster.ts";
import {
  acpErrorMessage,
  getSetting,
  listSources,
  resolveConflicts,
  setSetting,
  syncSource,
} from "./db.ts";
import type {
  ConflictResolution,
  SourceInstance,
  SyncConflict,
} from "./types.ts";

function App() {
  const [settingsVisible, setSettingsVisible] = createSignal(false);
  const [managerVisible, setManagerVisible] = createSignal(false);
  const [clearVisible, setClearVisible] = createSignal(false);
  const [syncing, setSyncing] = createSignal(false);
  const [syncError, setSyncError] = createSignal<string | null>(null);
  const [lastSynced, setLastSynced] = createSignal<string | null>(null);
  const [conflicts, setConflicts] = createSignal<SyncConflict[] | null>(null);
  // Conflicts pause the sync loop; this state is needed to resume afterward.
  const [pendingSourceId, setPendingSourceId] = createSignal<string | null>(
    null,
  );
  const [pendingSources, setPendingSources] = createSignal<SourceInstance[]>(
    [],
  );
  const [pendingUnmatched, setPendingUnmatched] = createSignal<Set<string>>(
    new Set(),
  );
  const [pendingSummary, setPendingSummary] = createSignal<SyncSummaryEntry[]>(
    [],
  );
  const [conflictSyncedAt, setConflictSyncedAt] = createSignal<string | null>(
    null,
  );
  const [unmatchedStatuses, setUnmatchedStatuses] = createSignal<
    string[] | null
  >(null);
  const [syncSummary, setSyncSummary] = createSignal<SyncSummaryEntry[] | null>(
    null,
  );

  onMount(async () => {
    setLastSynced(await getSetting("last_synced_at"));
    void initTheme();
  });

  /** Sync a batch of sources; pauses at the first conflict and stashes the rest for resume. */
  async function syncBatch(
    sources: SourceInstance[],
    unmatched: Set<string>,
    summary: SyncSummaryEntry[],
    initialSyncedAt: string | null,
  ): Promise<{ done: boolean; syncedAt: string | null }> {
    let syncedAt = initialSyncedAt;
    for (let i = 0; i < sources.length; i++) {
      const s = sources[i];
      const result = await syncSource(s.id);
      summary.push({
        label: s.label,
        sourceType: s.sourceType,
        count: result.importedCount,
      });
      for (const u of result.unmappedStatuses) unmatched.add(u);
      if (result.conflicts.length > 0) {
        setPendingSources(sources.slice(i + 1));
        setPendingUnmatched(unmatched);
        setPendingSummary(summary);
        setConflicts(result.conflicts);
        setPendingSourceId(s.id);
        setConflictSyncedAt(result.syncedAt);
        setSyncSummary(summary);
        if (unmatched.size > 0) setUnmatchedStatuses([...unmatched].sort());
        return { done: false, syncedAt };
      }
      syncedAt = result.syncedAt;
    }
    return { done: true, syncedAt };
  }

  async function handleSync() {
    setSyncing(true);
    setSyncError(null);
    setConflicts(null);
    setPendingSourceId(null);
    setPendingSources([]);
    setPendingUnmatched(new Set());
    setPendingSummary([]);
    setConflictSyncedAt(null);
    setUnmatchedStatuses(null);
    setSyncSummary(null);
    try {
      const sources = await listSources();
      const enabled = sources.filter((s) => s.enabled);
      const unmatched = new Set<string>();
      const summary: SyncSummaryEntry[] = [];
      const { done, syncedAt } = await syncBatch(
        enabled,
        unmatched,
        summary,
        null,
      );
      if (done) {
        if (unmatched.size > 0) setUnmatchedStatuses([...unmatched].sort());
        if (syncedAt) await finishSync(syncedAt);
        setSyncSummary(summary);
        setSyncing(false);
      }
      // When !done (conflicts paused), keep syncing=true so the Sync
      // button stays disabled while the conflict modal is open.
    } catch (e) {
      setSyncError(acpErrorMessage(e));
      setSyncing(false);
    }
  }

  /** Apply resolutions, then resume the paused sync loop. */
  async function handleResolve(resolutions: ConflictResolution[]) {
    const sourceId = pendingSourceId();
    if (!sourceId) return;
    try {
      await resolveConflicts(sourceId, resolutions);
      setConflicts(null);
      setPendingSourceId(null);
      // Resume the paused loop with the stashed state.
      const remaining = pendingSources();
      const unmatched = pendingUnmatched();
      const summary = pendingSummary();
      const syncedAt = conflictSyncedAt();
      setPendingSources([]);
      setPendingUnmatched(new Set());
      setPendingSummary([]);
      setConflictSyncedAt(null);
      const { done, syncedAt: finalSyncedAt } = await syncBatch(
        remaining,
        unmatched,
        summary,
        syncedAt,
      );
      if (done) {
        if (unmatched.size > 0) setUnmatchedStatuses([...unmatched].sort());
        if (finalSyncedAt) await finishSync(finalSyncedAt);
        setSyncSummary(summary);
      }
    } catch (e) {
      setSyncError(acpErrorMessage(e));
    } finally {
      setSyncing(false);
    }
  }

  async function finishSync(syncedAt: string) {
    await reload();
    await setSetting("last_synced_at", syncedAt);
    setLastSynced(syncedAt);
    toaster.success({ title: "Sync complete", description: lastSyncedLabel() });
  }

  function lastSyncedLabel(): string {
    const value = lastSynced();
    if (!value) return "";
    const d = new Date(value);
    return `Synced ${
      d.toLocaleString([], { dateStyle: "medium", timeStyle: "short" })
    }`;
  }

  return (
    <div class="h-full flex flex-col bg-base">
      <a
        href="#main-board"
        class="sr-only focus:not-sr-only focus:absolute focus:z-50 focus:top-2 focus:left-2 focus:px-3 focus:py-1.5 focus:bg-accent focus:text-base focus:rounded"
      >
        Skip to board
      </a>
      <header class="flex items-center justify-between px-6 py-4 bg-base border-b border-rule text-ink safe-top">
        <div class="flex items-center gap-2 min-w-0">
          <img
            src="/kansolo-icon.svg"
            alt=""
            width={24}
            height={24}
            class="w-6 h-6 shrink-0"
          />
          <h1 class="font-serif text-[1.6rem] text-ink truncate">
            Kansolo
          </h1>
          {lastSynced() && (
            <span class="text-xs text-ink-muted truncate">
              {lastSyncedLabel()}
            </span>
          )}
        </div>
        <div class="flex gap-2 shrink-0">
          <button
            type="button"
            data-testid="sync-button"
            class="px-3 py-2 text-[0.8rem] font-medium rounded border border-rule text-ink hover:bg-elevated transition-colors disabled:opacity-50"
            disabled={syncing()}
            onClick={() => void handleSync()}
          >
            <Show when={syncing()}>
              <span
                class="inline-block w-3 h-3 mr-1.5 align-middle border-2 border-ink/30 border-t-ink rounded-full animate-spin"
                aria-hidden="true"
              />
            </Show>
            {syncing() ? "Syncing…" : "Sync"}
          </button>
          <span class="sr-only" aria-live="polite">
            {syncing() ? "Syncing…" : ""}
          </span>
          <button
            type="button"
            class="px-3 py-2 text-[0.8rem] font-medium rounded border border-rule text-ink hover:bg-elevated transition-colors disabled:opacity-50"
            disabled={syncing()}
            onClick={() => setClearVisible(true)}
          >
            Clear
          </button>
          <button
            type="button"
            class="px-3 py-2 text-[0.8rem] font-medium rounded border border-rule text-ink hover:bg-elevated transition-colors"
            onClick={() => setManagerVisible(true)}
          >
            Workspaces
          </button>
          <button
            type="button"
            class="px-3 py-2 text-[0.8rem] font-medium rounded border border-rule text-ink hover:bg-elevated transition-colors"
            onClick={() => setSettingsVisible(!settingsVisible())}
          >
            Settings
          </button>
        </div>
      </header>
      {syncError() && (
        <div
          class="px-3 py-2 bg-p-urgent/15 border-b border-p-urgent/40 text-p-urgent text-sm"
          role="alert"
          aria-live="polite"
        >
          {syncError()}
        </div>
      )}
      {unmatchedStatuses() && (
        <div
          class="px-3 py-2 bg-p-high/15 border-b border-p-high/40 text-ink text-sm"
          role="status"
          aria-live="polite"
        >
          <span class="font-semibold">
            Unmapped source statuses (sent to Backlog):
          </span>{" "}
          <span class="font-mono text-xs">
            {unmatchedStatuses()!.join(", ")}
          </span>
          <span class="block text-xs text-ink-secondary mt-0.5">
            Add these to a column in the source's status mapping (Settings).
          </span>
        </div>
      )}
      <Settings
        open={settingsVisible()}
        onOpenChange={(o) => {
          if (!o) {
            setSettingsVisible(false);
            void reload();
          }
        }}
      />
      <ClearSourceModal
        open={clearVisible()}
        onOpenChange={(o) => {
          if (!o) {
            setClearVisible(false);
            setLastSynced(null);
            void setSetting("last_synced_at", "");
          }
        }}
      />
      <Board />
      <AgentManager
        open={managerVisible()}
        onOpenChange={setManagerVisible}
      />
      {
        /* Single global permission dialog — renders the head of the
          module-level FIFO queue fed by all run panels. */
      }
      <PermissionDialog />
      <MergeModal
        conflicts={conflicts() ?? []}
        open={!!conflicts()}
        onOpenChange={(o) => {
          if (!o) {
            setConflicts(null);
            setPendingSourceId(null);
            setPendingSources([]);
            setPendingUnmatched(new Set());
            setPendingSummary([]);
            setConflictSyncedAt(null);
            setSyncing(false);
          }
        }}
        onResolve={(resolutions) => void handleResolve(resolutions)}
        onCancel={() => {
          setConflicts(null);
          setPendingSourceId(null);
          setPendingSources([]);
          setPendingUnmatched(new Set());
          setPendingSummary([]);
          setConflictSyncedAt(null);
          setSyncing(false);
          void reload();
        }}
      />
      <Show when={syncSummary() !== null && !conflicts()}>
        <SyncSummaryModal
          entries={syncSummary()!}
          syncedAt={lastSynced() ?? undefined}
          onClose={() => setSyncSummary(null)}
        />
      </Show>
      <Portal>
        <Toaster toaster={toaster} class="z-[100]">
          {(toast) => (
            <Toast.Root
              class="relative bg-surface border border-border-subtle rounded-[var(--radius-card)] shadow-2xl px-4 py-3 min-w-[240px]"
              data-type={toast().type}
            >
              <Toast.CloseTrigger
                class="absolute top-1.5 right-1.5 text-ink-secondary hover:text-ink text-xs leading-none px-1"
                aria-label="Close"
              >
                ×
              </Toast.CloseTrigger>
              <Toast.Title class="text-sm font-semibold text-ink pr-4">
                {toast().title}
              </Toast.Title>
              <Toast.Description class="text-xs text-ink-secondary">
                {toast().description}
              </Toast.Description>
              <Show when={toast().action}>
                <Toast.ActionTrigger class="mt-2 text-xs font-semibold text-accent hover:text-accent-hover">
                  {toast().action?.label}
                </Toast.ActionTrigger>
              </Show>
            </Toast.Root>
          )}
        </Toaster>
      </Portal>
    </div>
  );
}

export default App;
