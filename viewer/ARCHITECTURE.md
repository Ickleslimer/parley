# Parley Conversation Viewer Architecture

## Boundaries

- The viewer is an independent Windows-only Tauri 2 application. It is not a member of Parley's root Cargo package or a root workspace.
- Rust owns file access, parsing, history, source selection, settings, window lifecycle, tray behavior, underlay attachment, and all persistence.
- TypeScript receives bounded snapshots or paged records through explicit commands. It never reads files, opens sockets, starts processes, or renders HTML from event content.
- The production CSP permits only packaged assets and Tauri IPC. There is no shell, HTTP, updater, global-shortcut, notification, or filesystem guest plugin.
- Event logs are opened read-only and are never created, renamed, deleted, truncated, or locked against Parley writers.

## Event Engine Contract

The event engine lives only below `src-tauri/src/event_engine/` and exposes a thread-safe `EventEngine` with these operations:

- `new()` creates an empty engine with no source.
- `set_source(Option<PathBuf>)` accepts only absolute paths, resets file-generation state, and never creates the path.
- `poll()` performs one nonblocking read pass and returns whether observable state changed.
- `status()` returns source state, generation, byte count, counts, last event time, and cumulative diagnostics.
- `session_page(cursor, limit)` returns sessions newest-first.
- `exchange_page(session_id, cursor, limit)` returns exchanges newest-first for one session.
- `search(query, cursor, limit)` performs case-insensitive local substring search over exact content.
- `event_content(event_id)` returns one full exact event body and metadata.
- `widget_snapshot()` returns only the newest exchange with bounded exact excerpts.

All public result types serialize with camelCase names matching `src/contracts.ts`. Page limits are clamped to 1 through 200. The widget excerpt limit is 420 Unicode scalar values per message. If a prompt contains an envelope line whose trimmed prefix is exactly `task:`, the excerpt is the exact remainder of that line and `excerptExtracted` is true. Otherwise the excerpt is an exact prefix, never a model-generated summary.

## JSONL Rules

- Accept only `schema_version: 1` and `event_type` values `request`, `response`, or `error`.
- Parse the complete schema-v1 fields without inferring model or reasoning metadata.
- Pair records by `exchange_id` and group by `session_id`.
- Requests display `source` to `target`; responses display `target` to `source`; errors are Parley execution records.
- A request without completion uses exactly `Request logged; no response event yet`.
- Deduplicate `event_id` only within one file generation.
- Reopen the source every 250 ms with read, write, and delete sharing. Track file identity plus byte offset, retain incomplete trailing bytes, and rebuild on replacement or truncation.
- Handle UTF-8 BOM, CRLF, Unicode split across reads, concurrent append, missing/reappearing files, and bodies above 60,000 characters.
- Skip malformed lines, unsupported records, and physical lines above 8 MiB while incrementing diagnostics.

## Runtime Contract

- Startup source precedence is `--event-log`, `PARLEY_EVENT_LOG`, saved selection, then no source. CLI paths must be absolute.
- `--show` and ordinary Start Menu launches show detail. `--autostart` opens only tray and widget.
- A second process forwards its arguments through the single-instance callback.
- Tray creation precedes underlay attachment. Tray failure keeps the widget detached and opens detail with a visible Exit action.
- Underlay failure keeps the widget hidden, reports degraded state, and enters bounded reattachment cycles. No always-on-top, click-through, or ordinary-window fallback is allowed.
- Closing detail hides it to tray. Only the Exit action or tray Exit terminates the process.
- Settings persist per user and include source, monitor, corner, offsets, width, height, and launch-at-login.

