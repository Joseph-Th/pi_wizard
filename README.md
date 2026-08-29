# Pi Wizard

Pi Wizard is a personal Windows desktop control surface for the Pi coding harness. The product is intentionally a shell around upstream Pi, not a fork of its agent runtime, not a replacement IDE, and not a cross-platform or web product.

The primary jobs are:

- start and resume Pi sessions without terminal ceremony;
- run several independent coding sessions and see their state at a glance;
- build finite reusable prompt chains that fill available Pi session slots automatically;
- independently start one Pi supervision session for a project when active manual or automated runs should be directed;
- choose Pi-discovered models for New Run, Automation, and Supervision, with a small saved custom provider/model catalog for valid identities Pi does not enumerate;
- steer a running agent or queue follow-up work using Pi's native queue semantics;
- isolate parallel edits with Git worktrees when requested;
- review tool activity and repository changes without rendering unbounded output;
- preserve compatibility with the Pi CLI's models, settings, extensions, commands, sessions, and authentication.

## Current state

**The current Windows desktop application is implemented through its deterministic hardening boundary. Runtime/session orchestration, parallel Git-worktree runs, bounded change review, crash recovery, accessibility contracts, scale verification, and optimized desktop builds are operational.**

The current desktop shell can discover Pi, register and launch existing projects, choose Pi-discovered model/thinking settings before a new run's initial task is submitted, independently control Pi context-file and extension discovery for a launch, launch/recover/conservatively clean isolated Git worktrees, enforce a durable live-run ceiling, browse and traverse bounded project-session discovery pages from a first-class on-demand Recent Sessions view, page persisted active-branch history, inspect Pi's full session tree and fork/clone/name sessions, control Pi's native automatic/manual compaction and native provider auto-retry command, navigate discovered slash commands by keyboard, persist session-scoped text/image drafts, send/steer/follow up/stop through Pi-native RPC semantics, answer backend-owned extension requests from a global deadline-prioritized Needs Attention queue, render bounded live assistant/tool output, and review changes through cancelable paged diffs with binary handling and hunk navigation. Writable session Resume refuses unterminated Pi JSONL tails rather than risking record concatenation, while session catalog previews normalize Pi's explicit skill-expansion wrapper so generated SKILL.md content does not obscure the user's task. Retry, summarization-retry, compaction abort/failure/overflow-retry, extension-error, and one-shot quiet-stream recovery state are projected as bounded notices; Pi Wizard never silently replays a prompt or manufactures a successful cancellation. The launch-options probe also detects whether the installed Pi RPC exposes `clear_queue`: supported builds keep the normal reusable Stop path, while current Pi 0.84.3 is handled safely by restoring the last bounded user queue event and terminating the exact Pi process so queued/extension continuation work cannot continue. Run/dashboard identity surfaces keep the registered project, execution class/root, model, thinking level, queue/compaction state, worktree branch, backend-owned elapsed lifetime, and the last explicitly computed change count visible without creating a second authority or polling Git. Backend change revisions make older review metadata visibly stale after subsequent tool/shell activity. An explicit Runtime diagnostic snapshot exposes bounded process/RPC/backlog/job counters and mounted-row/development long-task measurements without starting telemetry polling or durable logging. The Rust runtime owns process/recovery/Git/request identity and the Solid renderer remains a projection rather than an authority. The repository `full` lane includes scale/startup checks and an optimized Windows desktop build. `STATUS.md` tracks the exact implemented boundary.

The Automation view persists small ordered prompt chains and runs them through new Pi sessions under the same live-run ceiling as manual work. Parallel workers use unique Git worktrees; completed workers are closed to free capacity while their Pi sessions and worktrees remain available for review. Automation advances only on backend semantic run-state changes. Its catalog is loaded only while the Automation view is open, execution updates use a smaller execution-only IPC projection, and step rows retain only a bounded 1 KiB prompt preview. Automation owns no supervisor toggle, supervisor slot reservation, or supervisor lifecycle state.

Supervision is a separate first-class view and coordinator. Starting it chooses one registered project, model/thinking selection, and finite cycle budget. Its one ordinary Pi supervisor process consumes a normal live-run slot and can observe eligible manual or automated runs for that project, then issue strictly validated Send, Steer, or Follow-up directives to exact active RunIds. Stopping or failing supervision terminates only the internal supervisor run; observed workers remain user-owned. It does not copy transcripts, poll workers, or create another agent runtime.

The shared model picker automatically loads Pi-discovered models for the selected project and keeps **Pi default** as an explicit choice. A bounded credential-free custom model catalog can remember additional provider/model identities and optional labels; Pi still validates every identity at launch/probe time and remains the authority for credentials and model capability metadata.

Pi Wizard keeps its own durable state in a portable `pi-wizard-data` directory, not in Windows AppData. Repository builds place it at the project root (`<repo>\pi-wizard-data`) so `target` cleanup/rebuilds cannot delete user state; a standalone copied executable uses a sibling `pi-wizard-data` directory. That directory contains prompt chains, custom model profiles, project registrations, worktree recovery records, preferences, and session drafts through their existing bounded atomic stores. Custom models are added/removed at runtime from the model picker and are persisted in `model-profiles.json`; adding one does not require recompiling Pi Wizard. Upgrade migration prefers any prior `target\debug|release\pi-wizard-data` state, otherwise imports the older AppData `runtime-state`, and never overwrites an already-existing current portable root.

Every Windows Pi Wizard executable is built as a GUI-subsystem application, so opening the app does not create or attach a console/terminal window. On standard Windows npm Pi installs, Pi Wizard resolves the npm shim to Node plus Pi's CLI entry point and launches that directly with `CREATE_NO_WINDOW` instead of keeping a command-shell wrapper alive. A live run refuses an unresolved `.cmd`, `.bat`, or `.ps1` Pi launcher rather than retaining a shell in the background, and hard-stop `taskkill.exe` escalation is also created with no console window. Graceful RuntimeManager shutdown still owns normal lifecycle. A desktop-lifetime Windows kill-on-close Job Object provides the abrupt-exit backstop so inherited Pi/Node descendants cannot be orphaned if the GUI process terminates unexpectedly.

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

For an optional no-prompt compatibility check against the locally installed Pi CLI, run `python tools/smoke_live_pi.py`. This is deliberately separate from deterministic repository verification and does not send a model prompt or create a Pi session.
