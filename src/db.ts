import { invoke } from "@tauri-apps/api/core";
import type {
  Agent,
  AgentRun,
  ColumnId,
  ConflictResolution,
  DiffResult,
  KanbanCard,
  MergeResult,
  RunProcessInfo,
  RunUpdate,
  SkillManifest,
  SourceInstance,
  SourceTypeMeta,
  SyncResult,
  TreeSource,
} from "./types.ts";

export async function listCards(): Promise<KanbanCard[]> {
  return invoke<KanbanCard[]>("list_cards");
}

/** Cards for one column, ordered by position. Drives the per-column
 * lazy fetch so each column loads (and shows its loading state) on its own. */
export async function listCardsByColumn(
  column: ColumnId,
): Promise<KanbanCard[]> {
  return invoke<KanbanCard[]>("list_cards_by_column", { column });
}

export async function createLocalCard(
  title: string,
  column: ColumnId,
): Promise<KanbanCard> {
  return invoke<KanbanCard>("create_local_card", { title, column });
}

export async function updateCard(
  id: string,
  patch: Partial<
    Pick<
      KanbanCard,
      | "title"
      | "description"
      | "priority"
      | "column"
      | "sourceStatus"
      | "treeSourceId"
    >
  >,
) {
  await invoke("update_card", { id, ...patch });
}

/** Move a card to a column. Omit `position` to append at the end of the
 * target column (the Rust side computes max position + 1). */
export async function moveCard(
  id: string,
  column: ColumnId,
  position?: number,
) {
  await invoke("move_card", { id, column, position: position ?? null });
}

export async function deleteCard(id: string) {
  await invoke("delete_card", { id });
}

/** Delete every card sourced from a source instance (looked up by its
 * `sources.id`) and its sync snapshots. Local cards stay. The Rust command
 * resolves the instance id → source_type inside one transaction. */
export async function deleteAllSourceCards(sourceId: string): Promise<void> {
  await invoke("delete_all_source_cards", { sourceId });
}

export async function listSources(): Promise<SourceInstance[]> {
  return invoke<SourceInstance[]>("list_sources");
}

export async function addSource(
  sourceType: string,
  label: string,
  config: Record<string, unknown>,
  statusMapping: SourceInstance["statusMapping"],
  enabled: boolean,
): Promise<SourceInstance> {
  return invoke<SourceInstance>("add_source", {
    sourceType,
    label,
    config,
    statusMapping,
    enabled,
  });
}

export async function updateSource(
  id: string,
  label: string,
  config: Record<string, unknown>,
  statusMapping: SourceInstance["statusMapping"],
  enabled: boolean,
): Promise<void> {
  await invoke("update_source", { id, label, config, statusMapping, enabled });
}

export async function deleteSource(id: string): Promise<void> {
  await invoke("delete_source", { id });
}

export async function listSourceTypes(): Promise<SourceTypeMeta[]> {
  return invoke<SourceTypeMeta[]>("list_source_types");
}

/** Sync one source instance; returns conflicts + unmapped statuses. */
export async function syncSource(sourceId: string): Promise<SyncResult> {
  return invoke<SyncResult>("sync_source", { sourceId });
}

export async function resolveConflicts(
  sourceId: string,
  resolutions: ConflictResolution[],
): Promise<void> {
  await invoke("resolve_conflicts", { sourceId, resolutions });
}

export async function getSetting(key: string): Promise<string | null> {
  return invoke<string | null>("get_setting", { key });
}

export async function setSetting(key: string, value: string) {
  await invoke("set_setting", { key, value });
}

export async function getAllSettings(): Promise<Record<string, string>> {
  return invoke<Record<string, string>>("get_all_settings");
}

export async function listTreeSources(): Promise<TreeSource[]> {
  return invoke<TreeSource[]>("list_tree_sources");
}

export async function addTreeSource(
  label: string,
  path: string,
  editorCommand?: string,
): Promise<void> {
  await invoke("add_tree_source", { label, path, editorCommand });
}

export async function updateTreeSource(
  id: string,
  label: string,
  path: string,
  editorCommand?: string,
): Promise<void> {
  await invoke("update_tree_source", { id, label, path, editorCommand });
}

export async function deleteTreeSource(id: string): Promise<void> {
  await invoke("delete_tree_source", { id });
}

/** Pull a human-readable message from an `AcpError` or thrown value. */
export function acpErrorMessage(e: unknown): string {
  if (
    e && typeof e === "object" && "message" in e &&
    typeof (e as { message: unknown }).message === "string"
  ) {
    return (e as { message: string }).message;
  }
  return e instanceof Error ? e.message : String(e);
}

export async function acpListAgents(): Promise<Agent[]> {
  return invoke<Agent[]>("acp_list_agents");
}

export async function acpRegisterAgent(
  name: string,
  command: string,
  description: string,
  skills: string[],
  model: string | null,
  effort: string | null,
  systemPrompt: string,
): Promise<void> {
  await invoke("acp_register_agent", {
    name,
    command,
    description,
    skills,
    model,
    effort,
    systemPrompt,
  });
}

export async function acpUpdateAgent(
  name: string,
  command: string,
  description: string,
  skills: string[],
  model: string | null,
  effort: string | null,
  systemPrompt: string,
): Promise<void> {
  await invoke("acp_update_agent", {
    name,
    command,
    description,
    skills,
    model,
    effort,
    systemPrompt,
  });
}

export async function acpDeleteAgent(
  name: string,
  deleteRuns: boolean,
): Promise<void> {
  await invoke("acp_delete_agent", { name, deleteRuns });
}

export async function acpListSkills(): Promise<SkillManifest[]> {
  return invoke<SkillManifest[]>("acp_list_skills");
}

export async function acpListActiveRuns(): Promise<AgentRun[]> {
  return invoke<AgentRun[]>("acp_list_active_runs");
}

export async function acpCreateRun(
  cardId: string,
  agentName: string,
  skillNames: string[],
  model: string | null = null,
  effort: string | null = null,
): Promise<AgentRun> {
  return invoke<AgentRun>("acp_create_run", {
    cardId,
    agentName,
    skillNames,
    model,
    effort,
  });
}

/** Change a session config option ("model" / "effort") on a live run. */
export async function acpSetSessionConfig(
  runId: string,
  configId: string,
  value: string,
): Promise<void> {
  await invoke("acp_set_session_config", { runId, configId, value });
}

export async function acpResumeRun(runId: string): Promise<AgentRun> {
  return invoke<AgentRun>("acp_resume_run", { runId });
}

/** Most recent run for a card, regardless of status (active or terminal).
 * UI renders the latest run badge/panel when no active run exists. */
export async function acpLatestRunForCard(
  cardId: string,
): Promise<AgentRun | null> {
  return invoke<AgentRun | null>("acp_latest_run_for_card", { cardId });
}

/** Recent runs (newest first), any status. Compact feed for UI status
 * panels; the Rust side defaults to a 20-row limit when `limit` is unset. */
export async function acpListRecentRuns(limit?: number): Promise<AgentRun[]> {
  return invoke<AgentRun[]>("acp_list_recent_runs", { limit });
}

/** Permission auto-deny timeout in ms. Cached to avoid repeated DB reads.
 * Call `invalidatePermissionTimeoutCache()` after updating the setting. */
let cachedPermissionTimeoutMs: number | null = null;
export function invalidatePermissionTimeoutCache() {
  cachedPermissionTimeoutMs = null;
}
export async function acpPermissionTimeoutMs(): Promise<number> {
  if (cachedPermissionTimeoutMs !== null) return cachedPermissionTimeoutMs;
  const raw = await getSetting("acp_permission_timeout");
  let seconds = 300;
  if (raw) {
    const parsed = Number.parseInt(raw, 10);
    if (Number.isFinite(parsed) && parsed > 0) seconds = parsed;
  }
  cachedPermissionTimeoutMs = seconds * 1000;
  return cachedPermissionTimeoutMs;
}

export async function acpListUpdates(
  runId: string,
  cursor: number,
): Promise<RunUpdate[]> {
  return invoke<RunUpdate[]>("acp_list_updates", { runId, cursor });
}

/** Full persisted transcript for a run (panel history after app restart). */
export async function acpLoadRunHistory(runId: string): Promise<RunUpdate[]> {
  return invoke<RunUpdate[]>("acp_load_run_history", { runId });
}

/** Live process info for a run's agent subprocess (pid + uptime). */
export async function acpRunProcessInfo(runId: string): Promise<RunProcessInfo> {
  return invoke<RunProcessInfo>("acp_run_process_info", { runId });
}

export async function acpCancelRun(runId: string): Promise<void> {
  await invoke("acp_cancel_run", { runId });
}

export async function acpCompleteRun(runId: string): Promise<void> {
  await invoke("acp_complete_run", { runId });
}

export async function acpSendFollowup(
  runId: string,
  text: string,
): Promise<void> {
  await invoke("acp_send_followup", { runId, text });
}

export async function acpRespondPermission(
  requestId: string,
  approved: boolean,
): Promise<void> {
  await invoke("acp_respond_permission", { requestId, approved });
}

export async function acpDiffMain(cardId: string): Promise<DiffResult> {
  return invoke<DiffResult>("acp_diff_main", { cardId });
}

export async function acpMerge(
  cardId: string,
  force?: boolean,
): Promise<MergeResult> {
  return invoke<MergeResult>("acp_merge", { cardId, force });
}

export async function acpRemoveWorktree(cardId: string): Promise<void> {
  await invoke("acp_remove_worktree", { cardId });
}

export async function acpDeleteRun(runId: string): Promise<void> {
  await invoke("acp_delete_run", { runId });
}
