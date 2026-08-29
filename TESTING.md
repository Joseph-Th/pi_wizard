# Testing and Verification

This is the verification contract for Pi Wizard. Workspace policy requires local verification. Do not add GitHub Actions.

## 1. Current verification lanes

The workspace manifest exposes repository-owned semantic lanes:

```text
python tools/verify.py quick
python tools/verify.py standard
python tools/verify.py full
```

`quick` currently owns:

- Rust formatting check;
- locked deterministic `pi-wizard-core` tests with an explicit four-thread harness cap. This limits incidental Windows `.cmd`/Node/process-tree fixture overlap independently from the product's eight-session live-run ceiling instead of letting Cargo scale OS-process tests to the host CPU count; production lifecycle deadlines remain unchanged;
- a renderer/Tauri surface contract that enumerates every `runtime_*` command literal, requires a matching host `generate_handler!` registration, rejects newly introduced renderer-side Tauri API modules until their ACL is reviewed, and requires the main-window event listen/unlisten permissions;
- deterministic renderer crash-loop and accessibility structure contract tests;
- strict TypeScript type checking.

`standard` runs `quick`, then additionally owns:

- locked deterministic `pi-wizard-desktop` host tests;
- workspace Clippy for all targets with warnings denied;
- production Vite renderer build;
- Tauri host compilation as part of the workspace Clippy target graph.

`full` runs `standard`, then owns the deliberately expensive or OS-serialization-sensitive deterministic Windows desktop contract:

- ignored core fixtures, currently a 25+ MiB/10k-entry Pi JSONL history, a multi-megabyte tracked Git diff paged through fixed byte/scan ceilings, eight concurrent simulated streaming runs, a 1,200-file historical session catalog traversed through bounded continuation pages with fixed retained-candidate/scan/page ceilings, and confirmed idle-run Close/process-tree termination exercised serially so unrelated Windows process fixtures cannot create false quarantine results;
- an ignored desktop cold/warm app-owned-state startup measurement with a deliberately loose regression ceiling;
- desktop configuration/version/CSP/icon checks;
- an optimized Tauri Windows desktop build with `--no-bundle`;
- a post-build PE-header assertion that the release executable is `IMAGE_SUBSYSTEM_WINDOWS_GUI`, preventing a console/terminal window from being created when the packaged desktop app starts;
- a packaged-WebView smoke that launches that exact release executable under a loopback-only WebView2 DevTools port, exercises representative custom IPC plus `plugin:event|listen`/`unlisten`, navigates every primary surface, keyboard-resizes the real sidebar under production CSP, verifies normal workspace surfaces consume the available main width, and fails on visible ACL/CSP/runtime-update errors. It then launches byte-identical disposable copies beside deliberately empty portable state roots. One proves first-use Muse selection, New Run selection persistence across a real desktop restart, and favorites-first grouping without reading or mutating the user's real `pi-wizard-data`. Another starts an ordinary persisted fake-Pi run through the production New Run command, streams reasoning/final text into Live activity, deliberately leaves `get_state.messageCount` stale, and proves the final persisted answer still moves automatically into Conversation through live `get_entries` session-sync revision. That same packaged run verifies verbatim prompt rendering, sanitized Markdown/code highlighting with raw HTML kept inert, native session HTML export, and direct one-shot Bash live/final output while the fake Pi rejects Bash unless `excludeFromContext=true`; command output must not create additional conversation rows. It then starts a second long-lived direct Bash request, reloads the WebView, and proves authoritative hydration still marks the dashboard as command-running, withholds Close and conflicting composer/session mutations, keeps the command owner/status visible, and retains a working Cancel command that resolves the backend request. This is the release boundary that catches Tauri capability, CSP/layout, transcript-handoff, native-utility, and preference/UI integration mistakes which source-only or process-alive checks cannot detect.

Runtime diagnostics tests prove the snapshot is explicit/pull-only, serializes only bounded counter data, reports exact process ownership and bounded per-run state/backlog values, counts delivered UI events without retaining them, and uses a fixed recent RPC traffic window that decays to zero without a timer. Desktop tests prove active Git-review and session-catalog job counts come from the existing job owners, while the renderer source contract keeps diagnostics behind an explicit refresh action and development long-task observation behind `import.meta.env.DEV`.

The core also holds a Ready fake-Pi run steady after startup and proves there is no periodic dirty wakeup or runtime-revision churn during the observation window. This is the deterministic idle-CPU contract: the manager waits on owned channels/deadlines instead of polling. OS/WebView resident-memory, first-paint, and key-to-paint targets in `ARCHITECTURE.md` remain platform measurements rather than fake precision in a repository unit test; bounded state/backlog limits and the explicit diagnostics surface provide the regression evidence needed to investigate those measurements.

Use `quick` for ordinary core/frontend edits when it proves the changed contract; use `standard` for routine cross-surface completion or host/configuration changes; use `full` after changes to persistence bounds, packaging, large-history/diff behavior, or desktop build configuration.

Focused Rust tests use Cargo package/test filters directly when diagnosing one owner. The repository currently has no live-Pi test in routine verification because credentials, installed providers, and a mutable external Pi installation are not deterministic test dependencies.

An optional installed-Pi compatibility smoke is repository-owned and intentionally remains outside `quick`/`standard`/`full`:

```powershell
python tools/smoke_live_pi.py
```

It launches Pi only as `--mode rpc --no-session --no-context-files --no-extensions --no-approve --offline`, sends `get_state`, `get_available_models`, `get_available_thinking_levels`, `set_auto_retry(false)`, and a harmless empty `clear_queue` capability probe, bounds captured output/deadlines, verifies that no session file was created and that Pi returned at least one selectable model, then exits on stdin EOF. It never sends `prompt`, so it does not create a provider model request. `clear_queue` success is reported as supported; an exact unknown-command response is reported as unavailable rather than failing the smoke because current stable Pi 0.84.3 has this documented/runtime skew. Other `clear_queue` rejections still fail. Optional `--provider`, `--model`, and `--thinking` arguments can verify explicit launch selection without prompting the model.

`quick` runs `tools/test_smoke_live_pi.py` with Python bytecode writes disabled against disposable Python subprocesses to prove the optional smoke's parser, rejection handling, deadline termination, and streaming output cap without requiring an installed Pi. Python cache artifacts are ignored so verification never becomes a source-tree cleanliness requirement.

## 2. Deterministic Pi boundary tests

The core suite exercises framing, parsing, encoding, request correlation, bounded projections, lifecycle state, and a deterministic fake Pi subprocess that speaks through the same newline-delimited async transport used by production.

It should deterministically exercise:

- normal request/response;
- first-class `steer` and `follow_up`, including bounded image payloads;
- prompt-during-streaming behavior where Pi requires `prompt.streamingBehavior`;
- streaming text/thinking/tool-call blocks keyed by content index, including interleaving, sparse indices, aggregate block/byte ceilings, malformed known sub-events, and forward-compatible unknown sub-events, with `message_end` as the authoritative completed message;
- direct RPC `bash_execution_update` chunks correlated to their exact originating request ID, including bounded preview behavior when streamed output exceeds the final response;
- current direct Bash `excludeFromContext` request shape and final exit/cancel/truncation/full-output-path metadata;
- direct Bash admission is execution-root exclusive with model/session mutations and Close, while read-only probes/export and `abort_bash` remain allowed; the reciprocal idle-prompt regression holds ownership from successful prompt write through authoritative `agent_start` so a second prompt or Bash cannot enter that pre-event gap;
- tool start/update/end through typed accumulated-output projection;
- accumulated rather than delta tool output;
- steer/follow-up queue updates;
- `clear_queue` returning queued text before user-facing Stop sends abort;
- dedicated recovered-queue message/byte ceilings before queue strings are cloned into retained Stop state;
- `queue_update` text is copied only after those same message/byte ceilings pass, retained privately inside the controller rather than `RunRecord`/hydration, and updated from Pi's complete user steering/follow-up arrays;
- explicit `clear_queue` rejection may recover that latest bounded event snapshot only while exact-process termination is mandatory; clear-queue timeout/malformed-accepted paths do not reuse it because side effects are uncertain;
- an end-to-end fake-Pi fixture matching current stable 0.84.3's unknown `clear_queue` behavior proves Stop terminates the owned process, restores the last observed user steering/follow-up text ahead of the existing draft, and cannot leave continuation work running;
- Stop prepending recovered steering/follow-up text to existing unsent draft text without overwriting it, including non-destructive overflow failure;
- abort success, rejection, and no-response deadlines;
- provider auto-retry start/end event projection, `abort_retry` Stop during retry delay, normal `abort` after the retry attempt restarts, and exact-process escalation if retry cancellation is rejected;
- summarization-retry scheduling/attempt/finish projection plus exact-process Stop escalation because Pi exposes no dedicated summarization-retry abort RPC;
- active-compaction Stop behavior, where queues are still recovered but the app never pretends ordinary RPC abort can cancel compaction;
- typed compaction start/end reason, aborted, `willRetry`, and bounded optional error projection; overflow retry must remain Pi-owned and never cause client-side prompt replay;
- manual-compaction request barriers before `compaction_start` arrives;
- `get_state` reconciliation after a missed event/hydration gap, including recovery of Working state after a missed `agent_start`;
- one-shot quiet-stream advisory scheduling while Ready/Working, no repeat wakeup during unchanged silence, first-event clearing/rearming, exclusion of explicit retry/summarization/dialog waits, and preservation of the separate idle-no-periodic-work invariant;
- retained `get_state` state is byte-bounded independently of the transport frame, session identity has its own cursor-size ceiling, and incremental session-name events cannot bypass the retained-state budget;
- `get_entries(since)` append synchronization, branch/leaf movement, and unknown-cursor resync;
- a newest zero-message session whose Pi-advertised JSONL path does not exist yet is accepted only as an empty latest page before live synchronization has observed a persisted cursor; the same missing path is rejected once messages are reported, once a persisted cursor has been observed despite a stale zero message count, or for older-page navigation;
- empty history bootstraps live `get_entries(null)`, and a first-turn regression proves the null cursor advances to the first persisted entry at settlement; ordinary persisted latest pages still bootstrap only after exact run/session/path revalidation, and a later settled-turn regression proves the live session-sync revision advances even when no new `get_state` arrives and `messageCount` remains stale;
- `agent_settled` removes any orphan active-tool preview that missed its normal tool-end display event and conservatively advances change invalidation, while direct Bash remains owned by its independent request lifecycle;
- session state changes;
- `new_session`, `switch_session`, `fork`, and `clone` returning `success: true` with `data.cancelled: true` without changing the local session binding;
- accepted session replacement queues authoritative `get_state` before releasing its original waiter so the new session/draft identity is reconciled before later writer commands;
- persistent session replacement forces old-session draft durability before Pi sees the replacement; save failure/deadline keeps the old Pi session/draft binding and later work cannot overtake the transaction;
- extension UI requests;
- malformed JSON/framing;
- oversized line/payload rejection;
- child crash before and during a turn;
- stderr bursts;
- launch executable/environment identity mismatch;
- delayed response and backpressure;
- unsupported/unknown optional events.

High-frequency `message_update`, `bash_execution_update`, and `tool_execution_update` fixtures must additionally prove that transient updates do not directly invoke app-owned durable persistence. Semantic end/invalidation boundaries may schedule bounded persistence separately.

Automation tests additionally prove that saved chains round-trip through atomic bounded persistence, malformed/unsupported chain state quarantines independently, failed validation never mutates the existing catalog, and execution snapshots retain only bounded chain-step metadata plus UTF-8-safe prompt previews. Automation execution state contains no supervisor lifecycle fields. Desktop contracts distinguish catalog from execution invalidation, suppress identical execution updates, keep catalog hydration off ordinary startup/runtime refresh paths, preserve working workers when a chain is cancelled, and use the full compact execution UUID in generated worktree branch/path identity so two UUIDv7 executions sharing the same timestamp prefix cannot collide. The scheduler consumes the runtime manager's semantic state-change channel rather than renderer dirty/token traffic, and a live-limit change wakes capacity-blocked orchestration; the full eight-run fixture establishes the shared concurrency ceiling. Manual and automated starts remain subject to the same backend admission, launch serialization, exact execution-root ownership, and explicit provider/model/thinking launch selection. Worker completion tests cover both the backend-only assistant `message_end` generation and observed-real-activity fallback for user Stop/abort turns; the generation is explicitly absent from serialized renderer hydration and completion detection must not require `get_session_stats` probe traffic. Scheduler failure preserves already-running worker sessions while marking never-started steps explicitly failed. Desktop integration tests exercise the complete scheduler against real child-process RPC and disposable real Git repositories: a sequential local chain must launch one distinct Pi child per prompt, continue after an isolated prompt rejection, and release all live capacity; a four-step worktree chain must actually reach two simultaneous workers and retain four unique verified worktrees; generated worktree-parent creation must occur before the first Git mutation, while a proven non-mutating creation failure discards its recovery intent.

Supervision tests are separate from Automation tests. They prove the supervisor has its own coordinator/event/IPC state, is counted as one ordinary live Pi run, accepts a bounded set of registered `ProjectId` targets, rejects overlapping active supervision sets, and reacts only to semantic idle/result transitions. An already-idle run is considered once when supervision starts; each later `(session-replacement generation, session identity, assistant message_end generation)` is considered once when the run returns idle; a deliberate no-op and unrelated runtime wake cannot spin without another authoritative assistant result. Direct Bash removes an otherwise-idle worker from actionability; the real-child race fixture starts Bash after the supervisor turn begins and proves the autonomous directive is skipped, the worker remains untouched while Bash owns the execution root, and Bash completion causes the same deferred idle generation to be reconsidered. The workflow fake Pi records any `get_session_stats` call, and continuous/multi-project integration asserts that neither workers nor the supervisor worktree receive such probe traffic from orchestration. The session-replacement generation is backend-only/serde-skipped and advances on an accepted `new_session`/`switch_session`/`fork`/`clone` response before post-replacement `get_state` reconciliation, so stale autonomous decisions are invalidated even while hydration still carries the old session ID. The supervisor receives only bounded project/run/status/last-result summaries plus an optional bounded Automation-prompt playbook, rejects unknown/self/duplicate/invalid directives, and freshly revalidates targets before Send/Steer/Follow-up/Stop. Any directive crossing a session-version boundary is skipped; Send/Stop additionally require the same assistant-message generation that triggered the decision. Race fixtures cover an active manual takeover, a newer manual result that has already settled back to Idle, and a deliberately delayed post-switch `get_state` where the old session ID remains visible but the replacement generation has already advanced. Truly unknown/ineligible targets remain invalid. Stop is allowed without a message and uses exact runtime lifecycle ownership; other actions require bounded text and state-compatible targets. Explicit user Stop during the supervisor model turn prevents later worker directives and leaves the completed-decision counter unchanged. Stop requested before spawn is `Stopped` rather than `Failed`; once a supervisor child is registered its exact RunId is published before readiness completes, and a startup-stop fixture proves Stop terminates that exact Starting child. Each completed decision stores only one bounded user-facing `lastDecision` summary rather than retaining raw supervisor response history. The production request is continuous (`maxCycles: null`) while optional finite cycle bounds remain testable, terminate immediately after the last allowed completed decision, and every supervisor turn has a deadline. The desktop multi-project fixture starts two ordinary workers under two distinct registered ProjectIds, settles both independently, starts one supervisor over both, observes a real continuation directive arrive at each worker in the same cycle, verifies the bounded last-decision summary, and proves both workers remain alive afterward. A continuous fixture proves one continuation result triggers exactly one later no-op decision and that another semantic wake cannot create a third cycle.

Custom-model catalog tests prove bounded provider/model/name validation, atomic round-trip, duplicate identity replacement, corruption quarantine, aggregate-byte limits, and merging where Pi-discovered metadata wins over a matching custom identity. Model-preference tests separately prove that a fresh preference store defaults New Run to `opencode-go/muse-spark-1.2-contributor`, an explicit Pi-default selection persists as null, the last explicit New Run model and bounded favorite identities survive reopen, schema-1 live-run preferences migrate to schema 2 without losing their run ceiling, and failed atomic writes do not mutate in-memory preference state. A desktop child-process regression probes multiple fake Pi models through the dedicated model-catalog operation without depending on thinking-level or Stop-compatibility probing. Renderer contracts prove the model picker is reusable by New Run, Automation, and Supervision, loads Pi-discovered choices globally, retains Pi default as an explicit option, groups available favorites ahead of the ordinary model list, remembers New Run selection without clearing it on project/trust/context changes, and allows custom provider/model entries without storing credentials. Model-catalog discovery is a separate IPC/probe from secondary launch-option validation so those secondary failures cannot erase an already valid Pi model list.

Attachment tests must cover invalid base64, invalid MIME, count limit, per-image decoded-byte limit, aggregate decoded-byte limit, final encoded RPC limit, revalidation of restored attachment data at the backend boundary, image-only submission, generation changes while a submission is pending, restart restoration, and selected-model image capability. An explicitly text-only Pi model must reject image submission without clearing the draft; omitted model-input metadata must remain a compatibility/unknown state rather than being guessed false.

Manual-compaction tests must prove the desktop uses Pi's native `compact` RPC, the client-side compaction barrier prevents a composer race before `compaction_start`, and successful completion reconciles authoritative state rather than relying on an optimistic button state. Automatic-compaction tests must likewise prove the desktop sends Pi's native `set_auto_compaction`, treats it as session-visible state behind the manual-compaction barrier, and reflects only the later authoritative `get_state` value. Automatic-provider-retry tests must prove `set_auto_retry` uses Pi's native wire command and is blocked by manual compaction, while the UI does not fabricate a recoverable current-value mirror because `get_state` omits that flag.

Extension UI tests additionally cover bounded keyed `setStatus`/`setWidget` state, replacement accounting, clearing/releasing budget, title byte ceilings, fire-and-forget methods never entering pending-dialog ownership, and timed dialog requests becoming non-actionable after Pi's timeout window rather than leaving zombie prompts.

The fixture tests Pi Wizard's adapter and state machine. It must not become a second implementation of Pi behavior.

Live Pi smoke tests should exist separately and be optional where they require installed Pi/provider credentials.

## 3. Process lifecycle tests

Prove:

- one owner per child;
- exact PID/process identity use;
- graceful abort path;
- user-facing Stop clears/preserves queued messages before aborting;
- bounded abort and termination escalation deadlines;
- abort rejection/timeout cannot produce a false Idle/Stopped state;
- termination uncertainty quarantines the run and revokes further RPC writes;
- UI navigation has no lifecycle side effect;
- failed spawn leaves no fake running state;
- app shutdown has an explicit outcome for active children;
- a process exiting while its view is unmounted still updates runtime state;
- stdout and stderr ownership cannot deadlock the child through unread pipes.
- stderr is continuously drained into a fixed byte budget even when the renderer never opens diagnostics;
- exact child-handle termination is used for escalation; PID/executable-name lookup is never the lifecycle authority, and the Windows `taskkill.exe` exact-tree fallback uses `CREATE_NO_WINDOW` so hard-stop escalation cannot flash a console.
- standard Windows npm Pi installations resolve to direct Node + Pi CLI entrypoint invocation so the long-lived runtime has no command-shell wrapper; unresolved `.cmd`, `.bat`, and `.ps1` live launchers are rejected before spawn rather than retained as background shells;
- the Windows desktop establishes kill-on-close process containment before runtime children can be launched so abrupt desktop termination cannot orphan inherited descendants;
- a destructive Windows Job Object regression assigns a long-lived child to a kill-on-close job, drops the job handle, and requires the child to terminate within the bounded test deadline;
- stderr finalization is deadline-bounded even when a descendant inherits and retains a pipe handle;
- normal diagnostic EOF is distinguished from deadline-truncated diagnostic finalization;
- manager startup without an active async runtime is an explicit error rather than a panic;
- OS spawn remains `Starting` until the designated startup `get_state` handshake succeeds; startup RPC silence/rejection is deadline-bounded and cannot yield a false Ready/composer-available run;
- explicit Stop and application shutdown supersede/cancel the startup readiness timer rather than allowing it to relabel intentional termination as a protocol failure;
- the persistent manager survives repeated renderer-style hydration without restarting the child;
- normal Stop recovers queue text, settles the turn, and leaves the same child reusable;
- idle-run Close refuses active agent work, forces the current session draft through the bounded durability boundary before termination, terminates only the exact owned child tree/group, and releases execution-root ownership afterward;
- failed Close draft persistence does not kill the child or discard the unsaved draft;
- terminal-run Dismiss refuses live runs and removes only run-scoped hot state while preserving Pi session JSONL and session-scoped draft authority;
- global shutdown waits for all children owned at shutdown start and reports quarantine rather than silently abandoning uncertainty;
- extension dialog responses use the priority control plane and clear exact backend ownership after successful stdin write;
- high-frequency stream wakeups are coalesced per dirty run and bounded drains expose whether more work remains.

Renderer/process recovery tests should prove that a renderer reload does not restart or retarget a still-running Pi child, and that a versioned hydration snapshot may be applied twice without duplicating runtime state. Ordinary hydration must preserve transient backlog delivery and re-wake a listener that subscribed after an earlier dirty signal. Only explicit per-run recovery may discard stale queued frames, and it must not erase pending extension editor text that is outside the normalized event snapshot.

Desktop adapter tests additionally prove that the Tauri-owned runtime manager can hydrate under Tauri's async runtime, repeated canonical project launches share one `ProjectId`, reopening the persistent desktop state root preserves that ID across manager lifetimes, and extension/composer/project-launch payloads deserialize with the exact camelCase/snake_case contracts used by the renderer.

## 4. Session tests

Use generated/local fixtures rather than real user sessions.

Cover:

- current Pi JSONL structure needed for projection;
- truncated final line for read-only history/catalog tolerance;
- writable Resume refusal for both valid-but-unterminated and malformed unterminated JSONL tails, with byte-for-byte proof that Pi Wizard did not repair/mutate the authoritative file;
- skill-expanded first-user-message preview/search normalization that strips only Pi's explicit `<skill ...>...</skill>` generated wrapper and preserves trailing user arguments or a bounded skill placeholder;
- unknown entry type;
- branching/tree relationships;
- very large session with bounded latest-page open;
- prepend older page while preserving scroll anchor;
- session-catalog continuation reaches older sessions without skipping a page-boundary entry, remains bounded per page, and the 1,200-session scale fixture can traverse the complete static catalog;
- session-catalog cursors are bound to the canonical project, resolved directory, normalized query, exact candidate position, and a snapshot digest; external candidate changes, cross-query reuse, malformed cursors, or missing cursor positions fail closed instead of mixing observations;
- the stateless catalog may enumerate lightweight path/mtime metadata across the resolved directory on each explicit page so it can preserve exact newest-first ordering and detect external Pi/CLI changes; retained candidate metadata plus detailed header/preview reads remain independently bounded, and persistent indexing is added only if larger measurements justify it;
- renderer paging errors retain an explicit restart-from-newest path;
- derived index deletion/rebuild without data loss.
- active-session incremental reads through `get_entries(since)` using the last stable entry ID;
- unknown/stale entry cursor widens to an explicit resync rather than silently skipping entries;
- leaf/branch movement remains distinguishable from append-only continuation;
- `get_messages` is never required to hydrate an entire long live session for routine synchronization.

Never mutate a real `~/.pi/agent` directory in automated tests.

Project/registry fixtures cover:

- canonical path registration under a stable app `ProjectId`;
- reopening the exact canonical path succeeds;
- a different existing path never substitutes for the registered project;
- missing/renamed project roots become detached rather than falling back to global/another project;
- relocation requires an explicit operation and preserves the app `ProjectId`;
- symlink/alias paths are canonicalized before duplicate-project decisions where the platform permits the fixture;
- malformed/truncated/unsupported-version app-owned project/catalog state is quarantined or rebuilt without touching Pi JSONL sessions;
- one corrupt derived-state domain cannot prevent safe startup of the shell or access to intact authoritative sessions.
- failed project registration/relocation persistence does not mutate in-memory identity/indexes;
- duplicate roots/IDs and registry byte-limit violations quarantine rather than silently selecting one entry.

Launch/trust tests cover Pi saved/default trust inheritance, one-run approve/ignore overrides, and the fact that ignoring protected project resources does not implicitly add `--no-context-files`. Extension discovery is an independent launch policy with an explicit one-run disabled path for local/worktree/recovered-worktree/resumed-session launches, and the launch-options probe itself is extension-free so a broken installed extension cannot block access to recovery settings. The desktop launch contract also proves provider/model must be supplied as an exact pair, startup model/thinking/context-file selections are applied before Pi spawn for local/worktree/recovered-worktree runs, resumed sessions preserve explicit context-file/extension policies, and the launch-options probe uses an ephemeral Pi RPC process rather than creating a saved session. That same already-owned probe sends `clear_queue`: accepted bounded empty output means reusable Stop is available; an unknown-command rejection becomes `clearQueueSupported: false` so the launcher warns about exact-process Stop fallback without adding another startup/background child. Custom session directories are canonicalized before spawn. A separate optional live smoke may verify the installed Pi's current model/thinking discovery, extension-free/no-session launch flags, native `set_auto_retry`, and `clear_queue` support state without making live Pi/provider prompts part of routine deterministic verification.

Project-resource preflight tests use metadata-only fixtures and prove that the documented `.pi/settings.json`, `.pi/{extensions,skills,prompts,themes}`, `.pi/{SYSTEM.md,APPEND_SYSTEM.md}`, and current/ancestor `.agents/skills` locations are detected, while a bare `.pi` directory plus `AGENTS.md`/`CLAUDE.md` context files do not become protected trust resources. The preflight never resolves or mutates Pi's saved trust decision.

Desktop environment tests use controlled fixtures instead of the developer's shell and prove explicit executable paths take precedence, configured PATH beats desktop/probe PATH, shell-probe fallback becomes one coherent spawn environment, Pi and Git resolve under that same environment, resolved diagnostics do not serialize secret values, and missing Pi produces actionable failure instead of implicit current-directory/global fallback.

Backend renderer-backlog tests prove repeated assistant/tool/Bash updates coalesce by exact key, keyed replacement moves behind newer semantic state, display-only frames are evicted before semantic transitions, semantic-only exhaustion becomes an explicit rehydration condition, oversized display frames are dropped, and fixed per-frame overhead prevents zero-payload frame-count growth.

Composer draft tests use disposable app-state fixtures and cover:

- one draft per session/new-session identity;
- navigation cannot carry one session's draft into another;
- monotonically increasing generations;
- completion of an older save cannot mark a newer edit durable;
- persistence failure is retained as Failed and retryable;
- retry saves the current generation, never stale bytes;
- session switch/application exit bounded flush behavior;
- session replacement waits for the old session's dirty draft to become durable before sending Pi's replacement command;
- failed flush remains visible rather than being treated as success;
- extension `set_editor_text` mutations use the same draft owner and generation path;
- draft byte ceilings reject oversized updates without destroying the prior draft.
- a pre-session pending draft migrates only when the first authoritative Pi session identity arrives;
- a later session switch selects a different draft without carrying the previous session's text, and switching back restores the previous in-memory draft;
- the in-memory session-draft record count is bounded; at capacity only unowned Saved records are evicted, Dirty/Saving/Failed records are retained, and lack of a safe candidate fails closed rather than allocating without limit;
- a stale asynchronous load completion cannot recreate an already-evicted unowned draft record outside the cache ceiling;
- after a Saved session is evicted, revisiting it clears stale load-attempt bookkeeping and reloads its exact persisted draft rather than presenting an empty baseline;
- persisted draft restoration gates edit/submit until load success/absence/failure is known;
- hydration schema v10 exposes the currently active backend draft, durability/error, restore-pending flag, composer ownership, immutable worktree identity, backend-owned run start/terminal timestamps, monotonic change-invalidation revision, and bounded transient recovery projections rather than relying on renderer component state.

## 5. Worktree tests

Create disposable local Git fixtures and prove:

- exact repository root, current source branch, and base commit are resolved before creation;
- stale/default branch labels cannot substitute for the captured base commit;
- unique branch/worktree creation is transactional and verified after creation;
- canonical cwd persisted before child launch;
- two runs cannot be accidentally assigned the same isolated worktree;
- live worktrees are never pooled or reassigned between runs;
- Git operations use the run's canonical worktree identity rather than current UI navigation state;
- dirty worktree cleanup is rejected unless explicitly handled;
- explicit cleanup succeeds only for an exact, clean app-created worktree whose `HEAD` still equals the captured base, removes the worktree without force, deletes the branch with expected-old ref identity, and retires the journal only after a fresh both-absent proof;
- cleanup refuses clean worktrees containing task commits as well as dirty/untracked worktrees, preserving their path/branch and recovery record;
- cleanup is serialized against desktop run starts so a worktree cannot become live between the active-run proof and Git removal;
- navigation/project switching cannot change a run's root;
- failure halfway through creation rolls back only proven task-owned empty resources or leaves a classified orphan/recovery record.
- a durable recovery intent is committed before the first mutating Git command and survives a complete desktop-runtime reopen;
- successful creation upgrades the same recovery record with the verified canonical worktree/execution root, requested branch, and captured base commit before Pi is spawned;
- an absent recovery is retired only when a fresh probe proves both the requested branch and path absent; branch-only, path-only, wrong-repository, or wrong-branch states remain explicit and untouched;
- restart recovery accepts legitimate commits descended from the captured creation base and dirty task work, while rejecting rewritten ancestry or a worktree switched to another branch;
- a created recovery record can be retired from app-owned state only after Git resources were independently removed and a fresh absence probe proves that fact; the recovery registry itself never deletes a branch or worktree;
- worktree-registry malformed/oversized/duplicate state is quarantined independently without mutating Git or project/session authority;
- Windows canonical/verbatim path forms compare as one filesystem identity internally while Git-for-Windows receives a compatible subprocess path form.

## 6. Git review tests

Cover:

- clean/dirty status;
- renamed/deleted/binary files;
- binary changes return an explicit metadata marker without sending a binary patch into renderer text state;
- large files and huge change sets;
- payload/file/page/scan caps;
- streamed text pages concatenate to the exact Git patch without retaining the whole patch at once;
- continuation cursors are path-bound and SHA-256-prefix-bound, and a mutation to any earlier patch byte makes a later cursor stale instead of mixing two repository observations;
- small legal scan budgets still allow early pages, while later prefix rescans stop explicitly at the scan ceiling instead of growing without bound;
- timeout/cancellation;
- invalidation after known changes;
- every completed Pi tool and direct RPC Bash request advances the per-run change revision regardless of tool name/outcome, because partial or extension-defined mutations cannot be classified safely;
- a review summary/detail captured at an older change revision is marked stale, file detail is discarded, and refresh remains explicit rather than becoming background Git polling;
- a newer review-summary request invalidates ownership of older in-flight file detail so stale completion cannot repopulate the detail pane;
- no Git command from passive sidebar rendering;
- no synchronous diff work in the renderer path.

## 7. Renderer behavior tests

Automated UI tests should focus on contracts rather than pixel trivia:

- visible timeline row count remains bounded as history grows;
- scrolling away from either conversation/live edge disables forced autoscroll; when pinned to bottom, persisted final answers and live reasoning/tool/command/streaming output follow new content automatically; a message-count or live session-sync revision change that races a latest-history read cannot be marked already loaded merely because hydration advanced while that file read was in flight; the session-sync path independently covers a normal settled answer when `get_state.messageCount` remains stale;
- prepending history preserves viewport anchor;
- while a model turn is active, the live pane always states that the model is active even when no tool/reasoning delta is arriving; a prolonged quiet-stream advisory remains non-authoritative and never triggers retries or probes automatically;
- the persisted upper conversation contains only verbatim user prompts and final assistant answers, applies sanitized Markdown/code rendering only to the final assistant output, keeps fork actions in Session Tree, and never reintroduces reasoning/tool/Bash protocol rows; the always-mounted lower live pane owns bounded transient reasoning, active tool output, direct Bash output, and the in-progress answer, then drops completed reasoning/answer text after settlement rather than duplicating the final output above;
- abort/steer/follow-up controls remain reachable during streaming;
- pending request UI is keyed to run/request identity;
- dialog-local state resets when request ID changes and stale requests cannot remain actionable;
- session switch does not mount complete old history;
- initial hydration failure reaches actionable Retry/Relaunch instead of an indefinite spinner;
- initial hydration can be retried in place without restarting the renderer or backend-owned Pi children;
- hydration snapshots are idempotent and version/revision aware;
- hydration result application is request-ordered so an older async result cannot overwrite a newer recovery snapshot;
- renderer reload rebuilds from backend state without restarting live runs;
- same-ID extension-dialog hydration refreshes preserve partially typed input/editor state, while a new request ID resets dialog-local state;
- stale/expired extension dialog responses are rejected by exact backend ownership and trigger authoritative refresh rather than leaving a zombie actionable dialog;
- runtime listeners are installed before initial hydration so startup has no lost-wakeup window;
- the root renderer waits on an explicit `runtime_backend_ready` command before mounting the main App; only that bounded bootstrap handshake retries during Tauri initialization, then App installs runtime listeners before initial hydration and ordinary runtime updates remain event-driven with no polling loop;
- per-run dirty wakeups trigger bounded pulls, not interval polling or raw Pi event forwarding;
- overlapping dirty signals for one run do not create overlapping drain loops;
- hydration demand survives multi-batch bounded drain continuation so a display/semantic change in an early batch cannot be forgotten before backlog completion;
- IPC enum variant and struct-variant field casing is regression-tested against the TypeScript camelCase contract;
- repeated renderer failures trip a bounded crash-loop breaker instead of auto-reloading forever;
- the crash-loop policy itself is executable in the repository: crash counts 0/1 permit the two automatic reloads, count 2 stops automation, unavailable session storage disables automatic reload, malformed counts recover to zero, and displayed failure detail remains bounded;
- live-run admission changes remain backend-owned, enforce the configured maximum, reject excess starts before spawn, never terminate already-active runs when the ceiling is lowered, and persist atomically for later desktop launches;
- sidebar/dashboard ordering keeps Needs Attention first, then working/live runs, with retained terminal rows behind current work and newer UUIDv7 runs first inside each group;
- Recent Sessions reuses the same bounded catalog/resume component as New Run, performs no catalog RPC merely by mounting or switching projects, and invalidates an in-flight page response when the selected project changes;
- the global Needs Attention view renders the same exact request-ID-owned dialog controls used inside a run, orders finite remaining timeout windows before non-expiring requests, and resolves/cancels through the backend owner rather than rebinding request identity to the selected view;
- extension dialog timeout metadata is rendered as the bounded remaining value from the last authoritative sync, explicitly labeled as such, with no countdown interval that could turn passive UI into polling;
- navigation exposes current-page semantics, and selected run/sidebar/dashboard/attention identities derive the registered project from backend `ProjectId` records rather than guessing from execution-root strings;
- dashboard state consumes the already-bounded hydrated compaction/steer/follow-up/live-tool projections to distinguish working, queued, compacting, and attention states; dashboard Stop remains available for both active work and compaction without adding a polling timer;
- dashboard elapsed lifetime derives from backend-owned start/terminal timestamps; one minute-level renderer clock exists only while live runs exist, terminal durations freeze, and no backend/repository polling is introduced;
- a Git review summary explicitly loaded by the user may contribute only its already-known file count/truncation bit to dashboard metadata; dashboard mounting itself must never launch Git review work;
- failed and quarantined selected runs expose the backend-bounded failure kind/detail, exit code when known, truncation marker, and explicit termination-uncertain wording rather than only a state badge;
- an unexpected child exit with a known OS status preserves that status through `RunMutation::ProcessFailed` into hydration, while failure classes without an observed OS exit do not manufacture one;
- execution-folder opening accepts only a RunId from the renderer, derives the canonical execution root from backend hydration, and passes that root as one platform-opener argument without shell interpolation;
- a failed admission-preference write leaves both the preference owner and runtime manager at their previous ceiling;
- corrupt/out-of-range/oversized admission preferences are quarantined to the configured default without hiding an intact project registration or worktree recovery record;
- draft persistence failure is visible without blocking unrelated navigation;
- failed draft IPC synchronization cannot be overwritten by a later hydration; explicit edit/submit retries the newest local value;
- live assistant/reasoning/tool/direct-Bash presentation consumes only backend-bounded snapshots; reasoning remains continuous across Pi thinking/tool/thinking cycles while active, but settled assistant/reasoning blocks are no longer rendered in the lower pane once the authoritative agent-working state becomes false;
- persisted history parsing still preserves Pi reasoning separately in the backend read model while the renderer deliberately projects only user/final-assistant rows, so reopening a session cannot reintroduce execution-noise rows;
- session HTML export uses Pi's native RPC result and rejects an absent/oversized returned path; the one-shot command uses direct Bash with `excludeFromContext=true`, bounds final output before renderer retention, exposes cancellation, and invalidates Git review on completion without creating a PTY or shell-history owner; core race tests prove the accepted-Prompt/pre-`agent_start` handoff excludes a second Prompt and Bash, active Bash excludes composer/raw Prompt/compaction/session mutation/Close while leaving read/cancel operations available, and ordinary mutation resumes after completion; renderer contracts require authoritative active-Bash hydration to keep model/session controls and Close unavailable across reload while cancellation stays reachable; packaged-WebView coverage proves both utilities complete through the real UI, streamed Bash appears only in Live activity, and Bash output does not create model-conversation rows;
- hydration snapshots with any schema other than the renderer's supported v10 contract are rejected before application instead of partially applying incompatible state;
- Run details, Changes, and Session Tree are mutually exclusive, closed-by-default inspector mounts; opening Changes triggers the bounded summary request, switching away cancels in-flight review work, and Session Tree cannot retain a late result after unmount;
- bounded Pi compaction/retry/summarization/extension-error/quiet-stream notices are hydrated rather than reconstructed from component memory; overflow `willRetry` explicitly says the desktop does not resubmit the prompt;
- the native automatic-retry control remains an explicit command action and explains that current state cannot be recovered from `get_state`;
- Stop remains reachable while the agent is working, compacting, or in a modeled retry state and reports recovered-queue merge failure without claiming the text was restored;
- slash-command suggestions support bounded Arrow Up/Down wrapping and Enter staging, keep the selected row visible via nearest scrolling, and do not create an app-owned input-history store;
- keyboard focus and accessibility labels are correct.

Desktop portable-state tests prove repository builds resolve `pi-wizard-data` outside `target`, standalone executables use an executable-sibling root, legacy `target\debug|release\pi-wizard-data` is preferred over legacy AppData during migration, nested legacy AppData state migrates intact when needed, and an existing current portable root is never overwritten by migration input. The shared root is consumed by the existing prompt-chain, custom-model, project/worktree, preference, and draft stores, so those domains survive clean rebuilds without compile-time configuration.

## 8. Performance regression fixtures

Performance evidence should be executable and retained as numeric summaries, not screenshots alone.

Minimum scenarios:

1. app shell warm start;
2. idle CPU for a fixed quiet window;
3. eight simulated streaming runs while continuously generating composer input;
4. opening a 10k+ message/part session;
5. opening a 25-50 MB JSONL session to its newest page;
6. navigating among hundreds/thousands of session metadata rows;
7. inspecting a very large diff file-by-file;
8. sustained tool-output burst that exceeds the display preview cap.
9. sustained multi-run token/tool streaming while draft/catalog persistence is intentionally slow, proving disk latency cannot serialize unrelated streams;
10. renderer reload during an active run followed by bounded hydration recovery.
11. startup with malformed/disposable derived project/catalog state while authoritative Pi sessions remain available.
12. launch from a minimal GUI-style PATH while the configured environment resolver discovers the expected Pi/Git/toolchain binaries.
13. page through a multi-megabyte tracked diff while recording current-page bytes and prefix-scan high-water marks.

Record at least elapsed time, peak/app-owned memory where practical, IPC backlog high-water mark, and mounted timeline rows.

Performance tests must fail on unbounded growth conditions even if absolute hardware timings vary.

## 9. Manual desktop checks

Manual review remains necessary for:

- native window/menu behavior;
- system theme and scaling;
- IME/text composition;
- accessibility tree and keyboard traversal;
- high-DPI diff/Markdown rendering;
- Windows child/process-tree shutdown;
- GUI-launched Pi sees the intended PATH/toolchain environment on Windows without exposing provider secrets in diagnostics;
- CSP behavior in packaged/static builds and absence of unexpected remote script/content loading;
- installer/uninstaller behavior when packaging changes.

These checks supplement, not replace, deterministic process/protocol tests.

The release configuration check additionally rejects production `'unsafe-inline'` styles, script eval, form submission, missing base/frame restrictions, or loss of the explicit self-only script/style directives. The only inline-style allowance is in Tauri `devCsp`, paired with the exact loopback Vite WebSocket; renderer source contracts keep Session Tree indentation on bounded static classes rather than runtime `style` attributes.
