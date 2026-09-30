# Long-Lived Shared Context Contract

This contract keeps stateful Codex-to-Grok context usable when one Codex task
has accumulated a very large rollout. It extends the existing shared-context
bridge without changing Parley event schema v1 or context journal schema v1.

## Canonical peer session

- Generic Parley remains isolated when conversation mode is omitted. Locked
  profiles may set `PARLEY_GROK_DEFAULT_CONVERSATION_MODE=canonical`.
- Canonical mode is limited to Grok with explicit Codex source context in
  `auto` mode. The caller omits both target-session fields.
- The locked profile, Codex source session, and numeric
  `PARLEY_GROK_CANONICAL_EPOCH` identify one logical conversation. Parley
  derives a UUIDv8-shaped native binding from those values plus the validated
  canonical worker cwd.
- Each canonical worktree therefore owns one native Grok session and one
  independent cursor. An absent binding creates and seeds that session; a
  committed binding resumes it. An uncertain binding also selects resume but
  still requires the existing explicit `replay` or `skip` recovery. No failure
  triggers an automatic retry or rollover.
- Incrementing the epoch is the deliberate rollover mechanism when a native
  Grok session can no longer be restored. The new target receives a bounded
  newest-whole-turn seed, while later exact deltas and the durable interaction
  profile preserve the logical Two Chairs conversation.
- Switching to another approved worktree creates its native binding with a
  bounded seed from the newest whole visible Codex turns. Returning to a prior
  worktree resumes that binding with the exact delta after its own committed
  cursor. This keeps one logical conversation without asking Grok to restore a
  cwd-scoped native session from the wrong worktree.
- The existing execution lease remains the active-owner boundary for the
  derived target. Probe jobs force `isolated` mode and keep explicit session
  IDs.
- Live canonical calls require `PARLEY_ASYNC_JOBS_ENABLED=true` with an
  absolute `PARLEY_JOB_STATE_DIR`; canonical dry-run remains read-only without
  that lease configuration.
- Dry-run lookup is read-only: it may inspect an existing journal but creates
  no state directory, lock file, journal, prompt file, or event.

## Reader invariants

- Rollouts are opened read-only with read, write, and delete sharing.
- Only newline-terminated physical records are committed to a scan.
- UTF-8 BOM and CRLF handling retain the existing byte-offset and fingerprint
  semantics.
- Records no larger than 8 MiB are parsed exactly as before.
- A larger record is never retained. It is streamed through a bounded JSON
  validator and may be skipped only when its root object contains exactly one
  string `type` field whose value is one of `compacted`, `event_msg`,
  `session_meta`, or `world_state`.
- Duplicate root `type` fields, a nested-only `type`, malformed JSON, invalid
  UTF-8, an unknown type, or a large `response_item` or `turn_context` fails
  closed. Errors contain offsets and reason classes, never record content.
- An incomplete trailing record never advances the complete byte cursor. Once
  it exceeds 8 MiB, only bounded validator state and its byte count remain in
  memory.

## Planning invariants

- New-session seed selection uses one forward streaming pass and retains only
  the newest whole visible turn groups that fit the configured seed bound.
- If the newest whole turn exceeds the bound, planning fails closed.
- After selection, Parley rereads only the selected byte range to calculate
  its exact record count and fingerprint.
- Delta, replay, and cursor verification stream their ranges without retaining
  excluded records or hidden payloads.
- FNV-1a fingerprint input remains the UTF-8 bytes of every non-empty physical
  record in the range after one trailing CR is removed, joined by one LF.
  Existing schema-v1 cursors therefore remain valid when their source bytes
  are unchanged.
- A persistent index is not a correctness dependency. It may be added later
  only as a disposable measured optimization.

## Durable interaction profile boundary

Transcript repair and durable user preferences are separate layers. The
optional interaction profile is a small, versioned, user-approved file. Locked
deployments keep the active source of truth outside the Git worktree; the
checked-in file is only a validation and review example. A profile may describe
presentation and collaboration preferences, but it can never grant tools,
permissions, paths, continuity authority, scientific dispositions, or task
scope. Current policy and the current request always outrank it. Agents may
propose profile changes, but activation requires the user's approval of the
exact text.
