import { test, vi } from "vitest";
import { Show } from "solid-js";
import {
  cleanup,
  fireEvent,
  render,
  waitFor,
  within,
} from "@solidjs/testing-library";
import { Toast, Toaster } from "@ark-ui/solid/toast";
import Board from "../../Board.tsx";
import { toaster } from "../toaster.ts";
import { installDom, resetDom, teardownDom } from "./setup.ts";
import type { KanbanCard } from "../../../types.ts";

installDom();

const cardA: KanbanCard = {
  id: "a",
  title: "Card A",
  description: "",
  priority: "medium",
  column: "backlog",
  source: "local",
  position: 1,
  createdAt: "2024-01-01T00:00:00Z",
  updatedAt: "2024-01-01T00:00:00Z",
};
const cardB: KanbanCard = {
  id: "b",
  title: "Card B",
  description: "",
  priority: "low",
  column: "backlog",
  source: "local",
  position: 2,
  createdAt: "2024-01-01T00:00:00Z",
  updatedAt: "2024-01-01T00:00:00Z",
};

function ToasterMount() {
  return (
    <Toaster toaster={toaster}>
      {(t) => (
        <Toast.Root data-type={t().type}>
          <Toast.Title>{t().title}</Toast.Title>
          <Toast.Description>{t().description}</Toast.Description>
          <Show when={t().action}>
            <Toast.ActionTrigger>{t().action?.label}</Toast.ActionTrigger>
          </Show>
        </Toast.Root>
      )}
    </Toaster>
  );
}

test("EditModal race: switching cards while dirty prompts discard, no corruption", async () => {
  resetDom();
  toaster.dismiss();
  const internals: {
    invoke: (...a: unknown[]) => Promise<unknown>;
    convertFileSrc: (p: string) => string;
  } = {
    invoke: () => Promise.resolve(),
    convertFileSrc: (p) => `asset://localhost/${p}`,
  };
  (globalThis.window as unknown as { __TAURI_INTERNALS__: unknown })
    .__TAURI_INTERNALS__ = internals;
  const invokeStub = vi.spyOn(internals, "invoke").mockImplementation((
    ...args: unknown[]
  ) =>
    Promise.resolve(
      args[0] === "list_cards_by_column" &&
        (args[1] as { column?: string })?.column === "backlog"
        ? [cardA, cardB]
        : args[0] === "list_tree_sources"
        ? []
        : undefined,
    )
  );

  const { getByText, baseElement } = render(() => (
    <>
      <Board />
      <ToasterMount />
    </>
  ));
  const board = () => baseElement.querySelector("#main-board") as HTMLElement;
  const cardArticle = (title: string) =>
    within(board()).getByText(title).closest("article") as HTMLElement;
  const queryDialog = () =>
    baseElement.querySelector('[role="dialog"]') as HTMLElement | null;

  await waitFor(() => {
    if (!board().textContent?.includes("Card A")) {
      throw new Error("cards not loaded");
    }
  });
  (within(cardArticle("Card A")).getByText("Edit") as HTMLButtonElement)
    .click();
  await waitFor(() => {
    if (queryDialog() === null) throw new Error("modal did not open");
  });

  const dialog = queryDialog() as HTMLElement;
  const editTab = dialog.querySelector('[data-value="edit"]') as HTMLElement;
  editTab.click();
  await waitFor(() => {
    if (!dialog.querySelector("#edit-title")) {
      throw new Error("Title input not visible");
    }
  });
  fireEvent.input(dialog.querySelector("#edit-title") as HTMLInputElement, {
    target: { value: "Card A edited" },
  });

  (within(cardArticle("Card B")).getByText("Edit") as HTMLButtonElement)
    .click();
  await waitFor(() => {
    if (!baseElement.textContent?.includes("Discard unsaved changes?")) {
      throw new Error("discard toast for card A not shown");
    }
  });
  (getByText("Discard", { selector: "button" }) as HTMLButtonElement).click();

  await waitFor(() => {
    if (queryDialog() === null) {
      throw new Error("modal should remain on card B");
    }
  });
  if (!board().textContent?.includes("Card A")) {
    throw new Error("card A title corrupted in board");
  }
  if (!(queryDialog() as HTMLElement).textContent?.includes("Card B")) {
    throw new Error("modal should now edit card B");
  }

  invokeStub.mockRestore();
  cleanup();
  teardownDom();
});
