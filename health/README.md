# Two Chairs Peer Health

`health` is an independent Windows-only Rust package. It is deliberately not a
member of Parley's root Cargo package or workspace.

It builds three capability-separated executables:

- `parley-health-supervisor.exe` samples Codex rate-limit evidence, consumes the
  atomic inbox, maintains the append-only journal and atomic snapshot, and is
  the only component permitted to play the bundled chime.
- `parley-health-query.exe` accepts no arguments and only reads `snapshot.json`.
  It never refreshes a provider, writes a file, plays sound, or starts a child.
- `parley-health-hook.exe` handles the dedicated Grok `StopFailure` and deny-only
  `PreToolUse` hook entries.

State is retained under `%LOCALAPPDATA%\Parley\health`. Uninstall removes the
executables, autostart registration, and dedicated Grok hook file but preserves
this directory as evidence.

## Supervisor controls

```powershell
parley-health-supervisor.exe
parley-health-supervisor.exe --configure-r3 <git-common-dir> <main-root>
parley-health-supervisor.exe --allow-query-root <root>
parley-health-supervisor.exe --refresh-installation
parley-health-supervisor.exe --remove-hooks
parley-health-supervisor.exe --shutdown
```

`--shutdown` signals the exact single-instance supervisor through a named
current-user event. It does not enumerate or terminate unrelated processes.
`--allow-query-root` adds only a root from which the exact installed read-only
query may run; it does not admit `StopFailure` evidence from that repository.

## Evidence boundaries

Health schema v2 stores sanitized classes, identifiers, timestamps, provider
usage fields, and transport references only. Readers accept schema v1 and v2
journals, inbox records, durable state, cached scope, and snapshots, then
normalize new writes to v2. Any other schema version is rejected. The package
never stores prompts, replies, credentials, raw environment values, command
arguments, or raw provider error payloads. Silence is never evidence of
failure. Viewer acknowledgement suppresses a repeat alert but never means
recovery.

Only `quota_exhausted`, `mcp_stdout_undelivered`, and explicit
`handoff_unacknowledged` incidents are audible. A handoff incident is opened
only by `peer_alert_requested` and keeps the supplied incident id. It is
recovered only by `handoff_received` for that same id when the receipt time is
greater than or equal to the alert evidence time. The global acoustic cooldown
is 60 seconds and each incident can sound once. The user-triggered test chime
creates no incident and remains available while incident sounds are muted.

## Chime

`assets/two-chairs.wav` is a 44.1 kHz mono PCM two-note chime shorter than one
second. Recreate it deterministically with:

```powershell
.\tools\generate-chime.ps1
```
