export type ColumnId = "backlog" | "ongoing" | "done";

/** Priority values; the index in `PRIORITIES` is the severity rank. */
export type Priority = "low" | "medium" | "high" | "urgent";

export const PRIORITIES: readonly Priority[] = [
  "low",
  "medium",
  "high",
  "urgent",
];

export interface KanbanCard {
  id: string;
  title: string;
  description: string;
  priority: Priority;
  column: ColumnId;
  source: string;
  position: number;
  sourceRef?: string;
  sourceStatus?: string;
  treeSourceId?: string;
  /** Owning source instance id (FK → sources.id); null for local cards. */
  sourceInstanceId?: string;
  createdAt: string;
  updatedAt: string;
}

export type StatusMapping = Record<"backlog" | "ongoing" | "done", string[]>;

/** Git tree source folder, selectable from the card edit dropdown. */
export interface TreeSource {
  id: string;
  label: string;
  path: string;
  /** Optional per-source editor command; falls back to the global setting. */
  editorCommand?: string;
}

/** External source instance (Jira project, GitHub repo, etc.). */
export interface SourceInstance {
  id: string;
  sourceType: string;
  label: string;
  config: Record<string, unknown>;
  statusMapping: StatusMapping;
  enabled: boolean;
  createdAt: string;
}

/** Pluggable source type metadata (e.g. jira, github). */
export interface SourceTypeMeta {
  source_type: string;
  label: string;
}

/** Field where local and remote both diverged from the snapshot. */
interface FieldConflict {
  field: string;
  local: string;
  remote: string;
}

/** Card whose local and remote state both diverged from the snapshot. */
export interface SyncConflict {
  sourceRef: string;
  card: KanbanCard;
  conflicts: FieldConflict[];
  remote: KanbanCard;
}

/** Sync run result against one source instance. */
export interface SyncResult {
  conflicts: SyncConflict[];
  unmappedStatuses: string[];
  syncedAt: string;
  importedCount: number;
}

/** Per-card field choices for resolving a batch of sync conflicts. */
export interface ConflictResolution {
  sourceRef: string;
  choices: Record<string, "local" | "remote">;
}

export interface McpStatus {
  running: boolean;
  port: number | null;
}

/** Agent (built-in or custom) that can run against a card. */
export interface Agent {
  name: string;
  command: string;
  description: string;
  builtIn: boolean;
  enabled: boolean;
  skills: string[];
  createdAt: string;
}

/** Agent run against a card, tracked in the `agent_runs` table. */
export interface AgentRun {
  id: string;
  cardId: string;
  agentName: string;
  sessionId: string | null;
  worktreePath: string;
  branch: string;
  status: string;
  output: string | null;
  stopReason: string | null;
  error: string | null;
  mergedAt: string | null;
  skills: string[];
  createdAt: string;
  finishedAt: string | null;
}

/** Skill metadata, parsed from `SKILL.md` frontmatter. */
export interface SkillManifest {
  name: string;
  description: string;
  path: string;
}

/** Run updates, streamed to the GUI via Tauri events. */
export type RunUpdate =
  | { type: "sessionUpdate"; text: string }
  | { type: "sessionId"; sessionId: string }
  | { type: "completed"; output: string; stopReason: string }
  | { type: "failed"; error: string }
  | { type: "cancelled" }
  | {
    type: "permissionRequest";
    requestId: string;
    description: string;
    timeoutMs?: number;
  }
  | { type: "permissionTimeout" }
  | { type: "waitingForInput"; stopReason: string };

/** Tauri event payload for a single run update. */
export interface AcpUpdateEvent {
  runId: string;
  update: RunUpdate;
}

/** Tauri event payload when the active runs set changes. */
export interface AcpActiveRunsChangedEvent {
  runs: AgentRun[];
}

/** Diff between agent branch and main. */
export interface DiffResult {
  text: string;
  truncated: boolean;
}

/** Merge of an agent branch back into main. */
export interface MergeResult {
  success: boolean;
  conflicts: string[];
  repoBlocked: boolean;
}
