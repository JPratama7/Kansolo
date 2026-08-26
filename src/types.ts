export type ColumnId = 'backlog' | 'ongoing' | 'done';

/** Ordered low → urgent. Index in `PRIORITIES` reflects severity. */
export type Priority = 'low' | 'medium' | 'high' | 'urgent';

export const PRIORITIES: readonly Priority[] = ['low', 'medium', 'high', 'urgent'];

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
  sourcePath?: string;
  createdAt: string;
  updatedAt: string;
}

export type StatusMapping = Record<'backlog' | 'ongoing' | 'done', string[]>;

/** A registered git tree source folder, selectable from the card edit dropdown. */
export interface TreeSource {
  id: string;
  label: string;
  path: string;
  /** Optional per-source editor command; falls back to the global setting. */
  editorCommand?: string;
}

/** A registered external source instance (Jira project, GitHub repo, etc.). */
export interface SourceInstance {
  id: string;
  sourceType: string;
  label: string;
  config: Record<string, unknown>;
  statusMapping: StatusMapping;
  enabled: boolean;
  createdAt: string;
}

/** Metadata describing a pluggable source type (e.g. jira, github). */
export interface SourceTypeMeta {
  source_type: string;
  label: string;
}

/** One field where local and remote both diverged from the snapshot. */
interface FieldConflict {
  field: string;
  local: string;
  remote: string;
}

/** A card whose local and remote state both diverged from the snapshot. */
export interface SyncConflict {
  sourceRef: string;
  card: KanbanCard;
  conflicts: FieldConflict[];
  remote: KanbanCard;
}

/** Outcome of a sync run against one source instance. */
export interface SyncResult {
  conflicts: SyncConflict[];
  unmappedStatuses: string[];
  syncedAt: string;
}

/** Per-card field choices for resolving a batch of sync conflicts. */
export interface ConflictResolution {
  sourceRef: string;
  choices: Record<string, 'local' | 'remote'>;
}

export interface McpStatus {
  running: boolean;
  port: number | null;
}
