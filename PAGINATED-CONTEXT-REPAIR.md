# Paginated shared-context repair

Status: complete, activated and canonical-delivery verified, 20 October 2026.
See PAGINATED-CONTEXT-RESULT.md for the final receipt. The checkpoint sections
below preserve earlier progress and are not the final status.

## Objective and boundaries

Restore automatic Codex history sharing in the existing native Grok session.
Preserve both physical source rollouts and every existing cursor record. Do not
select an active branch by mtime, reset a cursor, replay or skip context, change
permissions, or disturb the recovered desktop viewer.

This detached worktree starts at `329c1cc2f91a97503b972dac6fd28a6d97388467`
plus the accepted async-image patch, SHA-256
`69b11aaeb69f310ccb4f8ce971c2fbc6b99e5298a524b68f851edb7d9e99327b`.
Those existing changes are baseline, not part of this repair. Main and the
accepted async-image worktree are untouched.

## Evidence

The active native index points to the October 20 suffixed rollout. Its stable
session ID matches the September 19 origin. Its explicit history base retains
the origin through byte 3222474611, excluding ordinal 203969. The record ending
at that boundary has ordinal 203968; the child's metadata has ordinal 203969.
The existing shared cursor ends at 3222195708, so the retained gap cannot be
discarded when switching to the child.

The original physical log is over 3.22 GB. Logical streaming is required; no
second raw archive will be made. The Windows system SQLite DLL is present, but
the existing root crate has no dependencies. A proposed read-only index bridge
must not add Python, an external SQLite executable, or runtime network access.

## Work sequence

1. Co-frame active-source and identity rules with Grok.
2. Implement bounded lineage resolution and logical streaming with compatible
   append-only cursor identity evidence.
3. Exercise synthetic fork, replacement, truncation, boundary and cursor cases;
   independently review the actual diff with Grok.
4. Build and stage the CLI; use a verified host-owned reload if promotion is safe.
5. Prove the canonical registered-client path, or report activation pending.

## Baseline validation

`cargo test context:: -- --test-threads=4`: 66 passed, 0 failed.

Grok co-frame job: `da2b2ae6-f0ea-4216-b962-cb79ba209db9`, maintenance resume
of `f09b6874-3b8e-875a-87ba-314c7400ae54`, read-only, no automatic transcript
injection. This route bootstraps diagnosis; it is not proof of repaired sharing.

## Implementation checkpoint

The co-frame report is saved in COFRAME-20261020.md, genuinely acknowledged and
terminal succeeded. The explicit decision and the remaining SQLite-selection
dissent are in DECISION-PAGINATED-20261020.md. Source review is running as job
`d826688d-9c39-4e9d-a816-de488904b238` on the same native session, reading the
actual frozen source files. No other writer is active in this worktree.

- Focused context tests: 80 passed, including 14 new native Windows tests.
- Full root suite: 491 passed, 4 ignored; 9 CLI integration tests passed.
- Production clippy and release build passed; fmt check passed.
- All-target clippy found two pre-existing manual_repeat_n warnings and one
  new test-only cloned-reference warning. The latter remains to be corrected
  after the frozen-source review; the former are unrelated baseline debt.
- Promotion helper tests: 4 passed; reused exact config replacement tests:
  6 passed. No configuration changes have been made.
- Real-source canonical dry run passed, targeting the existing f09 session:
  build/live-dry-run-1.json. Prompt-buffer size was 9399 scalars. Live journal
  SHA-256 stayed 3189c9c78a46e25c7bd60138ed3363b530fbe7fea0c6aad80de64044dba590ec.
- Candidate SHA-256: ea1534dcb872c2e7c7f8e2203afe69c6142698ab49d2d77d90d21b92346153d8.
- Host-owned reload control verified through the current registered profile.
  Activation, actual registered canonical model round trip and final acceptance
  are still pending. The healthy viewer is untouched.

The initial raw-reader test caught over-strict path verification after a legal
rename; raw test readers retain handle-based behavior, while resolved production
sources verify segment identities. A new junction fixture initially used a
forward-slash path rejected by mklink; its native path construction was corrected.
Neither test failure was treated as evidence that the live source had changed.
