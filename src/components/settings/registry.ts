// Maps source type → settings component.
// This is the only TS file that changes when adding a new source type
// (besides the new component file itself).
// Static imports avoid dynamic-import/HMR-proxy issues (stale chunks,
// `Comp is not a function` from cached HTMLDivElement).
import type { Component } from 'solid-js';
import type { SourceInstance, StatusMapping } from '../../types.ts';
import JiraSettings from './JiraSettings.tsx';

export interface SourceSettingsProps {
  instance: SourceInstance;
  onSave: (config: Record<string, unknown>, statusMapping: StatusMapping) => void;
}

export const SETTINGS_REGISTRY: Record<string, Component<SourceSettingsProps>> = {
  jira: JiraSettings,
};
