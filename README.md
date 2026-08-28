# Pi Wizard

Pi Wizard is a personal Windows desktop control surface for the Pi coding harness. The product is intentionally a shell around upstream Pi, not a fork of its agent runtime, not a replacement IDE, and not a cross-platform or web product.

The primary jobs are:

- start and resume Pi sessions without terminal ceremony;
- run several independent coding sessions and see their state at a glance;
- steer a running agent or queue follow-up work using Pi's native queue semantics;
- isolate parallel edits with Git worktrees when requested;
- review tool activity and repository changes without rendering unbounded output;
- preserve compatibility with the Pi CLI's models, settings, extensions, commands, sessions, and authentication.

## Current state

**The current Windows desktop application is implemented through its deterministic hardening boundary. Runtime/session orchestration, parallel Git-worktree runs, bounded change review, crash recovery, accessibility contracts, scale verification, and optimized desktop builds are operational.**

The current desktop shell can discover Pi, register and launch existing projects, launch/recover/conservatively clean isolated Git worktrees, enforce a durable live-run ceiling, start or resume boundedly discovered project sessions, page persisted active-branch history, inspect Pi's full session tree and fork/clone/name sessions, select discovered models and thinking levels, control Pi's native automatic/manual compaction, run discovered slash commands, persist session-scoped text/image drafts, send/steer/follow up/stop through Pi-native RPC semantics, handle extension UI requests, render bounded live assistant/tool output, and review changes through cancelable paged diffs with binary handling and hunk navigation. The Rust runtime owns process/recovery/Git identity and the Solid renderer remains a projection rather than an authority. The repository `full` lane includes scale/startup checks and an optimized Windows desktop build. `STATUS.md` tracks the exact implemented boundary.

## Product principles

1. **Pi owns agent semantics.** Pi remains the source of truth for agent execution, session history, models, tools, extensions, and commands.
2. **The GUI owns orchestration and presentation.** It manages Pi subprocesses, workspaces, worktrees, bounded projections, native windows, and review surfaces.
3. **Idle means idle.** No hidden repository polling, eager transcript hydration, or background work just because an item is visible in a sidebar.
4. **Large histories are normal input.** Transcript, tool output, and diffs are virtualized, paged, truncated, or loaded on demand by design.
5. **Autonomy labels describe real boundaries.** A worktree is Git isolation, not a sandbox. Pi project trust is resource loading, not execution isolation.
6. **Navigation does not own process lifetime.** Switching views must not implicitly abort, restart, or retarget an active Pi process.
7. **One runtime path.** Interactive GUI operations go through Pi RPC rather than a parallel reimplementation of Pi behavior.

## Authority map

| Question | Authority |
| --- | --- |
| What is this project? | `README.md` |
| What must contributors preserve? | `AGENTS.md` |
| What user experience are we building? | `DESIGN.md` |
| How is it intended to work internally? | `ARCHITECTURE.md` |
| What research drove the design? | `RESEARCH.md` |
| What exists today? | `STATUS.md` |
| What is the implementation sequence? | `ROADMAP.md` |
| How will implementation be verified? | `TESTING.md` |

## Stack

- Tauri 2 desktop host
- Rust + Tokio backend
- SolidJS + TypeScript frontend
- Vite static frontend build, with no production localhost server
- one upstream `pi --mode rpc` subprocess per live runtime session
- Pi JSONL session files as authoritative conversation persistence
- a small derived catalog/index only where measurements justify it

The detailed rationale and boundaries are in `ARCHITECTURE.md`. Exact implemented scope is in `STATUS.md`.
