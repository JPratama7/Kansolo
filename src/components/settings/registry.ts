// Maps source type → settings component.
// This is the only TS file that changes when adding a new source type
// (besides the new component file itself).
// Static imports avoid dynamic-import/HMR-proxy issues (stale chunks,
// `Comp is not a function` from cached HTMLDivElement).
import JiraSettings from './JiraSettings.tsx';

export const SETTINGS_REGISTRY: Record<string, unknown> = {
  jira: JiraSettings,
};
