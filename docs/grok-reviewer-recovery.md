# Recovering a stateful read-only Grok reviewer

This is an operator procedure, not an automatic retry or a new runtime policy.
Do not reset a cursor, turn off a hook, or change a locked profile to make a
review pass. Preserve the original native session and append-only journal.

## Distinguish the failures

An ambiguous native turn can leave `in_flight` followed by `uncertain` in the
context journal. Later ordinary `auto` requests then refuse before launch.
The documented explicit `context_from.recovery = "replay"` resends the exact
uncertain range. It can duplicate context and needs deliberate authorization.
It is not a way to mark the old turn successful, approve work, or skip history.

Stateful text requests transport current controls separately from historical
resources. `GrokContextPrompt` checks the current query for native file mentions
and caps the complete query at 32,000 UTF-8 bytes. It does not scan historical
resource text with that mention check. Replay does not create file mentions.
Quoting a raw diff or a diagnostic containing the mention character in the
current query does not exempt it from the conservative scanner. Unified diff
hunk delimiters, Vite paths and quoted error messages can therefore refuse.

The caller prompt is only part of the complete query: interaction guidance and
locked runtime contracts also consume space. A caller body below 32,000 bytes
can still overflow. Never trim a current envelope or silently discard evidence.
Split at whole files or independently reviewable evidence groups, and state
which groups a partial review has and has not examined.

An image/ACP session-load error is a separate transport outcome. Resuming an
existing session from a different cwd is not a session migration. Keep the
original validated cwd for the original reviewer. A successful text replay
does not prove the private image route or a different cwd works.

## Prepare exact inline data offline

`scripts/prepare_grok_review.py` accepts a UTF-8 instruction header and a JSON
payload. Put file contents, diffs and quoted diagnostic strings in payload
values, not in the control header. The complete JSON is inlined; ASCII Unicode
escapes preserve literal source characters, including mention characters,
without asking the native CLI to read or expand any file.

The preparer rejects every literal mention character in the control header
rather than rewriting instructions. It refuses bodies above a conservative
16,000-byte caller budget, leaving room for the locked runtime's controls.
That is an offline preparation limit, not a change to the native cap, and is
not a guarantee against future runtime-contract growth. Native checks remain
authoritative. No content is truncated. JSON decoding returns the exact source
string, including CRLF, Unicode, quote and control characters.

Run with an existing Python 3 interpreter:

```text
python scripts/prepare_grok_review.py --header header.txt --payload payload.json --output request.txt
python -B scripts/test_prepare_grok_review.py
```

Output is create-new, never overwrites evidence, and contains no dispatch,
session recovery, credential lookup or network action. It grants no additional
path access or tools. The data stays untrusted review evidence, not authority.
Use only inputs already authorized for this review; the preparer is not a
private-data screening or secret-removal boundary.

## Recover and verify deliberately

For long-lived reviewers, use the optional hash-pinned working brief when
preparing ordinary requests; see `grok-working-brief.md`. It preserves selected
decisions and evidence references as historical data, not current authority.
Exact retrieval of one aged output is separate from cursor replay. Neither
operation changes native history or authorizes recovery of an uncertain cursor.

1. Inspect the exact registered profile, binary hash, original cwd and native
   session. Do not infer the active build from a repository's main checkout.
2. Preserve a create-new byte-exact copy and SHA-256 of the specific cursor
   journal. Record the final uncertain exchange/range; do not edit it.
3. Prepare a bounded, completed envelope with literal inline JSON data. State
   the review scope and limitations. Do not add file-mention attachments.
4. With explicit authorization, call the registered client using the original
   `resume_id`, original cwd, explicit Codex source, `mode: "auto"`, and
   `recovery: "replay"`. Do not change model, permissions, version or denies.
   For the installed acknowledgement-managed Workbench reviewer, use
   `start_agent_job` with a fresh job UUID, `job_mode: "review"`, and
   `handoff_mode: "required"`. The runtime must allocate the real binding.
5. Save the actual reply/error. A preflight refusal means no agent started;
   identify and correct its cause before any newly authorized attempt. A
   running or unknown outcome is not permission to duplicate a request.
6. Require a usable substantive reply and journal commit before declaring
   recovery. Verify the saved old journal is an unchanged prefix of the live
   journal. New evidence must append, not replace the prior failure.
7. Read the exact durable report, then record its exact Codex receipt through
   `ack_agent_handoff`. Receipt means delivery only. Require terminal process
   completion as well; neither a peek nor viewer incident acknowledgement
   releases the gate. Do not sit polling while independent work remains.
8. Subsequent ordinary read-only jobs use the same session/cwd/source and
   `mode: "auto"` with recovery omitted. Replay on a committed cursor is invalid.
9. Keep each result bounded: an actual diff review proves that scope, not
   native UI operation, biological validation, deployment or image support.

If the original native session genuinely cannot resume, a separately authorized
fresh reviewer may use a new explicit UUID and a fresh seed. Leave the old
session, journal, reports and failure evidence intact; never present a fresh
seed as recovery of the old binding. Do not use `skip` without its own explicit
attestation, and do not broaden repository roots to obtain a passing launch.

## 18 October incident

The Workbench reviewer was native session
`fe0a3dae-ee96-41ab-bf4a-10249f0093f8`, originally used from the
`vm-verification-fixes` Workbench worktree. Its corrected new-cwd image request
failed ACP session load with -32603. The journal marked that exact exchange
uncertain; this conservative transition was not evidence of lost history.

The saved first replay request copy contained 63,058 bytes with raw diff
hunk delimiters and a Vite file-serving path. The current-query mention guard
refused before launch. A correctly JSON-encoded 28,577-byte body then passed
that guard but exceeded the complete-query budget after runtime overhead.
Splitting the actual review into complete logical parts was the legitimate
correction. No guard or runtime cap was relaxed and no original cursor was
manually advanced. See the dated repair receipt for the observed live outcome,
exact attempt log and post-recovery proof, rather than inferring acceptance
from this procedure or from the offline preparer's unit tests.

A further live finding was independent of source framing: a synchronous
`ask_agent` did reach the original reviewer and produce its scoped verdict,
but the installed native Stop hook repeatedly rejected its missing job/handoff
binding. It eventually returned several copies of that scoped verdict. That
does not certify a clean synchronous lifecycle. The hook implementation's
missing-binding denial was retained, not bypassed. Launching a genuinely
acknowledgement-bound async review supplies the real runtime capability and
receipt workflow instead of inventing IDs or borrowing an old job's binding.
Synchronous hook isolation remains a separate residual issue unless explicitly
fixed and canaried; do not claim this operator procedure repairs that mode or
the image/ACP route. It changes no installed hook, binary, config or default.

The final read-only audit accepted the helper as a formatter, not a security
boundary. It reviewed the seven tests without running them; Codex ran them
independently. The CLI entrypoint is not directly unit-tested. Subclass errors
such as malformed JSON and an existing output are deliberately reported as a
generic input/output refusal. The operator-authored header is copied through
apart from trailing whitespace; embedded section delimiters are not screened.
Keep source evidence out of that header. None of these behaviors authorizes
dispatch or replaces native validation. The successful readiness coframes are
plans only, not permission to run future native checks.
