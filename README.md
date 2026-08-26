# Kansolo

Solo kanban desktop app with plugin-based external source sync. Built with Tauri 2, Rust, Deno, TypeScript, and SolidJS.

## Tech Stack

- **Tauri 2** — desktop shell
- **Rust** (edition 2021) — backend, all source/sync logic
- **Deno + TypeScript** — frontend toolchain
- **SolidJS** — UI framework
- **Vite 8** — bundler
- **Tailwind CSS 4 + daisyUI 5** — styling
- **SQLite** via `rusqlite` (bundled)
- **rmcp** — MCP server (streamable-http transport)

## Features

### Board

- Three-column board (Backlog / Ongoing / Done) with color-coded accent bars
- Priority-then-position sort (urgent → low, then by position)
- Card count badge per column
- Empty column hint
- Board reload after sync/clear

### Cards

- Inline add card at column bottom
- Card title + markdown description preview (clamped 2 lines)
- Priority strip + label (low/medium/high/urgent)
- Source ref badge for external cards (e.g. Jira key)
- Source path label for tree-source cards
- Edit modal with title/description/priority/source-path form
- Description edit/preview toggle
- Source path selector (registered tree sources)
- Title-empty validation
- Delete (local cards only)

### Drag & Drop

- Draggable cards, droppable columns
- Drop highlight ring on target column
- Drag visual (opacity + rotate)
- Cross-column move on drop, persists via `move_card`

### Sync

- Sync button with loading spinner
- Per-source sequential sync, pauses on first conflict
- Last-synced timestamp label
- Sync error banner
- Unmapped statuses warning banner
- Merge conflict modal with per-field local/remote picker
- "All local" / "All remote" per card
- Apply merge resumes sync
- Cancel sync from merge modal
- 3-way merge (remote vs local vs snapshot)

### Settings

- Fullscreen settings modal with sectioned fieldsets
- Source instances list (type chip + enabled toggle + edit + delete)
- Add source picker (data-driven from `list_source_types`)
- Edit source loads type-specific settings component from registry
- Toggle source enabled without opening editor
- Delete source with confirm dialog
- Close-to-tray toggle
- MCP enable + port + running status
- Editor command setting (global default, `{path}` placeholder)
- Tree sources list (label/path/editor command)
- Add/edit/delete tree sources
- Save/Cancel app settings

### Source Plugins

- `SourceProvider` trait in Rust (`source_type`, `display_label`, `fetch_raw`, `fetch_options`)
- Registry maps source type → provider
- `list_source_types` auto-exposes registered sources to UI
- `sync_source` / `resolve_conflicts` work for any source
- TS settings component registry (`src/components/settings/registry.ts`)
- Jira provider (Jira Cloud REST API v3, Basic auth, ADF → Markdown)
- JQL builder UI with live preview
- Jira project fetch button
- Status mapping editor (backlog/ongoing/done comma-separated lists)
- Reset status mapping to defaults

### MCP Server

- Embedded rmcp streamable-HTTP server on `127.0.0.1:{port}/mcp`
- Exposes `list_cards`, `get_card`, `move_card` tools
- Auto-starts on app launch if `mcp_enabled` setting is true
- Start/stop/restart via `mcp_apply` command
- Status query via `mcp_status`

### System

- System tray icon (Show/Hide/Quit)
- Close-to-tray (close button hides window)
- Open in editor (right-click card, per-source or global editor command)
- Clear source modal (delete all cards + snapshots of one source)
- DB migrations (9 ordered, idempotent, embedded SQL)

## Architecture

- All source logic (fetching, query building, mapping, 3-way sync merge) lives in Rust
- TS only renders UI and invokes Tauri commands
- Source instance config stored as JSON in `sources` table (`config_json`, snake_case keys)
- Cards reference external origin via `source_ref` / `source_status` columns
- SQLite at `{app_config_dir}/tasker.db` with WAL + busy_timeout

### Plugin Author Contract

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

## Development

```sh
deno task dev          # Vite dev server (port 1420)
deno task build        # frontend TS build
deno task tauri dev    # launch app
```

## Verification

```sh
deno task build                          # frontend TS build
deno test --allow-read --allow-env       # TS tests
cd src-tauri && cargo check && cargo test  # Rust
deno task tauri dev                      # launch app
```

## License

MIT
