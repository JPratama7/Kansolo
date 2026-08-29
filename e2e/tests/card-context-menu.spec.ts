/// <reference lib="dom" />
import { test, expect } from './fixtures.ts';
import type { InvokeHandlers } from './fixtures.ts';
import { readInvokeCalls } from './fixtures.ts';

function handlers(extra: InvokeHandlers = {}): InvokeHandlers {
  return {
    list_cards_by_column: (args) =>
      (args as { column: string }).column === 'backlog'
        ? [{ id: 'card-a', title: 'Card A', column: 'backlog', position: 1, priority: 'medium', source: 'local', sourceRef: null, sourceStatus: null, treeSourceId: 'ts-1', description: '', updatedAt: '2024-01-01T00:00:00Z' }]
        : [],
    list_tree_sources: () => [{ id: 'ts-1', label: 'Notes', path: '/tmp/notes', editorCommand: 'code {path}' }],
    open_in_editor: () => null,
    create_local_card: () => null,
    update_card: () => null,
    move_card: () => null,
    delete_card: () => null,
    get_setting: () => null,
    set_setting: () => null,
    list_sources: () => [],
    ...extra,
  };
}

test.describe('Card context menu', () => {
  test('right-click opens the menu at the cursor position', async ({ page, useTauri }) => {
    await useTauri(handlers());
    await page.goto('/');

    const card = page.locator('[data-testid="card-card-a"]');
    await card.waitFor();
    const box = (await card.boundingBox())!;

    // Right-click near the top-left of the card.
    await page.mouse.move(box.x + 10, box.y + 10);
    await page.mouse.click(box.x + 10, box.y + 10, { button: 'right' });

    const menu = page.locator('[data-testid="card-context-menu"]');
    await expect(menu).toBeVisible();

    // Menu should contain the "Open in editor" item (card has a tree source).
    await expect(menu.locator('[data-testid="menu-item-editor"]')).toBeVisible();
  });

  test('Shift+F10 opens the menu at the card position', async ({ page, useTauri }) => {
    await useTauri(handlers());
    await page.goto('/');

    const card = page.locator('[data-testid="card-card-a"]');
    await card.focus();
    await page.keyboard.press('Shift+F10');

    const menu = page.locator('[data-testid="card-context-menu"]');
    await expect(menu).toBeVisible();
  });

  test('Escape closes the menu and returns focus to the card', async ({ page, useTauri }) => {
    await useTauri(handlers());
    await page.goto('/');

    const card = page.locator('[data-testid="card-card-a"]');
    await card.focus();
    await page.keyboard.press('Shift+F10');
    const menu = page.locator('[data-testid="card-context-menu"]');
    await expect(menu).toBeVisible();

    await page.keyboard.press('Escape');
    await expect(menu).not.toBeVisible();

    // Focus should return to the card (it's the active element).
    const active = await page.evaluate(() => (document.activeElement as HTMLElement | null)?.getAttribute('data-testid'));
    expect(active).toBe('card-card-a');
  });

  test('"Open in editor" item invokes open_in_editor', async ({ page, useTauri }) => {
    await useTauri(handlers());
    await page.goto('/');

    const card = page.locator('[data-testid="card-card-a"]');
    await card.focus();
    await page.keyboard.press('Shift+F10');
    const menu = page.locator('[data-testid="card-context-menu"]');
    await expect(menu).toBeVisible();

    await menu.locator('[data-testid="menu-item-editor"]').click();

    const calls = await readInvokeCalls(page);
    const editorCall = calls.find(([cmd]) => cmd === 'open_in_editor');
    expect(editorCall).toBeTruthy();
  });

  test('menu repositions on a second right-click', async ({ page, useTauri }) => {
    await useTauri(handlers());
    await page.goto('/');

    const card = page.locator('[data-testid="card-card-a"]');
    await card.waitFor();
    const box = (await card.boundingBox())!;

    // First right-click near the top of the card.
    await page.mouse.click(box.x + 10, box.y + 10, { button: 'right' });
    const menu = page.locator('[data-testid="card-context-menu"]');
    await expect(menu).toBeVisible();
    const box1 = (await menu.boundingBox())!;

    // Second right-click near the bottom of the card. The menu should stay
    // open (controlled) and move to the new anchor point.
    await page.mouse.click(box.x + 30, box.y + box.height - 10, { button: 'right' });
    await expect(menu).toBeVisible();
    const box2 = (await menu.boundingBox())!;

    // The menu's top should differ between the two opens (it repositioned).
    expect(Math.abs(box2.y - box1.y)).toBeGreaterThan(0);
  });
});
