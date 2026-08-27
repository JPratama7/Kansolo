# AGENTS.md

## Plugin Author Contract

To add a new external source:

1. Add a Rust module `src-tauri/src/source/<type>.rs` implementing `SourceProvider`
   (implement `source_type()`, `display_label()`, `fetch_raw(config)`,
   and optionally `fetch_options(config)`).
2. Register it in `source::registry()` in `src-tauri/src/source/mod.rs`.
3. Add a TS settings component `src/components/settings/<Type>Settings.tsx`.
4. Register it in `src/components/settings/registry.ts`.

No schema change. No settings key.
`list_source_types` exposes it to the UI automatically.
`sync_source` / `resolve_conflicts` work for any source automatically.
The Settings screen renders its config form via the registered settings component.

Exception: a provider may expose its own Tauri command for UI-only work
that the generic sync pipeline doesn't cover (e.g. `preview_jql` builds a
JQL string from builder parts for live preview). Register such commands
in `src-tauri/src/lib.rs` alongside the generic ones. Keep them
provider-specific and prefixed; do not generalize the sync pipeline around
them.

## Tree Sources

Distinct from external source plugins. A tree source is a local folder
path the user registers (label + path + optional editor command); cards
link to one via `tree_source_id` (FK → `tree_sources.id`, `ON DELETE
RESTRICT`). Deleting a tree source is blocked while cards still reference
it. CRUD lives in `db::settings` (`list_tree_sources`, `add_tree_source`,
`update_tree_source`, `delete_tree_source`); the editor launch is
`editor::open_in_editor` (`{path}` placeholder, `sh -c`, detached).

## MCP Server

Embedded rmcp streamable-HTTP server (`src-tauri/src/mcp.rs`) on
`127.0.0.1:{port}/mcp`. Exposes three tools: `list_cards`, `get_card`,
`move_card`. Reads/writes the same SQLite file as the app (WAL +
busy_timeout). Lifecycle is TS-driven via the `mcp_apply` (start/stop/
restart) and `mcp_status` Tauri commands; the axum task + cancellation
token live in `McpState`. Auto-starts on launch when `mcp_enabled` is
true.

## Verification

- `deno task build` — frontend TS build
- `deno test --allow-read --allow-env` — TS tests
- `cd src-tauri && cargo check && cargo test` — Rust
- `deno task tauri dev` — launch the app

## Conventions

- All source logic (fetching, query building, mapping, 3-way sync merge) lives in Rust.
  TS only renders and invokes Tauri commands.
- Source instance config is stored as JSON in the `sources` table (`config_json`),
  snake_case keys (e.g. `base_url`, `email`, `token`, `jql_parts`).
- Cards reference their external origin via `source_ref`/`source_status`,
  and their optional tree source via `tree_source_id` (FK → `tree_sources.id`).
- DB migrations are ordered, idempotent, embedded SQL files under
  `src-tauri/migrations/`, registered in `db::MIGRATIONS`. Add new ones by
  appending the next numbered file + tuple entry; never edit an applied
  migration.
- When writing files, limit each write/edit to a maximum of 50 lines.

## Pitfalls

### Storing a component function in a Solid signal

Solid's `createSignal` setter treats a function argument as an updater:
`setSignal(fn)` calls `fn(prev)` and stores the return value. Passing a
component function directly — `setEditComponent(MyComponent)` — invokes
the component with `prev` (often `null`) as props, runs it outside the
render tree, and stores whatever DOM node it returns (e.g.
`HTMLDivElement`) instead of the function itself. Downstream
`createComponent(storedValue, props)` then fails with
`Comp is not a function. (In 'Comp(props)', 'Comp' is an instance of HTMLDivElement)`.

Fix: wrap the value in an updater that returns it:
`setEditComponent(() => MyComponent)`. The updater receives `prev`,
ignores it, and returns the component function unchanged. The signal
then stores the function reference, not its return value.
