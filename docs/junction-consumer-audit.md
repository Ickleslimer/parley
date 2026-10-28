# Auditing remaining junction consumers

The saved-cwd migration and junction retirement are different operations.
`audit_junction_consumers.py` reads an explicit document/database inventory and
native session grouping fields. It detects paths in dictionary keys and
embedded command strings as well as ordinary values. Matched values are hashed
rather than copied into reports. This is a read-only inventory, not an unlink
tool or a claim to inspect every application on the machine.

The database reader uses SQLite `mode=ro` and `query_only`. For Codex state it
reads only path columns from `threads`, not message history. Other configured
databases are limited to named path-related columns and 10,000 rows per table;
unscanned tables and errors remain explicit. No credential store is read.

`probe_native_grouping.py` uses the installed Grok executable with disposable
homes and a synthetic local Git repository. It sends initialization, listing,
filtering and load requests, never a model prompt. Profiles have no tools or
MCP inheritance; real authentication is not copied and API endpoints/proxies
point to an unreachable loopback port. It changes only its own fixture files
and, in the absent-link case, removes only its identity-verified synthetic
junction. Every real junction remains untouched. Do not point a fixture root
at an existing directory. Keep raw results and earlier failed probes.

The 23 October 2026 Grok 1.0.44 probes established:

- Physical session namespaces and cwds still list and load when `git_root_dir`
  names a missing path. The same UUID and non-system history marker survive.
- The `gitRoot` grouping/filter key is the stored string, not a physical-path
  identity. Filtering by the physical string excludes the stale-string group.
- The recorded repository root also gates Git divergence metadata. With a
  deliberately different saved/current commit, the physical root produces a
  divergence warning; a stale or equivalent junction string does not, whether
  the fixture junction exists or not. Thus this field is not purely cosmetic.
- Custom ACP methods need the underscore wire prefix, `_x.ai/session/list`.
  Bare names fail with -32601. Original failed-driver results remain evidence.

These are installed-binary results; the available older public source only
corroborates them. Offline catalog fallback is not evidence of registered model
selection. Synthetic Git-divergence behavior does not prove that every real
session currently has a changed commit.

An identity-bound root-field migration, if approved as the next operation, must
regenerate hashes after native processes are quiescent, preserve full backups,
modify exactly one JSON scalar, verify owner/group/DACL/protection and all other
bytes, then test filtering and divergence behavior. Preserve saved commit and
branch values: they are needed to detect real repository changes. Do not apply
an audit/dry-run report directly or globally replace history. Junction removal
needs its own per-link decision; retaining a junction does not cure a stale
root string's equality/filter behavior.

Evidence: `Parley-root/.parley-evidence/junction-consumers-20261023` in the R3
workspace. Validation: the `test_audit_junction_consumers.py` suite plus the
existing path-audit and relocation suites.
