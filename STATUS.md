# Status

## Current release boundary

Pi Wizard is an implemented personal Windows desktop application. The supported product includes Pi-native runtime/session orchestration, bounded multi-run operation, saved project presets, Git-worktree isolation and recovery, bounded session/history presentation, change review, durable drafts/preferences, finite Prompt chains, continuous multi-project Supervision, accessibility/recovery contracts, and optimized Windows builds.

The repository `full` verification lane is the release boundary. It includes deterministic core/desktop tests, strict lint/type checks, large-history/diff/concurrency fixtures, startup measurement, release configuration checks, a GUI-subsystem PE assertion, an optimized Tauri build, and packaged WebView smoke coverage.

`README.md` is the product overview, `DESIGN.md` owns interaction behavior, `ARCHITECTURE.md` owns internal contracts, and `TESTING.md` owns verification details. This file lists only current implemented capability and limitations.

## Compatibility notes

- Pi model and thinking choices are discovered from Pi rather than hardcoded. Provider credentials remain Pi-owned.
- Pi project trust, context-file loading, and extension discovery are independent launch policies.
- Write-capable Resume requires an LF-terminated Pi JSONL tail; Pi Wizard never repairs authoritative session files.
- Provider retry, summarization retry, compaction outcomes, extension errors, and quiet-working advisories are projected as bounded runtime state. Pi Wizard does not replay prompts automatically.
- Pi `set_auto_retry` is exposed as a command, but no durable enabled-state is invented when Pi does not report one through `get_state`.
- Pi 0.84.3 does not expose the documented `clear_queue` RPC command. On explicit unsupported-command rejection, Stop recovers only the private bounded user queue snapshot and terminates the exact Pi process so queued/custom continuation work cannot execute. Pi builds that support `clear_queue` keep the reusable Stop path.
- Standard Windows npm Pi installs use Pi's public `pi.cmd` launcher through a hidden app-owned `cmd.exe` wrapper. The runtime does not infer npm package-internal Node/CLI paths; wrapper and delegated descendants remain one exact owned process tree with no console window. Unsupported script hosts fail before live spawn.

## Implemented capability

| Area | Current behavior |
| --- | --- |
| Runtime | `RuntimeManager` owns live Pi RPC processes, lifecycle, admission, Stop/Close/Dismiss, exact request/dialog identity, bounded renderer delivery, startup readiness, and shutdown. Process lifecycle and agent activity remain separate; unconfirmed termination is `Quarantined`. |
| Sessions | Pi JSONL remains authoritative. Cold/resumed history is file-backed and paged; a brand-new zero-message session may legitimately advertise its future JSONL path before that file exists, so its newest page is represented as empty and seeds `get_entries(null)` rather than reporting missing history. A previously observed persisted cursor prevents stale `messageCount == 0` from hiding a later missing transcript; once entries are known, missing files remain errors. The validated latest page/live cursor revision lets the persisted conversation follow the first and later settled answers even before a later `get_state` updates message counts. A successful `get_entries` response that exceeds Pi Wizard's bounded hot-page projection now requests cold JSONL resynchronization instead of being mislabeled as a fatal protocol/process failure. Session search/resume, naming, tree inspection, fork/clone, usage, HTML export, compaction, and retry controls reuse Pi operations. |
| Projects | Registered projects are durable canonical-directory presets with stable `ProjectId` values. Missing paths stay detached until explicit relocation or removal. |
| Worktrees | Parallel Git work uses unique recoverable worktrees bound to an exact repository/base/branch/path. Recovery is non-destructive; cleanup requires a fresh exact identity/safety proof. |
| User data | Session drafts, project/worktree registries, custom model identities, and preferences persist under the portable `pi-wizard-data` root through bounded schema-versioned stores. Prompt-chain definitions are instead project-local at `<project>\.pi-wizard\prompt-chains.json`. Drafts are session-scoped and generation-safe. |
| Models | The shared picker discovers models from Pi globally and refreshes project-specific launch options separately. New Run has a durable model preference, favorites-first ordering, and a bounded credential-free custom identity catalog; Pi still owns credentials and capability truth. |
| Renderer | Solid is a bounded projection of backend state. Hydration/recovery is versioned, event wakeups trigger bounded pulls, large history stays windowed, the upper conversation preserves prompts verbatim and renders only final answers as sanitized Markdown, and the always-present lower live pane shows transient bounded reasoning/command/streaming activity with explicit active/idle/stall status and bottom-pinned auto-follow. Rapid tool calls stay out of the timeline and contribute only a stable high-level activity label. Dashboard cards add event-driven Pi context/token facts after session selection or turn settlement without interval polling; the wide right rail keeps aggregate run health and detailed selected-run usage. |
| Attention | Pi extension select/confirm/input/editor requests remain backend-owned by exact request ID and appear in a global Needs Attention surface. Fire-and-forget extension UI state and draft/editor state remain separate owners. |
| Git review | Status and diff work is explicit and backend-owned. Binary files return metadata; text diffs use bounded pages and stale-proof cursors; in-flight review jobs are cancelable/supersedable. |
| Prompt chains | Finite saved prompt chains are scoped to the selected registered project's canonical directory and stored in `.pi-wizard\prompt-chains.json`; there is no global/AppData prompt-chain catalog. They are sequential local-checkout workflows by construction. Every prompt launches a fresh ordinary Pi process/session, treats prompt acceptance only as submission, waits for a new `agent_settled`, requires a fresh final assistant `stop`, closes that process, and then advances. Tool-result messages cannot complete a step; provider errors, abort/length outcomes, tool-use-only settlement, extension commands, and pre-acceptance rejection are attached to the exact failed step while later prompts continue. The view retains the current chain/project/model selection while navigating elsewhere. Cancellation stops future launches without terminating the currently running prompt. |
| Supervision | Supervision is independent from Prompt chains and is continuous in production. One ordinary Pi supervisor run consumes one live slot and may cover multiple selected projects. Newly settled work is considered once per backend session-replacement/session/settlement/result version without `get_session_stats` polling; provider-error outcome text remains available to supervision even when Pi reports no last assistant text. The supervisor's own turn must settle with a fresh final `stop` response before directives are parsed. Autonomous directives are revalidated against the exact observed version, so session switches and newer completed work make stale Send/Stop decisions non-actionable. Active user direct Bash temporarily removes a run from idle actionability; user Stop abandons pending future directives, and Stop before/during supervisor startup is a normal exact-process `Stopped` path. |
| Run utilities | Run details expose Pi-native session HTML export and a bounded cancellable one-shot Bash command. Command output is capped before renderer retention and is excluded from model context. Backend-projected direct-Bash ownership survives renderer reload, blocks overlapping model/session mutation and Close, and keeps cancellation available; this is not a persistent terminal. |
| Windows host | Tauri runs as a GUI-subsystem application. Standard npm Pi installs run through the public `pi.cmd` launcher under a hidden app-owned `cmd.exe` wrapper; stdin/stdout remain the RPC transport, the wrapper PID owns the delegated child tree, and a desktop-lifetime kill-on-close Job Object backs abrupt-exit cleanup. |
| Diagnostics | Runtime diagnostics are explicit pull snapshots over existing bounded owners; they do not introduce passive telemetry, logging, or polling. |

## Known upstream limitation

- Direct in-place navigation to an arbitrary historical branch is not exposed by the current Pi RPC command surface. The full branch tree is visible and user-message nodes invoke Pi-native fork semantics; Pi Wizard does not invent a renderer-only branch mutation.

## Out of scope

Unless the user explicitly expands the product boundary, Pi Wizard does not include:

- a persistent/interactive terminal emulator, source editor, or file explorer;
- branch integration, commit, or conflict-resolution workflows;
- remote/mobile clients or a background daemon;
- scheduled autonomous jobs;
- a custom plugin/provider marketplace or application-owned provider authentication;
- a multi-harness abstraction beyond Pi;
- fake permission modes that Pi does not enforce;
- a built-in container/VM sandbox.

See `ROADMAP.md` for the entry criteria applied to future scope.
