# Grok image and synchronous lifecycle boundaries

An ordinary synchronous call is not an acknowledgement-required job. The
handoff Stop hook must still reject missing, partial, stale or unacknowledged
job capabilities when a call claims that scope.

The MCP server itself has static handoff configuration, including
`PARLEY_HANDOFF_STATE_DIR`. Passing that configuration unchanged to an ordinary
Grok child falsely claims job scope. Grok CLI and ACP invocations now explicitly
remove the seven inherited binding variables before applying their invocation
overrides. A managed asynchronous job supplies its real binding after removal.
Other harnesses and the parent server environment are unchanged. No hook,
permission, immutable denial, health-footer rule or receipt check is relaxed.

## Images

Images remain a Windows-only synchronous `ask_agent`/`par ask` capability.
Asynchronous image requests are still refused; this patch does not add an
asynchronous image schema or silently convert calls into jobs.

Start a native session with a text turn, then resume that exact session from
its original canonical worktree. Before context resolution or ACP launch, the
image path checks the native session's authoritative summary, its identifier
and canonical working directory. A missing session, unavailable/invalid
summary, ambiguous identity or mismatched cwd fails closed with a distinct
`image_session_*` diagnostic. A failed preflight does not advance or poison the
context cursor. Legitimate directory aliases are canonicalized, not treated
as a session migration.

The existing private image MCP broker delivers validated images. ACP load
and prompt requests do not contain inline image parts. Source paths and image
bytes are excluded from Parley's event log; normal authorized image delivery
to the native provider is not a claim that the provider never stores images.
Size/count limits, one-shot broker delivery, read-only profile, cancellation,
containment and temporary bundle cleanup remain in force.

A text success does not prove image delivery. Native acceptance needs a
matching-session synthetic image canary, actual broker consumption, a correct
blind visual answer and event/cursor privacy checks on the same candidate.
The initialized synthetic canary uses the existing uncontrolled image-input
profile. An empty initialized lane-state directory is not a controlled-lane
grant; controlled profiles still require their genuine authorized grants.
An ACP internal load failure after canonical identity passes remains a separate
transport failure, not permission to retry, change sessions or weaken checks.

## Activation

An unregistered side-by-side candidate is not an active registered MCP repair.
Preserve the old processes until a verified host-side reload/restart can be
performed as the same activation operation. After activation, require actual
registered-client checks; a separately spawned canary cannot prove the app's
cached transport has been replaced.
