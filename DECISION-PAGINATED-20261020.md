# Paginated context decision

chosen: Keep the original root path and physical identity, and stream explicitly
bounded ancestor segments followed by the active child payload. Child metadata
is validated and omitted from the logical stream; it is not conversation text.
Use the native index's exact thread pointer when the index exists. If the index
is absent, retain the existing single-source rule; never infer an active child
from unique linkage, timestamps or file size. Windows loads only system SQLite,
opens it read-only and binds the requested session as a SQL parameter. No new
crate, executable, network access or database writes are introduced.

rejected: Resetting or rebasing the live cursor, copying the full archive,
selecting a unique/latest linked leaf, or making the child the root identity.

dissent: Grok's initial recommendation was "SQLite is not part of this patch."
Codex rejected that specific suggestion because a unique child can be retained
but inactive while the thread points back at the original source. Synthetic
evidence now covers that counterexample and two retained child branches. The
index choice needs review against the actual implementation, not approval by
assumption. Other co-frame advice was incorporated.

evidence: The observed native pointer selects the suffixed October rollout;
history_base names the original physical rollout through byte 3222474611,
excluding ordinal 203969. Existing cursor offsets remain meaningful and the
278903-byte gap is retained. Boundary validation inspects only the bounded last
record, not a multi-gigabyte recount.

unknowns: Compressed ancestors, archived ancestors and cross-thread fork imports
are not implemented. They fail closed. Existing unread context may separately
exceed the unchanged per-call limits or contain prohibited native mentions.
This repair must not bypass those independent gates.

next: Review source and focused/full tests; validate a read-only real-source dry
run, then stage and activate only through a verified host-controlled MCP reload.

## Journal compatibility

Existing schema-1 records remain verbatim. A new append for linked history uses
internal cursor schema 2 with ordered per-segment file identities, physical and
logical bounds, and header fingerprints. The root and every already-shared
extent must still match. This allows a proven retained prefix to extend, but
never silently branches behind a committed cursor. Transcript range fingerprints
are unchanged; lineage evidence is stored separately. Old binaries fail closed
on schema 2 rather than interpreting logical offsets as physical-file offsets.
Rollback installers are preserved, but cursor downgrade is not automatic.
