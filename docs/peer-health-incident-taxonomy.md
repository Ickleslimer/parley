# Peer Health Incident Taxonomy

Health schema v3 narrows the formal incident surface without rewriting existing
evidence.

## Incidents

Only these closed classes project into `active_incidents`, `recent_incidents`,
`unread_count`, tray incident badges, incident acknowledgement, and event-log
handoff selection:

- `quota_exhausted`
- `mcp_stdout_undelivered`

Their existing strict classifiers, recovery rules, one-sound-per-identity
behavior, mute state, and global 60-second acoustic cooldown remain unchanged.

## Explicit handoff alert

`handoff_unacknowledged` is an explicit Grok handoff alert, not an incident.
Only the existing job-bound one-shot helper may open it after a durable report
is awaiting Codex receipt. It may play the same chime once under the existing
mute and cooldown controls. It remains recoverable only by the matching Codex
receipt and remains visible through the pending-handoff evidence path. It never
contributes to incident lists, incident unread counts, incident tray badges,
incident acknowledgement, or event-log handoff selection.

Elapsed time, silence, hook failure, process exit, catastrophe, or viewer access
never opens or resolves an explicit handoff alert.

## Diagnostics

`usage_sample`, `capacity_throttle`, `turn_error`, and `watchdog_killed` remain
sanitized health evidence and diagnostics. They never become incidents or
alerts and never contribute to incident lists, unread counts, tray badges,
acknowledgement controls, event-log handoff selection, or sound.

## Migration

- Readers accept health schemas v1, v2, and v3; new writes use v3.
- Existing journals, state, handoff evidence, and sound history remain
  append-only and are never rewritten or deleted.
- Replay projects old `capacity_throttle`, `turn_error`, and `watchdog_killed`
  records as diagnostics only.
- Replay projects old `handoff_unacknowledged` records through the explicit
  handoff-alert path, preserving receipt and sounded-identity evidence without
  adding them to incident surfaces or producing migration sounds.
- The legacy sanitized `incident_id` transport field remains readable for
  existing handoff evidence; it does not make that evidence a formal incident.
