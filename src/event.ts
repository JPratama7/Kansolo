import { listen, type EventCallback, type Options } from "@tauri-apps/api/event";

/** `listen` wrapper that returns an unlisten function even when Tauri
 * event internals are missing (e.g. unit tests). */
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
