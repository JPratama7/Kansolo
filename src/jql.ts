/// <reference lib="deno.ns" />

export type AssigneeMode = 'current' | 'specific' | 'any';

export type StatusMode = 'unresolved' | 'all' | 'specific';

/** Updated-within window. `any` = no updated clause. */
export type UpdatedWithin = 'any' | '7d' | '30d' | '90d';

/** ORDER BY field. Always DESC. */
export type OrderBy = 'updated' | 'priority' | 'created';

/** Builder parts stored as the `jql_parts` setting (JSON). */
export interface JqlParts {
  project: string;
  assigneeMode: AssigneeMode;
  assignee: string;
  statusMode: StatusMode;
  statuses: string[];
  updatedWithin: UpdatedWithin;
  orderBy: OrderBy;
}

export const DEFAULT_JQL_PARTS: JqlParts = {
  project: '',
  assigneeMode: 'current',
  assignee: '',
  statusMode: 'unresolved',
  statuses: [],
  updatedWithin: 'any',
  orderBy: 'updated',
};

export const UPDATED_WITHIN_OPTIONS: readonly UpdatedWithin[] = ['any', '7d', '30d', '90d'];
export const ORDER_BY_OPTIONS: readonly OrderBy[] = ['updated', 'priority', 'created'];
export const ASSIGNEE_MODE_OPTIONS: readonly AssigneeMode[] = ['current', 'specific', 'any'];
export const STATUS_MODE_OPTIONS: readonly StatusMode[] = ['unresolved', 'all', 'specific'];

/** Parse the stored `jql_parts` setting; fall back to defaults when unset/invalid. */
export function parseJqlParts(raw: string | undefined): JqlParts {
  if (!raw) return { ...DEFAULT_JQL_PARTS };
  try {
    const parsed = JSON.parse(raw) as Partial<JqlParts>;
    return {
      project: typeof parsed.project === 'string' ? parsed.project : '',
      assigneeMode: ASSIGNEE_MODE_OPTIONS.includes(parsed.assigneeMode as AssigneeMode)
        ? (parsed.assigneeMode as AssigneeMode)
        : DEFAULT_JQL_PARTS.assigneeMode,
      assignee: typeof parsed.assignee === 'string' ? parsed.assignee : '',
      statusMode: STATUS_MODE_OPTIONS.includes(parsed.statusMode as StatusMode)
        ? (parsed.statusMode as StatusMode)
        : DEFAULT_JQL_PARTS.statusMode,
      statuses: Array.isArray(parsed.statuses) && parsed.statuses.every((s) => typeof s === 'string')
        ? parsed.statuses
        : [],
      updatedWithin: UPDATED_WITHIN_OPTIONS.includes(parsed.updatedWithin as UpdatedWithin)
        ? (parsed.updatedWithin as UpdatedWithin)
        : DEFAULT_JQL_PARTS.updatedWithin,
      orderBy: ORDER_BY_OPTIONS.includes(parsed.orderBy as OrderBy)
        ? (parsed.orderBy as OrderBy)
        : DEFAULT_JQL_PARTS.orderBy,
    };
  } catch {
    return { ...DEFAULT_JQL_PARTS };
  }
}
