# Shared context from paginated Codex history

Codex can retain several physical rollouts for one stable chat ID. Parley must
not interpret them as unrelated duplicates, nor choose an active branch from
timestamps or file sizes.

On Windows, an existing Codex `state_5.sqlite` selects the exact active rollout
for the requested chat. Parley reads that row using system SQLite in read-only
mode, with a bound session parameter and a one-second busy timeout. It does not
run Python, install a database package, copy the database, or checkpoint it. A
missing row, corrupt/locked database, out-of-root path or mismatched header fails
closed. Without the index, only the existing unambiguous single-file resolution
is permitted. Index-backed selection on other platforms is currently unsupported.

For a linked rollout, `history_base.thread_id` identifies the physical prefix
rollout, not an arbitrary file with the same header ID. The exclusive byte and
ordinal boundary must agree at a complete record. Parley follows up to eight
same-chat segments, rejecting missing/ambiguous bases, cycles, reparse escapes,
invalid metadata or truncated prefixes. Compressed, archived and cross-chat
ancestors are not supported by this repair and are never guessed or silently
omitted.

The reader streams the bounded root prefix and subsequent child payloads. The
child's metadata is validated but is not duplicated as conversation content.
The original root cursor offsets remain unchanged. Appends to the live tip are
read normally; incomplete trailing records wait for a later call. Every source
file's identity and header, plus the active index selection, are checked before
context can be delivered.

Existing cursor records stay verbatim. New linked-history records use internal
cursor schema 2 to retain per-segment identity and geometry. An already-shared
extent must still map to the same physical bytes. A branch/revert behind that
cursor fails closed instead of silently jumping. Older binaries cannot read
schema 2 and fail closed; there is no automatic cursor downgrade or reset.
Event, health, handoff and scientific schemas do not change.

This is source-format support, not a relaxation of context admission. Existing
character limits, recovery authorization, prompt-injection boundaries, native
mention validation and uncertain-cursor checks continue to apply. No automatic
replay, skip, model retry, replacement native conversation or archive deletion
is introduced.
