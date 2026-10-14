# Continuation archive preflight repair

Base: 499fef57e7ed53a69a314372fd54b33aacd32277.
Branch: fix/continuation-archive-preflight. One Codex writer; no other writers.

The user approved this isolated repair after the fresh registered Workbench
review again refused before Grok started. The exact proposal was to repair
archive handling while preserving records and safeguards; the reply was
"Yep, go ahead." This is recorded as current-task solo-start authorization for
the bounded blocking repair, not normal tandem completion. Grok review of the
actual diff and test evidence remains required once the candidate can launch.

The guard loaded every profile's historical continuation manifests and performed
live grant-path checks while deserializing them, before worktree applicability
was established. Three September 29 canary manifests retain the former junction
paths. A Codex restart and Git pointer repair do not rewrite those immutable
records, and must not do so merely to make launches pass.

Scope: separate metadata parsing from publication/execution validation; retain
alias-aware review blocks for the same worktree, even when the grant expired.
No evidence deletion, manifest mutation, schema change, new dependency, generic
model change, relaxed permission, scientific change, GUI change, or main edit.
Registered binaries and live configuration remained unchanged during the first
candidate review. Accepted activation is recorded below.

## Validation and peer review

Root fmt, test, clippy with warnings denied, and release build passed. The suite
reported 452 passing unit tests, four ignored synthetic fixtures, and eight
passing CLI integration tests. All 29 continuation tests passed in the focused
run, including native Windows junction cases. Root Cargo files are unchanged.

An identical, no-model dry run under the locked development profile failed on
the old binary's junction guard and succeeded on the candidate. SHA-256 checks
confirmed the three original manifests and their journal remained byte-for-byte
unchanged. No archived record was removed, rewritten, expired away, or approved.

Grok independently examined the actual diff through a standalone candidate MCP
process with the complete unchanged locked development-profile environment.
Native session 496cf3af-7bc0-8e84-bcce-ab909a0dbd32 records grok-4.7 and xhigh.
The report accepted the repair while identifying a missing discriminating test:
canonical ancestor overlap where the saved junction is outside that ancestor.
Codex added that exact case to the alias regression, explicitly asserting no
lexical overlap, and documented the inability to follow deleted historical
paths without a durable identity. Production logic did not change after review.

Evidence: D:\Workplaces\R3 Bio\.parley-evidence\continuation-archive-preflight-20261014.
The exact report, test logs, dry-run comparison, binary hash, and archive hashes
are preserved. Standalone evidence is not registered-client verification.

DECISION: chosen = metadata-only archival parsing with alias-aware review
blocking; rejected = deleting records, rewriting grants, ignoring expiry,
weakening live path checks, or claiming a restart alone fixes the bug;
dissent = none on the bounded repair, with the resolvable-alias limitation
explicit; evidence = actual diff, native tests, A/B dry run and Grok review;
unknowns = deleted-path identity and the original install/update acceptance;
next = preserve
rollback, stage the accepted candidate, activate via host reload/restart, and
verify the actual registered clients before resuming the Workbench review.

## Accepted activation

Implementation checkpoint: 109f704. Final release SHA-256:
10ff1c48f2bde6fb4133b84009f42cf8cdee172a069fb0fed350370a16733337.
The final fmt/test/clippy/build run passed after the peer-requested test change.
Both old binaries remain available. The private Codex configuration backup is
in its existing config-backups directory. Exactly the development and Workbench
command lines changed; all other configuration lines were compared unchanged.
Two inline-verifier attempts were refused before execution by the command
guard; a native nonexecuting line comparison verified the change instead. No
guard or allowlist was weakened.

The supported host reload returned queued. Unlike the earlier attempt, actual
registered calls then used the new binary: development PID 42620 and Workbench
PID 38404 were independently verified by executable path and SHA-256. No MCP
server was externally stopped, and no desktop window was manipulated.

Development job 3b5c536a-193c-4298-9f45-d730eb445172 resumed native session
496cf3af-7bc0-8e84-bcce-ab909a0dbd32 with shared-context auto mode. Grok examined
and accepted the canonical-only ancestor regression through this registered
client. Codex received the exact durable handoff; the job succeeded with response
event event-1791997983989-42620-3. Receipt remained delivery only.

The original Workbench diagnostic review was then launched successfully through
its actual registered client: job 572c1359-c842-4d4e-ae2a-3b8d6e50b7d7, same
native session 9b22344e-7185-850a-8830-ca45eaf45a9e, request event
event-1791998009993-38404-2. Its substantive result is separate Workbench evidence,
not part of this repair's code acceptance. Both native summaries record
grok-4.7/xhigh. The original three archived manifests and journal still match
their pre-repair hashes. Neither source main changed, no GUI or scientific file
changed, and no push or release publication occurred.
