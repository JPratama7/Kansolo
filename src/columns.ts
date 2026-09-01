import type { ColumnId, StatusMapping } from "./types.ts";

export const COLUMNS: { id: ColumnId; title: string }[] = [
  { id: "backlog", title: "Backlog" },
  { id: "ongoing", title: "Ongoing" },
  { id: "done", title: "Done" },
];

export const DEFAULT_STATUS_MAPPING: StatusMapping = {
  backlog: ["To Do", "Backlog", "Open"],
  ongoing: ["In Progress", "In Review"],
  done: ["Done", "Closed", "Resolved"],
};
