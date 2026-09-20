import { test } from "vitest";
import { createSignal } from "solid-js";
import { cleanup, render, waitFor } from "@solidjs/testing-library";
import AgentRunPanel from "../../AgentRunPanel.tsx";
import { installDom, resetDom, teardownDom } from "./setup.ts";
import type { AgentRun } from "../../../types.ts";

installDom();

const baseRun = (status: string): AgentRun => ({
  id: "rh",
  cardId: "c1",
  agentName: "tester",
  sessionId: null,
  worktreePath: "/tmp/wt",
  branch: "branch-h",
  status,
  output: null,
  stopReason: null,
  error: null,
  mergedAt: null,
  skills: [],
  createdAt: "2024-01-01T00:00:00Z",
  finishedAt: null,
});

/** History the panel should load from acp_load_run_history. */
const history: unknown[] = [
  { type: "sessionId", sessionId: "s-old" },
  { type: "sessionUpdate", text: "🔧 Bash: ls" },
  { type: "sessionUpdate", text: "did the work" },
  { type: "restoringContext" },
  { type: "cancelled" },
];

test("AgentRunPanel: empty buffer loads persisted history from DB", async () => {
  resetDom();
  const [open, setOpen] = createSignal(true);
  let historyCalls = 0;
  // @ts-expect-error happy-dom window lacks Tauri internals typings
  globalThis.window.__TAURI_INTERNALS__ = {
    invoke: async (cmd: string) => {
      if (cmd === "acp_list_updates") return [];
      if (cmd === "acp_load_run_history") {
        historyCalls++;
        return history;
      }
      return null;
    },
    transformCallback: () => 0,
  };

  const [run] = createSignal<AgentRun | null>(baseRun("cancelled"));
  render(() => (
    <AgentRunPanel open={open()} onOpenChange={setOpen} run={run()} />
  ));

  // History entries render as transcript lines (assistant text + dim tool).
  // The panel renders through a Portal → query the document, not the container.
  await waitFor(() => {
    if (!document.body.textContent?.includes("did the work")) {
      throw new Error("history not rendered yet");
    }
  });
  if (!document.body.textContent?.includes("🔧 Bash: ls")) {
    throw new Error("tool line missing from history");
  }
  if (!document.body.textContent?.includes("Restoring context")) {
    throw new Error("restoringContext status line missing");
  }
  if (historyCalls !== 1) {
    throw new Error(`expected 1 history load, got ${historyCalls}`);
  }

  cleanup();
  teardownDom();
});

test("AgentRunPanel: transcript caps at 2000 entries (oldest dropped)", async () => {
  resetDom();
  const [open, setOpen] = createSignal(true);
  // First poll returns 2500 updates; the panel must keep only the last 2000.
  const big: unknown[] = Array.from({ length: 2500 }, (_, i) => ({
    type: "sessionUpdate",
    text: `line-${i}`,
  }));
  // @ts-expect-error happy-dom window lacks Tauri internals typings
  globalThis.window.__TAURI_INTERNALS__ = {
    invoke: async (cmd: string) => {
      if (cmd === "acp_list_updates") return big.splice(0, big.length);
      return null;
    },
    transformCallback: () => 0,
  };

  const [run] = createSignal<AgentRun | null>(baseRun("running"));
  render(() => (
    <AgentRunPanel open={open()} onOpenChange={setOpen} run={run()} />
  ));

  await waitFor(() => {
    if (!document.body.textContent?.includes("line-2499")) {
      throw new Error("newest line missing");
    }
  });
  // 2500 - 2000 = 500 dropped; line-499 is the last casualty.
  if (document.body.textContent?.includes("line-499")) {
    throw new Error("oldest entries should have been dropped");
  }
  if (!document.body.textContent?.includes("line-500")) {
    throw new Error("expected line-500 to survive the cap");
  }

  cleanup();
  teardownDom();
});
