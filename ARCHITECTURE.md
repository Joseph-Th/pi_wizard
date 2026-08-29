# Architecture

## 1. Architectural objective

Pi Wizard should add desktop orchestration around Pi while introducing the least possible duplicated runtime state.

The central architecture is:

```text
SolidJS renderer
    |
    | typed, bounded Tauri IPC
    v
Rust application core
    |-- RuntimeStore (authoritative live projection)
    |-- PiProcessManager
    |-- SessionCatalog
    |-- WorktreeService
    |-- GitReviewService
    |-- Settings/ProjectRegistry
    |
    +---- stdin/stdout JSONL ----> pi --mode rpc [cwd/session]
    +---- filesystem -----------> ~/.pi/agent/sessions/ (Pi-owned)
    +---- git ------------------> selected project/worktree
```

The renderer never talks directly to Pi, Git, or session files.

### Core/framework boundary

The implementation keeps domain/runtime foundations in `crates/pi-wizard-core`. That crate must remain independent of Tauri and frontend framework types. `src-tauri` is the desktop composition/adapter boundary, not an alternate owner of process, protocol, or run semantics.

This separation is intentional: protocol/state tests remain fast and deterministic, desktop framework upgrades cannot redefine core invariants, and the renderer can be replaced without moving Pi ownership rules.

### Desktop source layout

The source tree follows ordinary desktop application ownership boundaries instead of concentrating the product in one renderer file and one Tauri file:

```text
src/
  app/            shell/state composition and renderer recovery
  features/       attention, automation, models, projects, runs, sessions, supervision
  features/runs/  run contracts, history/tree, composer/live UI, presentation/notices
  lib/            typed IPC and small framework-neutral helpers
  types/          shared renderer IPC/view-model contracts
  styles/         application styles

src-tauri/src/
  app/            desktop runtime composition/state plus grouped general desktop command adapters
  commands/       thin feature adapters for automation, supervision, and models
  services/       automation, supervision, Pi-session, and internal-run orchestration
  platform/       Windows-only process containment
  lib.rs          module/re-export entry point only
  main.rs         process entry point only
```

Feature folders own feature-specific UI/service state; shared Pi/runtime semantics stay in `pi-wizard-core`. `src/app/App.tsx` is the renderer shell/state composition owner; run/session/composer implementation lives under `src/features/runs`. `src-tauri/src/app/mod.rs` owns desktop composition/runtime state while its general command adapters and wire tests live in `src-tauri/src/app/desktop_commands.rs`; `src-tauri/src/lib.rs` remains a thin module/re-export entry point.

## 2. Stack decision

### Desktop host: Tauri 2

Use Tauri 2 with Rust/Tokio because the application needs reliable subprocess ownership, filesystem access, Git commands, bounded event transport, and native packaging without bundling an Electron runtime.

Tauri uses the system webview. This keeps the core distribution model smaller than bundling a separate Chromium renderer while retaining mature HTML/CSS text, Markdown, diff, and accessibility capabilities.

### Renderer: SolidJS + TypeScript + Vite

Use a static SolidJS renderer. Solid's fine-grained reactive model is a good fit for many independently changing session/tool/status fields because updates can target affected DOM rather than requiring broad component rerenders.

Production builds contain static frontend assets. There is no Next.js application server, bundled localhost API, or Node backend in the runtime architecture.

### Pi integration: RPC subprocess

Launch upstream Pi as `pi --mode rpc` rather than importing the Pi SDK into the desktop process.

Reasons:

- RPC is explicitly intended for custom UIs and IDE integrations.
- It preserves a language/process boundary between Rust and Pi's Node runtime.
- Pi CLI upgrades remain independently observable and diagnosable.
- A Pi runtime failure does not corrupt the GUI process state directly.
- The desktop does not need to ship a second Node application server merely to host the SDK.
- It keeps the semantic boundary simple: Pi owns the agent; Pi Wizard owns the process and presentation.

The SDK remains a useful reference and potential future embedded adapter, but it is not the default production path.

## 3. Pi protocol adapter

The `PiRpcAdapter` is the only owner of Pi protocol details.

Responsibilities:

- spawn with a canonical cwd and explicit trust launch choice;
- use strict LF-delimited JSON framing on stdout;
- keep stderr separate from protocol stdout;
- serialize outbound requests and assign local correlation metadata where Pi's protocol needs it;
- parse Pi commands/events into typed internal events;
- map protocol errors to structured runtime failures;
- expose capability discovery instead of hardcoding model/thinking/command lists;
- preserve unknown-but-valid event data for diagnostics without crashing older clients;
- apply explicit message/output size bounds before forwarding data to the renderer.

The typed command surface follows current Pi RPC semantics rather than reconstructing TUI shortcuts. In particular, normal GUI queue actions use the first-class `steer` and `follow_up` commands. `prompt.streamingBehavior` remains available only for Pi's documented prompt-during-streaming behavior, including extension commands that must still go through `prompt`.

Pi dispatches RPC input asynchronously, so Pi Wizard also owns a client-side command barrier. Session replacement commands are a full quiescence barrier around the shared Pi session runtime. A manual `compact` request blocks composer/session-mutating submissions immediately, before `compaction_start` can arrive, while read-only probes and recovery/control commands remain available. This is ordering protection at the client boundary, not a second agent scheduler.

Image payloads are validated at the backend RPC boundary even if the picker or drag/drop surface already checked them. Limits cover image count, decoded bytes per image, aggregate decoded bytes, and final encoded RPC bytes. The backend accepts only validated `image/*` base64 payloads and must revalidate restored/persisted attachment metadata before reuse. Pi returns full model objects from state/capability discovery, but Pi Wizard retains only the narrow `input` fact needed for image routing: `supportsImages` is true/false when Pi explicitly declares the input list and unknown when older/partial payloads omit it. Explicit text-only state blocks image submission in the backend; the renderer warning/disabled picker is only a projection of that rule.

Manual context compaction remains a Pi-owned operation. The desktop may expose a compact control, but it sends Pi's native `compact` RPC, relies on the existing manual-compaction command barrier, and reconciles with `get_state` afterward. Pi Wizard does not estimate tokens or produce an independent summary to compete with Pi's context policy. Live `compaction_start`/`compaction_end` events additionally project bounded reason, aborted, `willRetry`, and optional error detail. An overflow compaction with `willRetry` means Pi owns the subsequent prompt retry; the desktop waits for Pi events and never resubmits the prompt itself.

Current Pi `message_update` events contain a nested `assistantMessageEvent` keyed by `contentIndex`, not a cumulative message snapshot. Text, thinking, and tool-call blocks can each start, stream deltas, and end independently. Live assembly therefore preserves content index and block kind, bounds both the number of resident blocks and their aggregate bytes, consumes `message_start` plus bounded deltas, and treats `message_end.message` as authoritative. A sparse or maliciously large content index must not become a vector-allocation request. `tool_execution_update.partialResult` is different: it is accumulated and replaces the prior preview. These stream shapes must never share one generic append rule.

Direct RPC `bash` is a third stream shape. `bash_execution_update` carries delta output plus the originating request ID, and Pi may stream more output through those events than appears in the truncated final Bash response. The adapter correlates these updates by exact request ID and keeps only a bounded live preview; it never assigns Bash output to whichever request happens to be oldest or currently visible.

One per-run `RunRpcController` owns pending request correlation, command barriers, writer-failure rollback, live assistant/tool/Bash projection, typed response completion, and application of protocol events to `RuntimeStore`. Tauri/process code must not independently reimplement those semantics.

The adapter supports a compatibility probe on startup:

1. resolve the actual Pi executable;
2. capture `pi --version` once per executable identity;
3. spawn RPC;
4. request state/capabilities through supported RPC calls;
5. feature-gate optional UI from discovered capabilities rather than version string comparisons where possible.

Version ranges may be used to reject a known-incompatible protocol, but feature detection is preferred for optional surfaces.

OS spawn is not RPC readiness. A newly owned child remains `Starting` until a designated bounded `get_state` handshake is accepted. That response may populate authoritative session/model/activity fields while the lifecycle is still Starting; only successful completion of the designated handshake promotes the run to `Ready`. The handshake has an explicit deadline in the manager's deadline selector, so a child that accepts stdin but never speaks RPC is terminated and failed rather than exposed as usable. User Stop or application shutdown supersedes and cancels the startup readiness timer.

## 4. Process ownership

`RuntimeManager` owns every live run, while one `RunProcess` actor owns each child process from spawn to terminal state.

The core process transport verifies that the launch executable is the same canonical Pi selected by the launch-environment resolver, spawns only private piped stdin/stdout/stderr, continuously drains stderr into a byte-bounded ring, and retains the original child handle plus spawn-time PID identity for lifecycle control. RPC is a trusted local stdio boundary; Pi Wizard does not expose the unauthenticated RPC stream on a network listener.

Process wrappers are part of the ownership problem. On Windows, supported standard npm shims are resolved to their direct Node entry point before live spawn, while unresolved shell-backed launchers are rejected. Hard termination still targets the exact captured PID tree with Windows tree semantics because Pi/Node may create descendants during normal operation. On Unix the child is launched as leader of an app-owned process group and hard termination targets that group. Neither path scans for `pi`, `node`, or shell processes by name. If tree/group termination cannot be confirmed, the run is quarantined.

Stderr EOF is diagnostic information, not permission to hold lifecycle completion forever. Final stderr draining shares the bounded lifecycle deadline; if inherited pipe handles prevent EOF, the drain task is cancelled and diagnostics record that EOF was not observed.

Stdin writing is separated from the stdout/process-control task. A blocked large write cannot prevent stdout consumption or priority hard termination. Ordinary RPC writes use a bounded channel; hard termination uses a separate priority control channel.

Each live run has a stable `RunId` and stores at least:

- child process identity/PID;
- Pi session identity when available;
- project identity;
- immutable canonical execution root;
- launch trust choice;
- model/thinking snapshot;
- started-at time;
- lifecycle state;
- bounded stderr/diagnostic ring buffer;
- pending Pi RPC requests/interactions;
- cancellation state.

Process lifecycle and agent activity are separate state axes. Pi's `agent_settled` event means the current agent work has settled and the same RPC process is ready for another prompt; it does not mean the child process has exited.

The process lifecycle is explicit:

```text
starting -> ready -> stopping -> exited
    |         |          |
    +---------+----------+-> failed
```

While the process is `ready`, agent activity is derived independently from bounded live state:

```text
idle <-> working
          |
          +-> aborting
          +-> waiting-for-input
```

`agent_start` and `agent_settled` mutate agent activity, not process lifetime. Process exit/failure clears hot activity. UI route changes never mutate either state axis.

### Stop semantics

The user-facing Stop operation means stop all currently scheduled work for that run. Pi's `abort` does not clear queued steering/follow-up messages, so on a Pi build that exposes `clear_queue`, Stop first sends that command, validates the returned queue against dedicated message/byte ceilings, preserves it as recoverable composer content, and then sends `abort`. A lower-level Abort-current-work action, if ever exposed, may intentionally leave queues intact, but it must use different language.

Compatibility cannot assume that the public latest RPC documentation matches the installed stable package exactly. Installed Pi 0.84.3 has `AgentSession.clearQueue()` internally but its RPC dispatcher rejects `clear_queue` as an unknown command. Pi Wizard therefore keeps a private bounded copy of the latest `queue_update` steering/follow-up arrays under the same Stop recovery limits. That copy is not serialized into `RunRecord` or renderer hydration and is not queue authority. It is used only when Pi explicitly rejects the Stop-owned `clear_queue` request. The runtime then terminates the exact owned process, which prevents both user queues and opaque extension-created continuation queues from running, and restores only the user-visible steering/follow-up text from the last Pi queue event. A timeout or malformed accepted response does not reuse this fallback because command side effects would be uncertain. The on-demand launch-options probe checks `clear_queue` on its already-ephemeral Pi child so the launcher can warn when Stop will require process termination.

When Stop completes, recovered steering messages followed by follow-up messages are prepended to any existing unsent backend draft, matching Pi's editor-recovery ordering rather than overwriting local input. The prospective merged draft is checked against the normal draft byte ceiling before mutation. If it would overflow, Stop still reports its lifecycle outcome and exact recovered queue separately while leaving the existing draft untouched with an explicit restore error. If Stop had to terminate the process, the UI says so explicitly and directs the user back to Pi session Resume rather than implying that the child remains reusable.

Stop is deadline-bounded. The runtime uses the configured abort deadline, then escalates termination against the exact captured child identity and waits only through the configured termination deadline. Never scan or kill `pi`, `node`, or shell processes by name.

Manual compaction is a special Stop case. Current Pi RPC does not expose a dedicated compaction-abort command. Stop still clears/preserves queues first, but if the run remains in active compaction it escalates through the exact-process termination path instead of sending ordinary `abort` and falsely reporting that compaction stopped. Provider retry delay is different: Pi exposes `abort_retry`, so Stop uses that native command and still waits for `agent_settled`. Once the retry attempt has restarted, ordinary `abort` applies to the active agent stream. Pi exposes no equivalent cancel command for the summarization-retry loop, so Stop fail-closes through exact owned-process termination rather than manufacturing cancellation semantics.

If termination still cannot be confirmed, the run becomes **Quarantined** rather than Idle or Stopped. Quarantine revokes the app writer/control path, prevents further RPC mutations, preserves diagnostics/process identity for recovery, and truthfully states that OS-process termination is uncertain. A rejected/timed-out abort cannot optimistically transition to Idle.

The UI distinguishes **Abort requested**, **Stopping**, **Stopped**, **Failed**, and **Termination uncertain** rather than optimistically declaring success.

## 5. Runtime store and IPC

The Rust `RuntimeStore` is the authoritative live projection used by all windows/views.

It owns:

- run state;
- current turn state;
- bounded streaming message buffers;
- active tool executions;
- message queue state;
- pending extension UI requests;
- latest known change invalidation state;
- process diagnostics.

The frontend receives normalized view events, not raw unbounded Pi traffic.

One persistent `RuntimeManager` is the application-level concurrency owner. It contains the authoritative `RuntimeStore`, per-run RPC controllers, process handles, request/extension response waiters, Stop transactions, bounded renderer queues, and shutdown state. Normal RPC/history/drain commands and lifecycle/extension-response controls use separate bounded manager channels so a burst of ordinary renderer traffic cannot place Stop or application shutdown behind the normal work queue.

`get_state` is also an authoritative reconciliation source. On startup/recovery the backend can restore streaming/compaction activity, session ID/path/name, model, thinking level, steering/follow-up modes, auto-compaction state, and message counts from the child without replaying renderer history. Event deltas remain the normal live path; the state response repairs gaps. A recovered Working state also arms the same one-shot quiet-stream advisory used after live events, covering a missed `agent_start` interval without turning state recovery into polling. Pi's current `get_state` does not report the auto-retry enabled flag, so `set_auto_retry` is exposed as an explicit native command without a fabricated durable/current-value mirror.

Authoritative recovery is still bounded retained state, not permission to retain a full valid RPC frame. Session identity is capped by the session-cursor byte limit, while the aggregate retained `get_state` model/session path/session name/session identity projection has its own per-run byte ceiling. Incremental `session_info_changed` events are checked against the same prospective retained-state budget before mutating `RuntimeStore`, so event traffic cannot grow state beyond the recovery boundary.

Successful Pi session-replacement responses do not establish the identity of the session now active in the child. For accepted `new_session`, `switch_session`, `fork`, and `clone`, the runtime manager therefore queues a `get_state` reconciliation before releasing the original replacement completion. In a persistent runtime, the replacement is not written to Pi until the old session's current draft is already durable or a bounded forced save completes. Composer edits/submissions and later managed commands cannot overtake that durability barrier. A persistence failure/deadline fails the replacement locally and leaves Pi bound to the old session. Extension-cancelled replacements do not rebind the run.

Renderer hydration is a versioned snapshot operation, not an assumption that the first IPC call succeeds. A hydration response carries a schema/version plus runtime revision. Current schema v9 carries immutable worktree identity, backend-owned run start/terminal timestamps and change-invalidation revision, the active session draft/durability and attachment metadata, whether persisted draft restoration is still pending, backend-derived composer availability/submission ownership, capabilities, live bounded assistant/tool/Bash projections, extension UI state, session-sync cursors, and bounded transient Pi recovery projections for compaction, provider retry, summarization retry, the latest extension error, and the non-authoritative quiet-stream advisory. Failure must enter an actionable Retry/Relaunch state rather than an indefinite loading spinner. Applying the same snapshot twice is idempotent, and a renderer must reject an unsupported schema before applying the snapshot.

The backend emits only a coalesced `run dirty` wake-up to Tauri. The renderer then pulls bounded normalized event pages. A dirty run is signaled once until its backlog has been drained. Ordinary hydration is deliberately non-destructive because some transient delivery, such as extension notifications and completed-tool events, is not represented in the authoritative snapshot; clearing all queues during hydration could erase another run's pending transient event. Hydration instead re-announces any queue that already has pending delivery, which also closes the late-subscriber/reload wake-up case.

If one run's semantic backlog declares renderer desynchronization, recovery is an explicit per-run transaction. It atomically returns authoritative hydration and discards only that run's stale queued frames while preserving out-of-snapshot editor text for redelivery. If the Tauri broadcast receiver itself lags, it asks for authoritative hydration rather than attempting to replay an unknown event gap. The renderer installs listeners before initial hydration and sequences concurrent hydration results so an older completion cannot overwrite newer authoritative state. Hydration demand is retained across continuation drains so a semantic/display update in an early bounded batch is not forgotten merely because more backlog remained. There is no passive interval poll.

Application exit is also a runtime-manager transaction. Tauri intercepts `ExitRequested`, asks the manager to terminate all child identities that were live when shutdown began, and prevents native exit until the bounded shutdown result returns. A renderer close/reload is not equivalent to process shutdown.

An explicit per-run **Close** operation is separate from both Stop and application shutdown. Stop cancels current scheduled agent work and keeps the Pi process reusable when the installed RPC surface can complete the native queue-clear/abort path; compatibility or failure escalation may instead terminate it as described above. Close is allowed only when the run is not actively working, forces the current session draft to durable storage first, then terminates only that run's exact owned child tree/group. Failure to persist the draft fails closed and leaves the process alive. After confirmed terminal transition, the execution-root ownership lock is released so another run may use that checkout. A separate **Dismiss** operation applies only to already-terminal runs and removes run-scoped hot/runtime projection state; it does not delete Pi session JSONL or session-scoped draft persistence.

Canonical execution roots are exclusive across live runs, not only for app-created Git worktrees. This prevents two local-checkout runs from concurrently editing the same filesystem root. The backend is authoritative for admission; renderer launch/session-recovery surfaces only mirror that ownership by routing the user to the existing run or suggesting worktree isolation.

The renderer is disposable. Reload/crash recovery reconstructs the window from the backend RuntimeStore, session-scoped draft owner, and selected navigation identity without restarting live Pi children. Automatic renderer reload attempts are bounded by a crash-loop breaker; repeated failure becomes an explicit recovery screen while backend runs remain supervised.

Run-list ordering is presentation-only and never mutates runtime state. The sidebar/dashboard sort the bounded hydration snapshot by Needs Attention, active work, other live processes, then terminal retention, with newer UUIDv7 RunIds first within a class. Selected-run identity remains keyed by RunId, so reordering cannot retarget an action. Failed/quarantined run surfaces display only the already-bounded `RunFailure` projection and exit code. A known OS exit code is retained when an unexpected child exit becomes `Failed`; protocol/spawn/internal failures that do not have such an OS observation keep the field absent rather than manufacturing a code. Platform folder opening likewise accepts only RunId at IPC, re-resolves that run's execution root in the backend, and passes it as one argument to the native folder opener rather than accepting renderer-supplied filesystem targets.

### Event coalescing

Pi can generate many streaming updates. Forwarding each token/tool partial as an independent desktop IPC event creates avoidable renderer pressure.

The bridge should:

- immediately deliver semantic state transitions and errors;
- coalesce compatible text/tool-output updates into short bounded windows without crossing assistant content indices, tool-call IDs, or direct-Bash request IDs;
- enforce a maximum queued-byte budget per run;
- replace accumulated tool-output previews when Pi reports an accumulated partial result rather than treating it as a delta;
- drop superseded display-only intermediate frames under pressure while never dropping terminal state or request ownership events.

The queue budget includes payload bytes, coalescing-key bytes, and fixed per-frame accounting so an attacker cannot create an unbounded queue of zero-length frames. If semantic frames alone consume the budget, the bridge reports a renderer-desynchronization/rehydration condition; it does not silently discard semantic state.

The target is responsive perception, not protocol-event fidelity in the DOM. The Pi session file remains the durable history.

### Persistence is never on the token hot path

`message_update`, `bash_execution_update`, and `tool_execution_update` are high-frequency transient display events. They may mutate bounded in-memory projections and enqueue coalesced renderer frames, but they MUST NOT directly trigger app-owned durable catalog/draft/index writes, fsyncs, or globally serialized persistence work.

Durable app-owned writes happen only at semantic boundaries such as draft debounce/flush, catalog metadata invalidation, terminal message/session transitions, or explicit settings changes. Streaming throughput from one run must not serialize unrelated sessions behind a global disk-write lock.

### Finite automation

Automation definitions are a separate schema-versioned app-owned domain. They contain only bounded chain metadata and ordered prompt text; they do not copy Pi transcripts or process state. Definition writes use atomic replacement and corruption quarantine like other recoverable app state. Active execution state is backend-owned memory because native application shutdown intentionally terminates owned Pi processes; renderer reload does not cancel an execution.

One automation execution is an event-driven state machine layered over the existing `RuntimeManager`. Admission always goes through the manager's live-run ceiling. A worker step starts one normal Pi session using the model/thinking selection captured when the chain starts. Local-checkout chains are sequential because canonical live execution roots remain exclusive. Parallel chains create unique Git worktrees from one explicitly inspected base snapshot; generated branch/path identity contains the full compact automation-execution UUID rather than a timestamp-heavy UUIDv7 prefix. The automation owner creates only the generated sibling worktree parent directory before journaling the exact creation intent and invoking the shared Git worktree primitive. A failure proven to have made no Git mutation discards that intent; indeterminate or mutating failures retain it for recovery. Verified worker worktrees remain journaled after completion. A worker turn becomes complete after it returns to genuinely idle with no Pi queue, compaction/retry, or pending extension dialog and either a new assistant message exists or real Pi activity was observed for that turn. The activity fallback matters when the user explicitly Stops/aborts a turn before Pi produces assistant text. The automation owner then closes that Pi process to release its live slot and advances to later prompts.

The runtime manager broadcasts a cheap semantic run-state wake-up in addition to the renderer's coalesced dirty wake-up. Automation consumes only semantic wake-ups, including successful live-capacity changes, so token/tool display traffic cannot drive scheduler work and renderer draining is not required for automation to progress. Launches, worktree creation, and live-limit mutations share one desktop serialization gate; admission and local execution-root ownership are rechecked under that gate. Cancellation is also rechecked before process spawn, including after a worktree creation has completed, so a cancelled not-yet-started step cannot race into a new Pi process. Any already-created worktree remains in the normal recovery journal rather than being deleted implicitly.

The complete automation scheduler is covered by deterministic desktop integration, not only helper contracts: local sequential chains launch distinct real child RPC processes and continue after an isolated prompt rejection, while parallel chains use disposable real Git repositories and must reach simultaneous live workers in unique verified worktrees.

Automation renderer projection is also separated by cost. Saved chain definitions are hydrated only while the Automation view is mounted. Catalog changes invalidate the full view model, while execution changes fetch only bounded execution snapshots. Identical execution mutations emit no invalidation, and each step snapshot carries only a UTF-8-safe prompt preview (1 KiB by default) rather than repeatedly cloning full saved prompt text across IPC.

Canceling a chain stops future launches but deliberately leaves already-running workers alone. User Stop remains the lifecycle operation for canceling work inside a Pi run.

### Independent supervision

Supervision has its own coordinator, snapshot/event surface, Tauri commands, renderer feature folder, and lifecycle. It is not stored inside `AutomationExecutionSnapshot`, is not started by `runtime_start_automation`, and does not change automation slot accounting.

Starting supervision selects one registered project plus an explicit supervisor model/thinking choice and finite cycle budget. The supervisor is itself a normal Pi run and therefore consumes one live-run slot. It uses a separate generated Git worktree and launches with project context files/extensions disabled so supervision is not coupled to arbitrary project extension UI. Its eligible targets are other live runs for that ProjectId, regardless of whether they were started manually or by Automation. The supervisor never targets its own RunId.

Supervision subscribes to the same semantic run-state wake-up used by Automation. It keeps bounded per-target assistant-message baselines and runs a cycle only after an eligible target has a new settled turn; token/tool display traffic never triggers supervision. The desktop sends bounded task/status plus bounded native `get_last_assistant_text` excerpts, waits for the supervisor's normal Pi turn to settle, then parses one narrow JSON object containing directives addressed to exact active worker RunIds. Each directive is bounded, duplicate targets in one cycle are rejected, and target state is freshly revalidated immediately before Pi's normal `prompt`, `steer`, or `follow_up` RPC command.

Supervision is explicitly finite. Runtime limits cap cycles and each model turn has a deadline. Reaching the configured cycle budget, receiving invalid autonomous output, or explicit user Stop ends the supervision owner and performs bounded termination of its app-owned supervisor process while leaving observed worker runs untouched. An abrupt desktop exit is handled by the Windows process-containment boundary described below.

### Model catalog

Pi capability discovery remains authoritative for models Pi reports, including names and image-input capability. A separate small schema-versioned custom-model catalog stores only user-supplied provider/model identity plus an optional display label. It stores no credentials and never claims input capability. Launcher/Automation/Supervision model pickers merge discovered and custom identities by `(provider, model)`, with Pi-discovered metadata winning for duplicates. Pi remains the final launch-time validator.

### Windows Pi invocation and process containment

Pi Wizard does not keep a command-shell shim alive as a production Pi process. When Windows discovery resolves the standard npm `pi.cmd`/`pi.ps1` installation, the environment owner resolves the corresponding `node.exe` plus `@earendil-works/pi-coding-agent/dist/bundle/cli.js` and the process owner spawns that Node entry point directly with `CREATE_NO_WINDOW`. The logical Pi shim path remains diagnostic/discovery identity; the runtime invocation target is the direct Node child. If a configured/discovered `.cmd`, `.bat`, or `.ps1` launcher cannot be resolved to a direct executable invocation, live-runtime spawn fails explicitly before starting it rather than retaining a background shell.

Graceful shutdown still belongs to `RuntimeManager`, which closes every owned Pi run and reports termination uncertainty explicitly. The Windows exact-tree `taskkill.exe` fallback is itself spawned with `CREATE_NO_WINDOW`. In addition, the Windows desktop establishes one kill-on-close Job Object before child work begins and keeps that handle for the process lifetime. Descendants inherit the job, so an abrupt desktop termination closes the job and the kernel terminates remaining descendants rather than leaving orphaned Pi/Node processes. Exact-process Stop/Close behavior and bounded diagnostic finalization remain in place for ordinary lifecycle operations.

### Composer draft ownership

Composer drafts are user-owned state, one record per session/new-session identity. They are backend-owned rather than component-local so navigation and renderer replacement cannot lose or cross-contaminate text.

The live runtime manager now contains an in-memory `SessionDraftStore` that maps each run to either a temporary pre-session owner or Pi's authoritative current session ID. GUI-created new sessions receive an explicit Pi session ID before spawn, allowing their draft owner to exist immediately. Runs whose identity is learned later migrate a temporary pending-run draft only when the first authoritative session ID arrives. Subsequent session switches do not migrate text: the target session gets its own draft, and switching back restores the previous session's record.

Pi extension `set_editor_text` mutates this same backend draft before renderer notification. Hydration schema v9 exposes the current draft plus restore/submission state, immutable worktree identity, lifecycle timing, the monotonic change-invalidation revision, and bounded transient recovery state so reload recovery is backend-owned rather than dependent on component state.

Draft persistence uses monotonically increasing generations. At most one write per draft is in flight. Completion for generation N cannot mark a newer generation N+1 durable, and a retry snapshots the newest current generation rather than replaying stale bytes. A bounded worker outside the token/process-manager hot path performs schema-versioned atomic replacement under the portable `pi-wizard-data` directory beside the desktop executable and reports completion back through session ID plus generation ownership. Corrupt files are quarantined independently. The UI exposes Dirty/Saving/Saved/Failed truthfully; persistence errors are not swallowed.

Persisted-draft restoration is asynchronous and therefore has an explicit restore-pending gate. The renderer cannot edit or submit the initially empty baseline before the saved value has either loaded, been proven absent, or failed visibly. The in-memory `SessionDraftStore` also has an explicit record-count ceiling. When that ceiling is reached, only an unowned session record whose current generation is `Saved` may be evicted; Dirty, Saving, Failed, or still-owned records are never discarded to make room. Eviction clears the manager's load-attempt/pending/debounce bookkeeping for that session so a later revisit may load the persisted draft again. A stale asynchronous load completion for an already-evicted unowned session is ignored rather than recreating a cache record outside the ceiling. If no safe record can be evicted, the session transition fails closed instead of allocating without bound or dropping user state. Session replacement forces old-session durability before Pi switches; application shutdown bypasses normal debounce and waits only through a separate bounded flush deadline.

Composer submission is also a backend transaction rather than a renderer button convention. Send is valid only for an idle Ready run; Steer/Follow-up only while the agent is working. The transaction captures exact RunId/request ID/draft generation before writing Pi. Pi acceptance clears only that submitted generation, so typing performed while the acknowledgement is in flight survives; rejection or transport failure preserves the submitted text.

### Extension interaction bridge

Response-bearing Pi extension `select`, `confirm`, `input`, and `editor` requests remain backend-owned by exact request ID and timeout. Tauri exposes a typed response adapter only; it does not reinterpret dialog semantics. The renderer's cross-run Needs Attention surface renders the typed request, keys local state to the request ID, disables duplicate submission while a response write is in flight, and refreshes authoritative state after either success or a stale/expired-request rejection. Fire-and-forget notify/status/widget/title/editor-text methods remain separate owners and are never manufactured into pending dialogs.

## 6. Session persistence and catalog

Pi's JSONL session files under Pi's normal session directory remain authoritative. Pi Wizard does not rewrite them into an application-owned conversation database.

`SessionCatalog` is a derived read model. It may persist lightweight metadata so startup does not require parsing every session:

- path/session ID;
- canonical cwd/project key;
- name/title;
- file size/mtime;
- latest user prompt preview;
- last activity timestamp;
- optional token/turn summary when cheaply known;
- index schema/version.

Any catalog database is disposable and rebuildable from Pi files. It never owns session semantics.

For an active RPC session, the preferred synchronization primitive is Pi `get_entries` with the last stable entry ID as `since`. Pi documents session entries as an append-only tree with stable IDs, making that ID a durable cursor across client restarts. `leafId` detects branch movement in the same round trip. An unknown cursor is a classified resync condition, not a reason to silently drop history.

Cold/offline history still comes from Pi-owned JSONL files. `get_messages` is not the routine synchronization path because it hydrates the whole current conversation and omits historical/abandoned branches that `get_entries` can represent.

### Index policy

- No full repository-wide or session-wide scan blocks app startup.
- Session discovery is explicit/on-demand. Without a derived index, establishing exact newest-first modification-time order and a stale-continuation snapshot may enumerate lightweight path/mtime metadata across the resolved session directory; retained candidate metadata remains bounded to the next working window.
- Catalog preview/search normalization may remove only Pi-generated wrappers whose structure is explicit enough to reverse safely. Current skill invocation persistence wraps generated SKILL.md context in `<skill ...>...</skill>`, so the read model shows trailing user arguments or a bounded `[skill] name` placeholder. The JSONL is never rewritten. Generic prompt-template expansion has no equally stable wrapper and is therefore not guessed back into an invocation.
- Detailed session header/preview reads are separately bounded per visible page. Do not parse whole histories merely to rank or list sessions.
- Parse session contents incrementally and cancelably.
- Do not index full tool outputs by default.
- Full-text search, if introduced, should prioritize user prompts and assistant text, with explicit resource budgets.
- Stateless continuation cursors fail stale when the candidate metadata snapshot changes, because an external file modification may change global ordering. A future persistent derived index may invalidate only affected entries, but it must still detect external Pi/CLI changes before serving a continuation as current.
- Migration work is versioned, bounded, cancelable, and never runs as an unbounded startup monopoly.

A SQLite/FTS or watched derived index is permitted only if measurements justify its extra owner. The current 1,200-session complete-traversal Windows fixture is fast enough that persistence is not justified. Any future index remains disposable, cannot become Pi-session authority, and must preserve stale/external-change semantics; the architecture depends on the `SessionCatalog` contract rather than SQLite itself.

### Recoverable app-owned state

Pi Wizard's own registries, indexes, drafts, preferences, and hydration metadata are never allowed to become a second authority for Pi sessions or project contents. Persisted domains are schema-versioned and isolated so one malformed record/file cannot poison unrelated state.

The desktop uses one portable app-owned state root named `pi-wizard-data` beside the executable. Prompt chains, custom model identities, project registrations, worktree recovery records, preferences, and session drafts therefore survive executable rebuilds/replacement in place without depending on Windows AppData. When upgrading from the older AppData layout, the desktop migrates the legacy `runtime-state` tree only when no portable root already exists; an existing portable root always wins so migration cannot overwrite newer local state.

Durable writes use an atomic replace strategy appropriate to the platform. On parse/schema/integrity failure, quarantine the affected app-owned state and start from a safe empty/rebuildable projection where possible. Do not delete, rewrite, or hide intact Pi JSONL merely because a project/catalog/preferences file is corrupt. A previous failed launch must have a safe-start path that can bypass disposable indexing/project-selection state instead of repeating the same crash or CPU-heavy repair loop indefinitely.

Preferences are a separate persistence domain rather than fields embedded into project/worktree/runtime snapshots. The first persisted preference is the live-run admission ceiling. It is schema-versioned, byte-bounded, atomically replaced, and validated against the configured runtime maximum before use. The preference file is committed before the manager ceiling changes, so a persistence failure cannot create a false durable UI state. Corrupt, oversized, unsupported-schema, or out-of-range preference files are quarantined without altering project/worktree/draft/session authority; startup uses the configured safe default and exposes a bounded recovery notice.

### Compatibility and migration policy

Pi's semantic version is diagnostic input, not the normal feature gate. Startup compatibility is established by resolving and version-probing the exact executable that will be spawned, then completing the bounded RPC `get_state` handshake and parsing required typed responses. Optional UI is driven by discovered models, thinking levels, commands, model input capability, and forward-compatible event parsing. Unknown valid events remain ignorable/diagnostic; malformed known protocol shapes fail the affected run rather than being guessed into another meaning. A version-range rejection is reserved for a specifically known incompatible Pi release and must be backed by an executable regression fixture or upstream protocol evidence.

App-owned durable schemas follow read-old/write-current migration. A known older schema may be migrated only by an explicit tested decoder (for example the existing schema-1 text draft to schema-2 attachment-aware draft). The migrated value is validated under current byte/count limits and any later write uses only the current schema. An unknown newer schema, malformed state, duplicate identity, or out-of-range value is never downgraded heuristically: the affected app-owned domain is quarantined or treated as rebuildable while Pi JSONL, Git contents, and unrelated app-owned domains remain untouched. Schema changes therefore require an explicit version bump, a fixture for the previous supported representation, and a failure fixture for unsupported future state.

### Project identity

A registered project has an opaque `ProjectId` plus one canonical filesystem root. Display names, Git remotes, repository names, last-opened UI state, and session metadata are hints only; none may silently rebind a `ProjectId` to a different directory.

The app-owned project registry persists only opaque `ProjectId` plus canonical root, separately from Pi session JSONL. Registration and explicit relocation use atomic whole-file replacement and are committed to disk before the in-memory indexes change. The registry has entry/byte ceilings; malformed, oversized, unsupported, or duplicate-ID/root state is quarantined and safe startup continues with an empty rebuildable registry. Missing persisted roots remain representable as detached bindings because registry restore does not require the path to exist.

At open/launch time the backend verifies the canonical root. If it is missing or no longer resolves to the stored root, the project becomes **Detached/Moved** and cannot launch a run until the user explicitly relocates or removes that registration. Opening a symlink/alias canonicalizes before duplicate-project comparison. Worktrees remain separate execution roots attached to the project/run, not alternate project identities inferred from Git metadata.

## 7. Transcript read model

The renderer consumes `TimelineItem` projections rather than the raw session document.

Requirements:

- page by semantic turns or bounded byte windows;
- virtualize mounted timeline rows;
- cap renderer-resident decoded history independently of total session size;
- preserve scroll anchoring when prepending older pages;
- do not autoscroll if the user has left the live edge;
- store large tool outputs out of the reactive DOM state and load their preview/details on demand;
- release inactive session view state after a bounded cache policy while keeping live run state in Rust.

This is a correctness requirement for large histories, not an optional optimization.

## 8. Worktree service

The `WorktreeService` owns Git worktree creation and identity. A separate app-owned `WorktreeRegistry` is a recoverable journal for creation transactions; neither owner performs implicit Git cleanup.

For a worktree run:

1. Resolve repository root, current branch identity, and exact base commit explicitly before creation. Never infer `main`, `origin/HEAD`, or a stale UI branch label.
2. Persist/show the exact base commit plus intended new branch/worktree identity as the creation plan **before the first mutating Git command**.
3. Create a uniquely named branch/worktree as one recoverable transaction.
4. Canonicalize the resulting path and verify the newly created worktree is on the requested branch at exactly the captured base commit.
5. Persist that path and Git identity into the run identity before Pi starts.
6. Spawn Pi with that exact cwd.
7. Never retarget the run because the user switches project/session UI.

The recovery journal has its own stable `WorktreeId`, entry/byte ceilings, atomic whole-file writes, and corruption quarantine. An intent exists before Git mutation and is upgraded with the verified `CreatedWorktree` identity after creation. A desktop/Pi launch crash can therefore leave an explicit recovery record instead of an unclassified orphan.

Restart reconciliation is deliberately non-mutating. A plan is considered **Not Created** only when both its requested branch and path are proven absent. A surviving path must resolve to the same Git common repository and requested branch. The captured creation base must still be an ancestor of current `HEAD`, not necessarily equal to it, because legitimate agent commits and dirty work after creation must remain recoverable. A wrong branch, rewritten ancestry, branch-only mutation, path-only mutation, or unrelated repository remains a classified partial/conflicting recovery and is never auto-deleted. After the user independently removes both Git resources, a fresh absence proof may retire only the app journal record.

If creation fails after creating only task-owned resources, rollback is allowed only when their ownership and emptiness are proven. Otherwise record an orphan/recovery item rather than destructively guessing. Worktrees are not pooled or silently reassigned between live runs. Cleanup is explicit; a worktree with uncommitted or unpushed work is never silently deleted. The first cleanup surface is deliberately narrower still: a fresh recovery probe must prove the exact recorded repository/branch/path, no live run may use the worktree, the working tree must be clean, and current `HEAD` must equal the captured creation base. Only then may the service run non-forced `git worktree remove` and delete the task branch with an expected-old object ID. Any failure/partial mutation keeps the recovery journal; the journal is retired only after a fresh probe proves both branch and path absent. The first product does not need automatic branch merging.

Every Git operation for a run resolves from that run's canonical execution root and verifies repository/worktree identity before mutation. UI-selected project state is not an acceptable substitute for the run binding.

Git worktrees prevent parallel agents from writing the same checkout. They do not restrict host filesystem, shell, network, credentials, or process access.

## 9. Git review service

`GitReviewService` runs outside the renderer and only on demand or explicit invalidation.

It provides bounded structured results:

- repository status summary;
- changed file metadata;
- per-file diff pages/hunks;
- binary/too-large markers;
- current revision/worktree identity.

Rules:

- no continuous Git polling for sidebar cards;
- no full diff concatenation when metadata will answer the current view;
- no synchronous JS diff algorithms on large content;
- commands have time/output bounds and cancellation;
- cached diff results are keyed to repository/worktree revision plus relevant working-tree metadata and invalidated on known mutations.
- because Pi extensions may define arbitrary tool names, the runtime does not guess which tools are file-mutating. Every completed Pi tool and direct RPC Bash request conservatively advances a monotonic per-run `changeRevision`. A renderer may retain already-known summary metadata, but any summary/detail whose captured revision differs is stale; detail is discarded and Git is rerun only after explicit user review action.
- binary classification happens through bounded Git metadata before a text patch is requested, and the renderer sequences summary/detail requests so stale asynchronous detail cannot reclaim the visible review slot after a newer refresh.
- tracked text review is byte-window paged. A continuation cursor contains only the project-relative path, raw byte offset, and SHA-256 digest of the exact preceding diff prefix. The next request re-runs Git, stream-discards and hashes that prefix, and fails closed if the digest changed. Only the current page is retained; a separate scan ceiling bounds the cost of re-reading the prefix, so extremely large later offsets stop explicitly rather than making one page request unbounded. Producing a page reads only enough bytes to fill the page plus at most one byte proving another page exists, then terminates the Git subprocess.

## 10. Project trust and security

Pi project trust controls loading of project-local Pi settings/resources/extensions/packages. It does not sandbox tools or model-requested actions.

Pi stores trust decisions by canonical directory and lets the closest saved current/parent decision apply before the global `defaultProjectTrust`. Pi Wizard's default launch policy is therefore **Use Pi trust settings** (no trust CLI override). A user may explicitly choose a one-run **Approve project resources** or **Ignore protected project resources** override when needed. The application does not silently edit `trust.json`.

RPC mode cannot show Pi's interactive trust prompt. With no applicable saved decision, Pi's `defaultProjectTrust: ask` behaves non-interactively by ignoring protected resources; `always` trusts and `never` ignores. The GUI may preflight protected-resource presence to explain what will happen, but Pi remains the authority for saved/default trust resolution and user/global extensions may also participate in the trust event.

Important: declining/ignoring project-resource trust does **not** disable context files. Pi still loads `AGENTS.override.md`, `AGENTS.md`, and `CLAUDE.md` unless context loading is separately disabled with `--no-context-files`. Pi Wizard represents this as an independent advanced launch policy and says so literally in the UI.

The application should not silently change global Pi trust settings merely to make RPC convenient.

### Isolation vocabulary

The runtime models three independent dimensions:

1. **Project resource trust**: whether Pi loads project-local resource code/configuration.
2. **Git isolation**: local checkout vs separate worktree.
3. **Execution isolation**: host process vs a future real container/VM/policy sandbox.

The first implementation supports host execution and optional Git isolation. A later container/VM adapter can implement true execution isolation without changing the user/session model.

The packaged renderer uses an explicit Content Security Policy. Production script/style sources are self-only and do not require `'unsafe-inline'` or script eval; Vite's loopback WebSocket and inline development-style injection are isolated to Tauri's separate `devCsp`. Remote scripts/content are not part of the core architecture. Tauri IPC is the only renderer-to-host control path, and future capabilities/commands must be granted narrowly rather than exposing generic shell/filesystem authority to the WebView.

## 11. Extension UI bridge

Pi extensions can request UI operations over RPC. Support only the protocol-defined desktop-safe interaction set at first:

- select;
- confirm;
- input;
- editor/text input;
- notify/status/title where meaningful;
- bounded widgets when representable safely.

Pending interactions are keyed by `(RunId, RequestId)` in Rust. A rendered dialog is a view of that object, not its owner.

Fire-and-forget methods have separate bounded owners: `setStatus` and `setWidget` are keyed replacement maps with entry/byte ceilings, `setTitle` is a bounded scalar, `set_editor_text` mutates the session draft owner, and `notify` is transient/rate-limited presentation state rather than an unbounded persistent log or unconditional OS notification stream.

Dialog requests may contain a Pi-side timeout. The backend records enough timeout metadata to make a timed-out request non-actionable locally; a late UI response must not be sent after the request has expired. Pi remains responsible for resolving the extension call itself.

Dialog-local sub-state is also keyed/reset by request ID. Reusing a mounted component for a new request must not carry an approval stage, selection, editor buffer, or optimistic completion state from the previous request. Fire-and-forget extension UI methods never create fake pending-response ownership.

Closing a window/view must not leak or silently resolve a pending request. Teardown must either preserve the request for another view or explicitly reject/cancel it through the owning protocol path.

## 12. Concurrency and resource policy

Parallel agents are useful until local model clients, language servers, build tools, or shell processes saturate the machine.

The runtime exposes a configurable live-run admission limit rather than an unbounded fan-out API. A validated build/runtime limit is the hard maximum; the manager owns a mutable runtime ceiling within `1..=maximum`. Starts at or above the ceiling are rejected before child spawn. Lowering the ceiling below the current active count never terminates existing runs, and raising it cannot exceed the configured maximum. The desktop preference owner persists the selected ceiling independently with atomic replace/quarantine semantics and applies it to a newly spawned manager before normal desktop use. If queued starts are introduced later, they must be explicit first-class state rather than silently spawned or hidden in the renderer.

Expensive application-owned jobs also have separate limits:

- session indexing;
- Git diff generation;
- syntax highlighting of large blocks;
- Markdown parsing of cold history.

No one queue should block unrelated lightweight UI commands.

## 13. Failure semantics

Every externally visible operation has an owning failure object.

Examples:

- Pi executable unavailable;
- incompatible/invalid RPC framing;
- child exits unexpectedly;
- project trust unresolved;
- worktree creation conflict;
- session file corrupt/truncated;
- diff command timeout/output cap;
- IPC payload rejected as too large;
- extension UI request unsupported.

Failures preserve identity and retryability. The UI must not collapse them into a generic toast if the user needs to know which run/request failed.

## 14. Performance budgets

These are **acceptance targets** for the personal Windows desktop application. Benchmarks refine them on the Windows machine(s) where the app is actually used.

| Surface | Initial target |
| --- | --- |
| App shell first interactive paint | < 800 ms warm OS/webview, excluding Pi child startup |
| Idle app-owned CPU with no active work | effectively 0% sustained; no periodic polling loops |
| App-owned baseline resident memory | target < 120 MB before Pi child processes, measured per platform |
| Composer input while 4 runs stream | p95 key-to-paint < 32 ms |
| Visible timeline DOM | bounded window, never proportional to total session length |
| Session open | render latest bounded page without parsing entire history |
| IPC event backlog | byte-bounded per run; no unbounded token queue |
| Tool output preview | bounded; full output loaded explicitly |
| Diff | metadata first; file/hunk data loaded on demand |
| Passive sidebar | zero per-row process polling and zero per-row filesystem watchers |

### Scale fixtures

Performance tests should include at least:

- a 10k+ message/part session;
- a 25-50 MB Pi JSONL session;
- a tool-heavy live stream with large partial outputs;
- a project with hundreds/thousands of historical sessions;
- a large change set including at least one file too large for normal inline diff rendering;
- eight concurrent simulated Pi runs with bursty events.

Exact fixture sizes can evolve from real Pi data, but the architecture must remain bounded as these totals grow.

## 15. Observability

Developer diagnostics expose bounded counters instead of giant logs. The normal desktop surface is an explicit pull snapshot; opening a view does not start sampling and no periodic diagnostic timer or durable log exists:

- RPC events/second and bytes/second per run;
- coalesced vs delivered UI event counts;
- RuntimeStore bytes by run;
- mounted timeline row count;
- active indexing/diff jobs;
- process count by owner;
- dropped superseded display frames;
- renderer long-task measurements in development builds.

Recent RPC throughput uses one fixed-size in-memory time window per retained run, while cumulative queue counters are saturating scalars. Process ownership and active Git/session-catalog job counts come directly from their existing owners. Mounted timeline rows are sampled by the renderer only on explicit diagnostic refresh; development long tasks use the browser's event-driven performance observer. If textual tracing is added later it must use ring buffers and size caps. Turning on diagnostics or tracing must not reproduce the CPU/disk/memory amplification they are intended to find.

## 16. Desktop launch environment

Pi Wizard must not assume a GUI-launched Windows process inherits the same environment as the user's terminal. Explorer/start-menu launches can expose a different PATH or stale process environment. Pi and its shell tools inherit the environment used to spawn the RPC child, so environment parity is part of execution correctness.

The backend resolves a launch environment before Pi discovery/spawn. Precedence is: explicit user-configured executable/environment settings, the desktop process environment, then a bounded Windows-appropriate user-shell/environment probe when required. Do not parse shell startup files manually or run an unbounded interactive shell. Cache only non-secret provenance needed to invalidate/reprobe; never persist provider keys, tokens, or complete environment dumps.

Environment acquisition and environment selection are separate owners. Selection/provenance is Tauri-independent and chooses one complete in-memory environment that is then reused for Pi, Git, compatibility probes, and the final child spawn. The platform shell probe may supply a candidate snapshot, but it cannot cause Pi to be discovered under one PATH and later spawned under another. Process spawn rejects executable-identity mismatch.

Diagnostics report which Pi/Git executable was selected and the source/class of PATH resolution without echoing secret values. If the resolver cannot establish a usable environment, launching remains possible only through an explicit configured executable/profile rather than silently claiming parity with the CLI.

Pi's own `AI_AGENT=pi` and `PI_CODING_AGENT=true` markers are set by the CLI/RPC entry point; Pi Wizard must not fake or depend on SDK-only behavior when using the subprocess path.
