# Local relocation without runtime junction dependencies

Use physical, already-authorized repository, worktree and evidence paths in
active registrations and settings. Do not relax reparse-point refusal or add
aliases to allowed roots to accommodate a directory move.

`scripts/audit_local_paths.py` is an offline operator tool, not a new runtime
resolver. It checks explicitly supplied alias/physical pairs by filesystem
identity, inventories structured active documents, checks both directions of
Git worktree pointers, and separately inventories native Grok summary cwd
dependencies. It does not modify registrations, sessions, grants or logs.

Supply a JSON layout containing `aliases` (old root to physical root),
`repository`, `viewer_settings`, `grok_sessions`, and `active_documents`.
Run `python scripts/audit_local_paths.py <layout> --output <new-report>`.
An optional `--viewer-proposal <new-file>` creates a settings proposal only.
Output files must not already exist. A log-path proposal requires an identical
file identity and exact SHA-256-matched bounded prefix through both names.
Concurrent append is allowed; replacement or mutation of the compared prefix
is not. Only `selectedLog` and `selectedLogs` can change.

Separately inspect process launches, installed shortcuts, autostart entries,
tasks and source defaults. The script's `audited_live_paths_clear` covers only
its enumerated inputs, not an exhaustive machine-wide claim. Historical
receipt text is not an active path and must not be rewritten. Native session
summary cwd fields are different: their old path can still be needed to resume
that session. Keep them separate from current live configuration.

Apply a reviewed proposal only with an exact preimage hash, verified rollback
copy, unchanged ACL, and no concurrently writing viewer. Restore the same
installed viewer using its nonactivating `--autostart` mode. Do not rebuild or
replace the GUI for a settings-only repair. Leave registered MCP processes
alone when their launch paths are already correct.

The audit deliberately never certifies junction retirement. Removing a link
requires complete consumer coverage and a separate safe historical-session
migration if any native cwd still uses it. Never rewrite cursor journals or
signed/fingerprinted grants as a string replacement. Never delete a junction
recursively or alter its target to test runtime behavior.

Regression command: `python -m unittest discover -s scripts -p test_audit_local_paths.py -v`.
