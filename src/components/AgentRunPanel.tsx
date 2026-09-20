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
import { STATUS_LABEL } from "./ui/consts.ts";
import { panelResize } from "./ui/panelResize.ts";
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
  acpCompleteRun,
  acpDiffMain,
  acpErrorMessage,
  acpListUpdates,
  acpLoadRunHistory,
  acpMerge,
  acpRemoveWorktree,
  acpResumeRun,
  acpRunProcessInfo,
  acpSendFollowup,
  acpSetSessionConfig,
  getAllSettings,
} from "../db.ts";

export interface AgentRunPanelProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  run: AgentRun | null;
}

/** Status → breathing-dot modifier class. */
const STATUS_DOT: Record<string, string> = {
  pending: "live",
  running: "live",
  completed: "ok",
  failed: "err",
  cancelled: "muted",
};

/** One rendered entry in the transcript. The raw RunUpdate union maps
 * to this display-only shape so the JSX switch stays flat and the stream
 * can also carry locally-sent user messages. */
type ThreadMsg =
  | { kind: "assistant"; text: string }
  | { kind: "tool"; text: string }
  | { kind: "user"; text: string }
  | { kind: "session"; sessionId: string }
  | { kind: "status"; text: string; tone: "ok" | "err" | "muted" }
  | { kind: "permission"; description: string }
  | { kind: "permTimeout" }
  | { kind: "waiting" };

function updateToThread(u: RunUpdate): ThreadMsg | null {
  switch (u.type) {
    case "sessionUpdate": {
      // Tool calls / thoughts render as dim inline lines, agent text as
      // the plain stream.
      if (u.text.startsWith("🔧") || u.text.startsWith("💭")) {
        return { kind: "tool", text: u.text };
      }
      return { kind: "assistant", text: u.text };
    }
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
    case "restoringContext":
      return { kind: "status", text: "Restoring context…", tone: "muted" };
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
 * Each hunk starts with `@@` and runs until the next `@@` or end of text. */
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

/** 3661 → "1h 1m 1s"; 65 → "1m 5s". */
function formatElapsed(secs: number): string {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  return [h > 0 ? `${h}h` : null, h > 0 || m > 0 ? `${m}m` : null, `${s}s`]
    .filter(Boolean)
    .join(" ");
}

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
  // On-the-fly session config (model / effort) for the live run.
  const [liveModel, setLiveModel] = createSignal("");
  const [liveEffort, setLiveEffort] = createSignal("");
  const [configBusy, setConfigBusy] = createSignal(false);
  // Permission requests go to the module-level FIFO queue in
  // PermissionDialog.tsx; a single global dialog renders the queue head. The
  // panel keeps no local state.

  // props.run is a fresh object each poll refresh; Solid tracks the props
  // getter, not values — memoize scalars so effects fire on change.
  const runIdMemo = createMemo(() => props.run?.id ?? null);
  const runStatusMemo = createMemo(() => props.run?.status ?? null);
  const [runId, setRunId] = createSignal<string | null>(null);
  const [hasActive, setHasActive] = createSignal(false);

  // Resizable panel — shared helper. Size persists per-user via
  // the generic settings key/value store (agent_run_w / agent_run_h).
  const resize = panelResize("agent_run_w", "agent_run_h", 672, 560, 480, 360);

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
        setLiveModel("");
        setLiveEffort("");
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
      } else if (updates().length === 0) {
        // Buffer gone (app restart) → load the persisted transcript.
        try {
          const history = await acpLoadRunHistory(runId);
          if (Array.isArray(history) && history.length > 0) {
            applyUpdates(history);
            return;
          }
        } catch {
          // Non-fatal — fall through to the output fallback.
        }
        if (isTerminal() && props.run?.output) {
          // No persisted stream either (pre-migration run); show the
          // accumulated ACP output.
          setUpdates([{ kind: "assistant", text: props.run.output }]);
          setCursor(1);
        }
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
    setUpdates((prev) => {
      const next = [...prev, ...msgs];
      // Scrollback cap: drop oldest entries beyond ~2000.
      return next.length > 2000 ? next.slice(next.length - 2000) : next;
    });
    setCursor((c) => c + newUpdates.length);
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

  async function applySessionConfig(configId: string, value: string) {
    const id = runId();
    if (!id || !value.trim()) return;
    setConfigBusy(true);
    try {
      await acpSetSessionConfig(id, configId, value.trim());
      toaster.success({ title: `Applied ${configId}: ${value.trim()}` });
    } catch (e) {
      toaster.error({
        title: `Set ${configId} failed`,
        description: acpErrorMessage(e),
      });
    } finally {
      setConfigBusy(false);
    }
  }

  async function runAction(
    failTitle: string,
    fn: (run: AgentRun) => Promise<void>,
  ) {
    const run = props.run;
    if (!run) return;
    setBusy(true);
    try {
      await fn(run);
    } catch (e) {
      toaster.error({ title: failTitle, description: acpErrorMessage(e) });
    } finally {
      setBusy(false);
    }
  }

  const handleCancel = () =>
    runAction("Cancel failed", async (run) => {
      await acpCancelRun(run.id);
      toaster.success({ title: "Run cancelled" });
    });

  /** Check the agent subprocess: append pid + uptime as a status line. */
  const handleCheckProcess = () =>
    runAction("Check failed", async (run) => {
      const info = await acpRunProcessInfo(run.id);
      const text = info.pid
        ? `Agent running · PID ${info.pid} · ${formatElapsed(info.elapsedSecs)}`
        : `No agent process found · status=${info.status} · ${formatElapsed(info.elapsedSecs)}`;
      setUpdates((prev) => [
        ...prev,
        { kind: "status", text, tone: info.pid ? "ok" : "muted" },
      ]);
      if (updatesEl) updatesEl.scrollTop = updatesEl.scrollHeight;
    });

  const handleDone = () =>
    runAction("Done failed", async (run) => {
      await acpCompleteRun(run.id);
      toaster.success({ title: "Run completed" });
    });

  const handleDiff = () =>
    runAction("Diff failed", async (run) => {
      setDiff(await acpDiffMain(run.cardId));
      setShowDiff(true);
    });

  const handleMerge = () =>
    runAction("Merge failed", async (run) => {
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
    });

  const handleRemoveWorktree = () =>
    runAction("Remove failed", async (run) => {
      await acpRemoveWorktree(run.cardId);
      toaster.success({ title: "Worktree removed" });
    });

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
            ref={resize.ref}
            class="agent-panel relative flex flex-col overflow-hidden"
            style={{ width: resize.panelW() ? `${resize.panelW()}px` : undefined }}
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
                  aria-label="Check agent process"
                  title="Check agent process (pid + uptime)"
                  disabled={busy()}
                  onClick={handleCheckProcess}
                >
                  ?
                </button>
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
                          return <pre class="agent-turn-text">{m.text}</pre>;
                        case "tool":
                          return <p class="agent-tool">{m.text}</p>;
                        case "user":
                          return (
                            <p class="agent-turn-user">{`❯ ${m.text}`}</p>
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
                    <button
                      type="button"
                      class="agent-done"
                      data-testid="agent-done"
                      disabled={busy()}
                      onClick={handleDone}
                    >
                      Done
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
              <Show when={hasActive()}>
                <div class="agent-actionbar items-center gap-2">
                  <input
                    type="text"
                    class="text-xs rounded px-2 py-1 bg-base text-ink border border-border-subtle outline-none focus:border-accent min-w-0 flex-1"
                    placeholder="Model (e.g. opus)"
                    value={liveModel()}
                    onInput={(e) => setLiveModel(e.currentTarget.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        void applySessionConfig("model", liveModel());
                      }
                    }}
                    disabled={configBusy()}
                  />
                  <input
                    type="text"
                    class="text-xs rounded px-2 py-1 bg-base text-ink border border-border-subtle outline-none focus:border-accent min-w-0 flex-1"
                    placeholder="Effort (e.g. high)"
                    value={liveEffort()}
                    onInput={(e) => setLiveEffort(e.currentTarget.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        void applySessionConfig("effort", liveEffort());
                      }
                    }}
                    disabled={configBusy()}
                  />
                  <button
                    type="button"
                    disabled={configBusy() || (!liveModel().trim() && !liveEffort().trim())}
                    onClick={() => {
                      if (liveModel().trim()) {
                        void applySessionConfig("model", liveModel());
                      }
                      if (liveEffort().trim()) {
                        void applySessionConfig("effort", liveEffort());
                      }
                    }}
                  >
                    Apply
                  </button>
                </div>
              </Show>
            </footer>

            <div
              class="settings-grip"
              {...resize.gripProps("Resize agent run panel")}
            />
          </Dialog.Content>
        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
  );
}
