import { Show, createSignal, onMount } from 'solid-js';
import Board, { reload } from './components/Board.tsx';
import Settings from './components/Settings.tsx';
import MergeModal from './components/MergeModal.tsx';
import ClearSourceModal from './components/ClearSourceModal.tsx';
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
    try {
      const sources = await listSources();
      const enabled = sources.filter((s) => s.enabled);
      const unmatched = new Set<string>();
      let lastSyncedAt: string | null = null;
      for (const s of enabled) {
        const result = await syncSource(s.id);
        for (const u of result.unmappedStatuses) unmatched.add(u);
        if (result.conflicts.length > 0) {
          setConflicts(result.conflicts);
          setPendingSourceId(s.id);
          if (unmatched.size > 0) setUnmatchedStatuses([...unmatched].sort());
          return; // wait for the user to resolve
        }
        lastSyncedAt = result.syncedAt;
      }
      if (unmatched.size > 0) setUnmatchedStatuses([...unmatched].sort());
      if (lastSyncedAt) await finishSync(lastSyncedAt);
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
  }

  function lastSyncedLabel(): string {
    const value = lastSynced();
    return value ? `Synced ${new Date(value).toLocaleString()}` : '';
  }

  return (
    <div class="h-full flex flex-col bg-base">
      <header class="flex items-center justify-between px-3 py-2 bg-surface border-b border-border-subtle text-ink">
        <div class="flex items-center gap-2 min-w-0">
          <img src="/kansolo-icon.svg" alt="" class="w-6 h-6 shrink-0" />
          <h1 class="text-base font-bold tracking-tight truncate text-white">Kansolo</h1>
          {lastSynced() && (
            <span class="text-xs text-ink-muted truncate">{lastSyncedLabel()}</span>
          )}
        </div>
        <div class="flex gap-2 shrink-0">
          <button
            type="button"
            class="px-3 py-1.5 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors disabled:opacity-50"
            disabled={syncing()}
            onClick={() => void handleSync()}
          >
            <Show when={syncing()}>
              <span class="inline-block w-3 h-3 mr-1.5 align-middle border-2 border-base/40 border-t-base rounded-full animate-spin" aria-hidden="true" />
            </Show>
            {syncing() ? 'Syncing…' : 'Sync'}
          </button>
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
        <div class="px-3 py-2 bg-p-urgent/15 border-b border-p-urgent/40 text-p-urgent text-sm" role="alert">
          {syncError()}
        </div>
      )}
      {unmatchedStatuses() && (
        <div class="px-3 py-2 bg-p-high/15 border-b border-p-high/40 text-ink text-sm" role="status">
          <span class="font-semibold">Unmapped source statuses (sent to Backlog):</span>{' '}
          <span class="font-mono text-xs">{unmatchedStatuses()!.join(', ')}</span>
          <span class="block text-xs text-ink-secondary mt-0.5">
            Add these to a column in the source's status mapping (Settings).
          </span>
        </div>
      )}
      {settingsVisible() && <Settings onClose={() => setSettingsVisible(false)} />}
      {clearVisible() && (
        <ClearSourceModal
          onClose={() => {
            setClearVisible(false);
            setLastSynced(null);
            void setSetting('last_synced_at', '');
          }}
        />
      )}
      <Board />
      <Show when={conflicts()}>
        <MergeModal
          conflicts={conflicts()!}
          onResolve={(resolutions) => void handleResolve(resolutions)}
          onCancel={() => {
            setConflicts(null);
            setPendingSourceId(null);
            setSyncing(false);
          }}
        />
      </Show>
    </div>
  );
}

export default App;
