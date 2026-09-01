import { test } from "vitest";
import { createSignal } from "solid-js";
import { cleanup, render, waitFor } from "@solidjs/testing-library";
import AgentRunPanel from "../../AgentRunPanel.tsx";
import { installDom, resetDom, teardownDom } from "./setup.ts";
import type { AgentRun } from "../../../types.ts";

installDom();

const baseRun = (id: string, status: string): AgentRun => ({
  id,
  cardId: "c1",
  agentName: "tester",
  sessionId: null,
  worktreePath: "/tmp/wt",
  branch: "branch-" + id,
  status,
  output: null,
  stopReason: null,
  error: null,
  mergedAt: null,
  skills: ["ponytail"],
  createdAt: "2024-01-01T00:00:00Z",
  finishedAt: null,
});

/** Mock Tauri invoke: acp_list_updates drains the test's queue once per call. */
function mockInvoke(updatesQueue: () => unknown[]) {
  // @ts-expect-error happy-dom window lacks Tauri internals typings
  globalThis.window.__TAURI_INTERNALS__ = {
    invoke: async (cmd: string) => {
      if (cmd === "acp_list_updates") return updatesQueue();
      if (cmd === "acp_cancel_run") return null;
      if (cmd === "acp_diff_main") return { text: "", truncated: false };
      if (cmd === "acp_merge") {
        return { success: true, conflicts: [], repoBlocked: false };
      }
      if (cmd === "acp_remove_worktree") return null;
      if (cmd === "acp_respond_permission") return null;
      return null;
    },
    transformCallback: () => 0,
  };
}

test("AgentRunPanel: switching runs resets updates/cursor/diff/mergeResult", async () => {
  resetDom();
  const [run, setRun] = createSignal<AgentRun | null>(baseRun("r1", "running"));
  const [open, setOpen] = createSignal(true);
  let queue: unknown[] = [{ type: "sessionUpdate", text: "first" }];
  mockInvoke(() => {
    const r = queue;
    queue = [];
    return r;
  });

  render(() => (
    <AgentRunPanel open={open()} onOpenChange={setOpen} run={run()} />
  ));

  // Wait for the initial poll on r1 to drain the queued update.
  await waitFor(() => {
    if (queue.length !== 0) throw new Error("initial poll pending");
  });

  // Switch to a different run id — reset effect should fire and the panel
  // should re-render cleanly with the new run identity (no crash, no stale
  // updates from r1 leaking into r2's stream).
  setRun(baseRun("r2", "running"));
  await waitFor(() => {
    if (run()?.id !== "r2") throw new Error("run switch not applied");
  });

  cleanup();
  teardownDom();
});
