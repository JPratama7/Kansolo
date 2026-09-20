import { createSignal } from "solid-js";
import { setSetting } from "../../db.ts";

/** Pointer + keyboard resize state for a dialog panel, persisted to the
 * settings KV store. Shared by Settings and AgentRunPanel grips. */
export function panelResize(
  wKey: string,
  hKey: string,
  defW: number,
  defH: number,
  minW: number,
  minH: number,
) {
  const [panelW, setPanelW] = createSignal(0);
  const [panelH, setPanelH] = createSignal(0);
  let panelEl: HTMLDivElement | undefined;
  let state: { x: number; y: number; w: number; h: number } | null = null;

  function onResizeStart(e: PointerEvent) {
    e.preventDefault();
    e.stopPropagation();
    state = {
      x: e.clientX,
      y: e.clientY,
      w: panelEl?.offsetWidth ?? defW,
      h: panelEl?.offsetHeight ?? defH,
    };
    window.addEventListener("pointermove", onResizeMove);
    window.addEventListener("pointerup", onResizeEnd);
  }
  function onResizeMove(e: PointerEvent) {
    if (!state) return;
    const w = Math.min(
      Math.max(state.w + (e.clientX - state.x), minW),
      window.innerWidth * 0.9,
    );
    const h = Math.min(
      Math.max(state.h + (e.clientY - state.y), minH),
      window.innerHeight * 0.9,
    );
    setPanelW(w);
    setPanelH(h);
  }

  async function onResizeEnd() {
    window.removeEventListener("pointermove", onResizeMove);
    window.removeEventListener("pointerup", onResizeEnd);
    const w = panelW();
    const h = panelH();
    state = null;
    if (w > 0 && h > 0) {
      try {
        await setSetting(wKey, String(Math.round(w)));
        await setSetting(hKey, String(Math.round(h)));
      } catch { /* non-fatal: size just won't persist */ }
    }
  }

  const gripProps = (ariaLabel: string) => ({
    onPointerDown: onResizeStart,
    onKeyDown: (e: KeyboardEvent) => {
      const step = e.shiftKey ? 20 : 5;
      if (e.key === "ArrowRight" || e.key === "ArrowDown") {
        e.preventDefault();
        setPanelW((w) => Math.min(w + step, window.innerWidth * 0.9));
        setPanelH((h) => Math.min(h + step, window.innerHeight * 0.9));
      } else if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
        e.preventDefault();
        setPanelW((w) => Math.max(w - step, minW));
        setPanelH((h) => Math.max(h - step, minH));
      }
    },
    role: "separator",
    "aria-orientation": "vertical",
    "aria-label": ariaLabel,
    tabindex: 0,
  });

  return { panelW, panelH, setPanelW, setPanelH, ref: (el: HTMLDivElement) => (panelEl = el), gripProps };
}
