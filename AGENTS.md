# AGENTS.md

## Plugin Author Contract

To add a new external source:

1. Add a Rust module `src-tauri/src/source/<type>.rs` implementing `SourceProvider`
   (implement `source_type()`, `display_label()`, `fetch_raw(config)`,
   and optionally `fetch_options(config)`).
2. Register it in `source::registry()` in `src-tauri/src/source/mod.rs`.
3. Add a TS settings component `src/components/settings/<Type>Settings.tsx`.
4. Register it in `src/components/settings/registry.ts`.

No schema change. No new Tauri command. No settings key.
`list_source_types` exposes it to the UI automatically.
`sync_source` / `resolve_conflicts` work for any source automatically.
The Settings screen renders its config form via the registered settings component.

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
- Cards reference their external origin via `source_ref`/`source_status`.
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
