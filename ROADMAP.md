# Roadmap

The roadmap is intentionally ordered by architectural risk, not feature count. Implementation should prove the runtime and performance boundaries before adding breadth.

## Phase 0: design baseline

Status: **complete**

- research current Pi RPC/session/security behavior;
- research representative desktop harness UX and failure modes;
- establish product boundary;
- select Tauri/Rust + Solid + Pi RPC architecture;
- define persistence, worktree, safety, and performance contracts;
- define verification strategy.

No application code is part of Phase 0.

## Phase 1: runtime foundation

Purpose: establish stable ownership, protocol, lifecycle, bounds, and verification seams before connecting them into a product flow.

Build first:

- Rust workspace with a Tauri-independent `pi-wizard-core` crate and a thin Tauri host;
- static Solid/Vite renderer scaffold with no application-server runtime;
- centralized resource/payload limits;
- opaque typed identities and structured failure types;
- strict bounded LF-only JSONL framing;
- typed Pi outbound commands plus forward-compatible inbound classification;
- first-class Pi steer/follow-up, stable entry cursors, and bounded image payload contracts;
- canonical Pi launch specifications, including saved/default trust inheritance, explicit one-run trust override, separate context-file policy, and optional custom session directory;
- stable canonical project-binding primitives with explicit relocation semantics;
- run lifecycle/state reducer with invalid-transition rejection;
- explicit quarantine state for unconfirmed process termination;
- session-scoped generation-safe draft state independent of renderer lifetime;
- bounded persistent extension status/widget/title projection separate from dialog ownership;
- semantic response outcomes for extension-cancellable Pi session mutations;
- byte-bounded diagnostic and streaming projection primitives;
- packaged renderer Content Security Policy;
- deterministic protocol/state tests that do not require credentials or a live Pi process;
- repository-owned quick, standard, and full verification lanes.

Exit criteria:

- core protocol/state tests are deterministic and fast;
- malformed, oversized, CRLF, split-frame, and Unicode-separator framing cases are covered;
- launch arguments are deterministic; trust inheritance/overrides and context-file loading remain separate and testable;
- a registered project cannot silently bind to another canonical directory when its stored path is missing or stale;
- invalid lifecycle transitions cannot be represented through the public mutation API;
- bounded buffers demonstrate fixed memory ceilings under oversized input;
- attachment and draft ceilings are revalidated at the backend owner;
- old asynchronous draft-save completions cannot mark newer content durable;
- uncertain Stop cannot be represented as Idle/Stopped or accept further RPC writes;
- the Tauri shell and static renderer both build from repository-owned commands;
- framework-specific types do not leak into `pi-wizard-core`.

Do not optimize for an end-to-end demo during this phase. A visible prompt-to-response flow is Phase 2 work after the ownership boundaries above are proven.

## Phase 2: runtime integration and Pi-native UX

Status: **complete for the personal Windows application boundary**. Runtime/history/composer/capability/attachment/session-tree/recovery surfaces, first-class on-demand bounded Recent Sessions navigation, Pi-native retry/compaction/extension recovery projection, writable-session tail protection, extension-free launch recovery, keyboard slash-command navigation, and deterministic fake-Pi compatibility coverage are operational.

Add:

- Pi executable discovery/version probe;
- bounded desktop environment/PATH resolver with explicit configured-path precedence and secret-safe provenance diagnostics;
- one supervised `pi --mode rpc` child using the Phase 1 protocol/lifecycle owners;
- bounded backend-to-renderer event coalescing;
- send/stream/steer/follow-up/Stop and capability discovery;
- Stop transaction: preserve native `clear_queue` output when available; on explicit unsupported-command rejection recover only the private bounded `queue_update` user-text snapshot and terminate the exact process so no queued/custom continuation survives; abort with deadline and quarantine on uncertainty;
- virtualized minimal timeline;
- versioned/idempotent renderer hydration and bounded crash recovery;
- deterministic fake Pi subprocess lifecycle fixture;
- project registry;
- schema-versioned atomic/recoverable project-registry persistence with safe-start behavior when derived state is malformed;
- bounded paged Pi session discovery/resume with stale-cursor rejection, read-model normalization of explicit persisted skill wrappers, and write-capable Resume refusal for unterminated JSONL tails;
- lazy history pages plus live `get_entries(since)` synchronization;
- session names/tree/fork operations exposed through Pi;
- steer vs follow-up composer actions;
- slash-command discovery plus bounded keyboard palette navigation;
- extension select/confirm/input bridge;
- provider retry/summarization retry/compaction outcome/extension-error projection using current Pi event semantics;
- native `set_auto_retry` control and `abort_retry` Stop semantics without fabricating state Pi does not expose;
- one-shot quiet-stream advisory using the existing deadline scheduler, with no automatic prompt replay or passive polling;
- project trust preflight/launch handling;
- independent context-file and extension-discovery launch policies, including an extension-free recovery path when installed Pi extensions prevent startup;
- backend-owned session draft persistence with visible failure/retry semantics;
- bounded image attachment ingestion across picker/paste/drop/restore paths;
- derived session catalog with incremental indexing only if measurements require persistence; the current complete 1,200-session traversal measurement does not justify another persistent index/watcher owner.

Exit criteria:

- no localhost production server;
- no unbounded IPC queue;
- streaming deltas trigger no app-owned durable catalog/draft writes;
- child exit/restart/failure states are exact;
- abort rejection/timeout cannot be mistaken for idle, and uncertain termination is quarantined;
- app close cannot orphan an app-owned Pi child unintentionally;
- large JSONL fixtures do not block startup;
- session navigation never aborts a live run;
- pending extension requests survive navigation and cannot cross-answer another run;
- fire-and-forget extension status/widget/title state stays byte/entry bounded and never masquerades as pending dialogs;
- extension-cancelled session switches/forks leave the existing local session/run binding unchanged;
- missing/moved project paths are surfaced as detached and require explicit relocation; no automatic global/wrong-project fallback exists;
- a minimal desktop launch PATH cannot silently produce a materially different Pi tool environment from the resolved launch profile;
- the app can coexist with CLI-created sessions without rewriting their authority.
- renderer reload/recovery leaves active Pi children running and rehydrates from backend state.

## Phase 3: parallel orchestration

Status: **complete for the first-product scope**. Several independently owned runs, an actionable deadline-prioritized global Needs Attention queue backed by exact backend request identity, explicit registered-project/model/thinking/execution/queue/lifecycle-timing state in orchestration surfaces, revision-bound last-known change counts only after explicit review, exclusive canonical execution-root ownership, local-vs-worktree startup, immutable worktree identity, durable creation recovery, conservative explicit worktree cleanup, run-bound Git review, durable configurable admission control, explicit idle-run Close, terminal-run Dismiss, attention/working/live-first run ordering, bounded terminal retention, and an eight-run full-lane scale fixture are operational. Branch integration is deliberately outside the review-only first product.

Add:

- several live Pi runs;
- explicit bounded concurrency;
- multi-agent dashboard;
- global Needs Attention view;
- per-run lifecycle/diagnostic state;
- local-checkout vs Git-worktree run creation;
- safe explicit worktree cleanup.

Exit criteria:

- eight bursty simulated runs remain within input-latency and IPC backlog targets;
- each run's cwd is immutable and visible;
- each worktree records and verifies its exact base commit/branch and is never pooled between live runs;
- Git mutations are routed through run-owned worktree identity rather than current UI selection;
- switching projects/views cannot retarget or stop another run;
- partial worktree creation is transactional/recoverable and cleanup refuses unsafe deletion.

## Phase 4: change review

Status: **complete for the review-only first-product scope**. On-demand repository status, bounded changed-file metadata, one-file-at-a-time UTF-8-safe streamed byte-window diff paging, SHA-256 prefix-bound stale cursors, binary classification, independent page/scan ceilings, semantic hunk navigation, renderer stale-result invalidation, and backend-owned cancellation/supersession of active review jobs are operational. A cheap backend-derived "open execution folder" action is also implemented; editor/terminal integration remains optional scope expansion rather than an exit criterion.

Add:

- on-demand repository status;
- changed-file list;
- lazy per-file/hunk diff;
- too-large/binary handling;
- optional open-in-external-editor/terminal actions if they remain cheap.

Exit criteria:

- huge diffs cannot freeze the renderer;
- passive session rows spawn no Git commands;
- review data is invalidated correctly after agent tool mutations;
- file/hunk payloads are bounded and cancelable.

## Phase 5: hardening and packaging

Status: **complete for deterministic repository-owned work in the personal Windows app**. Renderer crash-loop protection, explicit recovery UI, retryable v9 hydration, corruption/write-failure fault injection, accessibility/keyboard contracts, Windows process lifecycle behavior, bounded terminal/runtime and session-draft caches, large-history/diff/concurrency/session-catalog fixtures, one-shot quiet-working-stream and steady-idle no-periodic-work regression fixtures, cold/warm app-owned-state startup measurement, explicit pull-based bounded runtime diagnostics, migration/version policy, and optimized Windows builds are implemented. Draft-cache pressure evicts only unowned Saved records and reloads persisted sessions on revisit; unsaved draft state fails closed rather than being discarded.

Add:

- Windows process lifecycle validation;
- crash/recovery UX;
- accessibility and keyboard pass;
- cold/warm startup benchmarks;
- bounded-state/no-periodic-work regression fixtures plus platform-observable memory/CPU/render diagnostics;
- migration/version compatibility policy for Pi RPC and any derived catalog.

## Phase 6: finite automation and supervised orchestration

Status: **complete for the requested lightweight workflow scope**.

Add:

- reusable schema-versioned prompt chains containing only a name and ordered prompts;
- one Automation view with prompt add/remove/reorder, project, concurrency, Git isolation, and supervisor controls;
- event-driven chain execution that fills ordinary RuntimeManager slots and starts one new Pi session per prompt;
- unique recoverable Git worktrees for parallel chain workers;
- completion detection through Pi-native session state/stats, followed by normal Close to release worker capacity without deleting session/worktree history;
- cancellation that prevents future launches/directives without killing already-running user workers;
- an optional supervisor implemented as one normal Pi session counted against the same live-run ceiling;
- bounded worker task/status/last-result context and a strict JSON Send/Steer/Follow-up directive contract targeting exact RunIds;
- on-demand Automation catalog hydration, execution-only invalidation/IPC, bounded prompt previews, and no-op execution update suppression;
- bounded supervisor lifetime through an explicit per-execution cycle ceiling plus per-turn deadline;
- supervisor failure isolation so malformed or rejected autonomous direction disables supervision while the deterministic prompt chain continues.

Exit criteria:

- the configured eight-run ceiling is exercised by the full concurrency fixture;
- automation performs no interval polling and token/tool display traffic does not wake its scheduler;
- manual and automated sessions share the same admission and execution-root ownership rules;
- parallel automation cannot write concurrently in one local checkout;
- saved chains have independent count/text/aggregate-byte ceilings and corruption quarantine;
- an LLM supervisor cannot address an unknown run, exceed prompt/directive limits, or bypass Pi-native composer semantics;
- supervision cannot make a finite chain unbounded, and cancellation is rechecked before any not-yet-started worker/supervisor process spawn;
- chain cancellation cannot silently terminate user worker sessions or auto-delete their worktrees.

## Later candidates, evidence required

- real container/VM/policy sandbox launch profiles;
- long-lived background daemon;
- scheduled autonomous tasks;
- integrated terminal;
- file explorer/editor;
- branch integration/commit flows;
- remote runtimes;
- multi-harness adapters.

These are not current gaps. Do not work on or report them as remaining work unless the user explicitly asks for that feature.
