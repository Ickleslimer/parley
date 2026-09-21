# Parallel lanes acceptance evidence

This document records the local Milestone 5 acceptance method. It is evidence,
not a permission grant, and it does not replace the runtime lane manifest.

## Accepted topology

- Integration is read-only while writer lanes are active.
- One Codex writer, one Grok parent writer, and at most two Grok child writers
  start from one immutable commit.
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

## Acceptance gates

The candidate is accepted only when independent evidence confirms:

- new and resumed native sessions report `grok-4.7` and `xhigh`;
- parent and two child process intervals overlap the Codex write interval;
- each lane changes only its exact grant and all process trees exit;
- child IDs, roles, worktrees, grants, results, and denials appear in the
  parent report;
- adversarial lane tests, root tests, clippy, and release builds pass;
- timeout and cancellation terminate contained descendants exactly once;
- hook operational, absent, crashing, and malformed-output canaries leave no
  route to shell, MCP, Task, recursive agents, or ungranted writes;
- event schema v1 and generic Parley defaults remain unchanged;
- widget preferences survive the installer upgrade;
- no push, pull request, deployment, main update, or evidence deletion occurs.

Any failed containment or independent permission canary keeps write-capable
native children disabled, regardless of model-level agreement.
