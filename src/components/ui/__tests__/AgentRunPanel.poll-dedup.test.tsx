import { test } from "vitest";
import { createSignal } from "solid-js";
import { cleanup, render, waitFor } from "@solidjs/testing-library";
import AgentRunPanel from "../../AgentRunPanel.tsx";
import { installDom, resetDom, teardownDom } from "./setup.ts";
import type { AgentRun } from "../../../types.ts";

installDom();

const baseRun = (status: string): AgentRun => ({
  id: "rd",
  cardId: "c1",
  agentName: "tester",
  sessionId: null,
  worktreePath: "/tmp/wt",
  branch: "branch-d",
  status,
  output: null,
  stopReason: null,
  error: null,
  mergedAt: null,
  skills: [],
  createdAt: "2024-01-01T00:00:00Z",
  finishedAt: null,
});

/** Asserts the in-flight guard: even if acp_list_updates resolves slowly and
 * the interval fires multiple times before it settles, each update batch is
 * appended exactly once (no double-append from overlapping polls). */
test("AgentRunPanel.pollUpdates: in-flight guard prevents double-append", async () => {
  resetDom();
  const [open, setOpen] = createSignal(true);

  // Track in-flight + total acp_list_updates invocations.
  let inFlight = false;
  let invocations = 0;
  let appended = 0;
  // Gate of pending resolvers so we can release them all at once.
  let gate: (() => void)[] = [];
  const queue: unknown[] = [{ type: "sessionUpdate", text: "batch-1" }];

  // @ts-expect-error happy-dom window lacks Tauri internals typings
  globalThis.window.__TAURI_INTERNALS__ = {
    invoke: async (cmd: string) => {
      if (cmd !== "acp_list_updates") return null;
      invocations++;
      // If a previous poll is still in flight, the guard should have
      // short-circuited before reaching invoke — so invocations should
      // never exceed 1 while in flight.
      if (inFlight) throw new Error("guard leaked: overlapping invoke");
      inFlight = true;
      const batch = queue.splice(0, queue.length);
      appended += batch.length;
      await new Promise<void>((resolve) => gate.push(resolve));
      inFlight = false;
      return batch;
    },
    transformCallback: () => 0,
  };

  const [run] = createSignal<AgentRun | null>(baseRun("running"));
  render(() => (
    <AgentRunPanel open={open()} onOpenChange={setOpen} run={run()} />
  ));

  // Wait for the first poll to be in flight (invoked once). Poll interval is
  // 1000ms and vitest waitFor defaults to a 1000ms timeout — race, so give it
  // a generous explicit timeout.
  await waitFor(
    () => {
      if (invocations !== 1) {
        throw new Error(`expected 1 invoke, got ${invocations}`);
      }
    },
    { timeout: 4000 },
  );
  // Let the interval fire a few more times while the first is still pending.
  await new Promise((r) => setTimeout(r, 1200));
  // Still only one in-flight invoke — the guard blocked the rest.
  if (invocations !== 1) {
    throw new Error(`guard failed: ${invocations} invocations`);
  }

  // Release the pending poll.
  gate.forEach((fn) => fn());
  gate = [];
  await waitFor(() => {
    if (appended !== 1) throw new Error(`expected 1 appended, got ${appended}`);
  });

  // Close the panel so the poll effect's onCleanup clears the interval,
  // then release any final pending poll before teardown.
  setOpen(false);
  await new Promise((r) => setTimeout(r, 50));
  gate.forEach((fn) => fn());
  gate = [];

  cleanup();
  teardownDom();
});
