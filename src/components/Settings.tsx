import { For, Show, createComponent, createMemo, createSignal, onMount } from 'solid-js';
import { Dynamic } from 'solid-js/web';
import { invoke } from '@tauri-apps/api/core';
import type { McpStatus, SourceInstance, SourceTypeMeta, StatusMapping, TreeSource } from '../types.ts';
import { DEFAULT_STATUS_MAPPING } from '../columns.ts';
import {
  addSource,
  addTreeSource,
  deleteSource,
  deleteTreeSource,
  getAllSettings,
  listSourceTypes,
  listSources,
  listTreeSources,
  setSetting,
  updateSource,
  updateTreeSource,
} from '../db.ts';
import { SETTINGS_REGISTRY } from './settings/registry.ts';

interface SettingsProps {
  onClose: () => void;
}

/** Coerce a stored setting string to a boolean; default false unless "true". */
function isTrue(v: string | undefined): boolean {
  return v === 'true';
}


export default function Settings(props: SettingsProps) {
  // --- Source instances (data-driven via listSourceTypes/listSources) ---
  const [sourceTypes, setSourceTypes] = createSignal<SourceTypeMeta[]>([]);
  const [sources, setSources] = createSignal<SourceInstance[]>([]);
  const [editing, setEditing] = createSignal<SourceInstance | null>(null);
  const [editLabel, setEditLabel] = createSignal('');
  const [editEnabled, setEditEnabled] = createSignal(true);
  // The settings component for the editing source's type (sync-loaded).
  const [EditComponent, setEditComponent] = createSignal<((p: {
    instance: SourceInstance;
    onSave: (config: Record<string, unknown>, statusMapping: StatusMapping) => void;
  }) => any) | null>(null);
  const [addPickerOpen, setAddPickerOpen] = createSignal(false);
  const [addType, setAddType] = createSignal<string>('');
  const [addLabel, setAddLabel] = createSignal('');

  // --- App-level (flat) settings: MCP, tray, editor, tree sources ---
  const [mcpEnabled, setMcpEnabled] = createSignal(false);
  const [mcpPort, setMcpPort] = createSignal(27816);
  const [mcpRunning, setMcpRunning] = createSignal(false);
  const [closeToTray, setCloseToTray] = createSignal(true);
  const [editorCommand, setEditorCommand] = createSignal('code');
  const [treeSources, setTreeSources] = createSignal<TreeSource[]>([]);
  const [newTreeLabel, setNewTreeLabel] = createSignal('');
  const [newTreePath, setNewTreePath] = createSignal('');
  const [newTreeEditor, setNewTreeEditor] = createSignal('');
  const [editingTree, setEditingTree] = createSignal<TreeSource | null>(null);
  const [editTreeLabel, setEditTreeLabel] = createSignal('');
  const [editTreePath, setEditTreePath] = createSignal('');
  const [editTreeEditor, setEditTreeEditor] = createSignal('');

  const [error, setError] = createSignal<string | null>(null);

  // Paired instance + component — both guaranteed non-null when truthy.
  // Avoids signal timing issues where editing() and EditComponent() could
  // be read at different times inside <Dynamic> props.
  const editView = createMemo(() => {
    const inst = editing();
    const comp = EditComponent();
    if (!inst || !comp) return null;
    return { inst, comp };
  });

  async function refreshSources() {
    setSources(await listSources());
  }

  onMount(async () => {
    setSourceTypes(await listSourceTypes());
    await refreshSources();
    const settings = await getAllSettings();
    setMcpEnabled(isTrue(settings['mcp_enabled']));
    setMcpPort(parseInt(settings['mcp_port'] ?? '27816', 10) || 27816);
    setCloseToTray(settings['close_to_tray'] !== 'false');
    setEditorCommand(settings['editor_command'] ?? 'code');
    try {
      const status = await invoke<McpStatus>('mcp_status');
      setMcpRunning(status.running);
    } catch {
      setMcpRunning(false);
    }
    setTreeSources(await listTreeSources());
  });

  /** Open the per-source editor: load the registry component for its type. */
  function startEdit(src: SourceInstance) {
    setError(null);
    setEditing(src);
    setEditLabel(src.label);
    setEditEnabled(src.enabled);
    setEditComponent(null);
    const Comp = SETTINGS_REGISTRY[src.sourceType];
    if (typeof Comp !== 'function') {
      setError(`No settings component registered for source type "${src.sourceType}".`);
      return;
    }
    setEditComponent(() => Comp as (p: {
      instance: SourceInstance;
      onSave: (config: Record<string, unknown>, statusMapping: StatusMapping) => void;
    }) => any);
  }

  /** Persist the edited source's config + status mapping + label + enabled. */
  async function handleSaveEdit(config: Record<string, unknown>, statusMapping: StatusMapping) {
    const src = editing();
    if (!src) return;
    const label = editLabel().trim() || src.label;
    try {
      await updateSource(src.id, label, config, statusMapping, editEnabled());
      setEditing(null);
      setEditComponent(null);
      await refreshSources();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function handleDeleteSource(id: string) {
    if (!window.confirm('Delete this source and all cards it sourced? Local cards stay.')) return;
    try {
      await deleteSource(id);
      if (editing()?.id === id) {
        setEditing(null);
        setEditComponent(null);
      }
      await refreshSources();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function handleToggleEnabled(src: SourceInstance, enabled: boolean) {
    try {
      await updateSource(src.id, src.label, src.config, src.statusMapping, enabled);
      await refreshSources();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function handleAddSource() {
    const type = addType().trim();
    const label = addLabel().trim();
    if (!type || !label) return;
    try {
      await addSource(type, label, {}, { ...DEFAULT_STATUS_MAPPING }, true);
      setAddPickerOpen(false);
      setAddType('');
      setAddLabel('');
      await refreshSources();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  /** Persist the app-level (flat) settings + apply MCP state, then close. */
  async function handleSaveAppSettings() {
    setError(null);
    try {
      await setSetting('mcp_enabled', mcpEnabled() ? 'true' : 'false');
      await setSetting('mcp_port', String(mcpPort()));
      await setSetting('close_to_tray', closeToTray() ? 'true' : 'false');
      await setSetting('editor_command', editorCommand().trim() || 'code');
      const status = await invoke<McpStatus>('mcp_apply', {
        enabled: mcpEnabled(),
        port: mcpPort(),
      });
      setMcpRunning(status.running);
      props.onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function handleAddTree() {
    const label = newTreeLabel().trim();
    const path = newTreePath().trim();
    if (!label || !path) return;
    await addTreeSource(label, path, newTreeEditor());
    setNewTreeLabel('');
    setNewTreePath('');
    setNewTreeEditor('');
    setTreeSources(await listTreeSources());
  }

  async function handleDeleteTree(id: string) {
    await deleteTreeSource(id);
    setTreeSources(await listTreeSources());
  }

  function startEditTree(src: TreeSource) {
    setEditingTree(src);
    setEditTreeLabel(src.label);
    setEditTreePath(src.path);
    setEditTreeEditor(src.editorCommand ?? '');
  }

  async function handleSaveEditTree() {
    const src = editingTree();
    if (!src) return;
    const label = editTreeLabel().trim();
    const path = editTreePath().trim();
    if (!label || !path) return;
    await updateTreeSource(src.id, label, path, editTreeEditor());
    setEditingTree(null);
    setTreeSources(await listTreeSources());
  }


  return (
    <div
      class="fixed inset-0 z-50 flex items-start justify-center pt-10 px-4 bg-black/50"
      role="dialog"
      aria-modal="true"
      onClick={props.onClose}
    >
      <section
        class="w-full max-w-2xl max-h-[85vh] overflow-y-auto board-scroll bg-surface rounded-[var(--radius-card)] border border-border-subtle shadow-2xl"
        aria-label="Settings"
        onClick={(e) => e.stopPropagation()}
      >
        <div class="sticky top-0 flex items-center justify-between px-4 py-3 bg-surface border-b border-border-subtle">
          <h2 class="text-base font-bold text-ink">Settings</h2>
          <button
            type="button"
            class="text-xl text-ink-secondary hover:text-ink leading-none px-1"
            aria-label="Close"
            onClick={props.onClose}
          >
            ×
          </button>
        </div>

        <div class="p-4 flex flex-col gap-4">
          {/* --- Source instances --- */}
          <fieldset class="border border-border-subtle rounded-[var(--radius-card)] p-3">
            <legend class="text-xs font-semibold text-ink-secondary px-1">Sources</legend>
            <div class="flex flex-col gap-2">
              <Show when={sources().length > 0}>
                <ul class="flex flex-col gap-1">
                  <For each={sources()}>
                    {(src) => (
                      <li class="flex items-center justify-between gap-2 text-sm text-ink">
                        <span class="min-w-0 truncate font-semibold">{src.label}</span>
                        <span class="text-[10px] font-mono text-ink-secondary bg-base/60 rounded px-1 py-0.5">
                          {src.sourceType}
                        </span>
                        <label class="flex items-center gap-1 text-xs text-ink-secondary">
                          <input
                            type="checkbox"
                            class="accent-accent"
                            checked={src.enabled}
                            onChange={(e) => void handleToggleEnabled(src, e.currentTarget.checked)}
                          />
                          enabled
                        </label>
                        <div class="flex gap-2 shrink-0">
                          <button
                            type="button"
                            class="text-xs text-ink-secondary hover:text-ink"
                            onClick={() => startEdit(src)}
                          >
                            Edit
                          </button>
                          <button
                            type="button"
                            class="text-xs text-ink-secondary hover:text-p-urgent"
                            onClick={() => void handleDeleteSource(src.id)}
                          >
                            Delete
                          </button>
                        </div>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>

              <Show when={addPickerOpen()}>
                <div class="flex flex-col gap-2 rounded border border-border-subtle p-2 bg-base/40">
                  <select
                    class="w-full text-sm rounded px-2 py-1.5 bg-base text-ink border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                    value={addType()}
                    onChange={(e) => setAddType(e.currentTarget.value)}
                  >
                    <option value="">(select type)</option>
                    <For each={sourceTypes()}>
                      {(t) => <option value={t.source_type}>{t.label}</option>}
                    </For>
                  </select>
                  <input
                    type="text"
                    class="w-full text-sm rounded px-2 py-1.5 bg-base text-ink placeholder:text-ink-muted border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                    value={addLabel()}
                    onInput={(e) => setAddLabel(e.currentTarget.value)}
                    placeholder="Label (e.g. Work Jira)"
                  />
                  <div class="flex gap-2 justify-end">
                    <button
                      type="button"
                      class="text-xs text-ink-secondary hover:text-ink"
                      onClick={() => { setAddPickerOpen(false); setAddType(''); setAddLabel(''); }}
                    >
                      Cancel
                    </button>
                    <button
                      type="button"
                      class="px-3 py-1 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors disabled:opacity-50"
                      disabled={!addType() || !addLabel().trim()}
                      onClick={() => void handleAddSource()}
                    >
                      Add
                    </button>
                  </div>
                </div>
              </Show>

              <Show when={!addPickerOpen()}>
                <button
                  type="button"
                  class="self-start text-sm text-accent hover:text-accent-hover underline-offset-2 hover:underline"
                  onClick={() => setAddPickerOpen(true)}
                >
                  + Add source
                </button>
              </Show>
            </div>
          </fieldset>

          {/* --- Per-source editor (loaded from SETTINGS_REGISTRY) --- */}
          <Show when={editing()}>
            <fieldset class="border border-border-subtle rounded-[var(--radius-card)] p-3">
              <legend class="text-xs font-semibold text-ink-secondary px-1">
                Edit source — {editing()!.sourceType}
              </legend>
              <div class="flex flex-col gap-3">
                <div>
                  <label class="block text-xs font-semibold text-ink-secondary mb-1" for="settings-edit-label">
                    Label
                  </label>
                  <input
                    id="settings-edit-label"
                    type="text"
                    class="w-full text-sm rounded px-2 py-1.5 bg-base text-ink border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                    value={editLabel()}
                    onInput={(e) => setEditLabel(e.currentTarget.value)}
                  />
                </div>
                <label class="flex items-center gap-2 text-sm text-ink">
                  <input
                    type="checkbox"
                    class="accent-accent"
                    checked={editEnabled()}
                    onChange={(e) => setEditEnabled(e.currentTarget.checked)}
                  />
                  Enabled
                </label>
                <Show
                  when={editView()}
                  fallback={<p class="text-xs text-ink-secondary">Loading settings…</p>}
                >
                  {(view) => {
                    const v = view();
                    return createComponent(v.comp, {
                    instance: v.inst,
                      onSave: (cfg: Record<string, unknown>, m: StatusMapping) => void handleSaveEdit(cfg, m),
                    });
                  }}
                </Show>
                <div class="flex gap-2 justify-end">
                  <button
                    type="button"
                    class="text-xs text-ink-secondary hover:text-ink"
                    onClick={() => { setEditing(null); setEditComponent(null); }}
                  >
                    Close
                  </button>
                </div>
              </div>
            </fieldset>
          </Show>

          <fieldset class="border border-border-subtle rounded-[var(--radius-card)] p-3">
            <legend class="text-xs font-semibold text-ink-secondary px-1">Window</legend>
            <label class="flex items-center gap-2 text-sm text-ink">
              <input
                id="settings-close-to-tray"
                type="checkbox"
                class="accent-accent"
                checked={closeToTray()}
                onChange={(e) => setCloseToTray(e.currentTarget.checked)}
              />
              Close button hides to tray instead of quitting
            </label>
          </fieldset>

          <fieldset class="border border-border-subtle rounded-[var(--radius-card)] p-3">
            <legend class="text-xs font-semibold text-ink-secondary px-1">MCP server</legend>
            <div class="flex flex-col gap-3">
              <label class="flex items-center gap-2 text-sm text-ink">
                <input
                  id="settings-mcp-enabled"
                  type="checkbox"
                  class="accent-accent"
                  checked={mcpEnabled()}
                  onChange={(e) => setMcpEnabled(e.currentTarget.checked)}
                />
                Enable MCP server (streamable HTTP)
              </label>
              <div>
                <label class="block text-xs font-semibold text-ink-secondary mb-1" for="settings-mcp-port">
                  Port
                </label>
                <input
                  id="settings-mcp-port"
                  type="number"
                  min={1}
                  max={65535}
                  class="w-full text-sm rounded px-2 py-1.5 bg-base text-ink border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                  value={mcpPort()}
                  onInput={(e) => setMcpPort(parseInt(e.currentTarget.value, 10) || 27816)}
                />
              </div>
              <p class="text-xs text-ink-secondary">
                Endpoint: <code class="font-mono">http://127.0.0.1:{mcpPort()}/mcp</code>
                {mcpRunning() ? ' — running' : ' — stopped'}
              </p>
            </div>
          </fieldset>

          <fieldset class="border border-border-subtle rounded-[var(--radius-card)] p-3">
            <legend class="text-xs font-semibold text-ink-secondary px-1">Editor</legend>
            <label class="block text-xs font-semibold text-ink-secondary mb-1" for="settings-editor-command">
              Editor command
            </label>
            <input
              id="settings-editor-command"
              type="text"
              class="w-full text-sm rounded px-2 py-1.5 bg-base text-ink placeholder:text-ink-muted border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
              value={editorCommand()}
              onInput={(e) => setEditorCommand(e.currentTarget.value)}
              placeholder="code"
            />
            <p class="text-xs text-ink-secondary mt-1">
              Default for the card right-click "Open in editor" action. Each tree source can override this. Use <code class="font-mono">{'{path}'}</code> as a placeholder for the card's source path; if omitted the path is appended.
            </p>
          </fieldset>

          <fieldset class="border border-border-subtle rounded-[var(--radius-card)] p-3">
            <legend class="text-xs font-semibold text-ink-secondary px-1">Tree sources</legend>
            <div class="flex flex-col gap-3">
              <Show when={treeSources().length > 0}>
                <ul class="flex flex-col gap-1">
                  <For each={treeSources()}>
                    {(src) => (
                      <li class="flex items-center justify-between gap-2 text-sm text-ink">
                        <Show
                          when={editingTree()?.id === src.id}
                          fallback={
                            <span class="min-w-0 truncate font-semibold">{src.label}</span>
                          }
                        >
                          <div class="flex flex-1 gap-2">
                            <input
                              type="text"
                              class="flex-1 text-sm rounded px-2 py-1 bg-base text-ink placeholder:text-ink-muted border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                              value={editTreeLabel()}
                              onInput={(e) => setEditTreeLabel(e.currentTarget.value)}
                              placeholder="Label"
                            />
                            <input
                              type="text"
                              class="flex-1 text-sm rounded px-2 py-1 bg-base text-ink placeholder:text-ink-muted border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                              value={editTreePath()}
                              onInput={(e) => setEditTreePath(e.currentTarget.value)}
                              placeholder="path/to/source/folder"
                            />
                            <input
                              type="text"
                              class="flex-1 text-sm rounded px-2 py-1 bg-base text-ink placeholder:text-ink-muted border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                              value={editTreeEditor()}
                              onInput={(e) => setEditTreeEditor(e.currentTarget.value)}
                              placeholder="editor cmd (optional)"
                            />
                          </div>
                        </Show>
                        <div class="flex gap-2 shrink-0">
                          <Show
                            when={editingTree()?.id === src.id}
                            fallback={
                              <button
                                type="button"
                                class="text-xs text-ink-secondary hover:text-ink"
                                onClick={() => startEditTree(src)}
                              >
                                Edit
                              </button>
                            }
                          >
                            <button
                              type="button"
                              class="text-xs text-accent hover:text-accent-hover"
                              onClick={() => void handleSaveEditTree()}
                            >
                              Save
                            </button>
                            <button
                              type="button"
                              class="text-xs text-ink-secondary hover:text-ink"
                              onClick={() => setEditingTree(null)}
                            >
                              Cancel
                            </button>
                          </Show>
                          <button
                            type="button"
                            class="text-xs text-ink-secondary hover:text-p-urgent"
                            onClick={() => void handleDeleteTree(src.id)}
                          >
                            Remove
                          </button>
                        </div>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
              <div class="flex gap-2">
                <input
                  type="text"
                  class="flex-1 text-sm rounded px-2 py-1.5 bg-base text-ink placeholder:text-ink-muted border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                  value={newTreeLabel()}
                  onInput={(e) => setNewTreeLabel(e.currentTarget.value)}
                  placeholder="Label"
                />
                <input
                  type="text"
                  class="flex-1 text-sm rounded px-2 py-1.5 bg-base text-ink placeholder:text-ink-muted border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                  value={newTreePath()}
                  onInput={(e) => setNewTreePath(e.currentTarget.value)}
                  placeholder="path/to/source/folder"
                />
                <input
                  type="text"
                  class="flex-1 text-sm rounded px-2 py-1.5 bg-base text-ink placeholder:text-ink-muted border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                  value={newTreeEditor()}
                  onInput={(e) => setNewTreeEditor(e.currentTarget.value)}
                  placeholder="editor cmd (optional)"
                />
                <button
                  type="button"
                  class="px-3 py-1.5 text-sm font-medium rounded border border-border-subtle text-ink-secondary hover:bg-elevated transition-colors"
                  onClick={() => void handleAddTree()}
                >
                  Add
                </button>
              </div>
            </div>
          </fieldset>

          {error() && (
            <p class="text-sm text-p-urgent" role="alert">{error()}</p>
          )}

          <div class="flex gap-2 justify-end">
            <button
              type="button"
              class="px-3 py-1.5 text-sm font-medium rounded text-ink-secondary hover:bg-elevated transition-colors"
              onClick={props.onClose}
            >
              Cancel
            </button>
            <button
              type="button"
              class="px-3 py-1.5 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors"
              onClick={() => void handleSaveAppSettings()}
            >
              Save
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}
