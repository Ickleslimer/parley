# Relocating historical Grok session bindings

This offline maintenance tool preserves native UUIDs and history when a verified
Windows worktree junction is being retired. It does not remove junctions, edit
Parley journals/grants/cursors, change profiles, or rebuild/install an app.

`scripts/relocate_grok_sessions.py plan` is read-only. Its inputs are the physical
native sessions root, the existing worktree junction, its verified physical
target, and an exclusive output manifest. An optional `--session` selects exact
UUIDs. Only `Parley/` worktree suffixes qualify. The current reviewer UUID is
explicitly excluded. Every target must already exist and be the same physical
directory reached through the old cwd. Reparse chains, unknown encodings,
duplicate UUIDs, collisions, missing paths and malformed JSON fail closed.

`apply --plan <manifest> --sha256 <exact hash> --transaction <new backup folder>`
requires all native Grok/agent processes to be quiescent. It does not stop them.
It regenerates and compares the plan, makes fsynced full backups outside the
active sessions tree, then moves each UUID directory into the encoded physical
cwd namespace. Only the original JSON scalar at `summary.info.cwd` changes;
serialization, other keys and all other files remain byte-identical. The same
directory file identity must survive. Original summary timestamps are restored.
The original security descriptor is captured before replacement. ReplaceFile can
carry staging-directory inherited permissions, so the tool explicitly restores
the original DACL and verifies owner, group, exact access entries and protection.
The auto-inherited bookkeeping flag alone is not an access-rights change.

Windows refuses a parent directory rename while child file handles are held.
The tool therefore holds deny-write handles for backup and verification, releases
them only for the rename between exact hash/quiescence checks, and reacquires
them immediately afterward. This is **not a concurrent-session migration API**.
Do not start a native session during maintenance. Any observed native process,
unexpected file or hash change stops the batch. Already verified earlier rows
remain migrated; later rows remain untouched. A started transaction cannot be
automatically reused. The step journal records completed work durably.

Rollback accepts only the exact original or approved post-change tree, the same
directory identity, an unoccupied original location, and a verified full backup.
It refuses divergent bytes rather than overwriting a session resumed after the
move. A failed row is rolled back if these conditions still hold; otherwise its
backup and transaction remain for investigation. The explicit crash-recovery
command is `rollback --transaction <folder> --session <UUID> --sha256 <hash of
that transaction's plan.json>`. Preserve both original and recovered evidence.

## Native proof and limits

The 23 October probes ran the installed Grok 1.0.44 binary with synthetic stores,
no real credentials, no model prompt, no MCP/tools, an isolated home and local
unreachable API endpoints. Updating only the summary failed native session/load
after the synthetic junction was removed. Updating the namespace and summary
loaded the same UUID and reported the physical cwd. A physical namespace with
an old summary also loaded, but Parley's summary-based discovery still requires
the metadata update. This establishes why both changes belong in the transaction.

Native startup itself can replace the system prompt and refresh summary fields.
The migration's exact-byte invariant is checked **before** a native resume;
later legitimate session writes must be recorded separately. Offline catalog
fallback is not evidence of the registered model pin or a successful model turn.
Use the registered locked profile for a real same-UUID pilot after migration,
review that evidence, then migrate the remaining dormant sessions.

`git_root_dir` and historical prompt paths are deliberately not rewritten. The
former needs its own repository-grouping test before claiming junction retirement
is complete; historical prose is not a runtime binding. The separate Grok home
junction is outside this task. Backups must never sit inside the native sessions
tree, where duplicate UUIDs would make resume ambiguous.

Validation: `python -m unittest discover -s scripts -p test_relocate_grok_sessions.py -v`
and the existing `test_audit_local_paths.py` suite. Native fixture driver:
`probe_native_session_paths.py`; every run uses a new evidence directory.
