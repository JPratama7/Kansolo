import { test } from "vitest";
import { createSignal } from "solid-js";
import { cleanup, render, waitFor } from "@solidjs/testing-library";
import AgentRunPanel from "../../AgentRunPanel.tsx";
import { installDom, resetDom, teardownDom } from "./setup.ts";
import type { AgentRun } from "../../../types.ts";

installDom();

const baseRun = (status: string): AgentRun => ({
  id: "r1",
  cardId: "c1",
  agentName: "tester",
  sessionId: null,
  worktreePath: "/tmp/wt",
  branch: "branch-1",
  status,
  output: "o",
  stopReason: null,
  error: null,
  mergedAt: null,
  skills: [],
  createdAt: "2024-01-01T00:00:00Z",
  finishedAt: null,
});

/** Regression: Board refreshes panelRun with a NEW object (same id + status)
 * every 2s poll while a run is active. Solid tracks the props.run getter
 * read itself, so effects depending on props.run re-fire per refresh — the
 * reset effect must key off a memoized id VALUE, not the object reference,
 * or it wipes the stream back to the "No output yet" fallback every poll
 * (the blink: empty ↔ session). */
test("AgentRunPanel: same-id refresh must not wipe the stream", async () => {
  resetDom();
  const [open, setOpen] = createSignal(true);
  const [run, setRun] = createSignal<AgentRun | null>(baseRun("running"));

  let batchNo = 0;
  // @ts-expect-error happy-dom window lacks Tauri internals typings
  globalThis.window.__TAURI_INTERNALS__ = {
    invoke: async (cmd: string) => {
      if (cmd !== "acp_list_updates") return null;
      // Batch 1: session id; batch 2+: one assistant update each.
      batchNo++;
      if (batchNo === 1) return [{ type: "sessionId", sessionId: "s-1" }];
      return [{ type: "sessionUpdate", text: `update-${batchNo}` }];
    },
    transformCallback: () => 0,
  };

  render(() => (
    <AgentRunPanel open={open()} onOpenChange={setOpen} run={run()} />
  ));

  await waitFor(
    () => {
      if (batchNo < 2) throw new Error(`expected batch 2, got ${batchNo}`);
    },
    { timeout: 4000 },
  );

  // Board-style refresh: fresh object, same id, same status. The stream
  // (session + updates) must survive without ever showing the empty state.
  for (let i = 0; i < 5; i++) {
    setRun(baseRun("running"));
    await new Promise((r) => setTimeout(r, 50));
    if (document.querySelector(".agent-empty")) {
      throw new Error(`stream wiped on refresh ${i}`);
    }
    if (!document.querySelector(".agent-system")) {
      throw new Error(`session message lost on refresh ${i}`);
    }
  }

  setOpen(false);
  await new Promise((r) => setTimeout(r, 50));
  cleanup();
  teardownDom();
});
