# Research Baseline

Research snapshot: **2026-08-27**.

This document records the external evidence behind the first design so later implementation does not have to rediscover why the boundaries exist. Product and architecture authorities remain `DESIGN.md` and `ARCHITECTURE.md`.

This is historical research, not active scope. References to other operating systems, browser/web delivery, competing products, distribution channels, signing, or portability must not be converted into Pi Wizard work unless the user explicitly asks to expand the project. The active project is the personal Windows desktop app described in `README.md` and `AGENTS.md`.

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

Takeaway: timeline + worktree + change review + multi-agent orchestration are clearly useful Pi-specific desktop jobs. Pi Wizard should keep those jobs while avoiding Electron and an integrated terminal in the first product.

### StarkInternationalAI/pi-desktop

https://github.com/StarkInternationalAI/pi-desktop

Observed design:

- Tauri 2 + Rust/Tokio + Lit;
- one Pi RPC process per session;
- SQLite/FTS session index;
- session tree and command discovery;
- extension UI bridging.

Takeaway: a Rust/Tauri + Pi RPC architecture is practical. Pi Wizard should retain the process boundary but make its performance/backpressure contracts explicit from the start and avoid assuming full-text indexing must block startup.

### justhil/pi-app

https://github.com/justhil/pi-app

Observed design:

- Pi SDK shell with timeline, side panels, queue, session tree, file/review/run/context surfaces;
- extension pop-ups;
- only recent messages load immediately, with more history loaded later.

Takeaway: lazy history is already recognized as important in Pi-specific GUIs. The broad file/run/voice/editor surface is useful evidence of possibilities, but not justification to put all of them in Pi Wizard v1.

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

Pi Wizard should borrow the orchestration model, not pretend Pi has Claude Code's permission engine. A Pi run is a separate Pi process/session, and a future restricted runtime must be enforced by a real execution boundary.

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

That boundary is the foundation for the first implementation.

## 10. Second audit: protocol and reliability gaps

Audit date: **2026-08-27**.

After the initial runtime foundation was implemented, a second pass compared the actual typed/core contracts against current Pi documentation and recent desktop-harness failures. This pass intentionally focused on gaps that could become expensive to repair after process/runtime integration.

### Pi RPC details that changed or needed stronger treatment

Current Pi RPC documentation makes several distinctions that the initial foundation did not yet encode strongly enough:

- `steer` and `follow_up` are first-class RPC commands and both accept image payloads. `prompt.streamingBehavior` is still supported for prompts sent while streaming, but it is not the general GUI queue abstraction.
- `abort` does not clear queued steering/follow-up messages. Pi's interactive Escape behavior clears the queue first, aborts, and restores the cleared text to the editor. Pi Wizard's user-facing Stop should preserve that semantic rather than dropping or unexpectedly executing queued user input.
- `get_entries` supports a `since` entry ID. Session entries form an append-only tree with stable IDs, so the last entry ID is a durable live synchronization cursor across client restarts. The response also exposes `leafId`, which makes branch movement observable. This is a better normal live-history primitive than repeatedly requesting the entire current conversation.
- `message_update` carries content deltas keyed by `contentIndex`; it does not contain a cumulative message snapshot. `message_end.message` is authoritative. In contrast, `tool_execution_update.partialResult` is accumulated and should replace the previous preview.
- Extension UI methods split into response-bearing dialogs and fire-and-forget state mutations. Pending request ownership must follow that distinction instead of treating every extension UI event as a dialog.

Primary sources:

- https://pi.dev/docs/latest/rpc
- https://pi.dev/docs/latest/sessions
- https://pi.dev/docs/latest/usage
- https://pi.dev/docs/latest/security

### Current Pi GUI failure reports

Recent Pi GUI issue reports exposed several foundation-level failure modes worth preventing before Pi Wizard connects a real child process:

- performing a durable catalog write/fsync for every streaming delta can serialize unrelated sessions and freeze the desktop even when the renderer itself is otherwise bounded;
- asynchronous draft persistence that swallows errors can silently lose user input or let an older retry overwrite newer content;
- Stop/abort without deadlines can hang, and optimistic local Idle state after abort rejection can allow commands to be sent into a still-running or uncertain runtime;
- image attachments require limits at every ingress path and again at the backend boundary;
- renderer reload/crash recovery needs a bounded breaker and a backend-owned rehydration source;
- initial hydration failure must become an actionable error state rather than an infinite spinner.

The transferable rules are now explicit in `ARCHITECTURE.md`, `DESIGN.md`, and the executable core where possible: no app-owned durable writes on token/tool-progress events, generation-safe draft ownership, deadline-bounded Stop with quarantine on uncertainty, backend attachment validation, idempotent/versioned hydration, and disposable renderer state.

### Codex/OpenCode feedback added to this audit

Current Codex reports continue to show renderer memory growth and hangs around long histories, multi-window use, and very large tool output. They also include worktree/thread binding bugs where an operation targets the wrong worktree or forks from a stale/default branch. This strengthens two Pi Wizard requirements: hot renderer state must stay bounded, and every live run must carry one immutable canonical worktree identity plus the exact base commit used to create it.

OpenCode feedback adds two adjacent UX/state lessons: drafts should be scoped to the session rather than one global composer buffer, and request/approval surfaces must disappear/reset when the underlying request identity changes or is no longer valid.

These findings do not justify importing Codex/OpenCode runtime semantics. They justify stricter ownership and recovery boundaries around Pi's own semantics.

### Tauri renderer security

The initial Tauri scaffold had `csp: null`, which left the static renderer without an application-level Content Security Policy. Current Tauri guidance recommends an explicit restrictive CSP and narrow IPC capabilities. Pi Wizard now separates the packaged CSP from development CSP: production permits only local scripts/styles/assets plus the Tauri IPC transport and required local/data/blob images, with inline styles and script eval absent; loopback Vite HMR gets its style/WebSocket allowances only through `devCsp`. Session-tree indentation was moved from an inline CSS custom property to one of 25 bounded static depth classes so production does not need `'unsafe-inline'`. Objects, external base URLs, form submission, and framing remain denied. Future renderer capabilities should remain purpose-built rather than exposing generic shell/filesystem authority.

Primary sources:

- https://v2.tauri.app/security/csp/
- https://v2.tauri.app/security/capabilities/

## 11. Third audit: identity, environment, and recoverability gaps

Audit date: **2026-08-27**.

This pass re-read current Pi usage/RPC/security/environment documentation and sampled newer Codex, Claude Code Desktop, and OpenCode Desktop reports. It found no reason to change the Tauri/Rust/Solid/Pi-RPC architecture. It did expose several ownership gaps that become costly once a desktop app starts persisting project mappings and launching real toolchains.

### Pi trust/context loading is more nuanced than a binary override

Pi stores saved trust decisions by canonical directory, with the closest current/parent decision taking precedence over `defaultProjectTrust`. Non-interactive RPC mode cannot prompt; without a saved decision, `ask` and `never` ignore protected project resources while `always` trusts them. User/global/CLI extensions can also participate in the trust event.

Crucially, project-resource trust does not control context-file loading: `AGENTS.override.md`, `AGENTS.md`, and `CLAUDE.md` are loaded regardless of trust unless `--no-context-files` is passed. Pi Wizard therefore changed its launch model from mandatory approve/ignore to three trust policies (inherit Pi, approve one run, ignore one run) plus a separate context-file policy.

Primary sources:

- https://pi.dev/docs/latest/security
- https://pi.dev/docs/latest/usage

### Pi RPC session mutation success can still mean cancelled

`new_session`, `switch_session`, `fork`, and `clone` can be cancelled by extension hooks. Pi reports these as protocol `success: true` with `data.cancelled: true`. A GUI that equates transport success with completed session mutation can rebind its local view/run to a session Pi never switched to. The core now exposes a semantic response outcome that keeps rejection, acceptance, and extension cancellation distinct.

Primary source: https://pi.dev/docs/latest/rpc

### Extension fire-and-forget UI needs its own bounded state owner

Pi RPC translates `setStatus`, `setWidget`, `setTitle`, `set_editor_text`, and `notify` into fire-and-forget extension UI requests. They are not pending dialogs. Keyed status/widget replacement can otherwise grow application state indefinitely if extensions generate many keys or large text blocks. Pi Wizard now has a bounded backend projection for status/widget/title state; editor text remains session-draft state and notifications remain transient/rate-limited presentation.

Primary sources:

- https://pi.dev/docs/latest/rpc
- https://pi.dev/docs/latest/extensions

### Desktop project identity/state corruption is a recurring wrong-workspace failure

Recent Codex/OpenCode reports show worktree/path/project mappings drifting to wrong directories, project folders renamed externally becoming phantom/global sessions, symlink paths producing duplicate/missing sessions, and corrupted derived state making intact history appear missing. These are more serious than cosmetic sidebar bugs because an autonomous coding agent can edit the wrong checkout.

Transferable constraints now locked into Pi Wizard:

- an opaque `ProjectId` binds to one canonical root;
- path mismatch/missing becomes Detached, never silent fallback;
- relocation is explicit;
- display name/remote/repository-name similarity never proves identity;
- project/catalog/preferences state is recoverable derived data, isolated from authoritative Pi JSONL;
- malformed derived state is quarantined/rebuilt and safe startup must remain possible.

Representative reports:

- https://github.com/openai/codex/issues/16525
- https://github.com/openai/codex/issues/24345
- https://github.com/openai/codex/issues/31879
- https://github.com/openai/codex/issues/29593
- https://github.com/openai/codex/issues/35296
- https://github.com/anomalyco/opencode/issues/30260
- https://github.com/anomalyco/opencode/issues/31716
- https://github.com/anomalyco/opencode/issues/37353
- https://github.com/anomalyco/opencode/issues/40986
- https://github.com/anthropics/claude-code/issues/65554
- https://github.com/anthropics/claude-code/issues/86276

### GUI launch environment parity is execution correctness

Pi's RPC/CLI process and its shell tools use the environment of the spawned process, and Pi exposes session/tool metadata through `PI_*` variables. Multiple current desktop harnesses have reports where GUI-launched agents cannot find Homebrew/Git/toolchain executables that work in the terminal because Dock/Finder/Start-menu processes inherit a different PATH or shell snapshot.

Pi Wizard therefore treats launch-environment resolution as a backend service rather than a side effect of Tauri startup. Discovery must be bounded, platform-aware, explicit-path-first, secret-safe in diagnostics, and shared by Pi/Git/toolchain probes so the app does not discover one executable under one environment and spawn the agent under another.

Primary/representative sources:

- https://pi.dev/docs/latest/environment-variables
- https://pi.dev/docs/latest/usage
- https://github.com/anthropics/claude-code/issues/44649
- https://github.com/openai/codex/issues/20220
- https://github.com/anomalyco/opencode/issues/15195

### Resulting foundation changes

The second audit changed the implemented foundation rather than only adding future notes:

- current Pi RPC command coverage now includes first-class steer/follow-up, stable entry cursors, model/thinking cycling, retry/compaction controls, session tree/stats/export operations, and bounded image payloads;
- attachment limits are centralized and revalidated when the RPC line is encoded;
- the writer now receives the full RuntimeLimits owner rather than one outbound-byte integer;
- user drafts have a Tauri-independent generation/durability reducer;
- process state has an explicit Quarantined terminal state for unconfirmed Stop escalation;
- stream-event classification mechanically distinguishes coalescible transient updates from semantic boundaries;
- the Tauri renderer has an explicit CSP;
- architecture/testing now require no token-triggered durable persistence, versioned/idempotent hydration and renderer recovery, exact-base transactional worktrees, and session-scoped drafts.

## 12. Fourth audit: streaming fidelity and lightweight desktop convergence

Audit date: **2026-08-27**.

This pass rechecked Pi's current RPC streaming contract and sampled current Pi-specific/lightweight coding-agent desktop clients. The high-level product direction still holds: a small process-owning shell around the harness is viable, while reliability depends more on precise stream identity and bounded UI state than on adding IDE surfaces.

### Pi assistant streaming is a content-block protocol, not one text buffer

Current Pi `message_update` events intentionally omit a cumulative message. Their nested `assistantMessageEvent` uses `contentIndex` across `text_*`, `thinking_*`, and `toolcall_*` start/delta/end events. Tool-call start also carries the call ID and tool name. Pi explicitly tells custom UI clients to assemble live partial state by content index and to treat `message_end.message` as authoritative.

The previous Pi Wizard projection had one bounded assistant text buffer. Although bounded, that shape would have lost ordering and identity as soon as text, thinking, and tool-call argument blocks interleaved. The core now projects ordered typed content blocks, caps the number of resident blocks, caps their aggregate bytes, avoids sparse-index vector allocation, and supports authoritative block-end replacement before the final message supersedes the live projection.

Pi direct RPC Bash has different correlation semantics again: `bash_execution_update` is a delta stream whose `id` matches the originating `bash` request, and the event stream can contain output omitted from the truncated final response. The core now exposes a typed parser that preserves this request identity rather than leaving future process integration to infer ownership from chronology.

Primary source:

- https://pi.dev/docs/latest/rpc

### Current lightweight clients reinforce the narrow-shell boundary

Current Pi desktop projects continue to converge on a small set of useful shell features: Pi RPC or SDK remains the runtime, projects/sessions are first-class, slash commands and capabilities are discovered dynamically, and process/session identity lives outside the visible chat component. Tauri/Rust RPC clients demonstrate that a native lightweight shell is practical, while other Pi clients show how quickly terminals, editors, permission layers, remote access, and broader IDE features expand the state surface.

Representative current projects:

- https://github.com/StarkInternationalAI/pi-desktop
- https://github.com/Inas1234/pi-desktop
- https://github.com/justhil/pi-app

Recent OpenChamber fixes are especially transferable even though its scope is broader: failed session creation restores the submitted draft, new drafts/sessions remain attached to the selected project, large session sidebars stay responsive while streaming, and completed streamed replies settle on complete text. These are all ownership/settlement guarantees rather than visual features, and they align with Pi Wizard's backend draft owner, canonical project binding, bounded history, and authoritative end-event rules.

Source:

- https://github.com/openchamber/openchamber/blob/main/CHANGELOG.md

### Resulting foundation changes

- typed parsing for Pi nested assistant stream events with forward-compatible unknown sub-events;
- typed request-correlated direct Bash stream updates;
- content-index-aware assistant live projection with aggregate byte and block-count ceilings;
- UTF-8-safe oldest-prefix eviction for aggregate bounded projections;
- regression coverage for interleaved/sparse assistant content and Bash correlation;
- correction of a misplaced runtime unit test that had accidentally lived inside the production `RuntimeStore` implementation.

## 13. Fifth audit: RPC concurrency, Stop truthfulness, process ownership, and backpressure

Audit date: **2026-08-27**.

This pass went beyond stream shape and rechecked the current Pi RPC implementation, recent Pi concurrency/compaction reports, direct Bash behavior, and desktop execution-environment failure modes before wiring a real Tauri process manager.

### RPC input is asynchronous, so the client needs semantic barriers

Current Pi RPC dispatch handles input frames asynchronously. An August 2026 Pi issue documents concurrent `new_session`/`switch_session`/`fork`/`clone` calls racing shared session teardown/replacement state. Pi Wizard already classified those operations as a full client-side session-replacement barrier; this audit confirmed that was not merely conservative serialization.

The same asynchronous boundary exposed a smaller local gap around manual compaction. A `compact` request can be sent before `compaction_start` reaches the runtime store, leaving a window where the composer still appears able to submit a prompt/steer/follow-up. Pi Wizard now holds session-mutating/composer commands behind the manual-compaction request itself, while still allowing read-only state probes and control operations needed for recovery.

Representative source:

- https://github.com/earendil-works/pi/issues/7862

### Stop cannot pretend Pi exposes a compaction-abort RPC

Current RPC documents `abort` for agent work and `abort_retry` for retry delay, but does not expose a separate manual-compaction abort command. Recent Pi reports have also covered overlapping/manual/auto compaction abort-controller races and queue behavior around compaction boundaries. A GUI Stop action therefore cannot truthfully map active compaction to ordinary RPC `abort` and immediately claim success.

Pi Wizard now models Stop as a transaction: clear and preserve steering/follow-up queues first, then abort active agent work, wait for `agent_settled`, and keep the healthy RPC process reusable. Rejection/no-response deadlines escalate to the exact owned child handle. If Stop finds active compaction after queue recovery, it follows that same process-termination escalation path rather than inventing a nonexistent RPC guarantee.

Primary/representative sources:

- https://pi.dev/docs/latest/rpc
- https://github.com/earendil-works/pi/issues/7738
- https://github.com/earendil-works/pi/issues/7650
- https://github.com/earendil-works/pi/issues/3189

### `get_state` is a recovery primitive, not only a settings panel response

Current `get_state` reports model, thinking level, streaming/compaction state, steering/follow-up modes, session path/ID/name, auto-compaction state, and message/pending counts. Those fields are sufficient to repair important live-runtime state after backend startup/reconnect or renderer replacement without replaying every historical display event.

Pi Wizard now decodes that response into an authoritative runtime observation. Edge events still keep normal live state current, but hydration/recovery can reconcile from Pi rather than trusting stale renderer memory.

Primary source:

- https://pi.dev/docs/latest/rpc

### Direct Bash has context and truncation semantics a GUI must preserve

Current direct RPC `bash` accepts `excludeFromContext`, streams output with the exact originating request ID, and returns final `output`, `exitCode`, `cancelled`, `truncated`, and optional `fullOutputPath`. Streamed output may exceed the bounded final response, and multiple direct Bash commands can occur before the next prompt.

Pi Wizard now represents the current command shape, keeps a separate bounded preview per request, and decodes final truncation metadata instead of treating direct Bash as an ordinary agent tool card or assigning output by chronology.

Primary source:

- https://pi.dev/docs/latest/rpc

### Desktop discovery and execution must use one environment

The previous architecture correctly required GUI/terminal environment parity but had no executable owner for it. The resolver now selects one secret-bearing in-memory environment from explicit configuration, the desktop process, or a bounded shell-probe result. Pi and Git are discovered inside that same selected environment, and process spawn rejects a Pi executable that does not match the resolved identity. Diagnostics expose only path/provenance/count metadata, never provider keys or complete environment values.

At that audit point the platform-specific bounded shell probe still remained to be wired. It is now implemented: Windows uses a bounded non-interactive PowerShell/pwsh environment probe only when the desktop environment cannot resolve Pi, and the resulting complete environment is fed back through the same Tauri-independent selection owner. The historical separation between probe acquisition and environment selection remains important because it keeps precedence and secret handling out of Tauri.

### Process supervision and renderer backpressure are now executable owners

A deterministic fake-Pi subprocess now exercises the real async reader/writer transport. The child owner uses private stdin/stdout RPC, continuously drains stderr into a byte ring, captures the original child handle/PID identity, and performs hard termination through that exact handle with a deadline outcome. No process-name scan is involved.

The renderer bridge also has a Tauri-independent byte-bounded backlog. Display-only assistant/tool/direct-Bash frames are keyed and replaceable; under pressure, superseded display frames may be evicted, but semantic transitions are never silently sacrificed. If semantic frames alone exhaust the cap, the bridge reports an explicit rehydration condition instead of allowing unbounded memory growth or silent UI/backend desynchronization.

### Resulting implementation changes

- current `bash.excludeFromContext` and startup `--thinking` support;
- manual-compaction pre-event command barrier;
- typed tool/queue/session/thinking events and `get_state`/`clear_queue`/Bash response projections;
- authoritative runtime `get_state` reconciliation;
- per-run RPC controller for correlation/barriers/projection/runtime application;
- queue-preserving deadline-bounded Stop transaction with compaction-aware escalation;
- secret-redacted launch-environment selection and executable provenance;
- supervised private-stdio Pi child transport with bounded stderr and exact-handle termination;
- deterministic fake-child round trip;
- byte-bounded semantic-preserving renderer event backlog.

## 14. Sixth and seventh audits: live manager, desktop recovery, and process-tree shutdown

Audit date: **2026-08-27**.

These passes rechecked current Pi RPC behavior, current lightweight Pi desktop clients, and the actual Tauri lifecycle while moving from isolated runtime primitives to a live backend owner. The architecture remained subprocess RPC. Current clients using Tauri/Rust continue to reinforce the useful boundary: Pi owns the agent/session semantics, while the desktop owns process identity, project/session navigation, bounded presentation, and lifecycle supervision.

Representative current clients:

- https://github.com/StarkInternationalAI/pi-desktop
- https://github.com/gustavonline/pi-desktop
- https://github.com/Inas1234/pi-desktop
- https://github.com/kdcokenny/picot

### RPC acknowledgement is not turn completion

Current Pi RPC accepts a prompt with a normal response while the turn continues asynchronously through events. `agent_settled`, not the prompt response, is the session-level boundary indicating that the current work and retries are fully settled. This makes a persistent backend owner necessary: renderer code must not treat one successful invoke result as the lifetime of a turn, and renderer navigation cannot own the Pi child.

Current RPC also confirms the existing Stop transaction: `clear_queue` before `abort` is the documented way to preserve queued steering/follow-up input, and direct Bash updates remain correlated by request ID. `get_state` remains the authoritative recovery probe after a missed renderer/event interval.

Primary source:

- https://pi.dev/docs/latest/rpc

### The live owner needs two independent backpressure boundaries

Joining process I/O and renderer delivery exposed an important distinction. Backpressuring the renderer must not backpressure Pi stdout/stderr, and a blocked stdin write must not stop the process owner from consuming stdout or issuing hard termination. The implementation therefore has:

- a process actor that owns stdout plus exact child control;
- a separate bounded stdin writer task;
- a bounded internal process-event channel whose exhaustion becomes a classified fatal condition rather than a hidden deadlock;
- a persistent runtime manager that normalizes those events into `RuntimeStore`/controller state;
- a separate byte-bounded renderer backlog downstream of semantic processing;
- one coalesced dirty-run signal followed by bounded renderer pulls instead of one desktop event per token.

### Windows command wrappers change the meaning of “exact child”

The first manager-level fake-Pi tests intentionally launched through a Windows `.cmd` wrapper. Shutdown hung even though the direct wrapper received a kill request. The reason was a descendant Node process retaining inherited stdout/stderr handles. This is representative of npm-installed command shims, not a synthetic-only failure.

The process contract was therefore tightened. Windows hard termination now targets the spawn-time PID tree (`/T`) rather than only the direct wrapper, and Unix launches Pi into its own process group for group termination. No executable-name scan is used. Diagnostic EOF finalization is separately deadline-bounded so a retained inherited pipe can never extend Stop/shutdown forever. If tree termination is not confirmed, the run remains quarantined.

This finding changes the earlier shorthand “exact child handle termination”: the lifecycle authority is the exact spawn identity and its owned process tree/group, never a later search for matching process names.

### Tauri is now an adapter around the runtime manager

The desktop host now owns one `RuntimeManagerHandle`. It exposes bounded hydration/drain/start/Stop/probe commands, forwards normalized dirty/rehydrate wakeups, and intercepts native `ExitRequested` until the manager completes bounded shutdown. Tauri does not parse Pi frames or own request correlation.

The renderer subscribes before its initial hydration, preventing a startup lost-wakeup race. Dirty signals are folded per run; each wakeup pulls bounded event pages, and a lag/desynchronization condition widens to a fresh versioned hydration snapshot. This preserves the zero-passive-polling requirement while keeping renderer reload disposable.

Current Tauri references used for this boundary:

- https://docs.rs/tauri/2.11.5/tauri/enum.RunEvent.html
- https://docs.rs/tauri/2.11.5/tauri/struct.ExitRequestApi.html
- https://docs.rs/tauri/2.11.5/tauri/trait.Emitter.html

### Project and compatibility state must agree with actual launch

The desktop host now caches environment plus parsed `pi --version` as one launch profile. A run cannot quietly skip the executable/version probe that the diagnostics surface used. Canonical project paths also share one process-local `ProjectId`, preventing multiple live runs of one checkout from being mislabeled as unrelated projects while the durable registry is still pending.

### Resulting implementation changes

- schema-versioned runtime hydration and revisioned capability/extension state;
- durable-cursor live `get_entries(since)` synchronization with explicit resync state;
- persistent runtime manager joining child/controller/store/Stop/backlog/shutdown ownership;
- separate manager control plane for Stop/shutdown/extension dialog responses;
- separate stdin writer and priority child-control plane;
- exact Windows PID-tree / Unix process-group hard termination;
- deadline-bounded stderr EOF finalization;
- manager-level fake-Pi tests for renderer reload, stream coalescing, Stop reuse, extension dialogs, and multi-child shutdown;
- Tauri lifecycle ownership and normalized dirty/rehydrate bridge;
- renderer listener-before-hydration and bounded no-poll event draining;
- stable camelCase IPC regression tests;
- process-local canonical project identity and shared environment/version launch profile.

## 15. Eighth audit: non-destructive recovery, extension attention, session-bound drafts, and startup readiness

Audit date: **2026-08-27**.

This pass rechecked current Pi RPC extension/session semantics and the current Tauri event model while auditing the live renderer bridge. It found several cases where a superficially convenient desktop implementation would weaken the ownership contracts already established.

### Tauri events are wake-ups, not the authoritative runtime stream

Tauri's event API is asynchronous application/window communication, not a replacement for an ordered high-throughput transport. Pi Wizard therefore keeps the event bridge advisory: one coalesced dirty-run notification wakes the renderer, which then pulls bounded normalized pages from Rust. This pass found that ordinary hydration was still clearing every run's backlog, which could erase transient events not represented in hydration. Hydration is now non-destructive and re-announces already-pending delivery; only explicit per-run desynchronization recovery discards stale queued frames.

Current Tauri reference:

- https://v2.tauri.app/develop/calling-frontend/

### Pi extension dialogs and fire-and-forget UI state need different renderer ownership

Current Pi RPC distinguishes blocking `select`, `confirm`, `input`, and `editor` extension UI requests from fire-and-forget `notify`, `setStatus`, `setWidget`, `setTitle`, and `set_editor_text`. Response-bearing requests carry exact IDs and optional agent-side timeouts. The renderer now exposes a typed cross-run Needs Attention surface for blocking dialogs and responds through the manager's priority control plane; stale or expired requests remain backend-rejected instead of becoming client-local zombie prompts. `set_editor_text` instead enters the backend session draft owner.

Primary source:

- https://pi.dev/docs/latest/rpc

### Accepted session replacement still requires authoritative identity reconciliation

Successful `new_session`, `switch_session`, `fork`, and `clone` responses tell the client whether an extension cancelled the operation, but they do not establish all authoritative state for the session that is now active. `get_state` remains the source for current session ID/path/name and runtime modes. The manager now queues that state reconciliation for accepted replacements before releasing the replacement waiter. This matters directly for session-scoped drafts: changing Pi sessions changes the active draft owner without copying the old session's text.

Primary source:

- https://pi.dev/docs/latest/rpc

### Process spawn is weaker than Pi RPC readiness

A child process existing at the OS level does not prove that it is a functioning Pi RPC endpoint. The previous manager promoted a child to Ready immediately after spawn and gave startup probes no deadline. A hung/wrong executable that kept stdin open could therefore appear usable indefinitely. The runtime now keeps the process Starting until a designated `get_state` handshake succeeds and terminates/fails a silent startup at a bounded deadline. Stop/application shutdown take ownership over that timer when intentionally terminating a starting process.

### Recovery data must remain bounded after parsing

The RPC frame ceiling protects transport memory but is too large to serve as a retained-state policy. Because `get_state` fields now drive long-lived runtime/session/draft ownership, this pass added a smaller retained-state budget plus an explicit session-ID ceiling. Incremental session-name events are checked against the same prospective budget so recovery and live event paths cannot disagree about allowable resident state.

### Resulting implementation changes

- non-destructive ordinary hydration with late-subscriber re-wake;
- explicit per-run renderer backlog recovery;
- request-ordered renderer hydration application;
- typed extension Needs Attention UI and exact response adapter;
- automatic post-session-replacement `get_state` reconciliation;
- hydration schema v2 with active session draft;
- in-memory session draft ownership and extension `set_editor_text` integration;
- explicit preallocated Pi session IDs for GUI-created new sessions;
- bounded startup RPC readiness handshake before Ready;
- bounded retained `get_state` and incremental session-name state.

## 16. Ninth and tenth audits: durable composer ownership, replacement ordering, Stop recovery, and stable projects

Audit date: **2026-08-27**.

These passes revisited the remaining user-data seams after the live manager became operational. Current Pi RPC/SDK behavior and recent lightweight desktop failures point to the same principle: session selection, composer text, queue recovery, and project identity cannot be inferred from whichever renderer selection happens to be current after an asynchronous boundary.

### Pi acknowledgement and session replacement require captured ownership

Pi acknowledges `prompt`, `steer`, and `follow_up` independently from later agent settlement. Pi's SDK runtime also replaces the active session object for new/switch/fork/clone operations, which invalidates state bound to the previous session. Recent OpenChamber reports show the practical failure mode when a desktop re-reads selected-session state after asynchronous setup: the first prompt of a newly selected session can land in the previously active session, and startup/session switching can race incomplete synchronization.

Pi Wizard now captures RunId, draft generation, and RPC request identity before composer submission. Acceptance clears only that generation; rejection/write failure preserves it. Session replacement is a separate manager transaction: if the old session has an unsaved persistent draft, the manager forces a bounded atomic save before writing Pi's replacement command. Later composer edits or managed commands cannot overtake it. After Pi accepts, the existing authoritative `get_state` reconciliation still establishes the new session identity.

Sources:

- https://pi.dev/docs/latest/rpc
- https://pi.dev/docs/latest/sdk
- https://github.com/openchamber/openchamber/issues/2222
- https://github.com/openchamber/openchamber/issues/1778

### Queue recovery is editor data, not Stop diagnostics

Current Pi documents the interactive interrupt sequence as clear queued steering/follow-up messages, abort current work, and restore the returned text to the editor. Pi has also fixed behavior where queue restoration could wipe already-unsent editor input. A desktop therefore cannot merely display `clear_queue` output in a Stop result and call the data preserved.

Pi Wizard now gives queue recovery dedicated retained-state count/byte ceilings before cloning strings out of the RPC response. On Stop completion the backend prepends steering messages, then follow-up messages, ahead of the current unsent session draft. The entire prospective merge is checked against the normal draft ceiling first; overflow leaves the existing draft untouched and returns an explicit recovery error.

Primary source:

- https://pi.dev/docs/latest/rpc

### Durable drafts need an asynchronous restore state

Atomic draft files are stored separately from Pi JSONL under the application data root. Generation correlation protects asynchronous writes, but restart recovery exposed another race: before disk load returns, a new runtime naturally has an empty Saved in-memory baseline. Treating that baseline as editable can let new typing win a race against restoration and silently hide the persisted text.

The runtime now exposes `draftRestorePending`. Edit/submission is gated until the load establishes saved text, proves no file exists, or records a visible failure. Shutdown bypasses normal debounce and waits through a dedicated draft-flush deadline. Corrupt draft files quarantine independently rather than failing the Pi child.

### Stable project identity must survive the desktop process

Current Pi-oriented desktop clients persist app-level project records separately from Pi sessions, while recent OpenChamber failures show why project selection cannot be a transient UI hint when sessions are created. Pi Wizard's previous canonical-root map prevented duplicate IDs only inside one process; restart could mint a different ProjectId for the same checkout.

The project registry now stores only schema-versioned `(ProjectId, canonical root)` mappings in the Tauri bundle-scoped app-data state directory. Whole-registry writes are atomic. Registration and relocation update memory only after durable commit. Duplicate, oversized, malformed, or unsupported registry state is quarantined and safe startup uses an empty derived registry; a missing root remains a detached registration and is never rebound by name, remote, or another path.

Sources:

- https://github.com/StarkInternationalAI/pi-desktop
- https://github.com/openchamber/openchamber/issues/1521
- https://docs.rs/tauri/latest/tauri/path/struct.PathResolver.html

### The first usable renderer still stays bounded

The Solid shell now exposes existing-project launch with explicit Pi resource-trust wording, backend-owned Send/Steer/Follow-up/Stop, draft durability failures, and a live assistant/thinking/tool/direct-Bash surface. It does not retain raw token frames or complete history. The renderer consumes the already-bounded Rust projection, and dirty-run hydration demand survives continuation drains without introducing passive polling. Historical session paging/virtualization remains the next separate read-model problem.

## 17. Eleventh audit: session discovery must not inherit an unbounded picker

Audit date: **2026-08-27**.

Current Pi keeps JSONL sessions authoritative and supports `/resume`, `--resume`, `--session`, session names, tree navigation, forks, clones, and compaction. Its current session picker builds rich searchable `SessionInfo` values by streaming complete session files to recover the latest name, first prompt, message count, and aggregate message search text. That is reasonable for a terminal picker, but importing the same behavior into a persistent desktop catalog would violate Pi Wizard's bounded-history design and reproduce a class of failures reported by desktop coding clients whose global prompt/history state grows with accumulated conversations.

Pi's session directory resolution also has more edge cases than the default `~/.pi/agent/sessions/--cwd--` layout suggests. `PI_CODING_AGENT_DIR` changes the agent root, `PI_CODING_AGENT_SESSION_DIR` overrides session storage, and merged global/project `sessionDir` settings apply before the default. Explicit custom session directories are flat in current Pi, so a GUI must filter by the JSONL header's canonical `cwd` rather than assuming directory membership proves project identity. Recent upstream fixes and reports around resume-directory mistakes reinforce that this path logic is compatibility behavior, not UI decoration.

Pi Wizard therefore implements current-project discovery as a bounded read model: directory candidates, header-filter bytes, detailed metadata files scanned, bytes read per JSONL preview, query size, result count, and serialized page size all have centralized ceilings. Flat custom directories first receive a cheap bounded header pass so sessions from unrelated projects cannot consume the selected project's richer preview budget. Matching files then receive a bounded head/tail preview to recover the first user text and recent session name when available. The catalog reports when its window or preview is incomplete and never stores aggregate transcript text. A selected path is canonicalized and its header cwd is revalidated before `pi --mode rpc --session <path>` is spawned.

Current Pi RPC does not turn historical rendering into a paging problem for the client automatically. `get_messages` returns the complete current conversation, while `get_entries` without `since` returns all append-order entries, including pre-compaction history and abandoned branches; its `since` cursor is forward-only catch-up after a known entry ID. Pi Wizard already uses that cursor for bounded live synchronization, but a resumed session with no prior cursor cannot safely issue an unbounded initial `get_entries` merely to paint history. Historical transcript UI therefore remains a separate bounded read-model problem, likely file-backed paging or a measured derived index rather than a full RPC hydration.

Sources:

- https://pi.dev/docs/latest/sessions
- https://pi.dev/docs/latest/session-format
- https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/main.ts
- https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/session-manager.ts
- https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/settings-manager.ts
- https://github.com/earendil-works/pi/issues/320
- https://github.com/earendil-works/pi/issues/5040
- https://github.com/sst/opencode/issues/12402
- https://github.com/sst/opencode/issues/16264

## 18. Twelfth audit: model input capability and compaction are user-visible contracts

Audit date: **2026-08-27**.

Current Pi RPC returns full model objects from both `get_state` and `get_available_models`. Those model objects include an `input` list such as `["text", "image"]`. This is not cosmetic metadata: Pi's own image routing uses the declared model input capability. Upstream reports show both failure directions when custom model metadata is wrong: a vision-capable endpoint can lose images when it is declared text-only, while a text-only endpoint can receive image payloads when it is incorrectly declared image-capable. OpenCode has seen the same class of GUI/provider mismatch around attachment controls.

Pi Wizard previously retained only model provider/id/name, so its newly added attachment surface could imply an image would be seen even when Pi explicitly declared the selected model text-only. The fix is deliberately narrow. Capability/state projection now retains `supportsImages: Option<bool>` derived from Pi's `input` list instead of copying arbitrary model metadata. Explicit false blocks image submission in the backend and disables new ingestion in the renderer. Existing image drafts remain preserved/removable so a model switch never destroys user data. Missing `input` remains unknown/permissive for compatibility with older or partial Pi payloads rather than inventing a capability probe.

Sources:

- https://pi.dev/docs/latest/rpc
- https://github.com/earendil-works/pi/issues/2687
- https://github.com/earendil-works/pi/issues/7461
- https://github.com/sst/opencode/issues/33542

Long-context feedback points to a second small control rather than a new context subsystem. Pi already owns manual and automatic compaction and exposes `compact` over RPC. Current Pi users report running many concurrent long sessions and repeatedly reaching context pressure; lightweight Pi desktops also expose compaction alongside model/thinking/session controls. Pi Wizard already had the client-side manual-compaction race barrier but no user surface. It now exposes an idle **Compact context** action that sends Pi's native command and reconciles with `get_state` after acceptance. It intentionally does not estimate context usage or synthesize its own summary.

Sources:

- https://pi.dev/docs/latest/compaction
- https://pi.dev/docs/latest/rpc
- https://github.com/earendil-works/pi/issues/92
- https://github.com/earendil-works/pi/issues/6606
- https://github.com/earendil-works/pi/issues/7020
- https://github.com/StarkInternationalAI/pi-desktop

The broader desktop feedback remains consistent with Pi Wizard's existing choices rather than requiring new owners. OpenChamber has repeatedly optimized long-session rendering through virtualized/lazy history and event coalescing, and recent reconnect reports show completed work remaining visually active until a reload when client state is not reconciled authoritatively. Pi Wizard's one-page cold history, bounded live projections, backend-owned process/activity state, and `get_state` recovery already address those classes without adding passive polling or another transcript store.

Sources:

- https://github.com/openchamber/openchamber/releases
- https://github.com/openchamber/openchamber/issues/3009

## 19. Fourteenth audit: durable worktree recovery and native context policy

Audit date: **2026-08-27**.

This pass rechecked current Pi RPC/session behavior, Pi worktree/cwd reports, and recent long-session/worktree failures in lightweight coding harnesses before reviewing the implemented worktree and Git-review code. The conclusions were corrective rather than expansive: worktree identity must survive crashes as durable app-owned metadata, recovery must tolerate legitimate task progress, and long-context policy should remain Pi-owned.

### Worktree creation needs a journal before Git mutates

Parallel harnesses repeatedly report wrong-directory or stale-worktree behavior when the UI selection is allowed to stand in for execution identity. Pi itself deliberately treats cwd as process/session state rather than a casual UI toggle, and upstream discussions note the difficulty of switching cwd inside a shared multi-session process. Pi Wizard therefore keeps one immutable execution root per run and one Pi process per live run.

The deeper gap was crash recovery. Verifying a worktree only after `git worktree add` is insufficient: the app can fail after Git created a branch/path but before the Pi run exists, leaving no durable mapping back to the logical `ProjectId` or captured base. Pi Wizard now writes a bounded, schema-versioned recovery intent before the first mutating Git command. The record has its own `WorktreeId`, original logical project, canonical repository/project roots, project-relative path, source branch when known, exact captured commit, requested new branch, and target path. After Git creation is verified, the same record is atomically upgraded with the canonical created worktree/execution roots. Corrupt derived recovery state is quarantined without touching Git.

Restart reconciliation is intentionally conservative and non-destructive. Only a plan whose branch and path are both proven absent is classified as never-created/safely retireable. Branch-only, path-only, wrong-repository, wrong-branch, or otherwise conflicting state remains visible for manual recovery. Pi Wizard does not delete branches/worktrees during reconciliation.

The audit also caught an initially over-strict recovery rule: requiring current `HEAD` to equal the captured creation commit would reject a perfectly valid task after the agent made commits. Immediate post-create verification still requires exact `HEAD == captured base`, but restart recovery instead requires the same Git common repository, the requested task branch, and proof via Git ancestry that the captured base remains an ancestor of current `HEAD`. This preserves legitimate descendant commits and dirty task work while still rejecting branch switching or rewritten ancestry. If the user later removes both Git resources independently, a fresh absence proof may retire only the app-owned journal entry.

Sources:

- https://pi.dev/docs/latest/rpc
- https://github.com/earendil-works/pi/issues/2992
- https://github.com/earendil-works/pi/discussions/2830
- https://github.com/earendil-works/pi/issues/320
- https://github.com/sst/opencode/issues/34803
- https://github.com/sst/opencode/issues/43982

### Long-session pressure should use Pi's controls, not a GUI summarizer

Current Pi exposes bounded session statistics plus both manual and automatic compaction controls over RPC. `get_session_stats` can report message/tool counts, token/cost totals, and current context usage; context tokens/percentage may legitimately be unknown immediately after compaction. The GUI therefore renders unknown values as unknown rather than synthesizing percentages. Manual compaction returns only the bounded reconciliation metadata the desktop needs (`firstKeptEntryId`, tokens before, estimated tokens after); it does not retain the generated summary in renderer state.

Recent OpenCode/OpenChamber feedback continues to show long-session latency, memory, stop-control delay, and repeated ever-growing history fetches. The appropriate Pi Wizard response is to keep history/review bounded and expose Pi's existing context policy, not build another summarizer or poll context continuously. Automatic compaction is now an explicit Pi-native control: the desktop sends `set_auto_compaction` and waits for `get_state` before reflecting the new value. Session usage remains an on-demand request, with one refresh after explicit manual compaction.

Sources:

- https://pi.dev/docs/latest/rpc
- https://github.com/openchamber/openchamber/blob/main/CHANGELOG.md
- https://github.com/sst/opencode/issues/34803
- https://github.com/sst/opencode/issues/43982
- https://github.com/sst/opencode/issues/12402

### A stuck Pi turn remains a Pi/runtime state, not a guessed desktop timeout

A recent Pi RPC report describes a tool-completed turn that can remain streaming indefinitely until an abort is sent. Pi Wizard already had a user-visible Stop path that preserves queued text, sends Pi's abort, waits for settlement, and escalates only under its explicit process-stop contract. This audit deliberately did **not** add a passive “no output for N seconds means done” state rewrite: builds, tools, extensions, and model calls can legitimately be quiet, and manufacturing Idle would violate the runtime authority boundary. A later audit implemented the safe diagnostic form anticipated here: a one-shot quiet-Working advisory driven by the existing deadline selector. It does not probe, retry, resubmit, cancel, or relabel the run, and Stop remains the explicit recovery control.

Source:

- https://github.com/earendil-works/pi/issues/7336

## 20. Final hardening audit: pull diagnostics and catalog indexing evidence

Audit date: **2026-08-28**.

The final hardening pass rechecked two architecture requirements that are easy to satisfy in a way that creates more background work than the product itself: developer observability and large session-catalog navigation.

### Diagnostics should observe bounded owners, not create another telemetry system

The runtime already owns the useful bounded counters at the places where work is admitted or discarded: pending RPC requests, command gates, live assistant/tool/Bash projections, the renderer event backlog, exact process handles, and active Git/session-catalog jobs. A separate logging thread, periodic sampler, or durable trace store would add idle CPU and another retention problem merely to report those owners.

Pi Wizard therefore exposes diagnostics as an explicit pull snapshot. Per-run state includes exact process ownership, retained `RuntimeStore` bytes, pending RPC/command/dialog counts, active live projections, backlog bytes/frames, cumulative coalesced/dropped/delivered UI counts, rehydration pressure, and a fixed one-second recent RPC event/byte window. The desktop adapter adds active Git-review and session-catalog job counts. The renderer samples mounted timeline-row count only when the user requests diagnostics, while development builds use the browser's event-driven `PerformanceObserver` for long-task measurements. No diagnostic timer, log file, or passive subprocess is introduced.

### Exact stateless recent-session ordering requires lightweight metadata enumeration

The original index-policy shorthand said to enumerate only enough metadata for the first visible page. Implementation evidence showed that this is too strong when three other requirements are kept simultaneously: recent sessions are globally ordered by current file modification time, Pi/CLI-created JSONL remains authoritative, and a continuation must fail closed if any candidate changes between pages. Without a derived index, knowing the newest files and computing that catalog snapshot necessarily requires one lightweight directory/metadata pass for an explicit page request.

The implementation still keeps expensive work bounded: only a fixed candidate heap is retained, only a bounded number of session headers/previews are read in detail per page, page entries/bytes are capped, and no catalog work runs at startup or while the session view is passive. Continuation cursors bind project, resolved session directory, query, exact position, and the catalog metadata snapshot. Any external candidate addition/removal/mtime change invalidates the continuation rather than mixing observations.

The existing Windows scale fixture creates 1,200 Pi session files and traverses the complete catalog through bounded pages. A focused run on 2026-08-28 completed the entire fixture in about **1.40 seconds**, including fixture creation and all continuation pages. That measurement does not justify adding SQLite, a filesystem watcher, or another persistent catalog owner. Such an index remains permitted only if larger real-world measurements show the stateless metadata pass is materially expensive; it must remain disposable and cannot weaken Pi JSONL authority or external-change detection.

## 21. Current Pi 0.84.x and lightweight-harness reliability audit

Audit date: **2026-08-28**.

This pass rechecked the implemented application against the current Pi RPC contract, new Pi 0.84.x regressions, and current lightweight agent-desktop reliability patterns. The useful findings were recovery and presentation corrections, not evidence for expanding Pi Wizard into an editor, terminal, or multi-harness framework.

### Writable Resume must reject an unterminated Pi JSONL tail

A current Pi issue shows that if a session file ends with a valid JSON record but no final newline, a resumed Pi session can append the next entry directly to that record and corrupt subsequent recovery. Read-only history can safely tolerate a truncated/unterminated final line, but a GUI that is about to hand a historical file back to Pi for writing has a stronger boundary.

Pi Wizard now validates the final byte during write-capable Resume. A non-empty session must end in LF before `--session` is launched. Both a syntactically valid unterminated final record and a malformed fragment fail closed, and the application never repairs the authoritative Pi file itself.

Source:

- https://github.com/earendil-works/pi/issues/8345

### Broken extension discovery needs a one-run recovery path

Pi 0.84.3 reports include startup failures caused by globally discovered extensions, while Pi itself provides `--no-extensions` / `-ne` as the supported way to bypass discovery. Treating that as a user installation problem would make a lightweight desktop less recoverable than the CLI.

Pi Wizard therefore models extension discovery independently from project trust and context-file loading. Local, worktree, recovered-worktree, and resumed-session launches can disable extension discovery for that child only. The ephemeral launch-options probe is always extension-free so a broken extension cannot prevent the user from reaching model/thinking/recovery choices. Unexpected exit before the RPC readiness handshake also gives a bounded recovery hint without copying arbitrary stderr into durable failure state.

Sources:

- https://pi.dev/docs/latest/extensions
- https://github.com/earendil-works/pi/issues/8620

### Retry and compaction events carry real control semantics

Current Pi RPC exposes `set_auto_retry`, `abort_retry`, provider retry start/end events, summarization retry events, extension errors, and compaction start/end outcomes. `abort_retry` is specifically for cancelling provider retry delay; after the retry attempt starts, ordinary agent `abort` is the relevant control. Pi does not expose an equivalent summarization-retry cancellation RPC. Compaction end also reports `aborted`, `willRetry`, and optional error text; for overflow recovery, `willRetry` tells the client that Pi owns automatic prompt retry.

Pi Wizard now preserves those distinctions under bounded transient hydration. Stop uses `abort_retry` during provider backoff, normal `abort` after the provider attempt restarts, and exact-process escalation when Pi exposes no truthful RPC cancellation path. Overflow compaction never causes the desktop to replay a prompt. The native `set_auto_retry` command is exposed explicitly, but current `get_state` does not report the enabled flag, so the renderer does not manufacture a recovered current setting.

Primary source:

- https://pi.dev/docs/latest/rpc

### Quiet streams deserve an advisory, not automatic replay

Recent Pi and lightweight desktop reports continue to show turns that remain logically streaming while no useful events arrive for long periods. OpenChamber has added stall/reconnect recovery in this class of failure, but coding turns can also be legitimately quiet during model, tool, extension, or build work. A hard local timeout that retries the prompt would risk duplicate side effects.

Pi Wizard now uses the existing runtime deadline selector for a one-shot two-minute advisory while Pi still reports a Ready/Working run and no explicit retry/summarization/dialog state explains the wait. It does not probe, retry, resubmit, cancel, or relabel the run. The first later Pi event clears the advisory and can arm another deadline. An authoritative `get_state` recovery that discovers Working after a missed `agent_start` can arm the same watch. This adds no interval polling and leaves the existing steady-idle no-periodic-work invariant intact.

Sources:

- https://github.com/earendil-works/pi/issues/8331
- https://github.com/openchamber/openchamber/blob/main/CHANGELOG.md

### Session list identity should not be generated skill context

Pi persists expanded skill content inside the first user message. A Pi issue documents the resulting resume-list failure mode: the session label becomes a long SKILL.md block rather than the user's actual task. The persisted format supplies an explicit `<skill ...>...</skill>` wrapper, which is enough to normalize the catalog read model safely.

Pi Wizard now strips only that explicit generated wrapper for session preview/search, preserving trailing user arguments or using a bounded `[skill] name` placeholder. The JSONL remains untouched. Generic prompt-template expansion is deliberately not reverse-engineered because it has no equally stable wrapper; guessing would create a second interpretation of Pi history.

Source:

- https://github.com/earendil-works/pi/issues/7424

### Lightweight command-palette ergonomics fit; input-history ownership does not

Current Pi desktop clients have converged on keyboard navigation for discovered slash-command palettes and keeping the selected row visible. That behavior is cheap, local, and bounded. Full prompt/input history is a different owner with retention/session semantics and is not necessary for Pi Wizard's command-center role.

Pi Wizard now supports Arrow Up/Down and Enter staging for its at-most-eight Pi-discovered command suggestions, synchronizes pointer selection, and scrolls the selected row only within the mounted palette. It does not add a command-history database or another transcript-like store.

Representative sources:

- https://github.com/gustavonline/pi-desktop
- https://github.com/gustavonline/pi-desktop/releases

### Public latest RPC docs can lead the current stable package

The final Stop audit found a concrete protocol skew. The current public RPC documentation describes `clear_queue` and recommends clients clear queued steering/follow-up work before `abort`. The installed/current stable 0.84.3 package contains the underlying `AgentSession.clearQueue()` implementation, which snapshots its user steering/follow-up arrays, clears them, calls the low-level agent `clearAllQueues()` to discard opaque custom continuations, and emits an empty `queue_update`. But 0.84.3's RPC-mode command dispatcher has no `clear_queue` case and returns `Unknown command: clear_queue`.

This makes an abort-only compatibility fallback incorrect: queued work could continue after abort. It also makes rejecting all of 0.84.3 unnecessarily destructive because the rest of the tested RPC surface is usable. Pi Wizard instead treats an explicit `clear_queue` rejection as a compatibility degradation. The controller retains the most recent user-visible `queue_update` text under the existing recovered-message/byte ceilings, but never sends it to renderer hydration. On rejection, Stop copies that bounded snapshot into its existing recovery transaction and terminates the exact Pi process, which also destroys extension custom queues that are absent from `pendingMessageCount`. Timeouts and malformed accepted responses do not reuse the snapshot because the clear side effect would be unknown. On Pi builds where `clear_queue` succeeds, its response remains authoritative and the child remains reusable after ordinary Stop.

The existing extension-free launch-options probe now tests `clear_queue` on its temporary no-session child and surfaces the result before a new run. The optional installed-Pi smoke performs the same non-billable compatibility check. On installed Pi 0.84.3 it reports `clear_queue` unavailable while all other smoke checks pass.

Sources:

- https://pi.dev/docs/latest/rpc
- https://github.com/earendil-works/pi/issues/8349
- installed `@earendil-works/pi-coding-agent` 0.84.3 `dist/core/agent-session.js` and `dist/modes/rpc/rpc-mode.js`, inspected 2026-08-28

### Audit conclusion

No current Pi/lightweight-harness evidence justifies weakening the established product boundary. Direct RPC Bash remains protocol support rather than an embedded terminal; arbitrary historical branch switching remains unavailable because current Pi RPC does not expose that mutation; automatic prompt replay is rejected because it could duplicate side effects; editor/file-explorer, branch-integration, remote, scheduler, daemon, and multi-harness work remain explicit later candidates rather than hidden gaps.
