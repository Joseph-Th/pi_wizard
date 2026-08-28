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
- repository-owned quick and standard verification lanes.

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

Status: **substantially implemented**. The remaining Phase 2 gap is the full visual session-tree inspector plus hardening of recovery UX; the runtime/history/composer/capability/attachment slices below are operational.

Add:

- Pi executable discovery/version probe;
- bounded desktop environment/PATH resolver with explicit configured-path precedence and secret-safe provenance diagnostics;
- one supervised `pi --mode rpc` child using the Phase 1 protocol/lifecycle owners;
- bounded backend-to-renderer event coalescing;
- send/stream/steer/follow-up/Stop and capability discovery;
- Stop transaction: preserve `clear_queue` output, abort with deadline, exact-process escalation, quarantine on uncertainty;
- virtualized minimal timeline;
- versioned/idempotent renderer hydration and bounded crash recovery;
- deterministic fake Pi subprocess lifecycle fixture;
- project registry;
- schema-versioned atomic/recoverable project-registry persistence with safe-start behavior when derived state is malformed;
- Pi session discovery/resume;
- lazy history pages plus live `get_entries(since)` synchronization;
- session names/tree/fork operations exposed through Pi;
- steer vs follow-up composer actions;
- slash-command discovery;
- extension select/confirm/input bridge;
- project trust preflight/launch handling;
- context-file policy messaging that reflects Pi's actual behavior (`AGENTS.md`/`CLAUDE.md` still load under `--no-approve` unless explicitly disabled);
- backend-owned session draft persistence with visible failure/retry semantics;
- bounded image attachment ingestion across picker/paste/drop/restore paths;
- derived session catalog with incremental indexing only if measurements require persistence.

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

Status: **substantially implemented**. Several independently owned runs, cross-run Needs Attention, local-vs-worktree startup, immutable worktree identity, durable creation recovery, and run-bound Git review are operational. Explicit Git cleanup/integration and configurable admission control remain.

Add:

- several live Pi runs;
- explicit bounded concurrency;
- multi-agent dashboard;
- global Needs Attention view;
- per-run lifecycle/diagnostic state;
- local-checkout vs Git-worktree run creation;
- safe explicit worktree cleanup.

Exit criteria:

- four bursty simulated runs remain within input-latency and IPC backlog targets;
- each run's cwd is immutable and visible;
- each worktree records and verifies its exact base commit/branch and is never pooled between live runs;
- Git mutations are routed through run-owned worktree identity rather than current UI selection;
- switching projects/views cannot retarget or stop another run;
- partial worktree creation is transactional/recoverable and cleanup refuses unsafe deletion.

## Phase 4: change review

Status: **substantially implemented**. On-demand repository status, bounded changed-file metadata, and one-file-at-a-time bounded tracked diffs are operational. Binary/hunk paging, cancellation polish, and optional external-editor actions remain.

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

Add:

- cross-platform process lifecycle validation;
- crash/recovery UX;
- installer/signing/update strategy appropriate to target platforms;
- accessibility and keyboard pass;
- cold/warm startup benchmarks;
- memory/CPU regression fixtures;
- migration/version compatibility policy for Pi RPC and any derived catalog.

## Later candidates, evidence required

- real container/VM/policy sandbox launch profiles;
- long-lived background daemon;
- scheduled autonomous tasks;
- integrated terminal;
- file explorer/editor;
- branch integration/commit flows;
- remote runtimes;
- multi-harness adapters.

Each candidate should be added only after identifying the user job, owner, idle/runtime cost, failure semantics, and smallest verification path. The default answer to scope expansion is not "never"; it is "prove the orchestration core stays small first."
