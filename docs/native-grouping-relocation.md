# Native repository grouping repair

`scripts/relocate_grok_grouping.py` is a separate Windows-only maintenance
transaction. It changes only top-level `summary.git_root_dir`, not native cwd,
UUID namespaces, saved commit/branch, transcripts or permissions. The installed
native binary uses this field for repository facets and the divergence warning.

The supported spelling class is the independently native-tested forward-slash
absolute worktree path ending in `/`. Mapping is a literal prefix swap restricted
to `Parley/` worktrees. Other spellings, duplicate scalar values, duplicate JSON
keys and missing or mismatched identities fail closed. Git toplevel must equal
the proposed native string exactly; its directory identity must also match.

Plan from current files, then review the plan and code before applying. Apply
requires the plan SHA-256, regenerates it, requires no native Grok/agent process,
and creates a new same-volume transaction outside the session store. Full-tree
fsynced backups and a durable journal precede writes. Hold file write-denial
handles, replace atomically, reacquire the new summary handle, and verify bytes,
directory identity, modification time, owner/group/DACL/protection for every
live file and directory. No rename and no history reserialization occurs.

Run one dormant pilot, review the actual journal, then freshly plan the remainder.
Review jobs must be terminal before native metadata writes. Never stop another
process to manufacture quiescence. Do not load real sessions to validate this
repair: native resume may legitimately rewrite their history and metadata.

Automatic rollback affects only the failing row, and only if its entire tree
is still the original or approved one-field result. Changed history, corrupt
backups, different directory identity or unrelated permissions refuse rollback.
Earlier verified rows remain migrated; replan remaining rows after review.
Explicit rollback requires the saved transaction plan's SHA-256. Preserve all
transactions, including failures. Neither old cwd manifests nor the read-only
grouping audit proposal is an apply manifest for this tool.

Junction retirement is a separate per-link decision. This repair does not unlink
anything, change registered MCPs, install binaries, or require a GUI rebuild.
