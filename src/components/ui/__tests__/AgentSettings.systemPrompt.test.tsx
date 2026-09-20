import { test } from "vitest";
import { cleanup, render, waitFor } from "@solidjs/testing-library";
import AcpSettings from "../../settings/AcpSettings.tsx";
import AgentRegistry from "../../settings/AgentRegistry.tsx";
import { installDom, resetDom, teardownDom } from "./setup.ts";

installDom();

const agent = {
  name: "tester",
  command: "echo hi",
  description: "Test",
  builtIn: false,
  enabled: true,
  skills: [],
  createdAt: "2024-01-01T00:00:00Z",
  model: null,
  effort: null,
  systemPrompt: "agent-level prompt",
};

test("AcpSettings: loads acp_system_prompt and saves it back", async () => {
  resetDom();
  const settings: Record<string, string> = {
    acp_system_prompt: "global prompt text",
  };
  const setCalls: [string, string][] = [];
  // @ts-expect-error happy-dom window lacks Tauri internals typings
  globalThis.window.__TAURI_INTERNALS__ = {
    invoke: async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "get_all_settings") return settings;
      if (cmd === "set_setting") {
        setCalls.push([args!.key as string, args!.value as string]);
        settings[args!.key as string] = args!.value as string;
        return null;
      }
      return null;
    },
    transformCallback: () => 0,
  };

  const { container } = render(() => <AcpSettings />);
  const ta = container.querySelector(
    "#acp-system-prompt",
  ) as HTMLTextAreaElement;
  await waitFor(() => {
    if (ta.value !== "global prompt text") {
      throw new Error(`system prompt not loaded, got: ${ta.value}`);
    }
  });

  ta.value = "updated global prompt";
  ta.dispatchEvent(new Event("input", { bubbles: true }));
  await waitFor(() => {
    if (ta.value !== "updated global prompt") throw new Error("input not applied");
  });

  (container.querySelector("button[type=button]") as HTMLButtonElement).click();
  await waitFor(() => {
    const call = setCalls.find(([k]) => k === "acp_system_prompt");
    if (!call || call[1] !== "updated global prompt") {
      throw new Error(`acp_system_prompt not saved: ${JSON.stringify(setCalls)}`);
    }
  });

  cleanup();
  teardownDom();
});

test("AgentRegistry: edit form loads systemPrompt and passes it to acp_update_agent", async () => {
  resetDom();
  const updateCalls: Record<string, unknown>[] = [];
  // @ts-expect-error happy-dom window lacks Tauri internals typings
  globalThis.window.__TAURI_INTERNALS__ = {
    invoke: async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "acp_list_agents") return [agent];
      if (cmd === "acp_list_skills") return [];
      if (cmd === "acp_list_active_runs") return [];
      if (cmd === "acp_update_agent") {
        updateCalls.push(args!);
        return null;
      }
      return null;
    },
    transformCallback: () => 0,
  };

  const { container } = render(() => <AgentRegistry />);
  // Open the edit form for the agent.
  await waitFor(() => {
    const edit = [...container.querySelectorAll("button")].find(
      (b) => b.textContent === "Edit",
    );
    if (!edit) throw new Error("edit button missing");
    (edit as HTMLButtonElement).click();
  });
  let ta: HTMLTextAreaElement | null = null;
  await waitFor(() => {
    ta = container.querySelector("#agent-system-prompt");
    if (!ta || (ta as HTMLTextAreaElement).value !== "agent-level prompt") {
      throw new Error("systemPrompt not loaded yet");
    }
  });

  ta!.value = "new agent prompt";
  ta!.dispatchEvent(new Event("input", { bubbles: true }));
  const submit = [...container.querySelectorAll("button")].find(
    (b) => b.textContent === "Update",
  );
  if (!submit) throw new Error("update button missing");
  (submit as HTMLButtonElement).click();
  await waitFor(() => {
    if (updateCalls.length === 0) throw new Error("acp_update_agent not called");
  });
  if (updateCalls[0].systemPrompt !== "new agent prompt") {
    throw new Error(
      `systemPrompt not passed through: ${JSON.stringify(updateCalls[0])}`,
    );
  }

  cleanup();
  teardownDom();
});
