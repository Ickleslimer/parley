# Handoff framing diagnostics and prevention

16 October 2026. Base95782e3b282aa888319ead0d393d0849d2e4c3e2;
isolated branch fix/handoff-framing-boundaries. No main, GUI, scientific,
dependency, workspace or schema changes. Existing evidence remains intact.

Workbench review9371a271 failed because its matching start marker was neither
on its own line nor at the existing independently tracked tool/message
boundary. The original preceding byte and boundary position were not retained.
An inline marker and flattened consecutive text fragments cannot be
distinguished retrospectively; neither is asserted as proven. The parser abort
also prevented observing the terminal end record, so the old missing-stop
wording was not independent evidence of a second provider failure.

Grok4.7/XHigh co-framed this repair through the locked development profile in
native session496cf3af-7bc0-8e84-bcce-ab909a0dbd32. Its findings drove closed
boundary/preceding-byte classifications, unchanged strict validation, and the
distinction between an unobserved stop after abort and a fully parsed absent
end. A follow-up synchronous review from a different cwd could not restore
that session (404); its local record still exists under the original cwd.
This is not evidence that the original history was lost. No report, receipt
or success is inferred from that failure, and no
native session or context journal is rewritten to make it pass.

The report instructions now explicitly require two LF characters before the
start marker and unadorned own-line markers. This prevents the ambiguous
concatenation pattern when the model follows the contract; it is not a claim
that prompting guarantees compliance. The runtime still rejects inline,
duplicated, oversized, malformed or unterminated reports. It never inserts a
newline, treats every text fragment as a message, reinterprets prose, or
extracts a report from an uncertain native transcript.

Diagnostics expose only closed boundary/preceding-byte classes, never the
rejected tail, thought text or raw tool content. Stop descriptions distinguish
unobserved_after_parser_error, unobserved_after_incomplete_record, an observed
closed stop reason, and a fully parsed missing end. Receipt, process-success,
durability and end_turn requirements remain unchanged.

Tests cover absent, before-marker and after-marker boundaries, CR/space/tab/other preceding
classes, split markers and UTF-8, explicit newlines after commentary without
a tool boundary, secret non-persistence, refusal of premature receipt, and
stop diagnostics. Existing collision, exact-report, timeout/cancel, containment
and duplicate-report tests remain authoritative. One earlier malformed test
expected the old missing label; only the intended diagnostic expectations were
updated. An exploratory rejected-tail persistence test failed, then that
approach was discarded following Grok's recommendation; persistence did not
expand.

An acknowledged candidate review delivered a durable report, accepted explicit
receipt and exited0 through the corrected contract, without a parser failure.
Job8cfb33b6-35a4-4be0-a092-6e49860c4218, native session2f737c9b-b995-4e33-a50e-e0947076fa22,
Grok4.7/XHigh. Its consequential dissent was the missing after-marker regression
and overbroad coverage wording. Codex added that observe-path case, tab coverage
and negative incomplete-stop assertions before promotion. Its receipt was
delivery only, not code approval. The candidate harness reached graceful EOF
and its local controller timed out, but the exact known server kept draining
the same job; the registered development client received its report and
recorded receipt. An independently held process handle verified server exit0,
without termination or a duplicate launch. Failed harness evidence is retained.

After the correction, root format,456unit tests4ignored plus8CLI integration,
strict clippy and release build pass. Final binary SHA-256:
8aede163c8f6abd4378bce45bf575f55b0bdd7af2ac4defae76ec7dac456b7be.
Grok's same-cwd focused follow-up in session2f737c9b accepted both corrections
with concrete reasoning against the actual diff. Production logic did not
change after its candidate review. Root Cargo files remain unchanged.

Candidate review/controller/test logs are preserved under
`D:\Workplaces\R3 Bio\.parley-evidence\handoff-framing-boundaries-20261016`.
Promotion and registered-client verification remain separate gates; the old
registered processes are never externally stopped. The Workbench footer now
has its own exact-image VM evidence; its final integrated peer review remains
a Workbench task, not acceptance inferred from this parser review.

## Registered activation

Final source checkpoint1f09830. Both candidate binaries were staged side by
side with the rollback10ff1c48 build preserved. The private configuration was
backed up; a parsed structural comparison proved only the development and
Workbench command fields changed. All locks, version/model/effort, scopes,
context, health and handoff configuration remained identical.

The supported mailbox returned queued for global MCP reload. No registered
process was externally stopped and no app restart or window operation occurred.
Both actual registered clients then completed round trips on the new image.
Development host verification returned verified,14tools, catalog hash
bdf4d2af4047ce1f96df48d98527de099dcf521fb26bc8aab56e817d8279305f.
Workbench explicitly started job9ad3be74-77de-4eba-9811-1a16fdc824ad with request
event1792148549455-49828-2 from verified new-image PID49828. It resumes the
original native session in its original cwd, using explicit replay of the
uncertain transcript range, not skip, session replacement or journal editing.
The old failed job is preserved. Its final Workbench review outcome is separate
and must still be received before claiming that handoff closed.
