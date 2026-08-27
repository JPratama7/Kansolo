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
 *   expect(t.calls).toContainEqual(['list_cards', undefined]);
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
  // Inject a small script that records calls and dispatches to handlers.
  // Handlers are stored on window so tests can swap them via evaluate.
  await page.addInitScript((initial) => {
    const store: { handlers: Record<string, (a: unknown) => unknown>; calls: Array<[string, unknown]> } = {
      handlers: {},
      calls: [],
    };
    (window as any).__kansoloMock = store;
    (window as any).__TAURI_INTERNALS__ = (window as any).__TAURI_INTERNALS__ || {};
    (window as any).__TAURI_INTERNALS__.invoke = async (cmd: string, args?: unknown) => {
      store.calls.push([cmd, args]);
      const h = store.handlers[cmd];
      if (!h) return undefined;
      return await h(args);
    };
    // Also stub @tauri-apps/api/core's invoke fallback path.
    (window as any).__TAURI_INVOKE__ = (window as any).__TAURI_INTERNALS__.invoke;
    for (const [k, v] of Object.entries(initial as Record<string, (a: unknown) => unknown>)) {
      store.handlers[k] = v;
    }
  }, handlers);

  return {
    calls: [],
    setHandlers(next) {
      // We can't sync the local `calls` array across the IPC boundary, so
      // tests should read calls via `page.evaluate(() => (window as any).__kansoloMock.calls)`.
      void page.evaluate((n) => {
        const store = (window as any).__kansoloMock;
        for (const [k, v] of Object.entries(n)) store.handlers[k] = v;
      }, next);
    },
  };
}

/** Helper to read recorded invoke calls from the page. */
export async function readInvokeCalls(page: Page): Promise<Array<[string, unknown]>> {
  return page.evaluate(() => (window as any).__kansoloMock.calls as Array<[string, unknown]>);
}

/** Test fixture that exposes a TauriMock via `useTauri`. */
export const test = base.extend<{ useTauri: (handlers?: InvokeHandlers) => Promise<TauriMock> }>({
  useTauri: async ({ page }, use) => {
    await use((handlers: InvokeHandlers = {}) => installTauriMock(page, handlers));
  },
});

export { expect };
