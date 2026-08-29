import { test, expect } from './fixtures.ts';
import type { InvokeHandlers } from './fixtures.ts';

/** Sync flow with no conflicts: returns one imported card. */
function syncOkHandlers(): InvokeHandlers {
  return {
    list_cards_by_column: (args) =>
      (args as { column: string }).column === 'backlog'
        ? [{ id: 'c1', title: 'Synced card', column: 'backlog', position: 1, priority: 'medium', source: 'jira', sourceRef: 'JIRA-1', sourceStatus: 'To Do', treeSourceId: null, description: '', updatedAt: '2024-01-01T00:00:00Z' }]
        : [],
    list_tree_sources: () => [],
    list_sources: () => [{ id: 'src-1', sourceType: 'jira', label: 'My Jira', enabled: true, configJson: '{}' }],
    sync_source: () => ({ importedCount: 3, conflicts: [], unmappedStatuses: [], syncedAt: '2024-06-01T12:00:00Z' }),
    get_setting: () => null,
    set_setting: () => null,
    resolve_conflicts: () => null,
    create_local_card: () => null,
    update_card: () => null,
    move_card: () => null,
    delete_card: () => null,
  };
}

/** Sync flow that surfaces one conflict. */
function syncConflictHandlers(): InvokeHandlers {
  return {
    list_cards_by_column: (args) =>
      (args as { column: string }).column === 'backlog'
        ? [{ id: 'c1', title: 'Local title', column: 'backlog', position: 1, priority: 'medium', source: 'jira', sourceRef: 'JIRA-1', sourceStatus: 'To Do', treeSourceId: null, description: '', updatedAt: '2024-01-01T00:00:00Z' }]
        : [],
    list_tree_sources: () => [],
    list_sources: () => [{ id: 'src-1', sourceType: 'jira', label: 'My Jira', enabled: true, configJson: '{}' }],
    sync_source: () => ({
      importedCount: 0,
      conflicts: [{
        sourceRef: 'JIRA-1',
        conflicts: [{ field: 'title', local: 'Local title', remote: 'Remote title' }],
      }],
      unmappedStatuses: [],
      syncedAt: '2024-06-01T12:00:00Z',
    }),
    resolve_conflicts: () => null,
    get_setting: () => null,
    set_setting: () => null,
    create_local_card: () => null,
    update_card: () => null,
    move_card: () => null,
    delete_card: () => null,
  };
}

test.describe('Sync flow', () => {
  test('click Sync -> SyncSummaryModal appears with imported count', async ({ page, useTauri }) => {
    await useTauri(syncOkHandlers());
    await page.goto('/');

    await page.locator('[data-testid="sync-button"]').click();

    // SyncSummaryModal should appear with the per-source row.
    const modal = page.locator('[data-testid="sync-summary-modal"]');
    await expect(modal).toBeVisible({ timeout: 10_000 });
    await expect(modal).toContainText('My Jira');
    await expect(modal).toContainText('3');
  });

  test('conflict resolution: MergeModal opens, pick All remote, Apply merge', async ({ page, useTauri }) => {
    await useTauri(syncConflictHandlers());
    await page.goto('/');

    await page.locator('[data-testid="sync-button"]').click();

    // MergeModal should open with one conflict for JIRA-1.
    const merge = page.locator('[data-testid="merge-modal"]');
    await expect(merge).toBeVisible({ timeout: 10_000 });
    await expect(merge).toContainText('JIRA-1');

    // Click "All remote" for this conflict.
    await page.locator('[data-testid="take-all-remote-JIRA-1"]').click();

    // Apply merge -> MergeModal closes.
    await page.locator('[data-testid="apply-merge"]').click();
    await expect(merge).not.toBeVisible({ timeout: 10_000 });
  });
});
