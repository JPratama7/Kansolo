import { Show, createSignal, onMount } from 'solid-js';
import { Toaster, Toast } from '@ark-ui/solid/toast';
import Board, { reload } from './components/Board.tsx';
import Settings from './components/Settings.tsx';
import MergeModal from './components/MergeModal.tsx';
import ClearSourceModal from './components/ClearSourceModal.tsx';
import SyncSummaryModal, { type SyncSummaryEntry } from './components/SyncSummaryModal.tsx';
import { toaster } from './components/ui/toaster.ts';
import {
  getSetting,
  listSources,
  resolveConflicts,
  setSetting,
  syncSource,
} from './db.ts';
import type { ConflictResolution, SyncConflict } from './types.ts';

function App() {
  const [settingsVisible, setSettingsVisible] = createSignal(false);
  const [clearVisible, setClearVisible] = createSignal(false);
  const [syncing, setSyncing] = createSignal(false);
  const [syncError, setSyncError] = createSignal<string | null>(null);
  const [lastSynced, setLastSynced] = createSignal<string | null>(null);
  const [conflicts, setConflicts] = createSignal<SyncConflict[] | null>(null);
  // Source id whose sync produced the current conflicts; needed to resolve.
  const [pendingSourceId, setPendingSourceId] = createSignal<string | null>(null);
  const [unmatchedStatuses, setUnmatchedStatuses] = createSignal<string[] | null>(null);
  const [syncSummary, setSyncSummary] = createSignal<SyncSummaryEntry[] | null>(null);

  onMount(async () => {
    setLastSynced(await getSetting('last_synced_at'));
  });

  /**
   * Sync every enabled source in turn. The first source that surfaces
   * conflicts pauses the loop — the user must resolve them before the run
   * is considered complete. Unmapped statuses from all sources are surfaced
   * together as a banner.
   */
  async function handleSync() {
    setSyncing(true);
    setSyncError(null);
    setConflicts(null);
    setPendingSourceId(null);
    setUnmatchedStatuses(null);
    setSyncSummary(null);
    try {
      const sources = await listSources();
      const enabled = sources.filter((s) => s.enabled);
      const unmatched = new Set<string>();
      const summary: SyncSummaryEntry[] = [];
      let lastSyncedAt: string | null = null;
      for (const s of enabled) {
        const result = await syncSource(s.id);
        summary.push({ label: s.label, sourceType: s.sourceType, count: result.importedCount });
        for (const u of result.unmappedStatuses) unmatched.add(u);
        if (result.conflicts.length > 0) {
          setConflicts(result.conflicts);
          setPendingSourceId(s.id);
          if (unmatched.size > 0) setUnmatchedStatuses([...unmatched].sort());
          setSyncSummary(summary);
          return; // wait for the user to resolve
        }
        lastSyncedAt = result.syncedAt;
      }
      if (unmatched.size > 0) setUnmatchedStatuses([...unmatched].sort());
      if (lastSyncedAt) await finishSync(lastSyncedAt);
      setSyncSummary(summary);
    } catch (e) {
      setSyncError(e instanceof Error ? e.message : String(e));
    } finally {
      setSyncing(false);
    }
  }

  /** Apply per-card field resolutions, then finish the paused sync run. */
  async function handleResolve(resolutions: ConflictResolution[]) {
    const sourceId = pendingSourceId();
    if (!sourceId) return;
    try {
      await resolveConflicts(sourceId, resolutions);
      setConflicts(null);
      setPendingSourceId(null);
      // Continue the run with the remaining enabled sources.
      const syncedAt = new Date().toISOString();
      await finishSync(syncedAt);
    } catch (e) {
      setSyncError(e instanceof Error ? e.message : String(e));
    } finally {
      setSyncing(false);
    }
  }

  async function finishSync(syncedAt: string) {
    await reload();
    await setSetting('last_synced_at', syncedAt);
    setLastSynced(syncedAt);
    toaster.success({ title: 'Sync complete', description: lastSyncedLabel() });
  }

  function lastSyncedLabel(): string {
    const value = lastSynced();
    if (!value) return '';
    const d = new Date(value);
    return `Synced ${d.toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' })}`;
  }

  return (
    <div class="h-full flex flex-col bg-base">
      <a href="#main-board" class="sr-only focus:not-sr-only focus:absolute focus:z-50 focus:top-2 focus:left-2 focus:px-3 focus:py-1.5 focus:bg-accent focus:text-base focus:rounded">
        Skip to board
      </a>
      <header class="flex items-center justify-between px-3 py-2 bg-surface border-b border-border-subtle text-ink safe-top">
        <div class="flex items-center gap-2 min-w-0">
          <img src="/kansolo-icon.svg" alt="" width={24} height={24} class="w-6 h-6 shrink-0" />
          <h1 class="text-base font-bold tracking-tight truncate text-white">Kansolo</h1>
          {lastSynced() && (
            <span class="text-xs text-ink-muted truncate">{lastSyncedLabel()}</span>
          )}
        </div>
        <div class="flex gap-2 shrink-0">
          <button
            type="button"
            data-testid="sync-button"
            class="px-3 py-1.5 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors disabled:opacity-50"
            disabled={syncing()}
            onClick={() => void handleSync()}
          >
            <Show when={syncing()}>
              <span class="inline-block w-3 h-3 mr-1.5 align-middle border-2 border-base/40 border-t-base rounded-full animate-spin" aria-hidden="true" />
            </Show>
            {syncing() ? 'Syncing…' : 'Sync'}
          </button>
          <span class="sr-only" aria-live="polite">{syncing() ? 'Syncing…' : ''}</span>
          <button
            type="button"
            class="px-3 py-1.5 text-sm font-medium rounded bg-elevated hover:bg-hover text-ink transition-colors disabled:opacity-50"
            disabled={syncing()}
            onClick={() => setClearVisible(true)}
          >
            Clear
          </button>
          <button
            type="button"
            class="px-3 py-1.5 text-sm font-medium rounded bg-elevated hover:bg-hover text-ink transition-colors"
            onClick={() => setSettingsVisible(!settingsVisible())}
          >
            Settings
          </button>
        </div>
      </header>
      {syncError() && (
        <div class="px-3 py-2 bg-p-urgent/15 border-b border-p-urgent/40 text-p-urgent text-sm" role="alert" aria-live="polite">
          {syncError()}
        </div>
      )}
      {unmatchedStatuses() && (
        <div class="px-3 py-2 bg-p-high/15 border-b border-p-high/40 text-ink text-sm" role="status" aria-live="polite">
          <span class="font-semibold">Unmapped source statuses (sent to Backlog):</span>{' '}
          <span class="font-mono text-xs">{unmatchedStatuses()!.join(', ')}</span>
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
            void setSetting('last_synced_at', '');
          }
        }}
      />
      <Board />
      <MergeModal
        conflicts={conflicts() ?? []}
        open={!!conflicts()}
        onOpenChange={(o) => {
          if (!o) {
            setConflicts(null);
            setPendingSourceId(null);
            setSyncing(false);
          }
        }}
        onResolve={(resolutions) => void handleResolve(resolutions)}
        onCancel={() => {
          setConflicts(null);
          setPendingSourceId(null);
          setSyncing(false);
        }}
      />
      <Show when={syncSummary() && !conflicts()}>
        <SyncSummaryModal
          entries={syncSummary()!}
          syncedAt={lastSynced() ?? undefined}
          onClose={() => setSyncSummary(null)}
        />
      </Show>
      <Toaster toaster={toaster}>
        {(toast) => (
          <Toast.Root
            class="bg-surface border border-border-subtle rounded-[var(--radius-card)] shadow-2xl px-4 py-3 min-w-[240px]"
            data-type={toast().type}
          >
            <Toast.Title class="text-sm font-semibold text-ink" />
            <Toast.Description class="text-xs text-ink-secondary" />
            <Show when={toast().action}>
              <Toast.ActionTrigger class="mt-2 text-xs font-semibold text-accent hover:text-accent-hover" />
            </Show>
            <Toast.CloseTrigger class="text-ink-secondary hover:text-ink text-xs ml-2" aria-label="Close" />
          </Toast.Root>
        )}
      </Toaster>
    </div>
  );
}

export default App;
