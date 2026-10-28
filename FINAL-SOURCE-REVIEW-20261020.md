=== TWO_CHAIRS_HANDOFF_READY_V1 job=aef17497-9c1a-4e11-be99-2bac1992ad7c handoff=cde0e622-0fb0-457c-92dd-382244bef603 ===
Disposition: accept the index-version guard and the promotion script's own controls. This review does not authorize apply, host reload, or the canonical canary. Receipt is delivery, not approval.

The guard is the right fail-closed. Before the state_5 existence test, active_rollout scans the Codex home and returns Unsupported when any directory entry is state_<N>.sqlite with N parsed as a u32 greater than 5. It does not open that file, does not choose the newest database, and does not write. state_5 still parses as 5 and is not refused. A later state_6 beside a populated state_5 is refused, including on the post-scan call. Names that are not that pattern, including wal and shm sidecars, are ignored. The check is case-sensitive, so a differently cased suffix would not match; the live name pattern is state_5.sqlite. The attested synthetic test and the suite count are Codex-reported, not re-run here. The from_ref clippy edit is test-only.

Promotion source, read and not executed. plan and apply are the only modes. Both require the config bytes to hash to d42fdbf4fb31cb15b765b26c8389d5021d846dcd28706d5a1b6c8d41ae1cd3d0 and both previous binaries, parley-widget-dev and r3-workbench-parley, to hash to 8c305cdb56665d5ec38bf17c254934b12ce54577795b1f3cac7ecfdacc9cb3b5. The candidate file must match the staged manifest. Those are the only two command replacements. Other profiles are outside that map. Apply copies the candidate beside the old exe, refuses a different file already at the destination, and leaves the old exe in place. ReplaceFileW swaps the config with the backup path under the config backup directory, flags 0, after an exclusive pending write and fsync. Readback must show the new config and the backup equal to the preimage. activation-state.json is written only after that readback, with status configuration_changed_host_reload_pending. An existing pending file, backup, or state file refuses a second apply. The script does not kill processes, grant permissions, print credential values, or reload MCP. The healthy viewer path is not referenced.

gates refuses a dry run whose candidate hash differs, whose exit code is not 0, or whose journalUnchanged is not true. It refuses a review record that is not accepted, not terminal, or hashed to a different candidate. It re-hashes every listed runtime source. The four tests cover those refusals and a changed runtime file. They do not exercise apply, ReplaceFileW, or the profile map. I read those paths in the script.

The pre-guard dry run, exit 0, journal SHA-256 3189c9c78a46e25c7bd60138ed3363b530fbe7fea0c6aad80de64044dba590ec, prompt buffer 9399, does not by itself satisfy the final gate. The release rebuild changes the candidate. Apply stays refused until live-dry-run-final.json names the rebuilt hash, exit 0, and an unchanged journal. Preserve both outputs.

Missing controls, binding on any apply:
- gates accepts an empty runtimeSources map. The apply-time record must list the frozen runtime set, including the active_index guard, or apply is not covered by this acceptance.
- onlyCommandFieldsChanged is a literal true. The script does not diff the TOML. Command-only behavior and unchanged env, permission, and model pins depend on the imported updated_configuration helper, which this turn did not re-read. That file is not hash-locked. Apply-time bytes must be the helper whose six tests passed.
- The script does not compare ACLs. Record the config ACL before apply and do not request reload if the readback ACL differs.
- apply itself does not know whether host reload is available. If reload is unavailable, stop at plan and leave the config and processes untouched. Do not call apply and then leave a swapped config waiting.

Canary shape, not authorized here: one canonical automatic-context read-only run in the original cwd, no resume id, genuine receipt and terminal success, append-only journal, and a new visible continuity marker that arrives only through automatic shared history. No new native conversation, replay, skip, source edit, or journal rewrite. No widget change.

The health query was denied out of scope. Unavailable evidence is not a peer-down signal.

TWO_CHAIRS_HANDOFF
peer: codex
evidence_class: unavailable
incident_id: none
as_of_ms: unknown
continuity: not_authorized
=== END_TWO_CHAIRS_HANDOFF_READY_V1 handoff=cde0e622-0fb0-457c-92dd-382244bef603 ===
