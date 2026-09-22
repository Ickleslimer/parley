# Parallel lanes acceptance evidence

This document records the local Milestone 5 acceptance method. It is evidence,
not a permission grant, and it does not replace the runtime lane manifest.

## Accepted topology

- Integration is read-only while writer lanes are active.
- One Codex writer and one Grok parent writer start from one immutable commit.
- Up to two Grok child writers are an optional ceiling, not a requirement.
  Child-bearing plans remain disabled until the exact installed CLI passes the
  typed-role capability and independent containment/permission canaries.
- Every writer has a distinct Git worktree and non-overlapping file grants.
- Grok children run at depth one with inherited locked model settings, no MCP,
  shell, web, workflow, memory, or recursive agent authority.
- Codex reviews complete diffs, runs independent tests, and integrates only
  provenance-bearing checkpoint commits in declared dependency order.

## Development canary

- Base commit: `c87c6752f2be5aec009f404e9cd28eb39f515aa1`
- Job ID: `c9d09f9a-b84e-46a8-8b4f-e89b44636991`
- Grok session: `d6564f64-10fb-47a4-bde1-af25577a03d9`
- Codex grant: `docs/parallel-lanes-acceptance.md`
- Grok parent grant: `README.md`
- Grok child 1 grant: `lanes/tests/adversarial.rs`
- Grok child 2 grant: `docs/parallel-lanes-operations.md`

The job returned `running` only after policy and shared-context preflight,
request logging, grant activation, Job Object containment, and successful Grok
spawn. This Codex-lane file was created while that contained process tree was
still active, proving writer overlap without sharing a worktree.

The contained parent then stopped without writing because its live tool catalog
did not expose a spawn tool. The installed Grok 1.0.40 documentation identifies
`spawn_subagent` as the `--tools` ID and reserves `Agent(...)` for deny-filter
syntax. The parent reported both missing child IDs, unchanged grants, and no
child results rather than inventing completion. The process tree exited cleanly
and all three Grok worktrees remained unchanged. A focused catalog regression
now requires `spawn_subagent` and rejects `Agent(...)` in the allowlist.

A second guarded run started from commit
`40f90b818e6eb2244c5cdb72555d332e464a5870` with job
`7ab8dc90-727a-4ba5-8ecf-750b2e9c534f` and Grok session
`0f3f941c-9168-4278-800e-354cdd88d359`. This evidence update occurred after
that job reported `running`, while its contained parent process was live.

That run also exited without child processes or lane writes. Its native
`tool_definitions.json` proved that `spawn_subagent` was absent even though the
literal internal name had been placed in `--tools`. Grok 1.0.40 expects the CLI
alias `task` in the allowlist and then exposes the internal tool as
`spawn_subagent`; no session on this machine exposes an internal `task` tool.
The captured history contained no spawn call, and all Grok lanes stayed clean.

A third guarded run started from commit
`9873a03061f158663e99483c4813a72305d6db5a` with job
`515893e9-9999-407b-8622-b923fbc649f8` and Grok session
`5ada7d6d-9269-4170-b897-2da5a5fa5c5b`. The parent catalog used the installed
CLI alias `task`; this evidence edit again occurred while the job was `running`.
The resulting catalog exposed `spawn_subagent`, and the parent emitted two real
spawn calls, but the captured schema had only `prompt`, `description`,
`background`, `isolation`, `resume_from`, and `cwd`. It omitted
`subagent_type` and named no Two Chairs child type. Both calls therefore omitted
the required role and Auto denied them as lacking a concrete inspectable effect.
No child session started, no Grok lane changed, and a later parent edit attempt
was also denied without writing. This proves the active parent must carry an
immutable `Agent(two-chairs-writer,two-chairs-reviewer)` catalog rather than
only receiving child definitions through `--agents`.

## Capability decision and parent fallback

The exact locked binary is `grok 1.0.40 (eb1a2256660d)`. Its live parent tool
schema does not expose a typed role field or any equivalent inspectable child
identity. Enabling write-capable native children would therefore require
weakening the lane contract. Parley now defaults
`PARLEY_GROK_SUBAGENT_TYPED_ROLES_READY` to false and rejects every
child-bearing plan before request logging or spawn. Guarded two-lane plans
remain available with native subagents forcibly disabled.

Dogfood 7 exercised that parent-only route from commit
`03d9e7f99870523e5f637106ba197fede2cd7d04` with job
`32f0a181-6822-416c-b2c0-1a5fd4062d9f` and session
`375470a3-f854-4304-8bef-f234124c84ec`. Codex committed its disjoint marker
while the contained Grok process was live. Grok attempted only its granted
file, but the canonical `\\?\` cwd did not match the normal drive path emitted
by its file tool. Auto correctly denied the write. The CLI then exited zero
with only progress text, exposing the need to validate terminal replies rather
than treating every zero exit as successful.

Commit `2e28c4de16d5115be851cd20212e1bcd550fb273` corrected both boundaries:
parent spawns use the normalized equivalent cwd while policy retains canonical
identities, and write jobs require the exact `TWO_CHAIRS_LANE_RESULT` block.

Dogfood 8 validated the correction:

- job: `c3a2b7bf-300d-44c1-a415-c57f4ec38e32`;
- Grok session: `aaba8ecc-55d7-47dc-a0c8-908097fd635f`;
- Codex checkpoint: `704d1053cc39c5f5326ec844ca7b72e91381ab35`;
- Grok-parent checkpoint: `de00c937f532dff21d90959bd872c879a8490395`;
- request/response exchange: `exchange-1790036485828-48564-1`;
- exact parent change: `docs/m5-canary-parent.md` only;
- native metadata: `grok-4.7`, `xhigh`;
- result: valid lane-result block, valid health footer, no child, no orphaned
  Parley or Grok process.

Parley reported the job running at `2026-09-22T01:21:28+01:00`; the Codex
marker was written at `01:21:52`; the Grok marker was created at `01:23:25`;
and the contained job succeeded at `01:23:42`. Integration stayed clean at the
recorded base throughout, proving overlapping isolated writers without sharing
a worktree.

## Fail-closed startup evidence

Two earlier development attempts never launched Grok:

1. Canonical Windows worktree paths exposed an invalid prefix-component probe.
   Preflight returned `ERROR_INVALID_FUNCTION`, all four lanes stayed clean,
   and no job, grant, request event, or native session was created.
2. Canonical `\\?\` paths were initially rejected while rendering exact Grok
   permission rules. The job journal and request/error event pair recorded the
   non-retriable pre-spawn failure; no Grok process or file change appeared.

Focused regressions now require path-chain validation to skip only synthetic
prefix/root components and permission rendering to strip only the extended
Windows prefix while continuing to reject actual rule metacharacters.

## Consequential diff review

Grok 4.7/XHigh reviewed the consequential integrated result through the
locked development profile. The first full-patch review ended with a provider
HTTP 500 (`c743d23f-5b9e-4622-8676-a9c22ad51104`), and the next bounded tree
review timed out without a complete reply
(`55f0254d-3bc4-4ad6-82b9-fe4b2edb81a2`). Neither failure was retried
automatically.

A narrower security-delta review then completed under job
`f14b2287-d259-46b0-aee8-03c5cf52ad23`, session
`b1ac494f-c413-4ff2-8f9c-58c2100f0197`. It requested defense-in-depth child
readiness validation at launch, whole-line terminal-result matching, clearer
normal-path permission evidence, and an R3 charter clarification about lane
checkpoint and integration timing. Commit
`a0407b9e474454cedc7059239e1d846785746d08` closed those findings; commit
`ab8e452aff59b3f4021536b4d67561e40f6ab82b` made the Windows extended-prefix
assertion unambiguous with an ordinary escaped string. Root format, all tests,
strict clippy, and release build passed after each commit.

The same Grok session reviewed the final one-line delta and reported no
remaining P0-P3 finding: **Milestone 5 accepted**. It independently retained
the fail-closed native-child decision for Grok CLI 1.0.40 and accepted the
guarded Codex plus Grok-parent topology with `--no-subagents`.

## Acceptance gates

The candidate is accepted only when independent evidence confirms:

- new and resumed native sessions report `grok-4.7` and `xhigh`;
- the active Grok writer process interval overlaps the Codex write interval;
- each lane changes only its exact grant and all process trees exit;
- when native children are enabled in a future exact-version profile, their
  IDs, roles, worktrees, grants, results, and denials appear in the parent
  report and all child-specific adversarial gates pass first;
- adversarial lane tests, root tests, clippy, and release builds pass;
- timeout and cancellation terminate contained descendants exactly once;
- hook operational, absent, crashing, and malformed-output canaries leave no
  route to shell, MCP, Task, recursive agents, or ungranted writes;
- event schema v1 and generic Parley defaults remain unchanged;
- widget preferences survive the installer upgrade;
- no push, pull request, deployment, main update, or evidence deletion occurs.

Any failed containment or independent permission canary keeps write-capable
native children disabled, regardless of model-level agreement.
