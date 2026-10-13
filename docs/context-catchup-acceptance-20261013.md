# Context catch-up acceptance, 2026-10-13

Two Chairs preserved the existing canonical Grok conversation and both locked
limits (128,000 delta scalars, 256,000 hard maximum). Explicit synchronous
receipt-only catch-up delivers contiguous whole unread turns; ordinary task
resumption still requires the entire remaining delta. There is no automatic
drain, retry, truncation, skip, reset, scientific work or implicit authority.
An oversized first unread turn remains fail-closed.

## Implementation and review

Base: `90523a1c8f0a0fd5d40c7286c77b3b69f87d05ee` preserves the previously
approved speed-mirror work without modifying its original dirty worktree.
Grok implemented the planner and nine focused tests in its disjoint lane;
Codex implemented request/policy/receipt/journal/CLI/MCP integration and
independent tests. Grok 4.7/XHigh review accepted the consequential integrated
source. Receipt proves delivery, not model comprehension or acceptance.

Preserved jobs: co-framing `50125f31-b64d-4705-9f98-35dcdd35d962`, writer
`95a9325f-b8e7-4738-a73c-57ed9142b32d`, final review
`7f19b11b-ec6c-47f1-a823-79d897ac7d45`. All ended successfully after receipt.

## Independent checks

- Root format check, clippy with warnings denied, and release build pass.
- 434 unit tests and eight CLI tests pass; three existing unit tests are ignored.
- 17 focused catch-up unit tests pass. A synthetic native Grok executable
  separately proves exact receipt commit, invalid receipt uncertainty, and
  subsequent refusal without another spawn. No real provider was used by it.
- Root Cargo manifest/lock and event/context/health/handoff schemas are unchanged.
  The CLI/MCP context-mode enum has the explicit additive `catchup` value.

## Live evidence

Existing native session: `2970324b-91ab-8f46-a7e3-acaa77a65cf3`, still created
2026-10-09, model Grok 4.7, effort XHigh. The catch-up call delivered 126,250
scalars from offset 1,191,176,104 through 1,555,557,810. It wrote one in-flight
and one committed transition, with exact receipt fingerprint `2ea826fa0fc66aaf`.
Its owned prompt files were reaped/removed; native session evidence remains.

The registered development client then resumed normally with the next exact
12,024-scalar delta through offset 1,578,449,651. It returned a newly persisted
visible marker absent from its current request. Both request/response pairs
remain event schema v1 and identify the same resumed native session. No older
release-review task was replayed, and no uncertain cursor was recovered.

Release SHA-256:
`3d4f5c62e3c0cec914c5469a46447fcc4f72b707afa0c31a8a612efe8d35d75f`.
Registered development, R3, Workbench, Nisaba and HLA profiles now use the
side-by-side `par-context-catchup-3d4f5c62e3c0cec9.exe`. Each actual registered
client passed a round trip. Configuration comparison proves only those five
command paths changed; every environment setting, limit, guard and unrelated
profile remains identical. The previous configuration and binaries survive.
Activation used the verified host reload channel, never external process kills
or a Codex restart. No desktop window was manipulated.

External evidence: `D:\Workplaces\.parley-evidence\context-catchup-20261013`.
Main Parley stays clean at `bf255c2`; R3 main retains only its two pre-existing
dirty policy documents. No scientific code, packs, viewer UI, push or public
deployment changed. The original HLA permission-cancellation defect is a
separate open issue, not part of this acceptance.
