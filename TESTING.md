# Testing and Verification

This is the verification contract for Pi Wizard. Workspace policy requires local verification. Do not add GitHub Actions.

## 1. Current verification lanes

The workspace manifest exposes repository-owned semantic lanes:

```text
python tools/verify.py quick
python tools/verify.py standard
```

`quick` currently owns:

- Rust formatting check;
- locked deterministic `pi-wizard-core` tests;
- strict TypeScript type checking.

`standard` runs `quick`, then additionally owns:

- locked deterministic `pi-wizard-desktop` host tests;
- workspace Clippy for all targets with warnings denied;
- production Vite renderer build;
- Tauri host compilation as part of the workspace Clippy target graph.

Do not turn these into a generic staircase. Use `quick` for ordinary core/frontend edits when it proves the changed contract; use `standard` for routine cross-surface completion or host/configuration changes. A future `full` lane should be added only when a distinct reproducible contract justifies it.

Focused Rust tests use Cargo package/test filters directly when diagnosing one owner. The repository currently has no live-Pi test in routine verification because credentials, installed providers, and a mutable external Pi installation are not deterministic test dependencies.

## 2. Deterministic Pi boundary tests

The core suite exercises framing, parsing, encoding, request correlation, bounded projections, lifecycle state, and a deterministic fake Pi subprocess that speaks through the same newline-delimited async transport used by production.

It should deterministically exercise:

- normal request/response;
- first-class `steer` and `follow_up`, including bounded image payloads;
- prompt-during-streaming behavior where Pi requires `prompt.streamingBehavior`;
- streaming text/thinking/tool-call blocks keyed by content index, including interleaving, sparse indices, aggregate block/byte ceilings, malformed known sub-events, and forward-compatible unknown sub-events, with `message_end` as the authoritative completed message;
- direct RPC `bash_execution_update` chunks correlated to their exact originating request ID, including bounded preview behavior when streamed output exceeds the final response;
- current direct Bash `excludeFromContext` request shape and final exit/cancel/truncation/full-output-path metadata;
- tool start/update/end through typed accumulated-output projection;
- accumulated rather than delta tool output;
- steer/follow-up queue updates;
- `clear_queue` returning queued text before user-facing Stop sends abort;
- dedicated recovered-queue message/byte ceilings before queue strings are cloned into retained Stop state;
- Stop prepending recovered steering/follow-up text to existing unsent draft text without overwriting it, including non-destructive overflow failure;
- abort success, rejection, and no-response deadlines;
- active-compaction Stop behavior, where queues are still recovered but the app never pretends ordinary RPC abort can cancel compaction;
- manual-compaction request barriers before `compaction_start` arrives;
- `get_state` reconciliation after a missed event/hydration gap;
- retained `get_state` state is byte-bounded independently of the transport frame, session identity has its own cursor-size ceiling, and incremental session-name events cannot bypass the retained-state budget;
- `get_entries(since)` append synchronization, branch/leaf movement, and unknown-cursor resync;
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

Attachment tests must cover invalid base64, invalid MIME, count limit, per-image decoded-byte limit, aggregate decoded-byte limit, final encoded RPC limit, revalidation of restored attachment data at the backend boundary, image-only submission, generation changes while a submission is pending, restart restoration, and selected-model image capability. An explicitly text-only Pi model must reject image submission without clearing the draft; omitted model-input metadata must remain a compatibility/unknown state rather than being guessed false.

Manual-compaction tests must prove the desktop uses Pi's native `compact` RPC, the client-side compaction barrier prevents a composer race before `compaction_start`, and successful completion reconciles authoritative state rather than relying on an optimistic button state. Automatic-compaction tests must likewise prove the desktop sends Pi's native `set_auto_compaction`, treats it as session-visible state behind the manual-compaction barrier, and reflects only the later authoritative `get_state` value.

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
- exact child-handle termination is used for escalation; PID/executable-name lookup is never the lifecycle authority.
- npm/script launchers with descendants are terminated as the exact spawn-time process tree/group rather than only killing the wrapper process;
- stderr finalization is deadline-bounded even when a descendant inherits and retains a pipe handle;
- normal diagnostic EOF is distinguished from deadline-truncated diagnostic finalization;
- manager startup without an active async runtime is an explicit error rather than a panic;
- OS spawn remains `Starting` until the designated startup `get_state` handshake succeeds; startup RPC silence/rejection is deadline-bounded and cannot yield a false Ready/composer-available run;
- explicit Stop and application shutdown supersede/cancel the startup readiness timer rather than allowing it to relabel intentional termination as a protocol failure;
- the persistent manager survives repeated renderer-style hydration without restarting the child;
- normal Stop recovers queue text, settles the turn, and leaves the same child reusable;
- global shutdown waits for all children owned at shutdown start and reports quarantine rather than silently abandoning uncertainty;
- extension dialog responses use the priority control plane and clear exact backend ownership after successful stdin write;
- high-frequency stream wakeups are coalesced per dirty run and bounded drains expose whether more work remains.

Renderer/process recovery tests should prove that a renderer reload does not restart or retarget a still-running Pi child, and that a versioned hydration snapshot may be applied twice without duplicating runtime state. Ordinary hydration must preserve transient backlog delivery and re-wake a listener that subscribed after an earlier dirty signal. Only explicit per-run recovery may discard stale queued frames, and it must not erase pending extension editor text that is outside the normalized event snapshot.

Desktop adapter tests additionally prove that the Tauri-owned runtime manager can hydrate under Tauri's async runtime, repeated canonical project launches share one `ProjectId`, reopening the persistent desktop state root preserves that ID across manager lifetimes, and extension/composer/project-launch payloads deserialize with the exact camelCase/snake_case contracts used by the renderer.

## 4. Session tests

Use generated/local fixtures rather than real user sessions.

Cover:

- current Pi JSONL structure needed for projection;
- truncated final line;
- unknown entry type;
- branching/tree relationships;
- very large session with bounded latest-page open;
- prepend older page while preserving scroll anchor;
- catalog invalidation after external CLI file changes;
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

Launch/trust tests cover Pi saved/default trust inheritance, one-run approve/ignore overrides, and the fact that ignoring protected project resources does not implicitly add `--no-context-files`. Custom session directories are canonicalized before spawn.

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
- persisted draft restoration gates edit/submit until load success/absence/failure is known;
- hydration schema v6 exposes the currently active backend draft, durability/error, restore-pending flag, composer ownership, and immutable worktree identity rather than relying on renderer component state.

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
- large files and huge change sets;
- payload/file/hunk caps;
- timeout/cancellation;
- invalidation after known changes;
- no Git command from passive sidebar rendering;
- no synchronous diff work in the renderer path.

## 7. Renderer behavior tests

Automated UI tests should focus on contracts rather than pixel trivia:

- visible timeline row count remains bounded as history grows;
- scrolling away from live edge disables forced autoscroll;
- prepending history preserves viewport anchor;
- active tool output replaces/updates the correct card;
- abort/steer/follow-up controls remain reachable during streaming;
- pending request UI is keyed to run/request identity;
- dialog-local state resets when request ID changes and stale requests cannot remain actionable;
- session switch does not mount complete old history;
- initial hydration failure reaches actionable Retry/Relaunch instead of an indefinite spinner;
- hydration snapshots are idempotent and version/revision aware;
- hydration result application is request-ordered so an older async result cannot overwrite a newer recovery snapshot;
- renderer reload rebuilds from backend state without restarting live runs;
- same-ID extension-dialog hydration refreshes preserve partially typed input/editor state, while a new request ID resets dialog-local state;
- stale/expired extension dialog responses are rejected by exact backend ownership and trigger authoritative refresh rather than leaving a zombie actionable dialog;
- runtime listeners are installed before initial hydration so startup has no lost-wakeup window;
- per-run dirty wakeups trigger bounded pulls, not interval polling or raw Pi event forwarding;
- overlapping dirty signals for one run do not create overlapping drain loops;
- hydration demand survives multi-batch bounded drain continuation so a display/semantic change in an early batch cannot be forgotten before backlog completion;
- IPC enum variant and struct-variant field casing is regression-tested against the TypeScript camelCase contract;
- repeated renderer failures trip a bounded crash-loop breaker instead of auto-reloading forever;
- draft persistence failure is visible without blocking unrelated navigation;
- failed draft IPC synchronization cannot be overwritten by a later hydration; explicit edit/submit retries the newest local value;
- live assistant/thinking/tool/Bash cards consume only backend-bounded snapshots and visibly report dropped bytes;
- Stop remains reachable while the agent is working or compacting and reports recovered-queue merge failure without claiming the text was restored;
- keyboard focus and accessibility labels are correct.

## 8. Performance regression fixtures

Performance evidence should be executable and retained as numeric summaries, not screenshots alone.

Minimum scenarios:

1. app shell warm start;
2. idle CPU for a fixed quiet window;
3. four simulated streaming runs while continuously generating composer input;
4. opening a 10k+ message/part session;
5. opening a 25-50 MB JSONL session to its newest page;
6. navigating among hundreds/thousands of session metadata rows;
7. inspecting a very large diff file-by-file;
8. sustained tool-output burst that exceeds the display preview cap.
9. sustained multi-run token/tool streaming while draft/catalog persistence is intentionally slow, proving disk latency cannot serialize unrelated streams;
10. renderer reload during an active run followed by bounded hydration recovery.
11. startup with malformed/disposable derived project/catalog state while authoritative Pi sessions remain available.
12. launch from a minimal GUI-style PATH while the configured environment resolver discovers the expected Pi/Git/toolchain binaries.

Record at least elapsed time, peak/app-owned memory where practical, IPC backlog high-water mark, and mounted timeline rows.

Performance tests must fail on unbounded growth conditions even if absolute hardware timings vary.

## 9. Manual desktop checks

Manual review remains necessary for:

- native window/menu behavior;
- system theme and scaling;
- IME/text composition;
- accessibility tree and keyboard traversal;
- high-DPI diff/Markdown rendering;
- platform-specific child shutdown;
- GUI-launched Pi sees the intended PATH/toolchain environment on Windows/macOS/Linux without exposing provider secrets in diagnostics;
- CSP behavior in packaged/static builds and absence of unexpected remote script/content loading;
- installer/uninstaller behavior when packaging changes.

These checks supplement, not replace, deterministic process/protocol tests.
