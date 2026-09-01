import {
  createEffect,
  createMemo,
  createSignal,
  For,
  on,
  onCleanup,
  Show,
} from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { DiffView } from "@git-diff-view/solid";
import { highlighter } from "../vendor/git-diff-lowlight.mjs";
import "@git-diff-view/solid/styles/diff-view.css";
import { toaster } from "./ui/toaster.ts";
import type {
  AcpUpdateEvent,
  AgentRun,
  DiffResult,
  MergeResult,
  RunUpdate,
} from "../types.ts";
import { safeListen } from "../event.ts";
import {
  acpCancelRun,
  acpDiffMain,
  acpErrorMessage,
  acpListUpdates,
  acpMerge,
  acpRemoveWorktree,
  acpResumeRun,
  acpSendFollowup,
  getAllSettings,
  setSetting,
} from "../db.ts";

export interface AgentRunPanelProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  run: AgentRun | null;
}

const STATUS_LABEL: Record<string, string> = {
  pending: "Queued",
  running: "Running",
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
};

/** Status → breathing-dot modifier class. */
const STATUS_DOT: Record<string, string> = {
  pending: "live",
  running: "live",
  completed: "ok",
  failed: "err",
  cancelled: "muted",
};

/** A single rendered entry in the chat thread. The raw RunUpdate union is
 * mapped to this display-only shape so the JSX switch stays flat and the
 * stream can also carry locally-sent user messages. */
type ThreadMsg =
  | { kind: "assistant"; text: string }
  | { kind: "user"; text: string }
  | { kind: "session"; sessionId: string }
  | { kind: "status"; text: string; tone: "ok" | "err" | "muted" }
  | { kind: "permission"; description: string }
  | { kind: "permTimeout" }
  | { kind: "waiting" };

/** Map a streamed RunUpdate to its display ThreadMsg. */
function updateToThread(u: RunUpdate): ThreadMsg | null {
  switch (u.type) {
    case "sessionUpdate":
      return { kind: "assistant", text: u.text };
    case "sessionId":
      return { kind: "session", sessionId: u.sessionId };
    case "completed":
      return {
        kind: "status",
        text: `Completed · ${u.stopReason}`,
        tone: "ok",
      };
    case "failed":
      return { kind: "status", text: `Failed · ${u.error}`, tone: "err" };
    case "cancelled":
      return { kind: "status", text: "Cancelled", tone: "muted" };
    case "permissionRequest":
      return { kind: "permission", description: u.description };
    case "permissionTimeout":
      return { kind: "permTimeout" };
    case "waitingForInput":
      return { kind: "waiting" };
    default:
      return null;
  }
}

/** Parse raw unified diff text into hunk strings for DiffView.
 * Each hunk starts with `@@` and includes all lines until the next `@@`
 * or end of text. */
function parseHunks(diffText: string): string[] {
  const lines = diffText.split("\n");
  const hunks: string[] = [];
  let current: string[] = [];
  for (const line of lines) {
    if (line.startsWith("@@")) {
      if (current.length > 0) hunks.push(current.join("\n"));
      current = [line];
    } else if (current.length > 0) {
      current.push(line);
    }
  }
  if (current.length > 0) hunks.push(current.join("\n"));
  return hunks;
}

/** Run status + updates stream + diff view + merge button + cancel button +
 * remove worktree button. Opens when clicking an AgentBadge. */
export default function AgentRunPanel(props: AgentRunPanelProps) {
  const [updates, setUpdates] = createSignal<ThreadMsg[]>([]);
  const [cursor, setCursor] = createSignal(0);
  const [diff, setDiff] = createSignal<DiffResult | null>(null);
  const [mergeResult, setMergeResult] = createSignal<MergeResult | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [showDiff, setShowDiff] = createSignal(false);
  const [diffMode, setDiffMode] = createSignal<"split" | "unified">("unified");
  // True when the agent stopped with EndTurn and is waiting for user input.
  const [waitingForInput, setWaitingForInput] = createSignal(false);
  const [followupText, setFollowupText] = createSignal("");
  const [sendingFollowup, setSendingFollowup] = createSignal(false);
  const [resuming, setResuming] = createSignal(false);
  // Permission requests are pushed to the module-level FIFO queue in
  // PermissionDialog.tsx; a single global dialog renders the queue head.
  // The panel keeps no local permission state.

  // Derived run identity + active flag. props.run is a fresh object on every
  // panelRun refresh (Board polls every 2s), and Solid tracks the props
  // getter read itself — an effect reading props.run re-fires on every
  // refresh even when .id/.status are unchanged. Memoize the scalar fields
  // so downstream effects depend on values, not object references.
  const runIdMemo = createMemo(() => props.run?.id ?? null);
  const runStatusMemo = createMemo(() => props.run?.status ?? null);
  const [runId, setRunId] = createSignal<string | null>(null);
  const [hasActive, setHasActive] = createSignal(false);

  // Resizable panel — mirrors Settings.tsx. Size persists per-user via
  // the generic settings key/value store (agent_run_w / agent_run_h).
  const [panelW, setPanelW] = createSignal(0);
  const [panelH, setPanelH] = createSignal(0);
  let panelEl: HTMLDivElement | undefined;
  let resizeState: { x: number; y: number; w: number; h: number } | null = null;

  let updatesEl: HTMLDivElement | undefined;
  // In-flight guard: prevents overlapping loadUpdates calls.
  let polling = false;

  // Mirror props.run identity/active into stable signals. Setting a signal
  // to an equal value is a no-op for downstream effects, so a panelRun
  // refresh with the same id+status does not retrigger the poll effect.
  createEffect(() => {
    setRunId(runIdMemo());
    setHasActive(
      runStatusMemo() === "pending" || runStatusMemo() === "running",
    );
  });

  // Restore saved panel size when the dialog opens.
  createEffect(() => {
    if (!props.open) return;
    void (async () => {
      try {
        const s = await getAllSettings();
        const w = parseInt(s["agent_run_w"] ?? "", 10);
        const h = parseInt(s["agent_run_h"] ?? "", 10);
        if (w > 0) setPanelW(w);
        if (h > 0) setPanelH(h);
      } catch { /* non-fatal: default size used */ }
    })();
  });

  function onResizeStart(e: PointerEvent) {
    e.preventDefault();
    e.stopPropagation();
    const el = panelEl;
    resizeState = {
      x: e.clientX,
      y: e.clientY,
      w: el?.offsetWidth ?? 672,
      h: el?.offsetHeight ?? 560,
    };
    window.addEventListener("pointermove", onResizeMove);
    window.addEventListener("pointerup", onResizeEnd);
  }
  function onResizeMove(e: PointerEvent) {
    if (!resizeState) return;
    const maxW = window.innerWidth * 0.9;
    const maxH = window.innerHeight * 0.9;
    const w = Math.min(
      Math.max(resizeState.w + (e.clientX - resizeState.x), 480),
      maxW,
    );
    const h = Math.min(
      Math.max(resizeState.h + (e.clientY - resizeState.y), 360),
      maxH,
    );
    setPanelW(w);
    setPanelH(h);
  }
  async function onResizeEnd() {
    window.removeEventListener("pointermove", onResizeMove);
    window.removeEventListener("pointerup", onResizeEnd);
    const w = panelW();
    const h = panelH();
    resizeState = null;
    if (w > 0 && h > 0) {
      try {
        await setSetting("agent_run_w", String(Math.round(w)));
        await setSetting("agent_run_h", String(Math.round(h)));
      } catch { /* non-fatal: size just won't persist */ }
    }
  }

  // Reset the stream/diff/merge state when switching to a different run.
  // `on` runs the effect whenever the id changes (including the first run).
  // Keys off runIdMemo, not props.run: the memo only fires downstream when
  // the id VALUE changes, so a same-id panelRun refresh does not wipe the
  // stream (see the memo comment above).
  createEffect(
    on(
      runIdMemo,
      () => {
        setUpdates([]);
        setCursor(0);
        setDiff(null);
        setMergeResult(null);
        setWaitingForInput(false);
        setFollowupText("");
      },
    ),
  );

  // Load buffered/persisted updates and subscribe to push events when the
  // panel opens. Events are filtered to the current run_id.
  createEffect(() => {
    const id = runId();
    const open = props.open;
    if (!open || !id) return;

    void loadUpdates(id);

    let unlisten: (() => void) | undefined;
    let cancelled = false;
    safeListen<AcpUpdateEvent>("acp:update", (event) => {
      if (cancelled) return;
      if (event.payload.runId !== id) return;
      applyUpdates([event.payload.update]);
    }).then((fn) => {
      if (cancelled) {
        fn();
        return;
      }
      unlisten = fn;
    });

    onCleanup(() => {
      cancelled = true;
      unlisten?.();
    });
  });

  async function loadUpdates(runId: string) {
    if (polling) return;
    polling = true;
    try {
      const newUpdates = await acpListUpdates(runId, cursor());
      if (!Array.isArray(newUpdates)) return;
      if (newUpdates.length > 0) {
        applyUpdates(newUpdates);
      } else if (updates().length === 0 && isTerminal() && props.run?.output) {
        // Buffer gone for a terminal run; show the persisted ACP output.
        setUpdates([{ kind: "assistant", text: props.run.output }]);
        setCursor(1);
      }
    } catch {
      // Non-fatal — event stream covers live updates.
    } finally {
      polling = false;
    }
  }

  function applyUpdates(newUpdates: RunUpdate[]) {
    const msgs = newUpdates
      .map(updateToThread)
      .filter((m): m is ThreadMsg => m !== null);
    setUpdates((prev) => [...prev, ...msgs]);
    setCursor((c) => c + newUpdates.length);
    // Auto-scroll to bottom.
    if (updatesEl) updatesEl.scrollTop = updatesEl.scrollHeight;
    for (const u of newUpdates) {
      if (u.type === "waitingForInput") {
        setWaitingForInput(true);
      } else if (
        u.type === "completed" || u.type === "failed" || u.type === "cancelled"
      ) {
        setWaitingForInput(false);
      }
    }
  }

  async function sendFollowup() {
    const id = runId();
    const text = followupText().trim();
    if (!id || !text) return;
    setSendingFollowup(true);
    try {
      await acpSendFollowup(id, text);
      setUpdates((prev) => [...prev, { kind: "user", text }]);
      setFollowupText("");
      setWaitingForInput(false);
    } catch (e) {
      toaster.error({
        title: "Send failed",
        description: acpErrorMessage(e),
      });
    } finally {
      setSendingFollowup(false);
    }
  }

  async function handleCancel() {
    const run = props.run;
    if (!run) return;
    setBusy(true);
    try {
      await acpCancelRun(run.id);
      toaster.success({ title: "Run cancelled" });
    } catch (e) {
      toaster.error({
        title: "Cancel failed",
        description: acpErrorMessage(e),
      });
    } finally {
      setBusy(false);
    }
  }

  async function handleDiff() {
    const run = props.run;
    if (!run) return;
    setBusy(true);
    try {
      const result = await acpDiffMain(run.cardId);
      setDiff(result);
      setShowDiff(true);
    } catch (e) {
      toaster.error({ title: "Diff failed", description: acpErrorMessage(e) });
    } finally {
      setBusy(false);
    }
  }

  async function handleMerge() {
    const run = props.run;
    if (!run) return;
    setBusy(true);
    try {
      const result = await acpMerge(run.cardId);
      setMergeResult(result);
      if (result.success) {
        toaster.success({ title: "Merged successfully" });
      } else {
        toaster.warning({
          title: "Merge conflicts",
          description: `${result.conflicts.length} file(s) in conflict`,
        });
      }
    } catch (e) {
      toaster.error({ title: "Merge failed", description: acpErrorMessage(e) });
    } finally {
      setBusy(false);
    }
  }

  async function handleRemoveWorktree() {
    const run = props.run;
    if (!run) return;
    setBusy(true);
    try {
      await acpRemoveWorktree(run.cardId);
      toaster.success({ title: "Worktree removed" });
    } catch (e) {
      toaster.error({
        title: "Remove failed",
        description: acpErrorMessage(e),
      });
    } finally {
      setBusy(false);
    }
  }

  async function handleResume() {
    const run = props.run;
    if (!run) return;
    setResuming(true);
    try {
      await acpResumeRun(run.id);
    } catch (e) {
      toaster.error({
        title: "Resume failed",
        description: acpErrorMessage(e),
      });
    } finally {
      setResuming(false);
    }
  }

  const isTerminal = () => {
    const s = props.run?.status;
    return s === "completed" || s === "failed" || s === "cancelled";
  };

  const skillsUsed = () => props.run?.skills ?? [];

  return (
    <Dialog.Root
      open={props.open}
      lazyMount
      unmountOnExit
      closeOnEscape
      onOpenChange={(e) => props.onOpenChange(e.open)}
    >
      <Portal>
        <Dialog.Backdrop class="agent-backdrop fixed inset-0 z-50" />
        <Dialog.Positioner class="fixed inset-0 z-50 flex items-stretch justify-center">
          <Dialog.Content
            ref={panelEl}
            class="agent-panel relative flex flex-col overflow-hidden"
            style={{ width: panelW() ? `${panelW()}px` : undefined }}
          >
            <header class="agent-header">
              <span
                class={`agent-dot agent-dot--${
                  STATUS_DOT[props.run?.status ?? ""] ?? "muted"
                }`}
                aria-hidden="true"
              />
              <div class="min-w-0 flex-1">
                <h2 class="text-sm font-semibold text-ink truncate leading-tight">
                  {props.run?.agentName ?? "…"}
                </h2>
                <p class="text-[11px] text-ink-secondary truncate">
                  {(props.run && STATUS_LABEL[props.run.status]) ?? "—"}
                  {" · "}
                  {props.run?.createdAt}
                </p>
              </div>
              <Show when={skillsUsed().length > 0}>
                <div class="hidden sm:flex flex-wrap gap-1 max-w-[40%]">
                  <For each={skillsUsed()}>
                    {(name) => <span class="agent-chip">{name}</span>}
                  </For>
                </div>
              </Show>
              <Show when={!isTerminal()}>
                <button
                  type="button"
                  class="agent-stop"
                  aria-label="Stop run"
                  disabled={busy()}
                  onClick={handleCancel}
                >
                  ■
                </button>
              </Show>
              <button
                type="button"
                class="text-xl text-ink-secondary hover:text-ink leading-none px-1"
                aria-label="Close"
                onClick={() => props.onOpenChange(false)}
              >
                ×
              </button>
            </header>

            <div ref={updatesEl} class="agent-stream board-scroll">
              <div class="agent-thread">
                <Show
                  when={updates().length > 0}
                  fallback={<p class="agent-empty">No output yet.</p>}
                >
                  <For each={updates()}>
                    {(m) => {
                      switch (m.kind) {
                        case "assistant":
                          return (
                            <article class="agent-turn">
                              <pre class="agent-turn-text">{m.text}</pre>
                            </article>
                          );
                        case "user":
                          return (
                            <article class="agent-turn-user">{m.text}</article>
                          );
                        case "session":
                          return (
                            <p class="agent-system">Session {m.sessionId}</p>
                          );
                        case "status":
                          return (
                            <p class={`agent-status agent-status--${m.tone}`}>
                              {m.text}
                            </p>
                          );
                        case "permission":
                          return (
                            <div class="agent-perm">
                              Permission requested — {m.description}
                            </div>
                          );
                        case "permTimeout":
                          return (
                            <div class="agent-perm agent-perm--timeout">
                              Permission timed out (auto-denied)
                            </div>
                          );
                        case "waiting":
                          return (
                            <p class="agent-system agent-system--waiting">
                              Waiting for your input
                            </p>
                          );
                        default:
                          return null;
                      }
                    }}
                  </For>
                </Show>

                <Show when={showDiff() && diff()}>
                  {(d) => {
                    const hunks = parseHunks(d().text);
                    const hasHunks = hunks.length > 0 &&
                      d().text.trim().length > 0;
                    return (
                      <article class="agent-turn">
                        <div class="flex items-center justify-between mb-1">
                          <p class="text-[11px] font-semibold text-ink-secondary">
                            Diff
                          </p>
                          <div class="flex items-center gap-2">
                            <Show when={hasHunks}>
                              <button
                                type="button"
                                class="text-[10px] px-1.5 py-0.5 rounded border border-border-subtle text-ink-secondary hover:text-ink transition-colors"
                                onClick={() =>
                                  setDiffMode((m) =>
                                    m === "split" ? "unified" : "split"
                                  )}
                              >
                                {diffMode() === "split" ? "Unified" : "Split"}
                              </button>
                            </Show>
                            <Show when={d().truncated}>
                              <span class="text-[10px] text-p-urgent">
                                truncated (1MB limit)
                              </span>
                            </Show>
                          </div>
                        </div>
                        <Show
                          when={hasHunks}
                          fallback={
                            <pre class="agent-turn-text text-ink-secondary">
                              (no changes)
                            </pre>
                          }
                        >
                          <div class="max-h-64 overflow-auto board-scroll">
                            <DiffView
                              data={{ hunks }}
                              registerHighlighter={highlighter}
                              diffViewMode={diffMode() === "split" ? 1 : 4}
                              diffViewHighlight={true}
                              diffViewFontSize={12}
                              diffViewWrap={true}
                            />
                          </div>
                        </Show>
                      </article>
                    );
                  }}
                </Show>

                <Show when={mergeResult()}>
                  {(r) => (
                    <div
                      class={`rounded-lg border p-3 text-sm ${
                        r().success
                          ? "border-col-done/40 bg-col-done/10"
                          : "border-p-urgent/40 bg-p-urgent/10"
                      }`}
                    >
                      <p
                        class={`font-semibold ${
                          r().success ? "text-col-done" : "text-p-urgent"
                        }`}
                      >
                        {r().success ? "Merge succeeded" : "Merge conflicts"}
                      </p>
                      <Show when={!r().success}>
                        <ul class="mt-1 text-xs text-ink-secondary">
                          <For each={r().conflicts}>
                            {(c) => <li class="font-mono">{c}</li>}
                          </For>
                        </ul>
                        <Show when={r().repoBlocked}>
                          <p class="text-xs text-p-urgent mt-1">
                            Repository is blocked — resolve conflicts in
                            terminal.
                          </p>
                        </Show>
                      </Show>
                    </div>
                  )}
                </Show>

                <Show when={props.run?.error}>
                  <div class="rounded-lg border border-p-urgent/40 bg-p-urgent/10 p-3">
                    <p class="text-xs text-p-urgent font-semibold">Error</p>
                    <p class="text-xs text-ink mt-1">{props.run?.error}</p>
                  </div>
                </Show>
              </div>
            </div>

            <footer class="agent-footer">
              <Show when={waitingForInput() && hasActive()}>
                <div class="agent-composer">
                  <div class="agent-composer-pill">
                    <textarea
                      placeholder="Reply to the agent…"
                      value={followupText()}
                      rows={1}
                      onInput={(e) => {
                        setFollowupText(e.currentTarget.value);
                        const el = e.currentTarget;
                        el.style.height = "auto";
                        el.style.height = `${Math.min(el.scrollHeight, 128)}px`;
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && !e.shiftKey) {
                          e.preventDefault();
                          void sendFollowup();
                        }
                      }}
                      disabled={sendingFollowup()}
                      class="agent-composer-input"
                    />
                    <button
                      type="button"
                      class="agent-send"
                      aria-label="Send"
                      onClick={() => void sendFollowup()}
                      disabled={sendingFollowup() || !followupText().trim()}
                    >
                      ↑
                    </button>
                  </div>
                </div>
              </Show>
              <Show when={isTerminal()}>
                <div class="agent-actionbar">
                  <button
                    type="button"
                    disabled={busy()}
                    onClick={handleDiff}
                  >
                    View diff
                  </button>
                  <Show when={props.run?.status === "completed"}>
                    <button
                      type="button"
                      class="agent-actionbar-primary"
                      disabled={busy()}
                      onClick={handleMerge}
                    >
                      Merge
                    </button>
                  </Show>
                  <button
                    type="button"
                    disabled={busy()}
                    onClick={handleRemoveWorktree}
                  >
                    Remove worktree
                  </button>
                </div>
              </Show>
              <Show when={!isTerminal() && !waitingForInput()}>
                <div class="agent-actionbar">
                  <button
                    type="button"
                    disabled={resuming()}
                    onClick={() => void handleResume()}
                  >
                    {resuming() ? "Resuming…" : "Resume"}
                  </button>
                </div>
              </Show>
            </footer>

            <div
              class="settings-grip"
              onPointerDown={onResizeStart}
              onKeyDown={(e) => {
                const step = e.shiftKey ? 20 : 5;
                if (e.key === "ArrowRight" || e.key === "ArrowDown") {
                  e.preventDefault();
                  setPanelW((w) => Math.min(w + step, window.innerWidth * 0.9));
                  setPanelH((h) =>
                    Math.min(h + step, window.innerHeight * 0.9)
                  );
                } else if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
                  e.preventDefault();
                  setPanelW((w) => Math.max(w - step, 480));
                  setPanelH((h) => Math.max(h - step, 360));
                }
              }}
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize agent run panel"
              tabindex={0}
            />
          </Dialog.Content>
        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
  );
}
