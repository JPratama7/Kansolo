import { test } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@solidjs/testing-library";
import App from "../../../App.tsx";
import { installDom, resetDom, teardownDom } from "./setup.ts";

installDom();

/** Asserts the sync loop pauses on the first conflicting source and resumes
 * after the user resolves — i.e. sync_source is invoked for BOTH sources,
 * resolve_conflicts is invoked for the first, and set_setting('last_synced_at')
 * is called with the last completed source's syncedAt ('t2' — the captured
 * conflicting source's 't1' is the base that later completed sources
 * overwrite), never a fresh now(). */
test("App.handleSync: pauses on conflict, resumes after resolve with captured syncedAt", async () => {
  resetDom();

  const calls: Record<string, unknown[]> = {};
  const record = (cmd: string, args?: unknown) => {
    (calls[cmd] ??= []).push(args);
  };

  // sync_source returns conflicts for source 1, success for source 2.
  const syncResponses: Record<string, unknown> = {
    "s1": {
      conflicts: [{
        sourceRef: "JIRA-1",
        card: {
          id: "c1",
          title: "local",
          description: "",
          priority: "medium",
          column: "backlog",
          source: "jira",
          position: 1,
          createdAt: "",
          updatedAt: "",
        },
        conflicts: [{ field: "title", local: "local", remote: "remote" }],
        remote: {
          id: "c1",
          title: "remote",
          description: "",
          priority: "medium",
          column: "backlog",
          source: "jira",
          position: 1,
          createdAt: "",
          updatedAt: "",
        },
      }],
      unmappedStatuses: [],
      syncedAt: "t1",
      importedCount: 1,
    },
    "s2": {
      conflicts: [],
      unmappedStatuses: [],
      syncedAt: "t2",
      importedCount: 2,
    },
  };

  // @ts-expect-error happy-dom window lacks Tauri internals typings
  globalThis.window.__TAURI_INTERNALS__ = {
    invoke: async (cmd: string, args?: Record<string, unknown>) => {
      record(cmd, args);
      switch (cmd) {
        case "list_sources":
          return [
            {
              id: "s1",
              sourceType: "jira",
              label: "Jira A",
              config: {},
              statusMapping: { backlog: [], ongoing: [], done: [] },
              enabled: true,
              createdAt: "",
            },
            {
              id: "s2",
              sourceType: "jira",
              label: "Jira B",
              config: {},
              statusMapping: { backlog: [], ongoing: [], done: [] },
              enabled: true,
              createdAt: "",
            },
          ];
        case "sync_source":
          return syncResponses[args!.sourceId as string];
        case "resolve_conflicts":
          return null;
        case "set_setting":
          return null;
        case "get_setting":
          return null;
        case "list_cards_by_column":
          return [];
        case "list_tree_sources":
          return [];
        case "acp_list_active_runs":
          return [];
        default:
          return null;
      }
    },
    transformCallback: () => 0,
  };

  const { getByTestId, baseElement } = render(() => <App />);

  // Click Sync.
  fireEvent.click(getByTestId("sync-button"));
  // Wait for the merge modal to appear (conflict on s1 pauses the loop).
  await waitFor(() => {
    if (!calls["sync_source"] || calls["sync_source"].length !== 1) {
      throw new Error("sync_source should be called once (paused at s1)");
    }
  });
  // s2 must NOT have been synced yet — loop is paused before resolve.
  const syncArgs1 = calls["sync_source"].map((a) =>
    (a as { sourceId: string }).sourceId
  );
  if (syncArgs1[0] !== "s1") {
    throw new Error(`first sync should be s1, got ${syncArgs1[0]}`);
  }
  if (calls["resolve_conflicts"]) {
    throw new Error("resolve should not run before apply-merge");
  }

  // Wait for the lazy-mounted MergeModal content (portaled to body) to
  // render its apply button, then click it directly.
  await waitFor(() => {
    if (!baseElement.querySelector('[data-testid="apply-merge"]')) {
      throw new Error("apply-merge button not mounted yet");
    }
  });
  fireEvent.click(
    baseElement.querySelector('[data-testid="apply-merge"]') as HTMLElement,
  );

  // Post-resolve, resolve_conflicts runs for s1, then s2 is synced, and
  // last_synced_at is set to 't1' (the conflicting source's syncedAt,
  // captured when the loop paused — not a fresh now()).
  await waitFor(() => {
    const syncArgs = calls["sync_source"].map((a) =>
      (a as { sourceId: string }).sourceId
    );
    if (syncArgs.length !== 2 || syncArgs[1] !== "s2") {
      throw new Error(
        `loop did not resume to s2; syncs=${JSON.stringify(syncArgs)}`,
      );
    }
    if (
      !calls["resolve_conflicts"] || calls["resolve_conflicts"].length !== 1
    ) {
      throw new Error("resolve_conflicts should be called once for s1");
    }
  });
  // finishSync awaits reload() (column fetches) before set_setting — wait
  // for the setting write rather than reading immediately after sync #2.
  await waitFor(() => {
    const setArgs = calls["set_setting"]?.map((
      a,
    ) => (a as { key: string; value: string }));
    const lastSynced = setArgs?.find((a) => a.key === "last_synced_at");
    if (!lastSynced) throw new Error("last_synced_at never set");
    // s2 completed last, so its syncedAt wins over the captured 't1'.
    if (lastSynced.value !== "t2") {
      throw new Error(
        `expected last completed syncedAt 't2', got '${lastSynced.value}'`,
      );
    }
  });

  cleanup();
  teardownDom();
});
