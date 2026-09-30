# Direct Grok Continuation

Direct Grok continuation is an emergency path in the Windows conversation studio. It lets the user send bounded instructions to the same canonical native Grok session used by the locked Two Chairs profile when Codex is unavailable. It is not a general chat launcher and does not weaken ordinary Parley jobs.

## Publication and scope

Codex must first publish one immutable continuation manifest through the locked MCP profile. Publication binds:

- the canonical worktree and its exact Git `HEAD`;
- the current Codex source session and canonical Grok epoch;
- non-glob writable file or tree grants beneath that worktree;
- the profile's locked model, reasoning, permission, event-log, context, and job-state settings;
- a matching permission-canary fingerprint and an expiry time.

The broker refuses ambiguous, expired, stale, or already reviewed manifests. Dirty paths outside the grant refuse publication and execution. Dirty paths inside the grant are allowed because the purpose is to continue already active work without discarding evidence. A recursive Windows change watcher also records writes, renames, and deletions that Git would hide as ignored content; any observed path outside the grant becomes a containment violation.

## User turns

The dedicated detail application accepts only bounded text. It does not expose a session picker, executable, cwd, model, permission mode, shell command, path grant, or validation-command editor. Rust creates the continuation UUID and invokes the fixed installed broker resource without a visible console.

Each turn loads the manifest's exact canonical Grok session through Grok's ACP stdio interface. This preserves native conversational continuity without advancing or rewriting the Codex shared-context cursor. Ordinary Parley calls remain locked to Auto; the continuation broker instead creates a synchronized one-use agent profile containing only `read_file`, `list_dir`, `grep`, `search_replace`, and `write`. Default tools, AGENTS files, skills, workflows, memory, inherited MCP servers, shell, web, and subagents are disabled. ACP client capabilities are empty, so Grok cannot ask Parley to perform client-side filesystem or terminal operations.

The ACP protocol pipe is used only for JSON-RPC control, visible reply chunks, and permission decisions; it is not an interactive prompt channel. Every `write` or `search_replace` request must identify the active native session, advertise stable mutating-tool metadata, name one unambiguous target that resolves only against the manifest's canonical worktree, and expose exactly one `allow_once` plus one `reject_once` option. The installed ACP may also advertise bounded `allow_always` or `reject_always` alternatives; Parley validates their kinds and unique identifiers but never selects them. The broker permits only one initial missing-or-pending mutating announcement and one status-less metadata refinement before permission. It canonicalizes the existing ancestor, refuses reparse redirection and unsafe Windows path forms, and selects the unique `allow_once` only when the target matches an exact manifest file grant or descendant of a tree grant. Other requests are rejected, and any mutating execution or terminal transition without the matching one-shot approval terminates the contained process tree. The locked Grok model and reasoning effort are re-applied and verified after session load. Continuation turns have a six-hour total ceiling, no idle timeout, no numeric turn cap, no automatic retry, and no shell, web, MCP, package-manager, destructive, or out-of-grant write authority. Read containment is a prompt-and-tool-catalog boundary rather than an OS sandbox; same-user Windows access remains outside Parley's security boundary.

The request is framed as `PARLEY_USER_CONTINUATION_REQUEST_V1`. Event-log records keep schema v1 and use `source=user`. These exchanges remain available in the full detail transcript but are intentionally omitted from the desktop widget.

## Evidence and review

After Grok exits, Parley captures the exact reply or error, Git-visible and watcher-observed changed paths, post-turn fingerprints, and an explicit `validation_state: not_executed` in create-new evidence. The broker never runs Cargo, npm, Python, tests, build scripts, or other worktree-loaded code after Grok edits; Codex performs validation only after recovering and reviewing the sealed diff. Repository-local Git aliases, executable filters, external diff or merge drivers, includes, and command-bearing core settings refuse observation before Git starts. Journals and evidence contain no credentials, environment dump, raw tool payload, or command-argument transcript. Evidence integrity is verified before it is shown or used for recovery.

Showing the exact result in the detail app records a user receipt. Receipt means only that the result reached the user; it is not approval, scientific acceptance, recovery, or continuity authorization. Every ordinary Grok call or job in the published worktree, including explicit isolated sessions, remains blocked until Codex retrieves the evidence, independently reviews the actual diff and tests, and records a matching review through the locked MCP profile. A live broker holds a separate runtime lock. If the broker disappears before sealing, the next gate observation records `broker_interrupted` and keeps Grok work blocked for inspection and republication. Parley never retries Grok automatically.

## Permission canary

Locked continuation publication requires a current canary for the exact Grok CLI and ACP broker profile. The canary uses a disposable directory beneath the approved root. It creates a native session, loads that exact session in new contained processes, proves one granted write succeeds through `allow_once`, proves an out-of-grant write is rejected, and proves a shell attempt cannot obtain mutation authority. The record binds the CLI version, model, reasoning effort, ACP transport, agent-profile hash, and permission-policy hash. A failed or version-stale canary disables direct continuation rather than relaxing permissions.
