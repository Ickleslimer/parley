=== TWO_CHAIRS_HANDOFF_READY_V1 job=43c2d53f-a28a-4635-b781-e39c1fe02baf handoff=3a2c9a70-ee41-4166-a3fd-c9da02e1bc9a ===
Smallest honest design, not a freeze and not approval. One review-only async image job in the new worktree. Live 14b54 binaries stay the rollback. No cloud body, because this code has no remote worker. Sibling profiles stay a later objective.

The trace changes the wait decision. `append_handoff_contract` tells the model to run the shell wait helper. The image profile can search and use only its private MCP tools, and `run_image_invocation` passes an empty grant list. Stop never waits. It denies an unacknowledged report and returns. A worker that calls the helper after the ACP process exits is too late. Shipping with no in-turn wait means every image job dies at Stop.

The one new capability is a zero-argument private wait tool on the async image profile only. It wraps the existing verified helper. It takes no path, command, or id from the model. It uses the job binding already on that process. It is not a shell grant, not a file grant, and not a wider search/use allowlist. The sync image profile does not get it. Peek and alert stay off this profile. The image prompt is the existing marker contract with that private tool named, and without the shell command strings. Refusing this one tool means do not ship. Overloading the health tool, faking an ack, using handoff-none, or making Stop wait are the shortcuts to reject.

Bindings. `acp_invocation` isolates the seven names and never receives what `configure_invocation` wrote, so an image child would lose the real job. After isolate, apply that observer map only when a real HandoffSession exists. Sync image calls pass an empty overlay so they stay out of Stop scope. Marker ids may stay in the prompt, as text jobs already do. The seven env values and helper paths stay out of the prompt and out of the ACP JSON.

Replies. `run_observed` currently reports process created and resumed only. Feed sanitized text fragments into the existing JobObserver parser. A non-empty `Captured` reply is not success. Success is a parsed ready report, valid broker consumption, not cancelled, and the private wait returned received. Deadline pending fails the job, cleans owned staging, and does not replay.

Intake. `prepare_for_job` still rejects catchup, and it still rejects images that are not this descriptor. A descriptor is an ordered path plus a claimed SHA-256. The path is opened once for a new job and is not part of the fingerprint. Text fingerprints stay byte-identical: no images key in `canonical_request_json`, and the extra hash field exists only when images are present. Lookup is before any file open. A hit returns the stored job, including interrupted and failed, and does not reopen a deleted file or start ACP. A new job reads once, rejects a hash mismatch before the lease, and keeps an immutable in-memory snapshot. An optional client token may be part of the image fingerprint only, so a deliberate second review is a new token rather than an automatic replay.

Execution and recovery. Stage only in the existing job-owned opaque bundle, during execution. Unfinished recovery stays interrupted, not resumed. Clean only owned staging paths, Drop, and the stale janitor. Do not delete the user's source file. Use the CancellationToken already on AcpRequest. No external pause or kill. No provider replay.

Acceptance boundary for the new worktree, before any promotion talk. Text fingerprint unchanged. Duplicate fingerprint does not open the file. Hash mismatch leaves no in-progress journal and no staging directory. Interrupted recovery does not spawn grok. The async ACP env has the real seven. The sync image env does not. Stdout without a parsed report does not succeed. The wait tool rejects arguments, is absent from the sync profile, and the image profile still has no shell or file tool. Unacknowledged Stop still denies. Acknowledged Stop stays silent. No Stop retry.

The denied health query was not retried. Unavailable evidence is not an incident and is not a peer-down signal.

TWO_CHAIRS_HANDOFF
peer: codex
evidence_class: unavailable
incident_id: none
as_of_ms: unknown
continuity: not_authorized
=== END_TWO_CHAIRS_HANDOFF_READY_V1 handoff=3a2c9a70-ee41-4166-a3fd-c9da02e1bc9a ===
