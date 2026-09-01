import { listen, type EventCallback, type Options } from "@tauri-apps/api/event";

/** Wrapper around `listen` that fails silently when the Tauri event
 * internals are not available (e.g. in unit tests). Returns an unlisten
 * function in both cases. */
export async function safeListen<T>(
  event: string,
  handler: EventCallback<T>,
  options?: Options,
): Promise<() => void> {
  const internals = (globalThis as unknown as { window?: { __TAURI_INTERNALS__?: { transformCallback?: unknown }; __TAURI_EVENT_PLUGIN_INTERNALS__?: { unregisterListener?: unknown } } }).window;
  if (
    typeof internals?.__TAURI_INTERNALS__?.transformCallback !== "function" ||
    typeof internals?.__TAURI_EVENT_PLUGIN_INTERNALS__?.unregisterListener !==
      "function"
  ) {
    return () => {};
  }
  try {
    return await listen<T>(event, handler, options);
  } catch {
    return () => {};
  }
}
