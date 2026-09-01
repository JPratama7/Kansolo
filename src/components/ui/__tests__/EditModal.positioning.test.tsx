import { test } from "vitest";
import { installDom, resetDom, teardownDom } from "./setup.ts";
import { cleanup, render } from "@solidjs/testing-library";
import EditModal from "../../EditModal.tsx";
import type { KanbanCard } from "../../../types.ts";

installDom();

const card: KanbanCard = {
  id: "c1",
  title: "Title",
  description: "",
  priority: "medium",
  column: "backlog",
  source: "local",
  position: 1,
  createdAt: "2024-01-01T00:00:00Z",
  updatedAt: "2024-01-01T00:00:00Z",
};

// Known column rect injected via getBoundingClientRect mock. EditModal
// does not call Tauri `invoke` (only Board does), so no invoke mock is needed.
const RECT = {
  left: 120,
  top: 80,
  width: 320,
  height: 600,
  right: 440,
  bottom: 680,
  x: 120,
  y: 80,
  toJSON: () => ({}),
} as DOMRect;
const ZERO = {
  ...RECT,
  left: 0,
  top: 0,
  width: 0,
  height: 0,
  right: 0,
  bottom: 0,
  x: 0,
  y: 0,
} as DOMRect;

function mockGBCR() {
  const orig = Element.prototype.getBoundingClientRect;
  Element.prototype.getBoundingClientRect = function () {
    return this.hasAttribute && this.hasAttribute("data-column-id")
      ? RECT
      : ZERO;
  };
  return () => {
    Element.prototype.getBoundingClientRect = orig;
  };
}

// Positioning wrapper is the parent div of Dialog.Content (.ticket-panel)
// inside Dialog.Positioner (see EditModal.tsx lines 374-399).
function findWrapper(): HTMLElement {
  const content = document.body.querySelector(".ticket-panel") as
    | HTMLElement
    | null;
  if (!content) {
    throw new Error("Dialog.Content (.ticket-panel) not rendered in portal");
  }
  return content.parentElement as HTMLElement;
}

test("EditModal: anchored wrapper matches column rect left/top/width", async () => {
  const col = document.createElement("div");
  col.setAttribute("data-column-id", "backlog");
  document.body.appendChild(col);
  const restore = mockGBCR();

  render(() => (
    <EditModal
      card={card}
      treeSources={() => []}
      open={true}
      onOpenChange={() => {}}
      onSave={() => {}}
    />
  ));
  await new Promise((r) => setTimeout(r, 10)); // lazyMount + createEffect flush

  const w = findWrapper();
  if (w.style.left !== "120px") {
    throw new Error(`left: expected 120px, got "${w.style.left}"`);
  }
  if (w.style.top !== "80px") {
    throw new Error(`top: expected 80px, got "${w.style.top}"`);
  }
  if (w.style.width !== "320px") {
    throw new Error(`width: expected 320px, got "${w.style.width}"`);
  }

  restore();
  col.remove();
  cleanup();
  resetDom();
});

test("EditModal: centered fallback (inset:0 + flex classes) when no column element", async () => {
  render(() => (
    <EditModal
      card={card}
      treeSources={() => []}
      open={true}
      onOpenChange={() => {}}
      onSave={() => {}}
    />
  ));
  await new Promise((r) => setTimeout(r, 10));

  const w = findWrapper();
  // happy-dom doesn't parse the `inset` shorthand into style.inset, so read
  // the raw style attribute instead.
  const styleAttr = w.getAttribute("style") || "";
  if (!/inset:\s*0\b/.test(styleAttr)) {
    throw new Error(`inset: expected "0" in style attr, got "${styleAttr}"`);
  }
  if (
    !w.className.includes("flex") || !w.className.includes("justify-center")
  ) {
    throw new Error(`expected centered flex classes, got "${w.className}"`);
  }

  cleanup();
  resetDom();
});

teardownDom();
