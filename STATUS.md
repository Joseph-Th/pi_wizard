# Status

## Current release boundary

Pi Wizard is an implemented personal Windows desktop application. The supported product includes Pi-native runtime/session orchestration, bounded multi-run operation, saved project presets, Git-worktree isolation and recovery, bounded session/history presentation, change review, durable drafts/preferences, finite Automation, independent Supervision, accessibility/recovery contracts, and optimized Windows builds.

The repository `full` verification lane is the release boundary. It includes deterministic core/desktop tests, strict lint/type checks, large-history/diff/concurrency fixtures, startup measurement, release configuration checks, a GUI-subsystem PE assertion, an optimized Tauri build, and packaged WebView smoke coverage.

`README.md` is the product overview, `DESIGN.md` owns interaction behavior, `ARCHITECTURE.md` owns internal contracts, and `TESTING.md` owns verification details. This file lists only current implemented capability and limitations.

## Compatibility notes

- Pi model and thinking choices are discovered from Pi rather than hardcoded. Provider credentials remain Pi-owned.
- Pi project trust, context-file loading, and extension discovery are independent launch policies.
- Write-capable Resume requires an LF-terminated Pi JSONL tail; Pi Wizard never repairs authoritative session files.
- Provider retry, summarization retry, compaction outcomes, extension errors, and quiet-working advisories are projected as bounded runtime state. Pi Wizard does not replay prompts automatically.
- Pi `set_auto_retry` is exposed as a command, but no durable enabled-state is invented when Pi does not report one through `get_state`.
- Pi 0.84.3 does not expose the documented `clear_queue` RPC command. On explicit unsupported-command rejection, Stop recovers only the private bounded user queue snapshot and terminates the exact Pi process so queued/custom continuation work cannot execute. Pi builds that support `clear_queue` keep the reusable Stop path.
- Standard Windows npm Pi installs run through direct Node + Pi CLI entry point with no console window. Unresolved script launchers are rejected for live runs.

## Implemented capability

| Area | Current behavior |
| --- | --- |
| Runtime | `RuntimeManager` owns live Pi RPC processes, lifecycle, admission, Stop/Close/Dismiss, exact request/dialog identity, bounded renderer delivery, startup readiness, and shutdown. Process lifecycle and agent activity remain separate; unconfirmed termination is `Quarantined`. |
| Sessions | Pi JSONL remains authoritative. Live history uses bounded `get_entries(since)` synchronization; cold/resumed history is file-backed and paged. Session search/resume, naming, tree inspection, fork/clone, usage, compaction, and retry controls reuse Pi operations. |
| Projects | Registered projects are durable canonical-directory presets with stable `ProjectId` values. Missing paths stay detached until explicit relocation or removal. |
| Worktrees | Parallel Git work uses unique recoverable worktrees bound to an exact repository/base/branch/path. Recovery is non-destructive; cleanup requires a fresh exact identity/safety proof. |
| User data | Session drafts, project/worktree registries, Automation chains, custom model identities, and preferences persist under the portable `pi-wizard-data` root through bounded schema-versioned stores. Drafts are session-scoped and generation-safe. |
| Models | The shared picker discovers models from Pi globally and refreshes project-specific launch options separately. New Run has a durable model preference, favorites-first ordering, and a bounded credential-free custom identity catalog; Pi still owns credentials and capability truth. |
| Renderer | Solid is a bounded projection of backend state. Hydration/recovery is versioned, event wakeups trigger bounded pulls, large history stays windowed, tool protocol is omitted from historical transcript presentation, and active tool work is reduced to compact user-facing activity. |
| Attention | Pi extension select/confirm/input/editor requests remain backend-owned by exact request ID and appear in a global Needs Attention surface. Fire-and-forget extension UI state and draft/editor state remain separate owners. |
| Git review | Status and diff work is explicit and backend-owned. Binary files return metadata; text diffs use bounded pages and stale-proof cursors; in-flight review jobs are cancelable/supersedable. |
| Automation | Finite saved prompt chains launch ordinary Pi sessions under the shared live-run ceiling. Local execution is sequential; parallel workers use unique worktrees. Cancellation stops future launches without terminating existing workers. |
| Supervision | Supervision is independent from Automation. One ordinary Pi supervisor run consumes one live slot, observes eligible project runs on semantic state changes, and may issue bounded validated Send/Steer/Follow-up directives to exact RunIds. |
| Windows host | Tauri runs as a GUI-subsystem application. Standard npm Pi installs resolve to direct Node + Pi CLI invocation with no console window; a desktop-lifetime kill-on-close Job Object backs abrupt-exit cleanup. |
| Diagnostics | Runtime diagnostics are explicit pull snapshots over existing bounded owners; they do not introduce passive telemetry, logging, or polling. |

## Known upstream limitation

- Direct in-place navigation to an arbitrary historical branch is not exposed by the current Pi RPC command surface. The full branch tree is visible and user-message nodes invoke Pi-native fork semantics; Pi Wizard does not invent a renderer-only branch mutation.

## Out of scope

Unless the user explicitly expands the product boundary, Pi Wizard does not include:

- an embedded terminal, source editor, or file explorer;
- branch integration, commit, or conflict-resolution workflows;
- remote/mobile clients or a background daemon;
- scheduled autonomous jobs;
- a custom plugin/provider marketplace or application-owned provider authentication;
- a multi-harness abstraction beyond Pi;
- fake permission modes that Pi does not enforce;
- a built-in container/VM sandbox.

See `ROADMAP.md` for the entry criteria applied to future scope.
