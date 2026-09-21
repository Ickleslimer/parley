# Parley Conversation Viewer Architecture

## Boundaries

- The viewer is an independent Windows-only Tauri 2 application. It is not a member of Parley's root Cargo package or a root workspace.
- Rust owns file access, parsing, history, source selection, settings, window lifecycle, tray behavior, underlay attachment, health-inbox writes, and all persistence.
- TypeScript receives bounded snapshots or paged records through explicit commands. It never reads files, opens sockets, starts processes, or renders HTML from event content.
- The production CSP permits only packaged assets and Tauri IPC. There is no shell, HTTP, updater, global-shortcut, notification, or filesystem guest plugin.
- Every event log is opened read-only with read/write/delete sharing and is never created, renamed, deleted, truncated, or locked against Parley writers.

## Multi-Source Event Engine

The event engine below `src-tauri/src/event_engine/` exposes one thread-safe `EventEngine` over an ordered source list:

- `set_sources`, `add_source`, and `remove_source` accept only absolute paths and never create a missing source.
- Each source owns its configured path identity, active Windows file identity, generation, byte cursor, incomplete tail, diagnostics, and event store.
- Canonical path aliases and distinct paths with the same Windows file identity are diagnosed and deduplicated. The first configured source owns ordering.
- Replacement, truncation, disappearance, and reappearance rebuild only the affected source generation.
- Request/completion pairing and event-ID deduplication never cross source-generation boundaries.
- Session, exchange, search, and widget results sort by timestamp, then configured-source order, then opaque key.

All retrieval uses source- and generation-qualified opaque keys. Event, exchange, and session keys are also type-qualified, so one key cannot be substituted for another even when raw IDs collide. Raw IDs remain display and peer-health matching fields only. A raw health event, exchange, or session ID that resolves in more than one source fails closed; when multiple identifiers are present, they must resolve to the same response.

`status()` returns aggregate totals plus `sources[]`; each source reports state, generation, byte count, counts, last event time, diagnostics, and alias information. Page limits are clamped to 1 through 200. Full exact content stays in Rust until requested by opaque event key.

## JSONL and Framing

- Accept only `schema_version: 1` and `event_type` values `request`, `response`, or `error`.
- Parse the complete schema-v1 fields without inferring model or reasoning metadata.
- Requests display `source` to `target`; responses display `target` to `source`; errors are Parley execution records.
- A request without completion uses exactly `Request logged; no response event yet`.
- Reopen every source each 250 ms, retain incomplete trailing bytes, and process only newline-terminated records.
- Handle UTF-8 BOM, CRLF, Unicode split across reads, concurrent append, missing/reappearing files, and bodies above 60,000 characters.
- Skip malformed lines, unsupported records, and physical lines above 8 MiB while incrementing source and aggregate diagnostics.

Widget excerpts are bounded to 420 Unicode scalar values and never model-generated. For framed requests, extraction searches for `PARLEY_CURRENT_REQUEST_V1` with the event's exact exchange ID, then uses the exact `task:` line remainder when present. The legacy first-`task:` fallback applies only to unframed events. Context source, mode, offsets, counts, truncation, and recovery are available only in detail diagnostics; the underlay remains conversation-only.

## Runtime and Settings

- `--event-log <absolute-path>` is repeatable. Source precedence is repeated CLI paths, one `PARLEY_EVENT_LOG`, saved `selectedLogs`, then no source.
- A second-instance launch with event-log arguments replaces the persisted list; native picker and detail controls add or remove sources.
- Settings migrate legacy `selectedLog` into an ordered one-entry `selectedLogs` list and write both fields for one-release downgrade compatibility.
- `--show` and ordinary Start Menu launches show detail. `--autostart` opens only tray and widget.
- Tray creation precedes underlay attachment. Tray failure keeps the widget detached and opens detail with a visible Exit action.
- Underlay failure keeps the widget hidden, reports degraded state, and enters bounded reattachment cycles. No always-on-top, click-through, or ordinary-window fallback is allowed.
- Closing detail hides it to tray. Only the Exit action or tray Exit terminates the process.
- Viewer settings (`%APPDATA%\com.ickleslimer.parley-viewer\settings.json`), `%LOCALAPPDATA%\Parley\health`, and `%LOCALAPPDATA%\Parley\context` all survive uninstall and upgrade.
- Once `autostartInitialized` is true, saved `launchAtLogin` stays authoritative: startup restores or clears the viewer Run entry to match it, including after uninstall removed the registration. First interactive detail launch still enables autostart when `autostartInitialized` is false.

## Frontend IPC

`src/ipc.ts` is the complete frontend authority boundary. Detail and widget views use only its `ViewerApi`; implementation files must not invoke arbitrary Tauri commands.

- `getStatus` and `getWidgetSnapshot` drive bounded live refresh across all sources.
- `listSessions`, `listExchanges`, `search`, and `getEventContent` use opaque keys for paging, search, and exact retrieval.
- `getSettings`, `saveSettings`, and `listMonitors` drive source ordering and geometry settings.
- `selectEventLog`, `setEventLogs`, `addEventLog`, and `removeEventLog` manage sources through Rust only.
- `setWidgetVisible`, `setLaunchAtLogin`, `showDetail`, and `exit` expose only required lifecycle actions.
- Content is rendered with DOM text nodes or `textContent` only. Event content must never enter `innerHTML`.

## Installer and Evidence

The current-user NSIS package bundles the viewer and health binaries. It owns only its exact viewer and supervisor autostart entries plus the two exact Grok hook entries. Uninstall removes those integrations but preserves per-user viewer settings in `%APPDATA%\com.ickleslimer.parley-viewer` (placement, monitor, dimensions, selected sources, and launch preference) together with `%LOCALAPPDATA%\Parley\health` and `%LOCALAPPDATA%\Parley\context` as evidence. Generic binaries contain no machine-specific event-log path; local sources are seeded after installation through repeated `--event-log` arguments.
