/**
 * Shared test setup: registers a happy-dom `GlobalWindow` onto `globalThis`
 * so component tests can use `document`, `window`, `IntersectionObserver`,
 * etc. Under vitest with `environment: "happy-dom"` these globals are
 * already provided, but this module is still imported by tests for the
 * `resetDom()` helper (clears document.body between cases).
 *
 * Idempotent: re-installing replaces the previous window cleanly. Tests
 * should call `resetDom()` between cases to clear the document body.
 */
import { GlobalWindow } from "happy-dom";

let installed: GlobalWindow | null = null;

/** Install happy-dom globals on `globalThis` (idempotent). */
export function installDom(): GlobalWindow {
  if (installed) return installed;
  const win = new GlobalWindow();
  installed = win;
  for (const key of Object.keys(win)) {
    // @ts-expect-error index assignment onto globalThis
    if (!(key in globalThis)) globalThis[key] = win[key];
  }
  // @ts-expect-error window may already be defined by Deno's lib.dom typings
  if (!globalThis.window) globalThis.window = win as unknown as Window;
  // @ts-expect-error document may be undefined in Deno's bare runtime
  if (!globalThis.document) globalThis.document = win.document;
  return win;
}

/** Clear the document body between test cases. */
export function resetDom(): void {
  const win = installed;
  if (!win) return;
  win.document.body.innerHTML = "";
}

/** Tear down the happy-dom window (called in test cleanup). */
export function teardownDom(): void {
  if (!installed) return;
  installed.happyDOM?.cancelAsync();
  installed = null;
}
