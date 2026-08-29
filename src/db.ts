import { invoke } from '@tauri-apps/api/core';
import type {
  ColumnId,
  ConflictResolution,
  KanbanCard,
  SourceInstance,
  SourceTypeMeta,
  SyncResult,
  TreeSource,
} from './types.ts';

export async function listCards(): Promise<KanbanCard[]> {
  return invoke<KanbanCard[]>('list_cards');
}

/** Cards for a single column, ordered by position. Used by the per-column
 * lazy fetch so each column loads (and shows its loading state) on its own. */
export async function listCardsByColumn(column: ColumnId): Promise<KanbanCard[]> {
  return invoke<KanbanCard[]>('list_cards_by_column', { column });
}

export async function createLocalCard(title: string, column: ColumnId): Promise<KanbanCard> {
  return invoke<KanbanCard>('create_local_card', { title, column });
}

export async function updateCard(
  id: string,
  patch: Partial<
    Pick<KanbanCard, 'title' | 'description' | 'priority' | 'column' | 'sourceStatus' | 'treeSourceId'>
  >,
) {
  await invoke('update_card', { id, ...patch });
}

/** Move a card to a column. Omit `position` to append at the end of the
 * target column (the Rust side computes max position + 1). */
export async function moveCard(id: string, column: ColumnId, position?: number) {
  await invoke('move_card', { id, column, position: position ?? null });
}

export async function deleteCard(id: string) {
  await invoke('delete_card', { id });
}

/** Remove every card sourced from `source` and its sync snapshots. Local cards stay. */
export async function deleteAllSourceCards(source: string): Promise<void> {
  await invoke('delete_all_source_cards', { source });
}

export async function listSources(): Promise<SourceInstance[]> {
  return invoke<SourceInstance[]>('list_sources');
}

export async function addSource(
  sourceType: string,
  label: string,
  config: Record<string, unknown>,
  statusMapping: SourceInstance['statusMapping'],
  enabled: boolean,
): Promise<SourceInstance> {
  return invoke<SourceInstance>('add_source', {
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
  statusMapping: SourceInstance['statusMapping'],
  enabled: boolean,
): Promise<void> {
  await invoke('update_source', { id, label, config, statusMapping, enabled });
}

/** Delete a source instance (and its cards/snapshots). */
export async function deleteSource(id: string): Promise<void> {
  await invoke('delete_source', { id });
}

export async function listSourceTypes(): Promise<SourceTypeMeta[]> {
  return invoke<SourceTypeMeta[]>('list_source_types');
}

/** Run a sync against one source instance; returns conflicts + unmapped statuses. */
export async function syncSource(sourceId: string): Promise<SyncResult> {
  return invoke<SyncResult>('sync_source', { sourceId });
}

export async function resolveConflicts(
  sourceId: string,
  resolutions: ConflictResolution[],
): Promise<void> {
  await invoke('resolve_conflicts', { sourceId, resolutions });
}

export async function getSetting(key: string): Promise<string | null> {
  return invoke<string | null>('get_setting', { key });
}

export async function setSetting(key: string, value: string) {
  await invoke('set_setting', { key, value });
}

export async function getAllSettings(): Promise<Record<string, string>> {
  return invoke<Record<string, string>>('get_all_settings');
}

export async function listTreeSources(): Promise<TreeSource[]> {
  return invoke<TreeSource[]>('list_tree_sources');
}

export async function addTreeSource(label: string, path: string, editorCommand?: string): Promise<void> {
  await invoke('add_tree_source', { label, path, editorCommand });
}

export async function updateTreeSource(id: string, label: string, path: string, editorCommand?: string): Promise<void> {
  await invoke('update_tree_source', { id, label, path, editorCommand });
}

export async function deleteTreeSource(id: string): Promise<void> {
  await invoke('delete_tree_source', { id });
}
