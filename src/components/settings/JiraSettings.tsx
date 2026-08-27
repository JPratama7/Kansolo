import { For, Show, createEffect, createSignal, onMount } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import type { SourceInstance, StatusMapping } from '../../types.ts';
import { DEFAULT_STATUS_MAPPING } from '../../columns.ts';
import { toaster } from '../ui/toaster.ts';
import {
  ASSIGNEE_MODE_OPTIONS,
  DEFAULT_JQL_PARTS,
  ORDER_BY_OPTIONS,
  STATUS_MODE_OPTIONS,
  UPDATED_WITHIN_OPTIONS,
  parseJqlParts,
  type AssigneeMode,
  type JqlParts,
  type OrderBy,
  type StatusMode,
  type UpdatedWithin,
} from '../../jql.ts';

/** Project entry returned by `fetch_source_options` for the Jira provider. */
interface JiraProject {
  key: string;
  name: string;
}

interface JiraSettingsProps {
  instance: SourceInstance;
  onSave: (config: Record<string, unknown>, statusMapping: StatusMapping) => Promise<void> | void;
}

/** Split a comma-separated status list: trim each entry, drop empties. */
function splitStatuses(input: string): string[] {
  return input
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

function isStringArray(v: unknown): v is string[] {
  return Array.isArray(v) && v.every((s) => typeof s === 'string');
}

/** Coerce a stored config value to a string; fall back to '' when absent. */
function cfgString(config: Record<string, unknown>, key: string): string {
  const v = config[key];
  return typeof v === 'string' ? v : '';
}

export default function JiraSettings(props: JiraSettingsProps) {
  const safeProps = props || {} as JiraSettingsProps;
  const [baseUrl, setBaseUrl] = createSignal('');
  const [email, setEmail] = createSignal('');
  const [token, setToken] = createSignal('');
  const [jqlParts, setJqlParts] = createSignal<JqlParts>({ ...DEFAULT_JQL_PARTS });
  const [statusesText, setStatusesText] = createSignal('');
  const [backlogStatuses, setBacklogStatuses] = createSignal('');
  const [ongoingStatuses, setOngoingStatuses] = createSignal('');
  const [doneStatuses, setDoneStatuses] = createSignal('');
  const [error, setError] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);
  const [projects, setProjects] = createSignal<JiraProject[]>([]);
  const [projectsLoading, setProjectsLoading] = createSignal(false);
  const [preview, setPreview] = createSignal('');
  const [previewLoading, setPreviewLoading] = createSignal(false);
  let previewTimer: ReturnType<typeof setTimeout> | null = null;

  /** Patch a single field of the jql parts signal. */
  function patchParts<K extends keyof JqlParts>(key: K, value: JqlParts[K]) {
    setJqlParts((prev) => ({ ...prev, [key]: value }));
  }

  // Load initial values from the instance config + status mapping.
  onMount(() => {
    const cfg = safeProps.instance?.config ?? {};
    setBaseUrl(cfgString(cfg, 'base_url'));
    setEmail(cfgString(cfg, 'email'));
    setToken(cfgString(cfg, 'token'));
    // `jql_parts` is stored as a JSON sub-object; parseJqlParts expects a
    // string, so stringify first when it's an object.
    const rawParts = cfg['jql_parts'];
    const partsStr =
      typeof rawParts === 'string'
        ? rawParts
        : rawParts !== undefined
          ? JSON.stringify(rawParts)
          : undefined;
    const parts = parseJqlParts(partsStr);
    setJqlParts(parts);
    setStatusesText(parts.statuses.join(', '));
    const mapping = safeProps.instance?.statusMapping ?? DEFAULT_STATUS_MAPPING;
    setBacklogStatuses(mapping.backlog.join(', '));
    setOngoingStatuses(mapping.ongoing.join(', '));
    setDoneStatuses(mapping.done.join(', '));
  });

  /** Fetch the user's visible projects from Jira and populate the dropdown. */
  async function loadProjects() {
    setProjectsLoading(true);
    setError(null);
    try {
      const result = await invoke<{ projects: JiraProject[] }>('fetch_source_options', {
        sourceId: safeProps.instance?.id ?? '',
      });
      const list = result.projects;
      if (isStringArray(list) || !Array.isArray(list)) {
        // Defensive: backend may return a non-array shape; coerce to [].
        setProjects([]);
      } else {
        setProjects(list as JiraProject[]);
      }
    } catch (e) {
      // Action-result error → toast (decision 8).
      toaster.error({
        title: 'Could not fetch projects',
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setProjectsLoading(false);
    }
  }

  /** Debounced JQL preview — re-runs whenever the builder fields change. */
  async function refreshPreview() {
    if (previewTimer) clearTimeout(previewTimer);
    previewTimer = setTimeout(async () => {
      setPreviewLoading(true);
      try {
        const parts = { ...jqlParts(), statuses: splitStatuses(statusesText()) };
        const jql = await invoke<string>('preview_jql', { jqlParts: parts });
        setPreview(jql);
      } catch (e) {
        setPreview('');
        // Action-result error → toast (decision 8).
        toaster.error({
          title: 'JQL preview failed',
          description: e instanceof Error ? e.message : String(e),
        });
      } finally {
        setPreviewLoading(false);
      }
    }, 300);
  }

  // Re-fetch the preview whenever any builder field changes (debounced).
  createEffect(() => {
    jqlParts();
    statusesText();
    void refreshPreview();
  });

  /** Reset the three mapping inputs to the defaults; not saved until Save. */
  function resetMapping() {
    setBacklogStatuses(DEFAULT_STATUS_MAPPING.backlog.join(', '));
    setOngoingStatuses(DEFAULT_STATUS_MAPPING.ongoing.join(', '));
    setDoneStatuses(DEFAULT_STATUS_MAPPING.done.join(', '));
  }

  async function handleSave() {
    const mapping: StatusMapping = {
      backlog: splitStatuses(backlogStatuses()),
      ongoing: splitStatuses(ongoingStatuses()),
      done: splitStatuses(doneStatuses()),
    };
    const finalParts: JqlParts = { ...jqlParts(), statuses: splitStatuses(statusesText()) };
    if (preview() === '') {
      // Form validation → inline `<p role="alert">` (decision 8).
      setError('JQL is empty — set at least one field (e.g. project or assignee).');
      return;
    }
    setError(null);
    setSaving(true);
    try {
      const config: Record<string, unknown> = {
        base_url: baseUrl().trim(),
        email: email().trim(),
        token: token().trim(),
        jql_parts: finalParts,
      };
      await safeProps.onSave?.(config, mapping);
    } catch (e) {
      // Action-result error → toast (decision 8).
      toaster.error({
        title: 'Save failed',
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setSaving(false);
    }
  }

  const INPUT =
    'w-full text-sm rounded px-2 py-1.5 bg-base text-ink placeholder:text-ink-secondary border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent';

  return (
    <div class="flex flex-col gap-4">
      <div>
        <label class="block text-xs font-semibold text-ink-secondary mb-1" for="jira-base-url">
          Base URL
        </label>
        <input
          id="jira-base-url"
          type="url"
          inputmode="url"
          name="base_url"
          autocomplete="off"
          class={INPUT}
          value={baseUrl()}
          onInput={(e) => setBaseUrl(e.currentTarget.value)}
          placeholder="your-domain.atlassian.net…"
        />
      </div>

      <div>
        <label class="block text-xs font-semibold text-ink-secondary mb-1" for="jira-email">
          Email
        </label>
        <input
          id="jira-email"
          type="email"
          inputmode="email"
          name="email"
          autocomplete="email"
          spellCheck={false}
          class={INPUT}
          value={email()}
          onInput={(e) => setEmail(e.currentTarget.value)}
          placeholder="you@example.com…"
        />
      </div>

      <div>
        <label class="block text-xs font-semibold text-ink-secondary mb-1" for="jira-token">
          API token
        </label>
        <input
          id="jira-token"
          type="password"
          name="token"
          autocomplete="off"
          spellCheck={false}
          class={INPUT}
          value={token()}
          onInput={(e) => setToken(e.currentTarget.value)}
          placeholder="API token…"
        />
      </div>

      <fieldset class="border border-border-subtle rounded-[var(--radius-card)] p-3">
        <legend class="text-xs font-semibold text-ink-secondary px-1">JQL builder</legend>
        <div class="flex flex-col gap-3">
          <div>
            <label class="block text-xs font-semibold text-ink-secondary mb-1" for="jira-project">
              Project key
            </label>
            <div class="flex gap-1.5">
              <Show
                when={projects().length > 0}
                fallback={
                  <input
                    id="jira-project"
                    type="text"
                    name="project"
                    autocomplete="off"
                    class={INPUT}
                    value={jqlParts().project}
                    onInput={(e) => patchParts('project', e.currentTarget.value)}
                    placeholder="SCRUM…"
                  />
                }
              >
                <select
                  id="jira-project"
                  class={INPUT}
                  value={jqlParts().project}
                  onChange={(e) => patchParts('project', e.currentTarget.value)}
                >
                  <option value="">(none)</option>
                  <For each={projects()}>
                    {(p) => <option value={p.key}>{p.key} — {p.name}</option>}
                  </For>
                </select>
              </Show>
              <button
                type="button"
                class="px-3 py-1.5 text-sm font-medium rounded border border-border-subtle text-ink-secondary hover:bg-elevated transition-colors disabled:opacity-50"
                disabled={projectsLoading()}
                onClick={() => void loadProjects()}
              >
                {projectsLoading() ? 'Loading…' : 'Fetch'}
              </button>
            </div>
          </div>

          <div>
            <label class="block text-xs font-semibold text-ink-secondary mb-1" for="jira-assignee">
              Assignee
            </label>
            <select
              id="jira-assignee"
              class={INPUT}
              value={jqlParts().assigneeMode}
              onChange={(e) => patchParts('assigneeMode', e.currentTarget.value as AssigneeMode)}
            >
              <For each={ASSIGNEE_MODE_OPTIONS}>
                {(mode) => <option value={mode}>{mode}</option>}
              </For>
            </select>
            <Show when={jqlParts().assigneeMode === 'specific'}>
              <input
                type="text"
                name="assignee"
                autocomplete="off"
                class={`mt-2 ${INPUT}`}
                value={jqlParts().assignee}
                onInput={(e) => patchParts('assignee', e.currentTarget.value)}
                placeholder="username or email…"
              />
            </Show>
          </div>

          <div>
            <label class="block text-xs font-semibold text-ink-secondary mb-1" for="jira-status">
              Status
            </label>
            <select
              id="jira-status"
              class={INPUT}
              value={jqlParts().statusMode}
              onChange={(e) => patchParts('statusMode', e.currentTarget.value as StatusMode)}
            >
              <For each={STATUS_MODE_OPTIONS}>
                {(mode) => <option value={mode}>{mode}</option>}
              </For>
            </select>
            <Show when={jqlParts().statusMode === 'specific'}>
              <input
                type="text"
                name="statuses"
                autocomplete="off"
                class={`mt-2 ${INPUT}`}
                value={statusesText()}
                onInput={(e) => setStatusesText(e.currentTarget.value)}
                placeholder="To Do, In Progress…"
              />
            </Show>
          </div>

          <div>
            <label class="block text-xs font-semibold text-ink-secondary mb-1" for="jira-updated">
              Updated within
            </label>
            <select
              id="jira-updated"
              class={INPUT}
              value={jqlParts().updatedWithin}
              onChange={(e) => patchParts('updatedWithin', e.currentTarget.value as UpdatedWithin)}
            >
              <For each={UPDATED_WITHIN_OPTIONS}>
                {(w) => <option value={w}>{w}</option>}
              </For>
            </select>
          </div>

          <div>
            <label class="block text-xs font-semibold text-ink-secondary mb-1" for="jira-order">
              Order by
            </label>
            <select
              id="jira-order"
              class={INPUT}
              value={jqlParts().orderBy}
              onChange={(e) => patchParts('orderBy', e.currentTarget.value as OrderBy)}
            >
              <For each={ORDER_BY_OPTIONS}>
                {(o) => <option value={o}>{o}</option>}
              </For>
            </select>
          </div>

          <div>
            <span class="text-xs font-semibold text-ink-secondary">Preview</span>
            <code class="font-mono text-xs text-ink bg-base p-2 rounded block mt-1 break-all" aria-live="polite">
              {previewLoading() ? '…' : (preview() || '(empty)')}
            </code>
          </div>
        </div>
      </fieldset>

      <fieldset class="border border-border-subtle rounded-[var(--radius-card)] p-3">
        <legend class="text-xs font-semibold text-ink-secondary px-1">
          Status mapping (comma-separated)
        </legend>
        <div class="flex flex-col gap-3">
          <div>
            <label class="block text-xs font-semibold text-ink-secondary mb-1" for="jira-mapping-backlog">
              Backlog
            </label>
            <input
              id="jira-mapping-backlog"
              type="text"
              name="mapping_backlog"
              autocomplete="off"
              class={INPUT}
              value={backlogStatuses()}
              onInput={(e) => setBacklogStatuses(e.currentTarget.value)}
            />
          </div>
          <div>
            <label class="block text-xs font-semibold text-ink-secondary mb-1" for="jira-mapping-ongoing">
              Ongoing
            </label>
            <input
              id="jira-mapping-ongoing"
              type="text"
              name="mapping_ongoing"
              autocomplete="off"
              class={INPUT}
              value={ongoingStatuses()}
              onInput={(e) => setOngoingStatuses(e.currentTarget.value)}
            />
          </div>
          <div>
            <label class="block text-xs font-semibold text-ink-secondary mb-1" for="jira-mapping-done">
              Done
            </label>
            <input
              id="jira-mapping-done"
              type="text"
              name="mapping_done"
              autocomplete="off"
              class={INPUT}
              value={doneStatuses()}
              onInput={(e) => setDoneStatuses(e.currentTarget.value)}
            />
          </div>
          <button
            type="button"
            class="self-start text-sm text-accent hover:text-accent-hover underline-offset-2 hover:underline"
            onClick={resetMapping}
          >
            Reset to defaults
          </button>
        </div>
      </fieldset>

      {error() && <p class="text-sm text-p-urgent" role="alert" aria-live="polite">{error()}</p>}

      <div class="flex gap-2 justify-end">
        <button
          type="button"
          class="px-3 py-1.5 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors disabled:opacity-50"
          disabled={saving()}
          onClick={() => void handleSave()}
        >
          {saving() ? 'Saving…' : 'Save'}
        </button>
      </div>
    </div>
  );
}


