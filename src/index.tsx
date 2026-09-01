/* @refresh reload */
import { render } from "solid-js/web";
import { Show } from "solid-js";
import App from "./App";
import { initTheme, themeReady } from "./theme.ts";
import "./index.css";

// Read the saved theme before the app shell mounts; index.tsx gates the
// UI on themeReady so nothing paints in the wrong theme.
void initTheme();

// Minimal loading screen shown while the theme loads from the DB.
function ThemeGate() {
  return (
    <div class="h-full flex items-center justify-center bg-base">
      <span
        class="inline-block w-6 h-6 border-2 border-ink/30 border-t-ink rounded-full animate-spin"
        aria-hidden="true"
      />
      <span class="sr-only" role="status">Loading…</span>
    </div>
  );
}

// Disable default browser chrome in production: context menu, reload,
// back/forward nav, devtools shortcuts. Keep devtools open in dev.
const isProd = import.meta.env.PROD;
if (isProd) {
  document.addEventListener("contextmenu", (e) => e.preventDefault());
  document.addEventListener("keydown", (e) => {
    const key = e.key;
    const ctrl = e.ctrlKey || e.metaKey;
    // F12 / Ctrl+Shift+I / Ctrl+Shift+J / Ctrl+U → inspect / view source
    if (
      key === "F12" ||
      (ctrl && e.shiftKey &&
        (key === "I" || key === "i" || key === "J" || key === "j")) ||
      (ctrl && (key === "u" || key === "U"))
    ) {
      e.preventDefault();
      return;
    }
    // Ctrl+R / F5 / Cmd+R → reload
    if ((ctrl && (key === "r" || key === "R")) || key === "F5") {
      e.preventDefault();
      return;
    }
    // Alt+Left / Alt+Right / Cmd+Left / Cmd+Right → nav back/forward.
    // Skip in text inputs/textarea: Ctrl+Arrow is word-jump and Alt+Arrow
    // is used by editors; blocking them breaks editing.
    const tag = (e.target as HTMLElement)?.tagName;
    const isText = tag === "INPUT" || tag === "TEXTAREA" || (e.target as HTMLElement)?.isContentEditable;
    if (!isText && (e.altKey || ctrl) && (key === "ArrowLeft" || key === "ArrowRight")) {
      e.preventDefault();
      return;
    }
  });
}

render(
  () => (
    <Show when={themeReady()} fallback={<ThemeGate />}>
      <App />
    </Show>
  ),
  document.getElementById("root") as HTMLElement,
);
