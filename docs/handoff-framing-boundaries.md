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
end. A follow-up synchronous review could not restore that remote session
(404). No report, receipt or success is inferred from that failure, and no
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

Tests cover absent and nonmatching boundaries, CR/whitespace/other preceding
classes, split markers and UTF-8, explicit newlines after commentary without
a tool boundary, secret non-persistence, refusal of premature receipt, and
stop diagnostics. Existing collision, exact-report, timeout/cancel, containment
and duplicate-report tests remain authoritative. One earlier malformed test
expected the old missing label; only the intended diagnostic expectations were
updated. An exploratory rejected-tail persistence test failed, then that
approach was discarded following Grok's recommendation; persistence did not
expand.

Root format, tests455passed4ignored plus8integration, strict clippy and release
build pass. Candidate live review, promotion, registered-client verification
and the separate Workbench footer evidence are not yet acceptance claims.
