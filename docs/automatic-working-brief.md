# Automatic working context

This feature requests a small model-generated summary in the ordinary Grok reply,
stores a valid proposal after successful completion, and includes it on the next
call. It adds no model call, provider, tool permission or native-history edit.
Summaries are fallible reported beliefs, never verified decisions, user rules,
approvals, continuity authority or scientific conclusions.

Enable administratively with `PARLEY_WORKING_BRIEF_ENABLED=true` on an existing
locked profile that has asynchronous execution leases and an event log. There
are no per-call flags for Codex to remember. The shared execution boundary serves
synchronous, asynchronous and image calls. Context-receipt-only catchup skips the
feature. Disabled profiles are unchanged. Activation is separate from building:
preserve registered processes until a verified host-owned reload is available.

## Lifecycle

1. After normal policy preparation and session/cwd resolution, acquire the
   existing execution lease. Read the binding-specific head, never scan for a
   guessed latest revision. The key includes profile, physical cwd and session.
2. Append explicitly untrusted historical context and an optional proposal
   template. The current request stays byte-for-byte intact. Generated historical
   JSON escapes native file-mention characters and delimiter brackets.
3. A proposal has exactly `prior_sha256`, `reported_objective`,
   `reported_decisions`, `open_questions`, and `evidence_references`. The last
   three are arrays of at most 16 nonempty strings. References are inert: they are
   never opened. Raw JSON is at most 4000 UTF-8 bytes, with no duplicate/unknown
   fields. Delimiters name the exact invocation exchange ID; this is a correlation
   check, not an authentication token or protection against model hallucination.
4. Parse only a successfully validated, saved assistant reply after context,
   health, cleanup and lane completion checks. Never inspect tool output or
   promote failed, cancelled, timed-out, uncertain or unsaved turns. The optional
   block goes before the health footer inside a genuine durable async report;
   existing handoff parsing and receipt requirements stay unchanged.
5. Exclusive-create and flush the immutable report with reply/event hashes and
   server-owned binding. Check the expected head hash, retain a checkpoint, and
   atomically replace the head. The existing cross-process profile/session lease
   serializes publication; a stale head refuses without merging.
6. The next call automatically receives that report. Missing/invalid proposals
   leave the last-good report selected and record a visible stale reason. No
   response-protocol failure or automatic retry is introduced. A persistence
   failure emits a stderr diagnostic and never changes the saved task result.

## Storage and limits

State lives under `PARLEY_JOB_STATE_DIR/working-brief/<binding-sha256>/`, outside
native Grok storage. Reparse paths are refused. Reports and head checkpoints are
immutable; `head.json` is the only automatically replaced selection pointer.
Restart reads that pointer; orphan reports from interrupted publication are
never guessed into use. No automatic replay or archival deletion is implemented.

An optional `seed.json` contains `sha256` and the exact string `body` of a
previous immutable operator brief (at most 6000 bytes). Its session/cwd/hash and
untrusted authority must match. Automatic publication never changes this seed.
It remains historical context, not higher-priority user policy. No seed is
required for a new session's first successful automatic proposal.

Current request plus generated context/template must fit 16000 bytes. If not,
omit the entire feature for that invocation and state `omitted_budget` where
space permits; never trim the task or let a new summary overwrite history it
did not receive. This conservative all-or-none rule differs from separately
dropping seed/report components. The native complete-query limit is unchanged.
If even the status cannot fit, stderr records it and the request is unchanged.
Missing/corrupt/unavailable state likewise does not block the user's task or
select an older revision. Update failure never means the model task failed.

A failed/crashed turn leaves the last successful summary and its dated event
references; there is no claim that it incorporates that failed turn. The next
successful reply can update it. Publication remains optional because forcing a
memory trailer would make real work fragile. Fluent but inaccurate summaries
remain possible; schema/hash checks establish provenance, not semantic truth.

Scope excludes direct interactive native Grok, `par -p` routing, other harnesses,
new permissions, hidden summarizer calls, automatic tool-output retrieval and
native pruning changes. The existing exact-ID/hash retrieval helper remains
available separately. No GUI or scientific schema is changed.
