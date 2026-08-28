# Pi Wizard Agent Guide

**BCA policy:** advisory

Follow the workspace `../AGENTS.md` and portfolio `../STANDARDS.md` first. Pi Wizard currently applies the Universal and Stateful Application profiles. If the application later exposes an agent-facing control API, explicitly add the Agent Tool profile rather than assuming it now.

## Read order

For a cold start, read:

1. `README.md`
2. `STATUS.md`
3. `DESIGN.md` for product or UI work
4. `ARCHITECTURE.md` for runtime, persistence, Git, process, or performance work
5. `TESTING.md` before implementation changes
6. `ROADMAP.md` only for sequencing or future-scope questions

Use `RESEARCH.md` when a decision needs its external evidence or competitor context.

## Non-negotiable invariants

- Do not fork or reimplement Pi's agent loop when an upstream RPC operation/event owns the behavior.
- Do not make the renderer authoritative for process state, request ownership, trust decisions, worktree identity, or pending interactions.
- Keep `crates/pi-wizard-core` free of Tauri and renderer-framework types; desktop/framework code adapts the core rather than owning its semantics.
- Treat process lifecycle and agent activity as separate state axes. In particular, Pi `agent_settled` is not process termination.
- Treat Pi `message_update`/stream updates as transient hot state. Never trigger durable app-owned writes or fsyncs directly from token/tool progress events.
- Use Pi `get_entries(since)` as the preferred live-session append cursor; do not routinely hydrate full messages when a stable incremental entry cursor answers the question.
- User-facing Stop clears/preserves queued messages before aborting. A process whose termination cannot be confirmed becomes quarantined; it must never be reported as idle/stopped or accept further RPC writes.
- Composer drafts are session-scoped backend-owned user data with generation-safe persistence. Renderer/component lifetime is not draft lifetime.
- Registered projects have stable app IDs bound to canonical paths. A missing/moved path becomes detached and requires explicit relocation; never silently fall back to another project, a global project, a matching display name, or a matching Git remote.
- App-owned registries/indexes/preferences are recoverable derived state. Corruption in one derived-state domain must be quarantined or rebuilt without hiding/deleting Pi session JSONL or blocking safe startup.
- Desktop environment discovery is a backend responsibility. Resolve a usable Pi/Git/toolchain environment explicitly and boundedly; do not assume a GUI-launched process has the same PATH as the user's terminal, and never persist/log secret environment values merely to debug discovery.
- Validate image attachments again at the backend RPC boundary regardless of picker/drag/drop checks.
- Do not write a second authoritative transcript store. Pi session JSONL remains authoritative.
- Do not eagerly load or mount complete large transcripts, full tool logs, or large diffs.
- Do not compute large diffs synchronously in the renderer.
- Do not add periodic Git/session/filesystem polling merely to keep passive UI fresh. Prefer event-driven invalidation and explicit refresh.
- A live session's canonical working directory is immutable for that process. UI navigation cannot silently retarget it.
- Worktree creation binds an explicit base commit and branch/path as a recoverable transaction. Never infer a default branch, pool/reassign live worktrees, or run Git mutations from UI navigation state instead of the run's canonical root.
- Never call a worktree a sandbox. Never call Pi project trust a permission sandbox.
- Pi project-resource trust and context-file loading are separate. `--no-approve` does not disable `AGENTS.md`/`CLAUDE.md`; only an explicit context-files policy may do that.
- Long-running or unattended work must show the actual execution boundary: host, Git-isolated worktree, or a future real container/VM sandbox.
- Do not kill processes by executable name or wildcard. Lifecycle actions target a process identity owned by the runtime manager.
- Keep production desktop architecture serverless from the user's perspective: no bundled Next.js/local HTTP application server for the core UI.
- Keep optional IDE-like surfaces out of the initial product unless evidence shows they are necessary for the core orchestration job.
- GitHub Actions are prohibited by workspace policy. Verification is repository-local.

## Change routing

| Change | Primary owner |
| --- | --- |
| Pi command/event interpretation | Pi RPC adapter in backend |
| Child lifecycle and backpressure | Runtime/process manager |
| Session catalog/read model | Session catalog/index owner |
| Worktree creation/removal | Worktree service |
| Diff generation/chunking | Git review service |
| GUI state projection | Backend runtime store + typed IPC contract |
| Timeline rendering | Virtualized frontend timeline |
| Commands/models/thinking choices | Runtime-derived Pi capabilities |
| Product interaction policy | `DESIGN.md` |
| Performance budgets | `ARCHITECTURE.md` and executable benchmarks once implemented |

## Design-before-code gate

Application code should not be introduced until a proposed implementation is consistent with these locked baseline decisions:

1. Tauri/Rust host and static frontend.
2. Pi RPC subprocess integration as the default runtime boundary.
3. Pi JSONL remains authoritative persistence.
4. Bounded/virtualized history and diff rendering is part of the first implementation, not later optimization.
5. Active sessions are independent from navigation state.
6. Git isolation and security isolation are represented separately.

If implementation evidence invalidates one of these decisions, update `RESEARCH.md`, then the owning design/architecture authority before changing the implementation direction.
