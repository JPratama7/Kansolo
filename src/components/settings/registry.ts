// Maps source type → settings component. Static imports avoid Solid's
// HMR pitfall where a dynamic import stores an HTMLDivElement, not the
// component function, and later `createComponent` fails.
import type { Component } from "solid-js";
import type { SourceInstance, StatusMapping } from "../../types.ts";
import JiraSettings from "./JiraSettings.tsx";

export interface SourceSettingsProps {
  instance: SourceInstance;
  onSave: (
    config: Record<string, unknown>,
    statusMapping: StatusMapping,
  ) => void;
}

export const SETTINGS_REGISTRY: Record<string, Component<SourceSettingsProps>> =
  {
    jira: JiraSettings,
  };
