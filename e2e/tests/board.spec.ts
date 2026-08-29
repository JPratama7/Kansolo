import { test, expect } from './fixtures.ts';
import type { InvokeHandlers } from './fixtures.ts';

/** A minimal in-memory card store backing the mocked Tauri commands. */
function cardStore(initial: Array<{ id: string; title: string; column: 'backlog' | 'ongoing' | 'done'; position: number; priority: 'low' | 'medium' | 'high' | 'urgent' }>) {
  const cards = initial.map((c) => ({ ...c, updatedAt: '2024-01-01T00:00:00Z' }));
  return {
    handlers: (): InvokeHandlers => ({
      list_cards_by_column: (args: Record<string, unknown>) => {
        const col = (args as { column: string }).column;
        return cards
          .filter((c) => c.column === col)
          .map((c) => ({ ...c, source: 'local', sourceRef: null, sourceStatus: null, treeSourceId: null, description: '' }));
      },
      move_card: (args: Record<string, unknown>) => {
        const a = args as { id: string; column: 'backlog' | 'ongoing' | 'done'; position: number | null };
        const c = cards.find((x) => x.id === a.id);
        if (c) {
          c.column = a.column;
          // position is null when appending (drag-drop) — land at end of column.
          const max = Math.max(0, ...cards.filter((x) => x.column === a.column).map((x) => x.position));
          c.position = a.position ?? max + 1;
          c.updatedAt = new Date().toISOString();
        }
        return null;
      },
      list_tree_sources: () => [],
      create_local_card: () => null,
      update_card: () => null,
      delete_card: () => null,
      get_setting: () => null,
      set_setting: () => null,
      list_sources: () => [],
    }),
  };
}

test.describe('Board drag-drop and keyboard move', () => {
  test('drag a card from Backlog to Done moves it', async ({ page, useTauri }) => {
    const store = cardStore([
      { id: 'card-1', title: 'Drag me', column: 'backlog', position: 1, priority: 'medium' },
    ]);
    await useTauri(store.handlers());
    await page.goto('/');

    // Card starts in Backlog column.
    const backlog = page.locator('[data-column-id="backlog"]');
    const done = page.locator('[data-column-id="done"]');
    await expect(backlog.locator('[data-testid="card-card-1"]')).toBeVisible();
    await expect(done.locator('[data-testid="card-card-1"]')).toHaveCount(0);

    // Drag the card into the Done column.
    const card = page.locator('[data-testid="card-card-1"]');
    await card.hover();
    await page.mouse.down();
    await done.hover();
    await page.mouse.up();

    // After drop, the card should appear in the Done column.
    await expect(done.locator('[data-testid="card-card-1"]')).toBeVisible({ timeout: 5000 });
    await expect(backlog.locator('[data-testid="card-card-1"]')).toHaveCount(0);
  });

  test('keyboard move via context menu: Shift+F10 -> ArrowDown -> Enter -> Move to Done', async ({ page, useTauri }) => {
    const store = cardStore([
      { id: 'card-2', title: 'Keyboard card', column: 'backlog', position: 1, priority: 'low' },
    ]);
    await useTauri(store.handlers());
    await page.goto('/');

    const card = page.locator('[data-testid="card-card-2"]');
    await card.focus();

    // Shift+F10 opens the context menu at the card.
    await page.keyboard.press('Shift+F10');
    const menu = page.locator('[data-testid="card-context-menu"]');
    await expect(menu).toBeVisible();

    // Arrow down past "Edit" (and possibly "Open in editor" which is hidden
    // because the card has no tree source) to the first "Move to ..." item,
    // then keep going to "Move to Done".
    // Items order: Edit, [Open in editor (hidden)], Move to Backlog (disabled),
    // Move to Ongoing, Move to Done.
    await page.keyboard.press('ArrowDown'); // Edit
    await page.keyboard.press('ArrowDown'); // Move to Backlog (disabled, skipped)
    await page.keyboard.press('ArrowDown'); // Move to Ongoing
    await page.keyboard.press('ArrowDown'); // Move to Done
    await page.keyboard.press('Enter');

    // Card should now be in the Done column.
    const done = page.locator('[data-column-id="done"]');
    await expect(done.locator('[data-testid="card-card-2"]')).toBeVisible({ timeout: 5000 });
  });
});
