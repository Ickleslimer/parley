# Parley Conversation Viewer Architecture

## Boundaries

- The viewer is an independent Windows-only Tauri 2 application. It is not a member of Parley's root Cargo package or a root workspace.
- Rust owns file access, parsing, history, source selection, settings, window lifecycle, tray behavior, underlay attachment, health-inbox writes, and all persistence.
- Rust alone reads peer-health and handoff evidence. TypeScript receives bounded serialized snapshots and never opens either evidence store.
- TypeScript receives bounded snapshots or paged records through explicit commands. It never reads files, opens sockets, starts processes, or renders HTML from event content.
- The production CSP permits only packaged assets and Tauri IPC. There is no shell, HTTP, updater, global-shortcut, notification, or filesystem guest plugin.
- Every event log is opened read-only with read/write/delete sharing and is never created, renamed, deleted, truncated, or locked against Parley writers.

## Multi-Source Event Engine

Search IPC runs on a blocking worker rather than the UI thread. It captures reference-counted immutable events and source-qualified identities while briefly holding the engine lock, then releases that lock before matching, sorting, or projecting result excerpts. The matcher lowercases Unicode scalars once and uses a prefix table to scan in linear time, preserving original scalar offsets and refusing match starts inside a lowercase expansion. Only the requested page receives full search-hit metadata and excerpts. Replaced source generations remain unresolvable through their old event keys.

The event engine below `src-tauri/src/event_engine/` exposes one thread-safe `EventEngine` over an ordered source list:

- `set_sources`, `add_source`, and `remove_source` accept only absolute paths and never create a missing source.
- Each source owns its configured path identity, active Windows file identity, generation, byte cursor, incomplete tail, diagnostics, and event store.
- Canonical path aliases and distinct paths with the same Windows file identity are diagnosed and deduplicated. The first configured source owns ordering.
- Replacement, truncation, disappearance, and reappearance rebuild only the affected source generation.
- Request/completion pairing and event-ID deduplication never cross source-generation boundaries.
- Session, exchange, search, and widget results sort by timestamp, then configured-source order, then opaque key.
- The detail transcript projects all exchanges into one global reverse-chronological sequence. The newest exchange appears first while request-before-completion order is retained within each exchange. Session and source changes never create presentation boundaries or appear in timeline/search rows; selected-event and source-management diagnostics retain their identifiers where needed.

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

Detail and legacy widget excerpts remain bounded to 420 Unicode scalar values and are never model-generated. The desktop chat uses a separate deterministic projection: framed requests expose only the exact matching current-request section, visibly mark omitted historical context, and fail closed when the exchange marker does not match. Within that section, or an otherwise unframed request, Rust may omit only a fully validated leading R3 envelope and fully validated terminal locked response and acknowledged-handoff contracts. Responses may omit only a fully validated acknowledged-handoff wrapper and terminal health footer. The retained colleague speech stays byte-for-byte exact, `DECISION` records remain visible, malformed or unknown blocks remain untouched, and errors remain exact. Automatic chat bodies are exact 4,000-scalar prefixes of that projection; full projected bodies are retrieved lazily by opaque event key. The detail inspector continues to expose the raw event without this display projection. Context source, mode, offsets, counts, truncation, and recovery remain detail diagnostics. The interactive surface is the sole desktop transcript renderer.

Conversation and session response previews reuse that validated response projection before their 420-scalar limit, so handoff wrappers and health footers do not consume preview space. Request previews retain their existing task extraction, errors remain exact, and search still matches raw evidence with raw scalar offsets. Selected-event retrieval and content lengths remain raw.

## Runtime and Settings

- `--event-log <absolute-path>` is repeatable. Source precedence is repeated CLI paths, one `PARLEY_EVENT_LOG`, saved `selectedLogs`, then no source.
- A second-instance launch with event-log arguments replaces the persisted list; native picker and detail controls add or remove sources.
- Settings migrate legacy `selectedLog` into an ordered one-entry `selectedLogs` list and write both fields for one-release downgrade compatibility.
- `--show` and ordinary Start Menu launches show detail. `--autostart` opens only tray and widget.
- Tray creation precedes underlay attachment. Tray failure keeps the widget detached and opens detail with a visible Exit action.
- Underlay failure keeps the widget hidden, reports degraded state, and enters bounded reattachment cycles. No always-on-top, click-through, or ordinary-window fallback is allowed.
- A startup helper-placement access denial adds optional `placementContext` metadata to the private `interactive-runtime.jsonl` diagnostic: the saved native error, calling thread, startup attempt, and bounded HWND/PID/thread/class/topmost observations for the helper, chosen anchor, and Explorer icon host. These are observations after failure, not proof that a transient window retained its identity. Disappeared or unidentified windows have null identity fields. Metadata is consumed once on the failing thread and never enters the error string, placement decisions, IPC, transcript, or persisted settings. Existing retry and fallback rules remain unchanged; diagnosis of the login failure still requires a natural-login record.
- Closing detail hides it to tray. Only the Exit action or tray Exit terminates the process.
- Viewer settings (`%APPDATA%\com.ickleslimer.parley-viewer\settings.json`), `%LOCALAPPDATA%\Parley\health`, and `%LOCALAPPDATA%\Parley\context` all survive uninstall and upgrade.
- `%LOCALAPPDATA%\Parley\jobs`, `%LOCALAPPDATA%\Parley\lanes`, and `%LOCALAPPDATA%\Parley\handoffs` also survive uninstall and upgrade as task evidence.
- Once `autostartInitialized` is true, saved `launchAtLogin` stays authoritative: startup restores or clears the viewer Run entry to match it, including after uninstall removed the registration. First interactive detail launch still enables autostart when `autostartInitialized` is false.
- Settings begins with one shared `Launch Parley at sign-in` control. It changes the existing login registration and persisted preference, not current widget visibility or placement. Login launches the shared viewer process with its widget and tray; the full conversation window remains closed until explicitly opened.

### Interactive desktop composition

- `widget` remains the `WorkerW`/`Progman` workshop-scene and geometry host. It never polls or paints transcript data and is not a second conversation fallback.
- `widget-surface` is a dynamic parentless top-level WebView containing only the paper conversation column. Rust owns its creation, styles, placement, z-order, readiness, destruction, and retry policy.
- The surface is `WS_POPUP` with `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`; `WS_EX_APPWINDOW`, `WS_EX_TOPMOST`, and click-through styles are forbidden. It is never reparented and never becomes topmost.
- The underlay reports the column rectangle in CSS pixels together with viewport size and device-pixel ratio. Rust accepts reports only from `widget`, validates finite in-viewport geometry, converts to physical coordinates, and requires alignment within one physical pixel.
- The surface reports document readiness only from `widget-surface`. Activation requires a healthy underlay, verified styles and null ownership, valid geometry, and a passing desktop-band z-order check.
- Runtime state is one of `passive`, `interactive-starting`, `interactive`, or `passive-fallback`. Fallback reasons are a closed enum. `passive` and `passive-fallback` retain only the neutral scene; they do not expose an older desktop transcript. Runtime fallback never rewrites the saved `desktopMode` preference.
- The surface and underlay change visibility atomically. Explorer loss destroys the surface before the underlay enters its existing bounded reattachment path.
- Desktop chat order is global across configured sources: timestamp, configured-source order, then opaque exchange key. Twenty-exchange pages render oldest to newest and anchor only by source-qualified opaque exchange key. A separate opaque history token changes on source generation or topology changes; stale or missing anchors return `resetRequired` and never recover by raw identifiers.
- The underlay paper column is an empty, accessibility-hidden layout box used only for validated surface geometry. The surface owns the sole desktop conversation and polite live region.
- Settings schema v3 keeps `desktopMode: interactive | passive`, changes fresh dimensions to 720 by 560 with a 480 by 420 minimum, and migrates only pre-v3 settings that reach the exact prior 560 by 360 default. Placement, sources, monitor, autostart, desktop mode, and all custom dimensions remain unchanged.

### Conversation studio presentation

- The detail transcript remains one global reverse-chronological projection. Its compact header exposes one watching-count line, a keyboard-operable `Utilities` menu, and Exit; degraded lifecycle or evidence states remain explicit banners or utility content.
- The transcript owns the primary canvas and its only scrollport. It renders quiet date separators and bounded exact excerpts, places `Load earlier messages` at the older-history end, and exposes `Jump to latest` whenever the newest exchange is outside the scrollport. Polling never assigns transcript scroll or focus.
- Message activation is explicit: click, Enter, or Space opens exact evidence, while arrows, Home, and End only move roving focus. Exact content remains available only through `getEventContent` and is written with `textContent`.
- The evidence drawer is closed by default. Wide layouts use an in-flow complementary column; compact layouts use a labelled modal sheet with an inert background and focus trap. Exact raw text precedes a collapsed technical-details disclosure. Activity, Sources, and Settings reuse that drawer through a utility menu rather than permanent tabs.
- The emergency direct-Grok composer is a bounded dock beneath the transcript. Its request and terminal result remain canonical event-log rows; the dock never duplicates raw prompt or reply content.

## Frontend IPC

`src/ipc.ts` is the complete frontend authority boundary. Detail and widget views use only its `ViewerApi`; implementation files must not invoke arbitrary Tauri commands.

- `getStatus` drives underlay lifecycle and geometry refresh. The underlay never requests transcript data.
- `getWidgetFeed` returns fixed 20-exchange pages and an opaque history token; `getWidgetMessage` returns one complete projected body by opaque event key; `openWidgetEvent` opens that exact raw event in detail. The surface owns Live latching, viewport retention, and its bounded 200-exchange cache. A genuine user scroll within one layout pixel of the bottom resumes Live and clears ordinary unread keys without pinning; tagged programmatic scrolls and fail-closed history replacement cannot resume it.
- `getConversationPage` returns 420-scalar previews in global newest-to-oldest exchange order, using validated speech prefixes for responses and existing exact/task-extracted prefixes for requests and errors. It supports opaque `before` history anchors for appending older exchanges and an opaque `around` anchor that places its target first with older context beneath it; stale, missing, mixed, or wrong-kind anchors fail closed with `resetRequired`.
- `reportWidgetSurfaceBounds` accepts only validated underlay geometry; `widgetSurfaceReady` accepts readiness only from the surface; `widgetSurfacePointerDown` lets only the surface synchronously restore and verify its desktop-band position before a mouse action; `retryInteractiveMode` explicitly clears only the runtime fallback latch.
- `listSessions` and `listExchanges` remain compatibility commands, while the detail window uses `getConversationPage`; `search` and `getEventContent` retain global search and exact raw retrieval.
- `getSettings`, `saveSettings`, and `listMonitors` drive source ordering and geometry settings.
- `getPeerActivity` returns the newest bounded handoff records, sanitized activity metadata, visible-output excerpts, and exact reports only after their stored fingerprints verify.
- `getContinuationStatus`, `submitContinuation`, `cancelContinuation`, and `receiveContinuation` are available only to `detail`. Rust resolves the fixed installed broker resource, creates turn IDs, bounds all output, and accepts only exact broker-reported manifest, continuation, and evidence identities. TypeScript cannot provide a process path, cwd, session, model, permission mode, grant, or executable validation command.
- `selectEventLog`, `setEventLogs`, `addEventLog`, and `removeEventLog` manage sources through Rust only.
- `setWidgetVisible`, `setLaunchAtLogin`, `showDetail`, and `exit` expose only required lifecycle actions.
- Content is rendered with DOM text nodes or `textContent` only. Event content must never enter `innerHTML`.

## Peer Activity and Acknowledged Handoffs

The interactive detail window polls the handoff-schema-v1 store read-only. It shows exact lifecycle tokens, process phase metadata, timestamps, bounded visible-output excerpts, sanitized activity classes, and fingerprint-verified reports. It never exposes capability hashes, process identifiers, hidden reasoning text, raw tool payloads, command arguments, prompts, or environment values. Missing, locked, malformed, and unavailable evidence remains explicit; neither the Rust command nor the TypeScript presenter emits a peer-failure verdict.

The desktop-underlay widget continues to consume only completed event-log exchanges. Pending handoffs and peer activity never enter its snapshot or rendering path. Health schema v3 projects only `quota_exhausted` and `mcp_stdout_undelivered` into incident lists, unread counts, and tray badges. Capacity throttles, turn errors, and watchdog terminations remain diagnostics. A Grok-triggered `handoff_unacknowledged` record is a separate explicit handoff alert shown with pending handoff evidence, not an incident. Viewing a report, expanding it, or acknowledging a health incident cannot record the Codex receipt required by Parley; only the locked MCP `ack_agent_handoff` command can do that, and receipt means delivery rather than acceptance.

## Direct Grok Continuation

The expanded composer sizes to its content instead of a fixed rem or viewport-height ceiling. It yields space to a readable transcript when the window is short, with its own scrollbar only when the complete form or expanded evidence exceeds the space available. The default four-row input remains manually resizable. Closing the dock restores the original transcript-first layout.

The composer accepts an in-memory draft even when a task is absent, expired, closed, ambiguous, running, or unavailable. Drafting conveys no submission or file authority. Send requires a successful status read reporting `armed` or `completed`, and a failed status poll disables submission until a subsequent successful read. Submission invalidates earlier status reads, and eligibility stays unknown until a successful read that started after submission settled. The textarea remains editable during submission; success clears only an unchanged submitted draft. Polling preserves the draft, focus, and caret; drafts are not persisted across window closure.

The studio contains an emergency-only direct composer backed by the fixed `parley-continuation-broker.exe` installer resource. Codex publishes the immutable manifest through the locked MCP profile; the viewer can only read its bounded status, submit text, cancel the exact running turn, and record display receipt for one exact terminal event. The broker owns state, policy, non-executing Git observation, recursive ignored-file change watching, grant, canary, session, runtime-lock recovery, and evidence integrity. It never executes repository code after Grok writes; tests wait for Codex review. Rust launches it without a console and bounds stdout and stderr; TypeScript never opens files or starts processes.

Direct continuation events use the existing event schema with `source=user`. They are retained in the full conversation studio and exact evidence drawer but filtered from every desktop widget feed and total. The direct request projection removes only the validated continuation wrapper; raw event retrieval stays exact. The dock contains no duplicate raw prompt or reply. For receipt, `completionEventId` must match exactly one `MessagePreview.eventId`; that preview's opaque `eventKey` binds the rendered terminal completion or error row. One rising-edge receipt may occur per evidence hash only while the ordinary latest view is active, the document is visible, and either at least 24 CSS pixels of that row (or its whole height when shorter) intersects the transcript scrollport or the compact exact-evidence dialog is open for that same event key. Request-only, ambiguous, mismatched, offscreen, search, historical, and hidden-document states remain unreceipted. The viewer never scrolls or retries to manufacture receipt. Receipt means display only. All ordinary Grok execution in the published worktree remains fail-closed until Codex retrieves and reviews matching evidence through the locked MCP tools.

## Installer and Evidence

The current-user NSIS package bundles the viewer, health, lane, handoff, and fixed continuation-broker binaries. It owns only its exact viewer and supervisor autostart entries plus the exact managed Grok health, lane, and handoff hook entries. Uninstall removes those integrations but preserves per-user viewer settings in `%APPDATA%\com.ickleslimer.parley-viewer` (placement, monitor, dimensions, selected sources, and launch preference) together with `%LOCALAPPDATA%\Parley\health`, `%LOCALAPPDATA%\Parley\context`, `%LOCALAPPDATA%\Parley\jobs`, `%LOCALAPPDATA%\Parley\lanes`, `%LOCALAPPDATA%\Parley\handoffs`, and `%LOCALAPPDATA%\Parley\continuations` as evidence. Generic binaries contain no machine-specific event-log, handoff, or continuation path; local sources are seeded after installation through repeated `--event-log` arguments.
