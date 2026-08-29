# Product Design

## 1. Product definition

Pi Wizard is a desktop command center for upstream Pi sessions. It should make the common high-value workflow faster than terminals without expanding into a general IDE.

The product succeeds when a developer can open a project, start one or several Pi agents, see which ones are working or need attention, steer them, review their changes, and continue existing Pi sessions while the application remains responsive under long histories and tool-heavy work.

## 2. Design goals

### Core

- Make Pi discoverable without hiding its native semantics.
- Make parallel autonomous work legible and easy to control.
- Keep the UI responsive while several sessions stream concurrently.
- Make session/worktree identity obvious so parallel changes do not collide accidentally.
- Keep context/model/thinking/tool activity visible without filling the screen with telemetry.
- Preserve a direct route back to the Pi CLI and normal project tooling.

### Quality

- Startup and navigation should not depend on hydrating every historical session.
- Typing, scrolling, aborting, and switching sessions remain responsive during streaming.
- Passive UI rows incur close to zero runtime work.
- Failures are attached to the process/session/request that owns them.
- The interface is keyboard-efficient but does not require memorized shortcuts.

## 3. Explicit non-goals for the first product

Pi Wizard is not initially:

- a VS Code replacement;
- a general Git client;
- a terminal multiplexer's replacement;
- a multi-harness abstraction across Codex, Claude Code, OpenCode, and Pi;
- a new Pi extension/package manager;
- a second model credential store;
- a security sandbox implemented in UI labels;
- a cron-style or always-on background automation scheduler.

These constraints are important. Existing agent desktops tend to accumulate editor, terminal, browser, preview, mobile, plugin, remote, and Git-management surfaces. Those can be useful, but they increase hot state, watchers, process count, renderer complexity, and failure coupling. Pi Wizard starts with the smallest surface that materially improves Pi orchestration.

Finite user-started automation is in scope as one independent feature. A saved chain is deliberately small: a name plus an ordered list of prompts. Starting a chain chooses the project, worker concurrency, local/worktree execution policy, and worker model/thinking selection. The runtime fills only available live-run slots, starts one new Pi session per prompt, and advances on backend run-state transitions rather than timers or repository polling. For concurrent work in one project, chains use unique Git worktrees; completed worker processes are closed to release capacity while their Pi session history and Git worktrees remain available for review. Automation has no supervisor toggle and no supervisor lifecycle state.

**Supervision** is a separate orchestration feature, not a mode of automation and not a replacement agent engine. A supervision session is one ordinary Pi session counted against the same live-run ceiling. The user starts it for a project independently from any automation chain, chooses its model/thinking level, and may stop it without cancelling worker runs. It observes bounded summaries of that project's eligible live runs only when semantic workflow state changes and may return a small validated set of Send/Steer/Follow-up directives addressed to exact RunIds. Manual sessions and automation workers are both eligible targets. Unknown runs, oversized messages, malformed output, and unsupported actions are rejected rather than guessed. There is no token-level supervisor loop and no periodic “are you done?” polling.

## 4. Mental model

The user sees four durable concepts:

**Project**  
A registered directory. It may contain many historical and live Pi sessions.

**Session**  
A Pi conversation/history. Pi owns its content and branching semantics.

**Run**  
A live Pi RPC process attached to a session and one immutable execution root.

**Execution root**  
Either the project's local checkout or an explicitly created Git worktree. A future contained runtime may use a container/VM root, but that is a different isolation class.

Keeping Session and Run separate prevents a common GUI failure: treating whatever tab is visible as if it were the running process itself.

## 5. Information architecture

### Application shell

The desktop window has three structural regions, but only two are normally visible:

1. **Sidebar**: projects, active sessions, recent sessions, and a compact Needs Attention entry.
2. **Main surface**: either the active session timeline or the multi-agent dashboard.
3. **Inspector drawer**: changes, session tree, or run details. It is closed by default and only one inspector is mounted at a time.

Avoid a permanent three-pane IDE layout. The primary object is an agent session, not a file tree.

### Top bar

Always shows the identity that matters for safe orchestration:

- project;
- branch/worktree label;
- local/worktree execution badge;
- model;
- thinking level;
- run state.

The execution badge uses literal language such as **Local checkout** or **Git-isolated worktree**. It never implies sandboxing.

### Sidebar

The sidebar prioritizes live state over historical volume:

- Active
  - Working
  - Needs attention
  - Queued
- Recent
- Projects

Historical sessions are paged/searchable rather than all materialized into a long reactive tree.

Each session row is intentionally cheap: title, project/worktree indicator, and coarse state. No per-row Git command, filesystem watcher, token calculation, or history hydration is allowed.

### Multi-agent dashboard

The dashboard is the orchestration home. It shows one compact card per live run:

- task/session title;
- project + worktree;
- working / needs attention / compacting / queued / ready / done / failed / termination uncertain;
- current high-level activity;
- elapsed time;
- change summary when already known;
- Stop and Open controls.

It is not a miniature transcript grid. Detailed streams stay inside the session view.

### Automation

Automation is a first-class main view rather than another mode hidden inside the composer. It contains a compact saved-chain list, an ordered prompt-card editor, start controls for project/concurrency/isolation/model, and bounded current execution progress with links to live worker runs.

Chains are finite. Cancel stops the chain from launching more work but does not kill already-running Pi sessions. Worker failures are attached to their exact step and do not silently discard later prompts. Automation never auto-deletes task worktrees.

### Supervision

Supervision is a separate first-class main view. It selects a registered project, a supervisor model/thinking level, and a finite cycle budget, then starts one dedicated Pi supervisor session in its own Git worktree. It can supervise live runs that were started manually or by Automation. Stopping supervision terminates only the app-owned supervisor process; it never implicitly stops, closes, or cancels worker runs.

Supervision status is presented separately from Automation execution status. A failed or malformed supervisor response ends/disables that supervision session without changing the ownership or lifecycle of the runs it was observing.

## 6. Session view

### Timeline

The timeline groups activity into semantic turns rather than exposing raw protocol events.

- user messages are always visible;
- assistant text streams in place;
- reasoning/thinking follows Pi settings and is collapsed by default when verbose;
- tool executions are compact cards;
- successful completed tool cards collapse to a one-line summary by default;
- active, failed, or explicitly expanded tools show a bounded output preview;
- compaction/retry/session events are lightweight notices rather than full message blocks; compaction abort/failure/overflow-retry and provider/summarization retry state remain visible when they affect recovery, and a quiet Working stream may show a one-shot advisory without being relabeled failed or idle.

Only a virtualized window of turn groups is mounted. Older content loads in bounded pages as the user navigates upward.

### Composer

The composer exposes Pi's native queue semantics directly.

When idle:

- **Send** starts the next turn.

When the agent is running:

- **Steer** queues a steering message using Pi's steering path;
- **Follow up** queues work for after the current run settles;
- **Stop** clears queued work, preserves the cleared text for recovery, and aborts the active run.

A successful ordinary Stop on a Pi build with native queue clearing does not create a terminal “stopped” process state. Pi remains alive and reusable, so the run returns to **Ready** after settlement. If the installed Pi RPC does not expose queue clearing, or Stop otherwise must escalate to process termination, the terminal state reflects the confirmed exit/failure or explicit termination uncertainty instead of pretending the reusable RPC abort succeeded. New Run capability discovery warns when the installed Pi build requires this process-terminating Stop path, and a completed hard Stop explains that the Pi session can be resumed.

The user should never have to guess whether a message will interrupt, steer, or wait. Keyboard shortcuts can mirror Pi, but the semantic action is visible in the control.

Slash-command autocomplete is populated from Pi's runtime command discovery rather than a hardcoded app list. The bounded palette supports Arrow Up/Down selection plus Enter to stage the selected command, keeps the active row visible, and does not create an application-owned input-history store. Model selectors merge Pi-discovered models with a small user-managed catalog of explicit provider/model pairs. Pi remains the launch-time validator, and Pi-discovered metadata wins when it matches a saved pair. Thinking choices come from Pi capability discovery for the selected model.

The composer supports Pi-native image attachments with explicit count/per-image/aggregate limits. Picker, paste, drag/drop, IPC, and restored drafts all pass through the same backend validation; an oversized attachment is rejected before it can become a large reactive/IPC payload. When Pi explicitly declares the selected model text-only, the UI disables new image ingestion and blocks submission of existing image drafts while keeping those images removable/preserved. Missing capability metadata stays compatible rather than being guessed as unsupported.

An idle session also exposes **Compact context**, which invokes Pi's native manual compaction. The label describes the user job, while Pi remains responsible for summary/context semantics and the runtime reconciles authoritative state after completion. Run details may also send Pi's native automatic-provider-retry enable/disable command. Because current `get_state` does not report that flag, the UI never presents a cached choice as authoritative current state after reload/recovery.

Draft text is session-scoped and durable. Switching sessions never carries a draft to another session and never silently discards it. The UI can remain visually quiet when persistence is healthy, but a failed save is visible and retryable. Extension-driven `set_editor_text` updates participate in the same draft state rather than replacing it behind the user's back.

### Attention requests

Extension-driven selects/confirms/inputs and other explicit UI requests are represented as request objects owned by the backend runtime store. They appear both in their session and in the global Needs Attention view.

The request's identity survives navigation. Opening another session cannot rebind, reuse, or silently answer a pending request.

If the same dialog component remains mounted while the backend advances to another request, all local dialog state resets from the new request ID. A stale request that Pi has already resolved/discarded disappears rather than leaving an actionable zombie prompt.

## 7. Starting work

The New Session sheet asks only for decisions that materially affect the run:

1. Project.
2. Execution root:
   - **Local checkout**
   - **New Git worktree** when the project is a Git repository.
3. Model and thinking level, defaulting to Pi configuration but allowing Pi-discovered or user-saved provider/model pairs.
4. Initial task.

Advanced options are collapsed. Project trust is handled before launching RPC when Pi detects protected project resources and no applicable decision exists. The user can run the process with resources approved or ignored for that run. The UI explains that this controls project resource loading only. Extension discovery is a separate per-launch recovery choice: users may inherit normal Pi extension discovery or disable extensions for that child when a broken installed extension prevents startup. Disabling extensions is not described as trust, context-file, or sandbox policy.

The normal trust selection is **Use Pi trust settings**, preserving Pi's saved canonical-directory decision and global fallback. One-run **Approve** and **Ignore protected resources** overrides remain available. The sheet explicitly notes that `AGENTS.md`/`CLAUDE.md` context instructions still load when protected resources are ignored; disabling context instructions is a separate advanced choice.

Project rows are backed by a stable app project ID and canonical path. If a folder is renamed/moved/deleted outside Pi Wizard, the row becomes **Detached** with **Relocate** / **Remove** actions. It never silently opens another checkout or sends new sessions to a global/default project merely because names/remotes look similar.

For autonomous parallel work, **New Git worktree** is the recommended Git-isolation default. It prevents agents from editing the same checkout, but the UI explicitly states that it does not restrict filesystem, network, shell, credentials, or host process access.

Before creation, the sheet shows the exact source branch/base commit that will seed the worktree. The user is never shown a branch label derived from stale cached project state. Once launched, that worktree/run binding is immutable.

The model control is usable without a separate hidden prerequisite. When a concrete project is selected, Pi Wizard loads Pi's available models for that project on demand and keeps an explicit refresh action. The same reusable model picker is used by New Run, Automation, and Supervision. A user may also add or remove a bounded custom provider/model entry so a valid Pi model can be selected even when the current provider does not enumerate it through `get_available_models`.

## 8. Review surface

The Changes inspector answers one question: what has this run changed in its execution root?

- Load repository status only when the inspector is opened or when an explicit post-turn invalidation says changes may have occurred.
- Compute diff outside the renderer.
- Render one file at a time for large change sets.
- Start with metadata and hunks, not a monolithic concatenated diff string.
- Apply strict display and payload bounds with an explicit Load More path.
- Large/binary files show metadata rather than forcing inline rendering.

The first version is review-only. Staging, committing, conflict resolution, and branch integration should remain external unless later evidence shows they are essential to the orchestration loop.

## 9. Session history and tree

Pi already stores branchable JSONL session trees. Pi Wizard should visualize that model instead of inventing a separate thread graph.

The Session Tree inspector supports:

- current branch path;
- fork points;
- forking through Pi-owned session operations, and direct historical branch switching only if Pi exposes a corresponding RPC operation;
- session rename where Pi exposes it.

Recent history should open on the latest bounded window. For a live Pi process, new history synchronizes incrementally with Pi's stable `get_entries(since)` entry cursor. Cold history is parsed from Pi JSONL incrementally as the user moves backward. Neither path hydrates an entire large transcript merely to open its latest turn. Session catalog previews may normalize Pi's explicit persisted skill wrapper so generated skill instructions do not hide the user's actual task, but Pi JSONL remains untouched. Before a historical session is resumed for writing, an unterminated JSONL tail is rejected rather than allowing Pi's next append to concatenate records.

## 10. Features borrowed deliberately

| Source | Keep | Why |
| --- | --- | --- |
| Pi | RPC integration, steer/follow-up queues, session tree/forking, compaction, dynamic models/thinking/commands, extension UI | Native semantics with no duplicate agent engine |
| Codex desktop | project/thread command-center model, parallel agent supervision, diff-centric review, worktree isolation | Strong orchestration mental model |
| Claude Code/Desktop | clear independent-session model, automatic worktree workflow, explicit run modes, separate-context workers | Makes parallel work understandable |
| OpenCode | discoverable permission-style UX patterns, model/provider flexibility, command palette patterns | Good compact control vocabulary, while Pi semantics remain authoritative |
| OpenChamber | lazy large-diff loading and bounded history controls | Direct response to known desktop performance failures |
| Existing Pi GUIs | timeline + collapsible tools, Pi-owned session compatibility, extension UI bridging | Proven Pi-specific utility |

## 11. Patterns intentionally not copied

- **Full transcript hydration on session open.** Large histories have caused renderer hangs/crashes in multiple agent desktops.
- **Synchronous renderer diff computation.** Huge diffs can freeze the UI even when the agent itself is healthy.
- **Reactive state proportional to every historical message.** A long-lived app must keep cold history cold.
- **Focus-dependent approval/request state.** Pending interactions belong to a runtime request ID, not the currently mounted component.
- **Per-token durable persistence.** Streaming deltas stay in bounded memory; they do not fsync catalogs or globally serialize unrelated sessions.
- **Best-effort invisible draft saves.** User text has explicit generation-based durability and surfaced failures.
- **Optimistic Stop state.** A timed-out/rejected abort never renders as Idle; uncertain termination is called out literally.
- **Reusable worktree pools.** A live run owns one immutable worktree identity until explicit cleanup.
- **Per-sidebar-row repository polling.** Passive rows cannot create continuous Git subprocess load.
- **Navigation-triggered process teardown.** A session keeps running when the user looks elsewhere.
- **Bundled IDE by default.** File browser/editor/terminal/preview panes are deferred until their value exceeds their process and UI cost.
- **Pretend safety modes.** The app will not label host execution as sandboxed or infer permissions Pi does not enforce.
- **Silent project fallback.** Missing/stale project paths never redirect sessions to another/global project.
- **Startup dependence on one derived-state blob.** Corrupt project/catalog/preferences state is isolated and recoverable rather than making authoritative Pi history appear lost.
- **Blind desktop PATH inheritance.** The GUI does not assume Dock/Start-menu launch has the same toolchain environment as a terminal.

## 12. Visual direction

The interface should be dense but quiet:

- native desktop spacing rather than oversized chat cards;
- one accent color for selection/activity, with status primarily expressed through text/icon shape rather than saturated color;
- monospaced treatment reserved for paths, commands, diffs, and code;
- minimal animation, limited to state transitions that communicate causality;
- no continuously animated activity when a static working indicator is sufficient;
- keyboard focus is always visible;
- theme follows system by default.

Performance is part of the visual design. A simpler DOM and fewer simultaneously mounted panels are preferred over decorative richness.
