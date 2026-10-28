# Automatic history sharing restored

RESULT: PASS — 20 October 2026.

## Delivered

The active `parley-widget-dev` and `r3-workbench-parley` registrations now use
`par-paginated-context-08dbca8c5eb5c4e7.exe`, 3721728 bytes, SHA-256
`08dbca8c5eb5c4e718b5a606af5a14210c45237d4796ed8a239d7643ed47f769`.
Other registered profiles were deliberately left unchanged.

The existing native Grok conversation `f09b6874-3b8e-875a-87ba-314c7400ae54`
received an actual automatic delta through the registered MCP client. No new
native session, reset, replay, skip, cursor rebase or source-log edit was used.
The previously working isolated maintenance route was used only to bootstrap
co-framing and source review, not as the acceptance canary.

## Cause and fix

Codex now retains more than one physical rollout for this stable chat. The old
resolver treated their identical stable header IDs as an ambiguous duplicate.
The active suffix identifies a different physical rollout, whose `history_base`
explicitly retains a bounded prefix of the original file.

Parley now selects the exact native index pointer read-only, validates that
bounded same-chat lineage, and streams its logical history without copying the
multi-gigabyte archive. It retains the original root identity and byte offsets,
then joins child payloads after their validated metadata. Every shared physical
extent remains identity-bound. Replacements, truncation, branch changes behind
the cursor, malformed boundaries, unknown newer database versions and ambiguity
fail closed. Character limits and all existing recovery/permission controls stay.

Old cursor records remain byte-for-byte intact. Linked-history appends use the
internal schema-2 extent evidence; old binaries refuse that schema rather than
misinterpret the virtual offsets. There is no automatic cursor downgrade.

## Independent evidence

- Canonical job `9123371a-0d67-44e6-ac59-e91f28606964`, exchange
  `exchange-1792535669513-16952-1`, new registered server PID16952.
- Grok independently quoted `history-check-7c9e4b2a61d8`, supplied only through
  newly injected Codex commentary, not the current canary prompt. It identified
  the user's latest requested repair correctly.
- Context range 3222195708 → 3254767373, 1558 physical records, 5866 visible
  scalars, not truncated. It crosses the retained prefix boundary 3222474611,
  including the previously unread 278903-byte prefix gap before child content.
- Durable report saved in CANONICAL-CANARY-20261020.md; genuine receipt for
  handoff `1af7f73b-915a-4354-8bec-dbf317a164b4`; terminal succeeded with
  completion event `event-1792535791539-16952-3`.
- `build/canonical-cursor-verification.json` independently checks unchanged old
  journal prefix and exactly one appended in_flight/committed pair. Before SHA
  `3189c9c78a46e25c7bd60138ed3363b530fbe7fea0c6aad80de64044dba590ec`;
  after SHA `b154f727b5d2a03de8890cdabf038e81f65786ece52d1acb24ad9003bdd66cc5`.

The final dry-run prompt buffer was 9550 scalars; that includes framing/current
request and is not the same quantity as the actual delta's 5866 visible scalars.

## Validation and collaboration

492 unit tests and 9 CLI integration tests passed; 4 tests remain ignored.
Fifteen new native Windows tests cover source selection, retained inactive forks,
legacy cursor continuity, nested histories, exact boundaries, missing/corrupt/
ambiguous indexes, replacements, truncation, BOM/Unicode, oversized sanitization,
reparse escape, owner mismatch, depth, and a newer native database version.
Fmt, production clippy and release build passed. All-target clippy passes with
only two disclosed pre-existing manual_repeat_n test warnings excepted; those
unrelated baseline lines were not changed. Six promotion-gate and six exact
configuration-replacement tests also passed.

Grok co-framed and reviewed actual source in jobs `da2b2ae6-f0ea-4216-b962-cb79ba209db9`,
`d826688d-9c39-4e9d-a816-de488904b238` and `aef17497-9c1a-4e11-be99-2bac1992ad7c`.
Reports are preserved with genuine receipts and terminal success. Its initial
SQLite dissent was surfaced and then withdrawn against actual counterexample
tests. Final promotion conditions were implemented and tested: exact nonempty
runtime hash set, SHA-pinned pure replacement helper, unchanged ACL, and verified
host-side reload availability. The live canary provided the final delivery review.

## Activation and preservation

Only two configuration command paths changed. Parsed configuration comparison
proves all model/effort, permission/environment, path roots and other profiles
unchanged. Original config is privately backed up under config-backups. SDDL and
inheritance are identical. The host-owned reload bridge performed reload-all;
both actual registered clients returned successful round trips. Server process
images match the accepted hash, parent app-server8708; no old8c servers remained
in the captured inventory. No external stdio-server termination was used.

The viewer stayed running as PID31448. Its settings SHA remains
`20d8a1aa2e8a8e899d209bb457c66a496ffc4afc3eddd964fe2f15a00a849d9e`.
No GUI rebuild/install, desktop manipulation, focus change, R3 product change,
scientific-schema change, network feature, credential or permission change was
made. No commit, branch creation, push, merge or main-checkout edit. The canonical
Parley checkout remains bf255c2274d0e0f57a2f59b4b23b50b05cbd2ed3 with its original
dirty README and backup preserved. This implementation is in a detached worktree
from329c1cc plus the accepted async-image baseline.

Staged source patch SHA `f83df03132d72e9b9b5ce6a1a8f0f98382f0b69d05d1d00f6185c56fa0105249`;
new source copies and per-file hashes are in build/staged. Rollback binaries,
both original logs, cursor preimage, failed test evidence and all review reports
remain. Own stale Rust output was pruned only after builds ended. D: free space
was about67.0GB before and67.0GB after, well above the floor.

## Remaining boundaries

Compressed/archived ancestors and cross-chat fork imports are unsupported and
fail closed. A native state schema newer than5 requires a reviewed adapter.
Older unmodified registered profiles have not been promoted. Existing overflow,
uncertain-state and current-request mention checks can still refuse a request;
this repair does not bypass them. The independent natural-reboot widget check
remains open; no reboot was performed for verification.
