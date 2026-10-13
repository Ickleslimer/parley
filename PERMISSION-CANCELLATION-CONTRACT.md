# Permission cancellation repair

Base: `08a94d82d0f7f656ea5c40c3775ffe846d0ac987`. Co-framing review:
`b04a5836-90af-4701-bca9-c5680b663d30`, canonical Grok session
`2970324b-91ab-8f46-a7e3-acaa77a65cf3`, Grok 4.7/XHigh.

## Frozen interface

`AskOutcome` adds `reply_result: Result<String, String>`. This is the exact
post-observer completion result used for event logging. Async workers and MCP
must consume it rather than recomputing a result from observer-consumed output.
Execution cancellation and timeout flags remain separate from native stop
reasons. Generic capture behavior stays unchanged.

The handoff stream retains the sanitized terminal status already admitted by
`decode_stream_line`. Only `end_turn` is a successful terminal reason. A
cancelled, refused, limited, missing or unknown terminal reason never promotes
an acknowledged report to normal success. No new durable fields or state enums.
Native cancellation is not attributed to permission timeout without separate
evidence. Preserve parser/incomplete/missing-end diagnostics and prioritize
actual Parley cancellation/watchdog evidence over native stop status.

## Lanes

- Codex: `src/ask.rs`, `src/mcp.rs`, `src/policy.rs`, `tests/permission_cancellation_cli.rs`.
- Grok: `src/job_runtime.rs`, `src/stream_activity.rs` and tests inside those files.
- Integration: read-only while lanes are active; Codex owns documentation,
  independent integration tests, Git, final build and activation.

Grok may call `outcome.reply_result` before the Codex change is integrated; its
standalone build will then fail at that known dependency. It must not change
other files to satisfy the compiler. Run focused tests after integration.

## Boundaries and acceptance

Preserve Auto, no-yolo, closed stdin, required sessions, immutable denies,
Job Objects, context outcomes and every event/context/health/handoff schema.
No automatic retry, permission answers, original HLA review replay, old cursor
recovery, native executable modification, global config mutation, GUI changes,
push or scientific changes. Prefer built-in reads and direct Git commands as
guidance only; do not call that enforced prevention.

Test observer-swallowed output, exit-zero cancellation, ordinary missing
report, acknowledged report followed by unsuccessful end, unknown reasons,
manual cancellation/watchdog precedence, live versus rehydrated job errors,
exact event-log matching and no hidden/raw payload disclosure. Run root format,
test, clippy and release checks before any registered binary change.

The native approximately 30-second permission wait remains a distinct open
question. No verified Auto-preserving switch was found in native 1.0.44 help
or installed documentation. Do not claim this diagnostic repair removes it.
