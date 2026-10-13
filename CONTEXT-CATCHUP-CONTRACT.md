# Explicit bounded context catch-up

Ordinary auto/delta resumes retain exact-through-EOF semantics and both existing
caps. No automatic model drain, retry, truncation, cursor reset, skip, profile
expansion, or new schema field is permitted.

An explicit synchronous `context-mode=catchup` accepts only the literal prompt
`Catch up shared context only.` and an existing committed Codex-to-Grok cursor.
Parley renders the current R3 envelope itself: receive this exact context prefix,
perform no task or file work, return the exact generated receipt line, then the
existing required health footer. No arbitrary task rides on a partial delta.
Async catch-up is rejected, so an early durable task report cannot commit it.
Uncertain or in-flight states remain fail-closed and require existing explicit
recovery; catch-up is not a recovery action.

The planner interface is:

`plan_catch_up(source: &ResolvedSource, reader: &mut SharedJsonlReader,
start_offset: u64, bounds: &ContextBounds) -> Result<ContextPlan, ContextError>`.

It selects the oldest unread whole turn groups fitting min(delta, hard), cuts
at the first rejected turn start, rescans the selected range and validates
exact records/messages. It returns the existing PlanKind::Delta, with no front
truncation and no ContextPlan field changes. The caller renders
`backlog_remains: true` from catch-up mode, not persisted metadata. A fitting
entire tail refuses catch-up and directs the caller to normal auto/delta. A
first unread turn too large or an invalid/non-progressing boundary fails closed.
No message or turn is split. Excluded records remain in the cursor fingerprint.

The receipt binds exchange ID, offsets and fingerprint. Only its exact first
line plus the locked footer (when required) permits the normal synchronous
commit. Invalid, missing, empty or indeterminate replies leave context uncertain.
Successful commit advances only to the selected cut; later task calls still
receive every remaining eligible record. All existing source identity, sharing,
anchor, journal locking and prompt-file lifetime checks remain in force.

Grok owns src/context/plan.rs and src/context/tests.rs in its isolated lane.
Codex owns policy, modes, journal APIs, CLI/MCP schemas, receipt validation,
transport, documentation, integration, activation and independent tests. The
integration checkout stays read-only while lanes write. Native children are off.

Measured live backlog: approximately 137,089 visible scalars across 547 messages;
largest observed turn 7,117. Measurement omitted oversized records; the existing
native dry-run independently passed at 256,000 and failed at 128,000, without
altering the real journal. Those caps have not been changed. Existing canonical
session and cursor are preserved. The user requested this repair on 2026-10-13.
