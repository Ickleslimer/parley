# Permission cancellation diagnostic repair

## Accepted and active

The misleading async `exited without producing any output` fallback is fixed.
Parley preserves the exact post-observer result used for completion logging in
MCP and async job results. Native `cancelled` remains distinct from an explicit
Parley cancellation or watchdog termination. Handoffs require `end_turn` as
well as the existing process, parsing, report and receipt checks for success.
Unknown terminal text is never returned or stored verbatim.

Locked Auto text calls also receive guidance to prefer built-in reads and
direct Git commands over avoidable pipelines. This grants nothing and is not
enforced permission-timeout prevention. Generic defaults are unchanged.

The roughly 30-second native permission wait is **not fixed**. Installed native
1.0.44 documentation does not establish a switch that retains Auto and converts
every unanswerable prompt into a tool-level denial. No speculative console
change, permission bypass, new allow rule or automatic retry was introduced.
The original HLA review and uncertain cursor were not replayed or recovered.

## Collaboration and provenance

- Co-framing: `b04a5836-90af-4701-bca9-c5680b663d30`.
- Grok writer: `5141ca54-88dd-436a-8e6d-4380c2f451da`, native session
  `cdd8e98a-ed79-8767-8c4c-50cac03a57f0`, verified Grok 4.7/XHigh.
- Integrated review: `46606668-a671-470d-9d87-8e0e97b63d3b` accepted the
  reporting repair, explicitly rejected claiming native prevention.
- Codex implemented result propagation and locked guidance, independently
  reviewed/integrated both lanes and ran final acceptance.
- Tests originally reserved a CLI filename in the lane contract; final coverage
  lives in `ask.rs`, `job_runtime.rs` and `stream_activity.rs`, plus external
  synthetic MCP evidence. The writer's temporary local compile shim was removed
  before integration; final tests ran on the actual combined code without it.

## Independent validation

Root `cargo fmt --check`, `cargo clippy -- -D warnings` and
`cargo build --release` pass. Root tests: 446 passed, four ignored fixture entry
points; the new ignored observer child is explicitly executed by its passing
parent. Eight CLI integration tests pass: 454 executed top-level tests total.
Focused handoff/stream checks: 32 and seven tests pass independently.

The synthetic release-binary MCP fixture produced an exit-zero native
`cancelled` end with swallowed output. Its exact error matched live
`get_agent_job`, the event log and a fresh MCP process after restart. One fixture
spawn, zero model invocations, no retry, no hidden/raw payload disclosure.
Initial fixture preflight checks correctly refused an invalid cwd and a
mismatched version; no provider fixture had started in those attempts.

Live development canary `e8fa2e8b-e7e1-40c6-bba8-9687fd367b7e` resumed the
existing canonical session `2970324b-91ab-8f46-a7e3-acaa77a65cf3`, published
`CANARY_PERMISSION_REPORT_591E`, received Codex receipt, then succeeded with one
exact response. Native metadata proves Grok 4.7/XHigh. Exchange
`exchange-1791927964619-36892-1` has one schema-v1 request and response.
This tests normal delivery, not the native approval timeout.

## Activation and rollback

Release SHA-256:
`591E55026E4F3F42E1244C261A78909BCF9E6EBB1C12D4E5EAF15901145EB413`.

Side-by-side executable: `par-permission-cancellation-591e55026e4f3f42.exe` in
the existing per-user `par-widget-dev` and `par-r3` directories. Development
activated first; after its live canary, the four R3 profiles activated through
the verified host reload channel. Actual registered round trips pass for
`parley-widget-dev`, `r3-parley`, `r3-workbench-parley`, `r3-nisaba-parley`,
and `r3-hla-parley`. The catalog remains 14 tools with unchanged schema hash.

Only the five executable command paths changed. All profile policy settings,
arguments and unrelated profiles remain identical to the saved TOML. Prior
context-catchup binaries and
`config-before-permission-cancellation-20261013.toml` are preserved. Main Parley
is clean; pre-existing dirty R3 policy documents are unchanged. No GUI,
scientific file, schema, dependency, global Grok config or credential change.
No push, external process kill, window manipulation or Codex restart.

Evidence:
`D:\Workplaces\.parley-evidence\permission-cancellation-repair-20261013` and
`D:\Workplaces\.parley-evidence\parley-permission-cancellation-20261013`.
