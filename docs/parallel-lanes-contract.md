# Parallel lanes contract

This document freezes the Milestone 5 interfaces before writer lanes split.
It is an implementation contract, not a permission grant. Runtime policy and
the caller's task envelope remain authoritative.

## Background job API

The MCP server adds `start_agent_job`, `get_agent_job`, `list_agent_jobs`, and
`cancel_agent_job`. `start_agent_job` requires a caller UUID and a
`job_mode` of `write`, `review`, or `probe`. It accepts the existing
`ask_agent` fields plus an optional `lane_plan`.

A write `lane_plan` contains:

- one immutable Git base commit and one integration worktree;
- exactly one `codex` lane and one `grok_parent` lane;
- zero to two `grok_child` lanes;
- a canonical worktree, role, and non-glob `file` or `tree` grants per lane.

Child-bearing plans have an additional fail-closed capability gate. They are
rejected unless `PARLEY_GROK_SUBAGENT_TYPED_ROLES_READY=true` and guarded mode
is active. The flag defaults to false and may be enabled only after the exact
installed Grok CLI exposes typed child roles and passes the independent tool,
hook-failure, containment, and inheritance canaries. Two-lane plans containing
only Codex and the Grok parent remain valid while this gate is closed.

The same job UUID plus the same canonical request fingerprint is idempotent.
The same UUID plus a different fingerprint is rejected. Start returns only
after request logging, context and health preflight, durable `running` state,
and contained process creation have succeeded.

Job schema v1 contains sanitized metadata only. States are `preparing`,
`running`, `cancelling`, `succeeded`, `failed`, `timed_out`, `cancelled`, and
`interrupted`. Exact prompts and replies remain in event schema v1. A usable
reply whose completion event cannot be written is preserved in a create-new
diagnostic escrow file.

## Process boundary

Locked asynchronous Grok runs use a Windows Job Object. Parley creates the
root process suspended with explicit pipes, assigns it to a kill-on-close job,
then resumes its retained primary thread. Cancellation and watchdog expiry
terminate the exact tree once and reap its root and readers. A process that was
created but could not be assigned, resumed, or waited is treated as started;
stateful context therefore becomes uncertain.

The synchronous `ask_agent` path uses the same contained runner and locks.
Generic non-Windows and unlocked invocations retain their existing process
path.

## Durable ownership

Job UUID, target session, profile writer slot, and canonical worktree leases
are cross-process. Job metadata is namespaced by caller, while worktree leases
are global beneath the configured job-state root. Startup converts durable
`preparing`, `running`, or `cancelling` records without a live worker to
`interrupted`.

Every configured worktree must exist below the approved cwd root, be a real
Git worktree with the declared common-dir identity and base commit, and be
distinct from every other lane. Writer grants may not overlap after canonical
resolution. Reparse points, sibling-prefix tricks, ambiguous new-file
ancestors, and missing paths fail closed.

## Subagents

Locked calls emit `--no-subagents` until guarded mode is explicitly enabled.
Guarded mode injects only `two-chairs-writer` and `two-chairs-reviewer`.
Children use pre-created worktrees with `isolation:none`, inherit the parent
model and effort, receive no MCP servers, and cannot use shell, web, MCP, Task,
or caller-supplied tool definitions. Depth is one and the child limit is two.

Guarded configuration alone does not authorize a native child. With no child
lane declared, Parley keeps `--no-subagents`, injects exact parent-lane
`Edit(...)` and `Write(...)` rules, denies the integration and Codex worktree
roots, and creates no child profile or grant state. On Windows, the tool-facing
spawn cwd and every allow/deny permission pattern strip the canonical `\\?\`
prefix; the stored canonical identities remain authoritative for policy and
lease checks. Parent file tools receive both normalized absolute and
worktree-relative rules.

Every write job must finish with the exact block below. A missing, mismatched,
or partial block is a non-retriable failed job even if Grok exits zero; captured
stdout remains available in the terminal job result.

```text
TWO_CHAIRS_LANE_RESULT
job_id: <caller UUID>
status: completed
```

`parley-lane-hook.exe` reads create-new grants beneath the configured lane
state root and validates role, cwd identity, path grants, model inheritance,
depth, and slot consumption. The journal stores no prompts, replies,
credentials, raw environment values, or raw command arguments.

## Ordering invariants

The existing ask order remains authoritative:

1. validate policy and context;
2. allocate exchange identity and resolve the exact prompt;
3. materialize prompt transport;
4. durably write the request event;
5. persist context in-flight and health request-started evidence;
6. persist job state and spawn the contained process;
7. commit usable context, otherwise mark it uncertain;
8. write response/error, health completion, and footer diagnostics;
9. reap before deleting the prompt file.

No timeout, cancellation, logging failure, transport failure, or restart
automatically retries Grok.

## Lane ownership for Milestone 5

- Codex: ask/process/MCP integration, Windows containment, policy, profiles,
  installer, Git integration, and final acceptance.
- Grok parent: `src/jobs/**` durable job state, recovery, idempotency, escrow,
  and focused tests.
- Grok child 1: independent `lanes/**` package, grant validation, hook binary,
  and tests. Hook registration remains Codex-owned.
- Grok child 2: black-box async MCP contract tests and documentation, without
  editing implementation modules.

The integration worktree is read-only while lane writers are active. Codex
checkpoints each stopped lane and cherry-picks accepted commits in dependency
order. Conflicts stop integration.
