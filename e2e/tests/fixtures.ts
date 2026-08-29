import { test as base, expect, type Page } from '@playwright/test';

/**
 * Tauri invoke mock.
 *
 * The app imports `invoke` from `@tauri-apps/api/core`. In the browser (no
 * Tauri runtime) that import resolves but `invoke` throws. We stub
 * `window.__TAURI_INTERNALS__.invoke` so the app's `invoke(...)` calls land
 * in a JS handler we control from the test.
 *
 * Usage:
 *   const t = await useTauri(page, handlers);
 *   await page.goto('/');
 *   // ... interact ...
 *   expect(t.calls).toContainEqual(['list_cards_by_column', { column: 'backlog' }]);
 *
 * Handlers is a map of command name -> ((args) => result | Promise<result>).
 * Unhandled commands return `undefined`.
 */

export type InvokeHandler = (args: Record<string, unknown>) => unknown | Promise<unknown>;
export type InvokeHandlers = Record<string, InvokeHandler>;

export interface TauriMock {
  /** All invoke calls in order: [command, args]. */
  calls: Array<[string, Record<string, unknown> | undefined]>;
  /** Replace handlers at runtime. */
  setHandlers(next: InvokeHandlers): void;
}

async function installTauriMock(page: Page, handlers: InvokeHandlers): Promise<TauriMock> {
  // The dispatcher runs in Node (via page.exposeFunction), so handler
  // closures over test-local state (e.g. an in-memory card store) survive —
  // something addInitScript's structured-clone serialization can't do, since
  // it silently drops function-valued args. The calls log lives in Node too;
  // tests read it via readInvokeCalls, which round-trips through the page.
  const calls: Array<[string, Record<string, unknown> | undefined]> = [];
  const live: { handlers: InvokeHandlers } = { handlers };
  await page.exposeFunction(
    '__kansoloDispatch',
    (cmd: string, args?: Record<string, unknown>) => {
      calls.push([cmd, args]);
      const h = live.handlers[cmd];
      return h ? h(args ?? {}) : undefined;
    },
  );
  await page.exposeFunction('__kansoloGetCalls', () => calls);
  await page.addInitScript(() => {
    (window as any).__TAURI_INTERNALS__ = (window as any).__TAURI_INTERNALS__ || {};
    (window as any).__TAURI_INTERNALS__.invoke = (cmd: string, args?: unknown) =>
      (window as any).__kansoloDispatch(cmd, args);
    // Also stub @tauri-apps/api/core's invoke fallback path.
    (window as any).__TAURI_INVOKE__ = (window as any).__TAURI_INTERNALS__.invoke;
  });

  return {
    calls,
    setHandlers(next) {
      live.handlers = next;
    },
  };
}

/** Helper to read recorded invoke calls from the page. */
export async function readInvokeCalls(page: Page): Promise<Array<[string, unknown]>> {
  return page.evaluate(() => (window as any).__kansoloGetCalls()) as Promise<Array<[string, unknown]>>;
}

/** Test fixture that exposes a TauriMock via `useTauri`. */
export const test = base.extend<{ useTauri: (handlers?: InvokeHandlers) => Promise<TauriMock> }>({
  useTauri: async ({ page }, use) => {
    await use((handlers: InvokeHandlers = {}) => installTauriMock(page, handlers));
  },
});

export { expect };
