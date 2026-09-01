/* @refresh reload */
import { render } from "solid-js/web";
import App from "./App";
import "./index.css";

// Block default browser chrome: context menu, reload, nav back/forward,
// devtools shortcuts. Only in production — keep devtools open in dev.
const isProd = import.meta.env.PROD;
if (isProd) {
  document.addEventListener("contextmenu", (e) => e.preventDefault());
  document.addEventListener("keydown", (e) => {
    const k = e.key;
    const ctrl = e.ctrlKey || e.metaKey;
    // F12 / Ctrl+Shift+I / Ctrl+Shift+J / Ctrl+U → inspect / view source
    if (
      k === "F12" ||
      (ctrl && e.shiftKey &&
        (k === "I" || k === "i" || k === "J" || k === "j")) ||
      (ctrl && (k === "u" || k === "U"))
    ) {
      e.preventDefault();
      return;
    }
    // Ctrl+R / F5 / Cmd+R → reload
    if ((ctrl && (k === "r" || k === "R")) || k === "F5") {
      e.preventDefault();
      return;
    }
    // Alt+Left / Alt+Right / Cmd+Left / Cmd+Right → nav back/forward.
    // Skip when focus is in a text input/textarea — Ctrl+Arrow is word-jump
    // and Alt+Arrow is used by some editors; blocking them there breaks editing.
    const tag = (e.target as HTMLElement)?.tagName;
    const isText = tag === "INPUT" || tag === "TEXTAREA" || (e.target as HTMLElement)?.isContentEditable;
    if (!isText && (e.altKey || ctrl) && (k === "ArrowLeft" || k === "ArrowRight")) {
      e.preventDefault();
      return;
    }
  });
}

render(() => <App />, document.getElementById("root") as HTMLElement);
