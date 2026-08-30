import { For, Show, createComponent, createEffect, createMemo, createSignal, type Component } from 'solid-js';
import { Portal } from 'solid-js/web';
import { Dialog } from '@ark-ui/solid/dialog';
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
  acpErrorMessage,
} from '../db.ts';
import { SETTINGS_REGISTRY, type SourceSettingsProps } from './settings/registry.ts';
import AgentRegistry from './settings/AgentRegistry.tsx';
import AcpSettings from './settings/AcpSettings.tsx';
import { ArkSelect } from './ui/ArkSelect.tsx';
import { toaster } from './ui/toaster.ts';

interface SettingsProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** Coerce a stored setting string to a boolean; default false unless "true". */
function isTrue(v: string | undefined): boolean {
  return v === 'true';
}

/** Section ids shown in the left rail; order is the navigation order. */
const SECTIONS = [
  { id: 'sources', label: 'Sources' },
  { id: 'window', label: 'Window' },
  { id: 'mcp', label: 'MCP server' },
  { id: 'editor', label: 'Editor' },
  { id: 'tree', label: 'Tree sources' },
  { id: 'agents', label: 'Agents' },
  { id: 'acp', label: 'Agent settings' },
] as const;
type SectionId = (typeof SECTIONS)[number]['id'];

export default function Settings(props: SettingsProps) {
  const [sourceTypes, setSourceTypes] = createSignal<SourceTypeMeta[]>([]);
  const [sources, setSources] = createSignal<SourceInstance[]>([]);
  const [editing, setEditing] = createSignal<SourceInstance | null>(null);
  const [editLabel, setEditLabel] = createSignal('');
  const [editEnabled, setEditEnabled] = createSignal(true);
  // The settings component for the editing source's type (sync-loaded).
  const [EditComponent, setEditComponent] = createSignal<Component<SourceSettingsProps> | null>(null);
  const [addPickerOpen, setAddPickerOpen] = createSignal(false);
  const [addType, setAddType] = createSignal<string>('');
  const [addLabel, setAddLabel] = createSignal('');

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
  // At most one persistent delete-confirmation toast at a time (decision 2B).
  const [pendingConfirmToastId, setPendingConfirmToastId] = createSignal<string | null>(null);

  const [activeSection, setActiveSection] = createSignal<SectionId>('sources');
  const [panelW, setPanelW] = createSignal(0);
  const [panelH, setPanelH] = createSignal(0);
  let panelEl: HTMLDivElement | undefined;
  let resizeState: { x: number; y: number; w: number; h: number } | null = null;

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

  // Load all settings data each time the dialog opens (fresh view per open).
  createEffect(() => {
    if (!props.open) return;
    void (async () => {
      try {
        setSourceTypes(await listSourceTypes());
        await refreshSources();
        const settings = await getAllSettings();
        setMcpEnabled(isTrue(settings['mcp_enabled']));
        setMcpPort(parseInt(settings['mcp_port'] ?? '27816', 10) || 27816);
        setCloseToTray(settings['close_to_tray'] !== 'false');
        setEditorCommand(settings['editor_command'] ?? 'code');
        const savedW = parseInt(settings['settings_w'] ?? '', 10);
        const savedH = parseInt(settings['settings_h'] ?? '', 10);
        if (savedW > 0) setPanelW(savedW);
        if (savedH > 0) setPanelH(savedH);
        try {
          const status = await invoke<McpStatus>('mcp_status');
          setMcpRunning(status.running);
        } catch {
          setMcpRunning(false);
        }
        setTreeSources(await listTreeSources());
      } catch (e) {
        toaster.error({
          title: 'Could not load settings',
          description: e instanceof Error ? e.message : String(e),
        });
      }
    })();
  });

  function requestClose() {
    props.onOpenChange(false);
  }

  function onResizeStart(e: PointerEvent) {
    e.preventDefault();
    e.stopPropagation();
    const el = panelEl;
    resizeState = {
      x: e.clientX,
      y: e.clientY,
      w: el?.offsetWidth ?? 640,
      h: el?.offsetHeight ?? 560,
    };
    window.addEventListener('pointermove', onResizeMove);
    window.addEventListener('pointerup', onResizeEnd);
  }
  function onResizeMove(e: PointerEvent) {
    if (!resizeState) return;
    const maxW = window.innerWidth * 0.9;
    const maxH = window.innerHeight * 0.9;
    const w = Math.min(Math.max(resizeState.w + (e.clientX - resizeState.x), 440), maxW);
    const h = Math.min(Math.max(resizeState.h + (e.clientY - resizeState.y), 340), maxH);
    setPanelW(w);
    setPanelH(h);
  }
  async function onResizeEnd() {
    window.removeEventListener('pointermove', onResizeMove);
    window.removeEventListener('pointerup', onResizeEnd);
    const w = panelW();
    const h = panelH();
    resizeState = null;
    if (w > 0 && h > 0) {
      try {
        await setSetting('settings_w', String(Math.round(w)));
        await setSetting('settings_h', String(Math.round(h)));
      } catch { /* non-fatal: size just won't persist */ }
    }
  }

  function startEdit(src: SourceInstance) {
    setError(null);
    setEditing(src);
    setEditLabel(src.label);
    setEditEnabled(src.enabled);
    setEditComponent(null);
    const Comp = SETTINGS_REGISTRY[src.sourceType];
    if (!Comp) {
      setError(`No settings component registered for source type "${src.sourceType}".`);
      return;
    }
    setEditComponent(() => Comp);
  }

  async function handleSaveEdit(config: Record<string, unknown>, statusMapping: StatusMapping) {
    const src = editing();
    if (!src) return;
    const label = editLabel().trim() || src.label;
    try {
      await updateSource(src.id, label, config, statusMapping, editEnabled());
      setEditing(null);
      setEditComponent(null);
      await refreshSources();
      toaster.success({ title: 'Source saved', description: label });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  /**
   * Delete confirmation via persistent toast (decision 2). The actual
   * deletion runs inside the toast action callback. Dedup: if a
   * confirmation toast is already shown, dismiss it before creating a new
   * one (decision 2B).
   */
  function handleDeleteSource(id: string) {
    const existing = pendingConfirmToastId();
    if (existing !== null) toaster.dismiss(existing);
    const src = sources().find((s) => s.id === id);
    const label = src?.label ?? id;
    const id_ = toaster.create({
      title: 'Delete this source and all cards it sourced?',
      description: 'Local cards stay.',
      type: 'warning',
      duration: Infinity,
      action: {
        label: 'Delete',
        onClick: () => {
          toaster.dismiss(id_);
          setPendingConfirmToastId(null);
          void (async () => {
            try {
              await deleteSource(id);
              if (editing()?.id === id) {
                setEditing(null);
                setEditComponent(null);
              }
              await refreshSources();
              toaster.success({ title: 'Source deleted', description: label });
            } catch (e) {
              toaster.error({
                title: 'Delete failed',
                description: e instanceof Error ? e.message : String(e),
              });
            }
          })();
        },
      },
    });
    setPendingConfirmToastId(id_);
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
      requestClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function handleAddTree() {
    const label = newTreeLabel().trim();
    const path = newTreePath().trim();
    if (!label || !path) return;
    try {
      await addTreeSource(label, path, newTreeEditor());
      setNewTreeLabel('');
      setNewTreePath('');
      setNewTreeEditor('');
      setTreeSources(await listTreeSources());
      toaster.success({ title: 'Tree source added', description: label });
    } catch (e) {
      toaster.error({ title: 'Add tree source failed', description: e instanceof Error ? e.message : String(e) });
    }
  }

  async function handleDeleteTree(id: string) {
    try {
      await deleteTreeSource(id);
      setTreeSources(await listTreeSources());
    } catch (e) {
      toaster.error({ title: 'Delete tree source failed', description: e instanceof Error ? e.message : String(e) });
    }
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
    try {
      await updateTreeSource(src.id, label, path, editTreeEditor());
      setEditingTree(null);
      setTreeSources(await listTreeSources());
    } catch (e) {
      toaster.error({ title: 'Save tree source failed', description: e instanceof Error ? e.message : String(e) });
    }
  }

  const INPUT =
    'w-full text-sm rounded px-2 py-1.5 bg-base text-ink placeholder:text-ink-secondary border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent';

  return (
    <Dialog.Root
      open={props.open}
      lazyMount
      unmountOnExit
      closeOnEscape
      closeOnInteractOutside
      aria-label="Settings"
      onOpenChange={(e) => props.onOpenChange(e.open)}
    >
      <Portal>
        <Dialog.Backdrop class="fixed inset-0 z-50 bg-black/50" />
        <Dialog.Positioner class="fixed inset-0 z-50 flex items-start justify-center pt-10 px-4">
          <Dialog.Content
            ref={panelEl}
            class="settings-panel relative flex flex-col w-[640px] h-[560px] max-w-[90vw] max-h-[90vh] bg-surface rounded-[var(--radius-card)] border border-border-subtle shadow-2xl overflow-hidden"
            aria-label="Settings"
            style={{
              width: panelW() ? `${panelW()}px` : undefined,
              height: panelH() ? `${panelH()}px` : undefined,
            }}
          >
        <div class="flex items-center justify-between px-4 py-3 bg-surface border-b border-border-subtle">
          <h2 class="text-base font-bold text-ink">Settings</h2>
          <button
            type="button"
            class="text-xl text-ink-secondary hover:text-ink leading-none px-1"
            aria-label="Close"
            onClick={requestClose}
          >
            ×
          </button>
        </div>

        {error() && (
          <div class="px-4 py-2 bg-p-urgent/15 border-b border-p-urgent/40 text-p-urgent text-sm" role="alert">
            {error()}
          </div>
        )}

        <div class="flex flex-1 min-h-0">
          <nav class="w-36 shrink-0 border-r border-border-subtle bg-base/40 py-2 flex flex-col gap-0.5">
            <For each={SECTIONS}>
              {(s) => (
                <button
                  type="button"
                  class={'text-left text-sm px-3 py-2 transition-colors border-l-2 ' +
                    (activeSection() === s.id
                      ? 'border-accent text-ink bg-elevated/50 font-semibold'
                      : 'border-transparent text-ink-secondary hover:text-ink hover:bg-elevated/30')}
                  onClick={() => setActiveSection(s.id)}
                >
                  {s.label}
                </button>
              )}
            </For>
          </nav>

          <div class="flex-1 min-w-0 overflow-y-auto board-scroll p-4">
            <Show when={activeSection()} keyed>
              {(id) => (
                <div class="settings-pane flex flex-col gap-4">
                  {id === 'sources' && (
                    <>
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
                              <ArkSelect
                                items={sourceTypes().map((t) => ({ label: t.label, value: t.source_type }))}
                                value={addType()}
                                onValueChange={setAddType}
                                placeholder="(select type)"
                                name="source_type"
                                class={INPUT}
                              />
                              <label class="block text-xs font-semibold text-ink-secondary" for="settings-add-source-label">
                                Label
                              </label>
                              <input
                                id="settings-add-source-label"
                                type="text"
                                class={INPUT}
                                name="source_label"
                                autocomplete="off"
                                value={addLabel()}
                                onInput={(e) => setAddLabel(e.currentTarget.value)}
                                placeholder="e.g. Work Jira…"
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
                                name="edit_label"
                                autocomplete="off"
                                class={INPUT}
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
                                  onSave: (cfg: Record<string, unknown>, m: StatusMapping) => handleSaveEdit(cfg, m),
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
                    </>
                  )}

                  {id === 'window' && (
                    <fieldset class="border border-border-subtle rounded-[var(--radius-card)] p-3">
                      <legend class="text-xs font-semibold text-ink-secondary px-1">Window</legend>
                      <label class="flex items-center gap-2 text-sm text-ink">
                        <input
                          id="settings-close-to-tray"
                          type="checkbox"
                          name="close_to_tray"
                          autocomplete="off"
                          class="accent-accent"
                          checked={closeToTray()}
                          onChange={(e) => setCloseToTray(e.currentTarget.checked)}
                        />
                        Close button hides to tray instead of quitting
                      </label>
                    </fieldset>
                  )}

                  {id === 'mcp' && (
                    <fieldset class="border border-border-subtle rounded-[var(--radius-card)] p-3">
                      <legend class="text-xs font-semibold text-ink-secondary px-1">MCP server</legend>
                      <div class="flex flex-col gap-3">
                        <label class="flex items-center gap-2 text-sm text-ink">
                          <input
                            id="settings-mcp-enabled"
                            type="checkbox"
                            name="mcp_enabled"
                            autocomplete="off"
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
                            name="mcp_port"
                            autocomplete="off"
                            inputmode="numeric"
                            min={1}
                            max={65535}
                            class={INPUT}
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
                  )}

                  {id === 'editor' && (
                    <fieldset class="border border-border-subtle rounded-[var(--radius-card)] p-3">
                      <legend class="text-xs font-semibold text-ink-secondary px-1">Editor</legend>
                      <label class="block text-xs font-semibold text-ink-secondary mb-1" for="settings-editor-command">
                        Editor command
                      </label>
                      <input
                        id="settings-editor-command"
                        type="text"
                        name="editor_command"
                        autocomplete="off"
                        class={INPUT}
                        value={editorCommand()}
                        onInput={(e) => setEditorCommand(e.currentTarget.value)}
                        placeholder="code…"
                      />
                      <p class="text-xs text-ink-secondary mt-1">
                        Default for the card right-click "Open in editor" action. Each tree source can override this. Use <code class="font-mono">{'{path}'}</code> as a placeholder for the card's source path; if omitted the path is appended.
                      </p>
                    </fieldset>
                  )}

                  {id === 'tree' && (
                    <fieldset class="border border-border-subtle rounded-[var(--radius-card)] p-3">
                      <legend class="text-xs font-semibold text-ink-secondary px-1">Tree sources</legend>
                      <div class="flex flex-col gap-3">
                        <Show when={treeSources().length > 0}>
                          <ul class="flex flex-col gap-1">
                            <For each={treeSources()}>
                              {(src) => (
                                <li class="flex flex-col gap-2 text-sm text-ink">
                                  <Show
                                    when={editingTree()?.id === src.id}
                                    fallback={
                                      <div class="flex items-center justify-between gap-2">
                                        <span class="min-w-0 truncate font-semibold">{src.label}</span>
                                        <div class="flex gap-2 shrink-0">
                                          <button
                                            type="button"
                                            class="text-xs text-ink-secondary hover:text-ink"
                                            onClick={() => startEditTree(src)}
                                          >
                                            Edit
                                          </button>
                                          <button
                                            type="button"
                                            class="text-xs text-ink-secondary hover:text-p-urgent"
                                            onClick={() => void handleDeleteTree(src.id)}
                                          >
                                            Remove
                                          </button>
                                        </div>
                                      </div>
                                    }
                                  >
                                    <div class="flex flex-col gap-2">
                                      <label class="sr-only" for={`tree-edit-label-${src.id}`}>Label</label>
                                      <input
                                        id={`tree-edit-label-${src.id}`}
                                        type="text"
                                        name="tree_label"
                                        autocomplete="off"
                                        class="w-full text-sm rounded px-2 py-1 bg-base text-ink placeholder:text-ink-muted border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                                        value={editTreeLabel()}
                                        onInput={(e) => setEditTreeLabel(e.currentTarget.value)}
                                        placeholder="Label…"
                                      />
                                      <label class="sr-only" for={`tree-edit-path-${src.id}`}>Path</label>
                                      <input
                                        id={`tree-edit-path-${src.id}`}
                                        type="text"
                                        name="tree_path"
                                        autocomplete="off"
                                        class="w-full text-sm rounded px-2 py-1 bg-base text-ink placeholder:text-ink-muted border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                                        value={editTreePath()}
                                        onInput={(e) => setEditTreePath(e.currentTarget.value)}
                                        placeholder="path/to/source/folder…"
                                      />
                                      <label class="sr-only" for={`tree-edit-editor-${src.id}`}>Editor command</label>
                                      <input
                                        id={`tree-edit-editor-${src.id}`}
                                        type="text"
                                        name="tree_editor"
                                        autocomplete="off"
                                        class="w-full text-sm rounded px-2 py-1 bg-base text-ink placeholder:text-ink-muted border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                                        value={editTreeEditor()}
                                        onInput={(e) => setEditTreeEditor(e.currentTarget.value)}
                                        placeholder="editor cmd (optional)…"
                                      />
                                    </div>
                                  </Show>
                                  <Show when={editingTree()?.id === src.id}>
                                    <div class="flex gap-2 justify-end">
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
                                      <button
                                        type="button"
                                        class="text-xs text-ink-secondary hover:text-p-urgent"
                                        onClick={() => void handleDeleteTree(src.id)}
                                      >
                                        Remove
                                      </button>
                                    </div>
                                  </Show>
                                </li>
                              )}
                            </For>
                          </ul>
                        </Show>
                        <div class="flex flex-wrap gap-2">
                          <label class="sr-only" for="new-tree-label">Label</label>
                          <input
                            id="new-tree-label"
                            type="text"
                            name="new_tree_label"
                            autocomplete="off"
                            class="flex-1 min-w-[120px] text-sm rounded px-2 py-1.5 bg-base text-ink placeholder:text-ink-secondary border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                            value={newTreeLabel()}
                            onInput={(e) => setNewTreeLabel(e.currentTarget.value)}
                            placeholder="Label…"
                          />
                          <label class="sr-only" for="new-tree-path">Path</label>
                          <input
                            id="new-tree-path"
                            type="text"
                            name="new_tree_path"
                            autocomplete="off"
                            class="flex-1 min-w-[120px] text-sm rounded px-2 py-1.5 bg-base text-ink placeholder:text-ink-secondary border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                            value={newTreePath()}
                            onInput={(e) => setNewTreePath(e.currentTarget.value)}
                            placeholder="path/to/source/folder…"
                          />
                          <label class="sr-only" for="new-tree-editor">Editor command</label>
                          <input
                            id="new-tree-editor"
                            type="text"
                            name="new_tree_editor"
                            autocomplete="off"
                            class="flex-1 min-w-[120px] text-sm rounded px-2 py-1.5 bg-base text-ink placeholder:text-ink-secondary border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent"
                            value={newTreeEditor()}
                            onInput={(e) => setNewTreeEditor(e.currentTarget.value)}
                            placeholder="editor cmd (optional)…"
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
                  )}
                  {id === 'agents' && (
                    <AgentRegistry />
                  )}
                  {id === 'acp' && (
                    <AcpSettings />
                  )}
                </div>
              )}
            </Show>
          </div>
        </div>

        <div class="flex gap-2 justify-end px-4 py-3 bg-surface border-t border-border-subtle">
          <button
            type="button"
            class="px-3 py-1.5 text-sm font-medium rounded text-ink-secondary hover:bg-elevated transition-colors"
            onClick={requestClose}
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

        <div
          class="settings-grip"
          onPointerDown={onResizeStart}
          onKeyDown={(e) => {
            const step = e.shiftKey ? 20 : 5;
            if (e.key === 'ArrowRight' || e.key === 'ArrowDown') {
              e.preventDefault();
              setPanelW((w) => Math.min(w + step, window.innerWidth * 0.9));
              setPanelH((h) => Math.min(h + step, window.innerHeight * 0.9));
            } else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
              e.preventDefault();
              setPanelW((w) => Math.max(w - step, 440));
              setPanelH((h) => Math.max(h - step, 340));
            }
          }}
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize settings"
          tabindex={0}
        />
          </Dialog.Content>
        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
  );
}
