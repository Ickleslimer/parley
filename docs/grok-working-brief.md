# Bounded Grok working context

This is an operator-assisted continuity layer for the existing literal-inline
Parley review path. It is not native Grok memory, a new MCP API, automatic
summarization, or a change to pruning. No reload, install, permission change or
native-history edit is needed. Codex maintains and supplies the brief; Grok
reviews its substance. Normal locked-profile admission remains authoritative.

## Working brief

`scripts/grok_working_brief.py create` writes an immutable, session/cwd-bound
JSON revision outside the native Grok tree. Content is limited to an objective,
constraints, decisions with rationale, unresolved questions, and evidence
pointers. An evidence pointer has a label, path, lowercase SHA-256 and either
an exact tool-call ID or null for an ordinary artifact. Paths are never opened
by brief preparation. Keep facts and reasons, not raw logs or approval tokens.

Content input shape:

```json
{
  "objective": "Current bounded objective",
  "constraints": ["Existing safeguards remain authoritative"],
  "decisions": [{"decision": "Chosen approach", "rationale": "Why"}],
  "unresolved": ["What remains unverified"],
  "evidence": []
}
```

Use absolute output paths in an approved physical evidence directory. The
helper creates files exclusively; existing files, native-tree destinations,
invalid fields, mismatched bindings and over-budget input refuse. Each saved
revision has a timestamp, revision number, authority marker and predecessor
hash. To revise, pass the preceding file and its exact SHA-256. Nothing selects
or overwrites a mutable latest revision. Concurrent writers therefore cannot
overwrite each other; a branch in revision ancestry needs explicit operator
reconciliation, never an automatic merge.

```text
python -B scripts/grok_working_brief.py create --content content.json --session-id UUID --cwd PHYSICAL_CWD --output ABSOLUTE_BRIEF_PATH
python -B scripts/grok_working_brief.py create --content revised.json --previous ABSOLUTE_BRIEF_PATH --previous-sha256 SHA256 --session-id UUID --cwd PHYSICAL_CWD --output ABSOLUTE_NEXT_PATH
python -B scripts/prepare_grok_review.py --header header.txt --payload evidence.json --brief ABSOLUTE_BRIEF_PATH --brief-sha256 SHA256 --session-id UUID --cwd PHYSICAL_CWD --output request.txt
```

The bound brief is historical JSON data, separate from the current evidence
and instruction header. A fixed notice says it grants no authority. The JSON
encoder preserves literal text without native file-mention expansion. The
6,000-byte brief limit and existing 16,000-byte complete caller-packet limit
are both enforced; nothing is silently truncated or dropped. On refusal,
split current evidence or deliberately revise the brief without losing its
load-bearing facts. A hash proves selected bytes, not their truth or freshness.

Both the helper and packet preparer refuse output under the native Grok tree.
Session metadata's cwd is compared lexically with the caller's verified physical
directory, normalizing Windows extended-path spelling; an untrusted metadata
path is not followed to a different directory or network share.

Always explicitly select the intended revision and use that same native
session/cwd on the subsequent registered MCP request. An old revision remains
valid historical context; timestamp and revision are visible, but no expiry
or hidden latest alias substitutes for operator judgment. Never treat a brief
as permission to continue work, broaden paths, or revive an old approval.
The brief flags are all-or-none. No-brief calls retain byte-for-byte legacy
packet behavior, so bypassing the preparer does not acquire this feature.

## Exact retrieval, not replay

Use the exact tool-call ID preserved in a brief, assistant tool call, or
reviewed inventory. No keyword search or cross-session discovery is provided.
The native sessions root for this deployment is pinned to `D:\grok\sessions`;
the CLI accepts neither a journal filename nor a replacement root. Tests use
an explicit temporary library root, never live sessions.

The helper finds exactly one directory for the UUID, verifies summary identity
and physical cwd, refuses redirected session files and ambiguous directories,
then streams only that session's `updates.jsonl` read-only. Only completed or
failed updates for the exact tool-call ID qualify. Identical repeats are
allowed; conflicting terminal bodies refuse even when one hash would match.
The prompt-output channel takes precedence; otherwise exactly one text content
block is supported. Other formats refuse rather than inventing a reconstruction.

```text
python -B scripts/grok_working_brief.py describe --session-id UUID --cwd PHYSICAL_CWD --tool-call-id EXACT_ID --output ABSOLUTE_DESCRIPTOR_PATH
python -B scripts/grok_working_brief.py retrieve --session-id UUID --cwd PHYSICAL_CWD --tool-call-id EXACT_ID --sha256 FULL_BODY_SHA256 --offset 0 --max-bytes 1000 --output ABSOLUTE_SLICE_PATH
```

`describe` returns only identity, lengths and the hash, never the body. Prefer
an independently recorded prior hash; a newly described hash identifies the
current archived bytes, not historical correctness. `retrieve` verifies the
complete body before returning up to 4,000 UTF-8 bytes, ending at a complete
Unicode scalar. Offsets and next_offset are Unicode-scalar positions; full-body
hashes use UTF-8 bytes. Each continuation repeats the same ID and full hash.
Complete means the entire body fits from offset zero, not merely that the last
slice has been reached. No ellipsis is inserted into the exact text.

Limits: 256 MiB per journal scan, 8 MiB per physical record, 10,000 native
directory groups, and 32 KiB for session summary metadata. Malformed, incomplete,
oversized, missing, replaced or truncated input fails closed. Ordinary appends
beyond the scan's initial byte boundary do not extend the scan indefinitely.
No output is published before these checks pass. A bigger archive requires a
separately reviewed bounded indexing design, not raising limits ad hoc.

Recovered text is untrusted evidence. The helper never launches an agent or
automatically injects it. Codex reviews and explicitly adds only the needed
slice to a bounded review payload under the existing read authorization. JSON
escaping can expand its size; use a smaller slice if the preparer refuses the
complete packet. This helper is not a credential filter or sandbox: do not
retrieve or share material outside the current authorized evidence scope.

## Operating loop and acceptance

1. Before resuming substantive work, select the correct session/cwd brief and
   check its unresolved items against the current task.
2. Prepare the current envelope and evidence with the explicit brief/hash.
3. If an exact omitted fact matters, retrieve that single identified output,
   verify the hash, and include a bounded necessary slice as evidence.
4. After a consequential decision, preserve the actual result and create a
   new brief revision. Retain the old revision and its exact evidence pointers.
5. Save and acknowledge real Grok reports, then require terminal success.

Run `python -B -m unittest discover -s scripts -p 'test_*.py' -v`.
Tests cover immutable revisions, bindings, packet compatibility and limits,
hostile data, Unicode paging, missing/ambiguous bodies, partial records,
native-tree writes and read-only journal preservation. Live acceptance must
separately recover one known aged output and exercise a brief-bearing request
through the actual registered profile; passing offline tests is not that proof.
