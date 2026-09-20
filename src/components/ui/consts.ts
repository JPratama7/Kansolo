import type { Priority } from "../../types.ts";

export const INPUT =
  "w-full text-sm rounded px-2 py-1.5 bg-base text-ink placeholder:text-ink-secondary border border-border-subtle outline-none focus:border-accent focus:ring-1 focus:ring-accent";

export const PRIORITY_STRIP: Record<Priority, string> = {
  low: "bg-p-low",
  medium: "bg-p-med",
  high: "bg-p-high",
  urgent: "bg-p-urgent",
};

export const STATUS_LABEL: Record<string, string> = {
  pending: "Queued",
  running: "Running",
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
};
