# Research Evidence

Evidence snapshot: **2026-08-27**. Refresh external claims when a new decision depends on them.

This document summarizes external evidence that supports Pi Wizard's current design constraints. It is not product authority: `DESIGN.md` owns user-facing behavior and `ARCHITECTURE.md` owns runtime contracts.

Competitor/platform references are evidence only. They do not expand the personal Windows desktop scope defined in `README.md` and `AGENTS.md`.

## 1. Method

The research pass prioritized:

1. current upstream Pi documentation;
2. current official documentation for Codex, Claude Code, OpenCode, Solid, and Tauri where relevant;
3. existing Pi desktop projects to see what Pi-specific surfaces are already useful;
4. recent issue reports from active coding-agent desktops, especially performance and concurrency failures;
5. features that improve orchestration without requiring Pi Wizard to become a general IDE.

Issue reports are evidence of failure modes, not proof that an entire competing product or framework is inherently defective. The transferable design constraint is what matters.

## 2. Pi: current upstream facts

Primary docs:

- https://pi.dev/docs/latest
- https://pi.dev/docs/latest/usage
- https://pi.dev/docs/latest/rpc
- https://pi.dev/docs/latest/sessions
- https://pi.dev/docs/latest/compaction
- https://pi.dev/docs/latest/settings
- https://pi.dev/docs/latest/security
- https://pi.dev/docs/latest/containerization
- https://pi.dev/docs/latest/extensions
- https://pi.dev/docs/latest/sdk
- https://pi.dev/news/2026/5/7/pi-has-a-new-home

### Project identity

Pi moved to Earendil Works in May 2026. The current repository is `earendil-works/pi` and current npm packages use the `@earendil-works` scope. The CLI remains `pi`, and existing configuration/session locations are retained.

### Pi is intentionally a small core

Pi describes itself as a minimal terminal coding harness extended through TypeScript extensions, skills, prompt templates, themes, and packages. That argues against rebuilding the agent inside the GUI.

### RPC is a first-class custom-UI boundary

`pi --mode rpc` exposes line-delimited JSON over stdin/stdout and is specifically documented for embedding Pi in custom UIs and IDEs.

The useful surface is substantially richer than prompt/response streaming. Current RPC documentation includes operations/events for:

- prompt, steer, follow-up, and abort;
- current state and messages;
- models and thinking levels;
- queue modes;
- compaction and retry state;
- shell/Bash execution;
- session naming, switching, forking, and related lifecycle operations;
- command discovery across extensions/templates/skills;
- structured message/tool execution streaming;
- extension UI requests and notifications.

The GUI can therefore be a semantic client of Pi rather than an emulation of its TUI.

### Session history is already branchable

Pi stores persistent sessions as JSONL and models branch relationships in the file. Current user commands expose resume/new/name/session/tree/fork/clone/compact/export/share semantics. A desktop thread tree should visualize this model, not create a competing conversation graph.

### Queue semantics are a differentiator

Pi explicitly distinguishes steering messages from follow-up messages. A GUI can make that distinction clearer than a terminal shortcut while preserving native behavior.

### Trust is not sandboxing

Current Pi security documentation is explicit:

- project trust decides whether project-local settings/resources/extensions/packages are loaded;
- RPC/noninteractive modes do not show the normal trust prompt, so launch overrides or prior trust decisions matter;
- Pi has no built-in sandbox;
- Pi tools/extensions run with the permissions of the Pi process;
- untrusted or unattended work should use a real container, VM, micro-VM, remote sandbox, or policy-controlled sandbox.

This is why Pi Wizard separates project-resource trust, Git worktree isolation, and execution sandboxing in its model.

## 3. Existing Pi desktop clients

These projects validate useful surfaces but also show how quickly scope can expand.

### minghinmatthewlam/pi-gui

https://github.com/minghinmatthewlam/pi-gui

Observed design:

- Electron shell around upstream Pi SDK/runtime;
- threaded timeline;
- worktrees per thread;
- integrated terminal;
- inline diff viewer;
- multi-agent orchestration;
- Pi session files retained as source of truth.

Takeaway: timeline + worktree + change review + multi-agent orchestration are useful Pi-specific desktop jobs. Electron and an integrated terminal are not required for those jobs.

### StarkInternationalAI/pi-desktop

https://github.com/StarkInternationalAI/pi-desktop

Observed design:

- Tauri 2 + Rust/Tokio + Lit;
- one Pi RPC process per session;
- SQLite/FTS session index;
- session tree and command discovery;
- extension UI bridging.

Takeaway: a Rust/Tauri + Pi RPC architecture is practical. The process boundary benefits from explicit performance/backpressure contracts, and full-text indexing is not a startup prerequisite.

### justhil/pi-app

https://github.com/justhil/pi-app

Observed design:

- Pi SDK shell with timeline, side panels, queue, session tree, file/review/run/context surfaces;
- extension pop-ups;
- only recent messages load immediately, with more history loaded later.

Takeaway: lazy history is useful in Pi-specific GUIs. Broader file/run/voice/editor surfaces are possible but are not required by Pi Wizard's current orchestration scope.

### Other Pi GUIs

Several 2026 Pi desktop repositories use Electron or Tauri, often adding terminals, editors, GitHub integration, remote runtimes, voice, packages, and provider management. The aggregate lesson is not that these features are bad. It is that the category has a strong tendency toward IDE-shaped scope. Pi Wizard differentiates through a smaller orchestration core and measurable hot-state limits.

## 4. Codex desktop

Primary product source:

- https://openai.com/index/introducing-the-codex-app/

Strong patterns to borrow:

- command-center framing for several agents;
- project + thread organization;
- parallel task execution;
- Git worktree isolation;
- diff-centric review before integration;
- sessions shared across CLI/other surfaces conceptually rather than trapped in one view.

### Performance feedback

Representative 2026 issues:

- https://github.com/openai/codex/issues/13809 — very large Git diffs make the app extremely laggy and memory-heavy.
- https://github.com/openai/codex/issues/21299 — long threads produce multi-second UI stalls on submit.
- https://github.com/openai/codex/issues/21134 — long active threads associated with renderer/app-server memory and log churn.
- https://github.com/openai/codex/issues/24427 — many threads in one project correlate with slow send/response startup.
- https://github.com/openai/codex/issues/27564 — selecting a thread with large JSONL events can hang/high-CPU the app.
- https://github.com/openai/codex/issues/37584 — recent Windows report of progressive lag, UI/backend desynchronization, and crashes.
- https://github.com/openai/codex/issues/38542 — recent report of memory growth with multiple windows and long-history threads.

Transferable constraints:

- never make hot UI state proportional to entire thread history;
- large diff preparation/rendering must be lazy and bounded;
- thread lists must not hydrate complete messages;
- diagnostics/logging itself needs byte/disk limits;
- backend process lifetime and UI state need explicit synchronization, not optimistic assumptions.

## 5. Claude Code and Claude Code Desktop

Primary docs:

- https://code.claude.com/docs/en/desktop
- https://code.claude.com/docs/en/worktrees
- https://code.claude.com/docs/en/agents
- https://code.claude.com/docs/en/sub-agents
- https://code.claude.com/docs/en/permissions
- https://code.claude.com/docs/en/permission-modes

Strong patterns to borrow:

- each parallel session has an independent context;
- worktree isolation is first-class for parallel coding;
- a dedicated agent view can supervise several runs without merging their transcripts;
- explicit mode vocabulary helps users understand autonomy/oversight tradeoffs;
- specialized agents preserve context by keeping side-work out of the main thread.

The relevant evidence is the orchestration model, not Claude Code's permission engine. A Pi run is a separate Pi process/session; any restricted execution mode requires a real enforcement boundary.

## 6. OpenCode

Primary docs:

- https://opencode.ai/docs
- https://opencode.ai/docs/permissions/
- https://opencode.ai/docs/agents/

Strong patterns to borrow:

- concise command palette/control vocabulary;
- explicit allow/ask/deny concepts demonstrate good safety communication where the underlying runtime enforces them;
- agent-specific configuration and provider/model flexibility;
- desktop/TUI parity as a product expectation.

Pi Wizard should not copy OpenCode's permission controls because Pi does not provide the same native enforcement model.

### Performance and state feedback

Representative issues:

- https://github.com/anomalyco/opencode/issues/34803 — desktop becomes very slow after long chats and visiting many sessions.
- https://github.com/anomalyco/opencode/issues/28844 — renderer can hang on sessions with very large message-part counts.
- https://github.com/anomalyco/opencode/issues/32486 — large persisted prompt history can make the whole GUI sluggish.
- https://github.com/anomalyco/opencode/issues/43982 — August 2026 report of visual lag in long conversations, including delayed stop UI.

Transferable constraints:

- renderer history must be windowed;
- persistent UI stores need hard size/retention policies;
- stop/abort controls cannot wait behind display work;
- list navigation should not instantiate full session state.

## 7. OpenChamber

Sources:

- https://github.com/openchamber/openchamber
- https://github.com/openchamber/openchamber/blob/main/CHANGELOG.md

Useful patterns include lazy loading for large diffs, explicit session history limits, stable scroll anchoring, worktree isolation, and session activity that remains correct when views are hidden. These are directly aligned with Pi Wizard's bounded-state goals.

Broader product surfaces beyond the personal Windows desktop app are not required for Pi Wizard.

## 8. Desktop stack research

### Tauri

Primary source:

- https://v2.tauri.app/reference/webview-versions/

On Windows, Tauri 2 uses the system WebView2 runtime. This avoids bundling a separate Chromium runtime while still providing the renderer needed for Markdown/code/diff UI.

### SolidJS

Primary sources:

- https://docs.solidjs.com/
- https://docs.solidjs.com/advanced-concepts/fine-grained-reactivity

Solid's fine-grained reactivity updates the DOM associated with changed signals rather than conceptually rerendering broad component trees. This is attractive for a desktop surface with many small independent status/stream changes.

Framework choice alone does not solve large-history performance. OpenCode issue reports demonstrate that even a Solid-based renderer can hang when the application constructs unbounded reactive message rows. Pi Wizard's main defense is bounded data/DOM architecture, with Solid as a complementary implementation choice.

## 9. Design conclusions

The strongest common pattern across successful harnesses is **parallel independent sessions plus clear supervision**, not maximum IDE integration.

The strongest recurring desktop failure pattern is **unbounded state crossing into a renderer**: full histories, giant diffs, giant persistent UI stores, broad reactivity, or verbose logs.

The strongest Pi-specific opportunity is that RPC already exposes the semantics a GUI needs. This makes a small, strict desktop shell realistic:

```text
Pi owns execution/history/capabilities.
Pi Wizard owns process orchestration, bounded projection, Git isolation, and review UX.
```

That authority boundary remains the basis of the current implementation.

## 10. Evidence-backed constraints

The following constraints are supported by upstream Pi documentation, current desktop-platform documentation, and repeated failure patterns across coding-agent desktops. `DESIGN.md` and `ARCHITECTURE.md` own the resulting product/runtime contracts; this section records only the external evidence route.

| Constraint | Evidence |
| --- | --- |
| Pi remains the agent/session authority; the desktop should consume Pi RPC rather than reproduce its loop. | Pi RPC, sessions, extensions, SDK documentation. |
| `steer`, `follow_up`, queue clearing, retry, compaction, session mutation, and `get_state` have distinct semantics and must not be collapsed into generic chat actions. | Pi RPC documentation. |
| Live assistant updates are content-indexed deltas; final messages are authoritative. Direct Bash updates are request-correlated. | Pi RPC documentation. |
| Project-resource trust is separate from context-file loading and from execution sandboxing. | Pi security and usage documentation. |
| Session replacement may be extension-cancelled even when the RPC response is transport-successful. | Pi RPC documentation. |
| Extension dialogs and fire-and-forget status/widget/title/editor updates require different ownership. | Pi RPC and extension documentation. |
| GUI-launched tool discovery must resolve and reuse one explicit environment rather than assuming terminal PATH parity. | Pi environment documentation plus desktop-harness launch-environment reports. |
| Hot renderer state, diff preparation, tool output, and session navigation must remain bounded and lazy. | Codex/OpenCode/OpenChamber long-history, large-diff, and renderer-latency reports. |
| Project/worktree identity must be canonical and immutable for a live run. Missing paths must not fall back by display name or remote similarity. | Coding-agent desktop wrong-workspace/worktree reports and Pi cwd/session semantics. |
| Drafts and pending interactions need backend/session/request ownership rather than component-focus ownership. | Pi RPC semantics plus desktop session-switch/draft failure reports. |
| Worktree creation needs durable intent and conservative recovery; Git isolation must not be presented as a security sandbox. | Pi cwd behavior, Git worktree semantics, and wrong-workspace reports. |
| Diagnostics should read bounded existing owners on demand instead of introducing passive telemetry or another durable log. | Product idle-work requirement and observed renderer/log growth failure modes. |

### Primary Pi references

- https://pi.dev/docs/latest/rpc
- https://pi.dev/docs/latest/sessions
- https://pi.dev/docs/latest/session-format
- https://pi.dev/docs/latest/usage
- https://pi.dev/docs/latest/security
- https://pi.dev/docs/latest/extensions
- https://pi.dev/docs/latest/sdk
- https://pi.dev/docs/latest/compaction
- https://pi.dev/docs/latest/environment-variables

### Desktop/platform references

- https://v2.tauri.app/security/csp/
- https://v2.tauri.app/security/capabilities/
- https://v2.tauri.app/develop/calling-frontend/
- https://docs.solidjs.com/advanced-concepts/fine-grained-reactivity

Representative external projects/issues remain useful when a future design decision needs comparative evidence; they are not product authority. Search the relevant upstream tracker at decision time rather than preserving a chronological incident diary here.

## 11. Compatibility evidence

These compatibility constraints are specific enough to retain as direct references:

- **Write-capable Resume:** a non-empty Pi JSONL session must end with LF before Pi Wizard resumes it for writing. Read-only history may tolerate an incomplete final line; Pi Wizard never repairs the authoritative file. Source: https://github.com/earendil-works/pi/issues/8345
- **Extension recovery:** Pi supports `--no-extensions`, so extension discovery is an independent one-run recovery policy rather than part of project trust or context-file loading. Sources: https://pi.dev/docs/latest/extensions and https://github.com/earendil-works/pi/issues/8620
- **Retry/compaction:** `abort_retry` applies to provider retry delay, ordinary `abort` applies after agent work resumes, and Pi exposes no generic summarization/compaction cancellation primitive that the desktop may invent. Source: https://pi.dev/docs/latest/rpc
- **Session preview normalization:** Pi persists generated skill content inside an explicit `<skill ...>...</skill>` wrapper; the catalog may reverse only that stable wrapper for preview/search while leaving JSONL untouched. Source: https://github.com/earendil-works/pi/issues/7424
- **Pi 0.84.3 queue clearing:** public RPC documentation describes `clear_queue`, but Pi 0.84.3's RPC dispatcher does not expose it. Pi Wizard treats explicit unsupported-command rejection as a compatibility degradation, recovers only the bounded user-visible queue snapshot, and terminates the exact Pi process. Source: Pi RPC documentation, https://github.com/earendil-works/pi/issues/8349, and the installed 0.84.3 package fixture used by repository tests.
- **Windows npm launchers:** standard npm Pi shims delegate to Node + `@earendil-works/pi-coding-agent/dist/bundle/cli.js`. Pi Wizard resolves that layout to direct Node invocation so the long-lived runtime is a real executable process with no console shell wrapper. Repository fixtures own this compatibility shape.

## 12. Research use

Use this file only when a current design or compatibility decision needs external support. When evidence changes a product/runtime contract, update the owning current document and tests. Do not append implementation passes, incident narratives, completed milestones, or debugging transcripts; version control owns that history.
