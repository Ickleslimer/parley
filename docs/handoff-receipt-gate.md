# Stop-gate protocol repair

16 October 2026. Isolated branch `fix/handoff-receipt-gate`, based on
`fa2363e`. This changes the independent handoff helper and managed-hook
registration in the health package. Parley's root runtime,
event/context/health/handoff schemas, dependencies, GUI, scientific
files, and existing failed jobs are unchanged.

## Evidence and cause

Workbench review `a415b789-17f1-49eb-a95d-b749861afc74` published its durable
report at 1792168775582 ms and failed at 1792168777154 ms, before receipt.
Its terminal stream record was `end_turn`. The late explicit receipt at
1792168999150 ms acknowledged delivery but correctly did not turn that failed
job into success. The report, failure, and late receipt remain preserved in
the Workbench H13 evidence directory.

The installed helper reproducibly emitted `decision: deny` with PreToolUse
permission fields for a Stop event and returned exit zero. Grok's installed
1.0.44 documentation specifies `decision: block` for Stop, whereas `deny`
belongs to PreToolUse. The upstream [hook contract](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/10-hooks.md#stop-decision-control)
documents the same distinction. This is a confirmed protocol defect that can
let a normally finishing model exit despite the supposed gate. Native
`hook_execution` updates independently confirm that the exact managed Stop
entry ran at 1792168775 seconds and reported `success`, followed by a separate
session-teardown invocation at 1792168776. The original journal's twelve
records never set `wait_until_ms`, and there is no tool activity after report
readiness. The original hook's exact stdout was not retained; the installed
binary's synthetic reproduction and the native outcome identify the defect
without claiming a captured incident-time stdout payload.

The first protocol-only native canary, job
`a62d9684-7929-497b-8e2c-e22111341f29`, still exited before receipt despite a
verified candidate helper hash. Its failure and late receipt are preserved;
it was never reclassified as success. That discriminating test exposed a
deeper launcher bug: managed Windows commands were only quoted paths. In
PowerShell those print a string instead of executing the helper. An actual
Windows shell fixture reproduced exit zero instead of the helper's exit two.
Thus the old native `success` hook statuses did not establish execution of
the executable. Grok independently traced and agreed with this cause.

All five managed Windows command fields now use a literal single-quoted call
operator with doubled apostrophes, preserve the native exit code, and block
with exit two when no native exit code exists. Actual Windows tests prove
execution with spaces, apostrophes, ampersands, and backtick characters in the
path, exact UTF-8 stdin including Unicode and mixed newline sequences, and
the missing-executable fallback. No PowerShell input/re-encoding pipeline was
necessary. Full-command recognition accepts only the exact legacy and new
managed forms; migration, repeated installation, and removal preserve
unrelated handlers. Absolute paths are required and controls rejected.

A further canary (`451f689a-6107-4402-b04f-04a8480ca5d4`) exposed Grok's
pre-shell environment expansion: `$null` and `$LASTEXITCODE` were interpreted
as missing required environment variables, so hooks never started. Its exact
native hook failure metadata and failed job are preserved. The final fixed
suffix uses `Get-Variable LASTEXITCODE -ValueOnly -ErrorAction Ignore` and an
integer type check, with no dollar variable references anywhere. Windows
installation paths containing dollar signs fail closed rather than relying
on unverified template escaping. This limitation is tested explicitly.

The documented dual event fields were another compatibility defect:
`hook_event_name: Stop` and `hookEventName: stop` falsely conflicted before
normalization. Only event names now normalize their documented spelling
aliases before comparison. Actual conflicting event types still block;
session, cwd, capability, and other identity fields remain exact.

A second opening allowed Stop while the bound state was still `pending`.
Report output and hook invocation are processed independently, so pending is
not proof that a required handoff may finish. It now blocks until receipt or
an explicit runtime catastrophe state, including before report durability.

## Repair and boundaries

- Stop denials emit only `decision: block` and a static, actionable reason.
- PreToolUse retains its existing deny-only JSON and exact binding checks.
- Blocked invocations also return exit 2 with static stderr feedback. This
  retains a blocking fallback for malformed or oversized scoped payloads whose
  event type cannot be determined, and for unusable stdout.
- Receipt and catastrophe allow stopping; an ordinary pending state does not.
- A ready report's feedback points to the exact wait command already supplied
  in the contract and explicitly prohibits repeating the report.
- Hooks remain quick, read-only state checks. They neither wait, record receipt,
  sound an alert, retry a model, nor rewrite a failed job.

The native hook remains fail-open if it is missing, crashes, times out, or
reaches Grok's eight-continuation override. The independent runtime therefore
continues to reject an exit without report, explicit receipt, clean stream end,
and successful process completion. This repair is prevention, not a claim that
hooks form an OS security boundary or guarantee a model will always comply.

## Regression coverage

New tests failed against the original implementation for pending-state
permission and incorrect Stop vocabulary. They now check all unreceived states,
binding failures, both native event-name spellings, actual helper subprocess
exit codes, receipt/catastrophe release, and malformed/oversized/conflicting
payloads. They verify static output does not leak rejected payloads, capabilities,
reports, or excerpts; hooks do not modify journals, snapshots, or alert inboxes.
Existing unsafe-command, session, process, file-identity, wait, alert, and schema
tests remain required. Root runtime tests still cover detached failure and late
receipt without promotion to success.

## Acceptance

Root formatting, 456 unit tests (4 ignored), 8 CLI integration tests, strict
clippy, and release build pass. Handoff formatting, 16 behavior tests, 2 schema
tests, strict clippy, and release build pass. Health formatting, the full suite
(including the child fixture exercised by its parent test), strict clippy, and
release build pass. Lanes formatting, tests, strict clippy, and release build
also pass. No root Cargo files, dependencies, schemas, viewer or Workbench
files changed.

Grok co-framed the diagnosis and reviewed the consequential actual diff through
the locked development profile, native session
`f4912dcb-0c09-4380-b087-b20bc1fcfae7`, Grok 4.7/XHigh. Its stdin and missing-exe
findings produced additional passing Windows tests. Its concern about
PreToolUse exit two was explicitly withdrawn after the version-matched
documentation check, and a real native denial canary independently verified it.

Final registered-client canary `ad00663f-a13f-4af2-bbb9-03536015268a`:

- The intentionally invalid helper invocation was denied by the hook before
  execution, without a permission prompt.
- The exact report became durable at 1792171342843 ms. The model deliberately
  attempted to stop; the native Stop gate continued it into the exact wait
  helper. The same root PID 47328 was independently verified alive while
  `awaiting_ack`, with `wait_until_ms` set and no receipt.
- Codex explicitly recorded receipt at 1792171405939 ms, 63.096 seconds after
  report readiness. The process then exited, and the same job succeeded with
  completion event `event-1792171419206-39640-15`. Request event:
  `event-1792171258012-39640-14`.
- Handoff `cd81ab91-554f-489f-9f22-d89f5ffc466d` retained fingerprint
  `cd66102c5ba2f4eb39f918b0b55b0031e7f958c8869b9f5f6d37896b23ff28ce`.
  The old failed jobs remain failed; their late receipts are delivery only.

The installed helper SHA-256 is
`18fa71d766eee32a28953215a4ec8de52ab1225582a70ddb9834267b8a19d624`.
Only five existing managed hook command fields changed; every other hook
field, the MCP configuration, registered server images, settings and GUI were
left unchanged. The exact previous binaries/configuration and all failed
canaries remain under the local `handoff-receipt-gate-20261016` evidence directory.
No MCP restart, registered-process termination, automatic retry, receipt inference,
desktop manipulation, scientific change, or push occurred.

This is an installed helper/registration hotfix, not a new GUI release. The
health registration generator is corrected in source and its release binaries
are built; existing rollback GUI installers are not relabelled as new builds.
Normal supervisor startup does not rewrite hooks. A later GUI package must be
built from the corrected source rather than reinstalling a rollback package.

The matching health supervisor was subsequently promoted in place so current
installation refresh/removal also recognizes the corrected commands. Exact
old PID 22344 exited after its graceful shutdown request; new PID 47116 runs
the verified installed image, SHA-256
`b56d631c94b9cc384ba4b0d4add9b573f9caefed47dac78e5928d462d0b3dc85`.
The initial controller could not verify the GUI-subsystem control process's
exit code and stopped before replacement; independent inspection proved the
old supervisor was gone and the old image/rollback were intact. A separate
guarded completion performed the replacement and hidden non-UI restart. That
controller failure is retained, not described as a passed exit-code check.
The viewer and registered MCP processes were untouched.

Separate diagnostic exposed by activating the previously inert health hook:
the development profile's health query is denied as out-of-scope. Its footer
correctly says unavailable, not healthy or exhausted. Cached scope remediation
is not folded into this receipt-gate repair, and no path grant was broadened.

Local source checkpoints: `459a7f9` (Stop protocol, pending-state and alias
repair) and `788f984` (Windows command generation and native shell fixtures).
The final evidence audit independently matched the completion event's UTF-8
content to `report.txt`, verified its fingerprint, and found exactly one v1
request/response pair. No wait helper or canary root process remained.
