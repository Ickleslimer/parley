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
parley-health-supervisor.exe --refresh-installation
parley-health-supervisor.exe --remove-hooks
parley-health-supervisor.exe --shutdown
```

`--shutdown` signals the exact single-instance supervisor through a named
current-user event. It does not enumerate or terminate unrelated processes.

## Evidence boundaries

Health schema v1 stores sanitized classes, identifiers, timestamps, provider
usage fields, and transport references only. It never stores prompts, replies,
credentials, raw environment values, command arguments, or raw provider error
payloads. Silence is never evidence of failure. Acknowledgement suppresses a
repeat alert but never means recovery.

Only `quota_exhausted` and `mcp_stdout_undelivered` incidents are audible. The
global acoustic cooldown is 60 seconds and each incident can sound once. The
user-triggered test chime creates no incident and remains available while
incident sounds are muted.

## Chime

`assets/two-chairs.wav` is a 44.1 kHz mono PCM two-note chime shorter than one
second. Recreate it deterministically with:

```powershell
.\tools\generate-chime.ps1
```
