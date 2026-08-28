# Status

## Phase

**Phase 2 runtime integration is substantially operational and early Phase 3/4 orchestration is implemented: durable Git-worktree creation/recovery and bounded on-demand repository review now sit alongside the existing project/session/runtime surfaces. Full session-tree visualization, explicit Git cleanup/integration, renderer crash-loop protection, and packaging remain incomplete.**

Research and the initial product/architecture baseline were completed on 2026-08-27.

The first executable foundation now exists and passes the repository `standard` verification lane.

A second research/audit pass on 2026-08-27 compared the live foundation against current Pi RPC/session/security documentation and recent Pi GUI, Codex, and OpenCode reliability reports. It tightened protocol, persistence, Stop/recovery, draft, attachment, worktree, renderer-recovery, and CSP contracts before runtime integration begins.

A third research/audit pass on 2026-08-27 focused on desktop environment parity, project/path identity, recoverable derived state, Pi trust/context-file semantics, cancellable session mutations, and bounded fire-and-forget extension UI state.

A fourth research/audit pass on 2026-08-27 rechecked Pi's current streaming RPC schema and current lightweight Pi/agent desktop implementations. It tightened content-indexed assistant streaming, direct RPC Bash correlation, aggregate hot-state bounds, and caught a misplaced runtime unit test that prevented an all-target core compile.

A fifth research/audit pass on 2026-08-27 focused on current Pi RPC concurrency, compaction/Stop behavior, direct Bash semantics, authoritative state recovery, desktop launch-environment parity, supervised subprocess ownership, and renderer backpressure. It moved several Phase 2 guarantees from design prose into Tauri-independent executable owners.

A sixth research/audit pass on 2026-08-27 added bounded shell/version probing, revisioned capabilities and extension UI projection, schema-versioned runtime hydration, and durable-cursor `get_entries(since)` synchronization. It also established the first Tauri hydration/probe IPC surface.

A seventh research/audit pass on 2026-08-27 joined the supervised child, RPC controller, RuntimeStore, Stop transaction, renderer backlog, and shutdown lifecycle under one persistent runtime manager. It also found and fixed a Windows wrapper-process defect where terminating an npm-style `.cmd` launcher could leave its Node descendant holding inherited pipes open indefinitely.

An eighth research/audit pass on 2026-08-27 tightened renderer recovery, extension interactions, session replacement, draft ownership, startup readiness, and retained runtime-state bounds. Ordinary hydration is now non-destructive, stale renderer backlog recovery is explicit per run, accepted Pi session replacements reconcile identity through `get_state`, extension `set_editor_text` enters a session-scoped backend draft owner, and a spawned child is not considered Ready until a bounded RPC handshake succeeds.

A ninth research/audit pass on 2026-08-27 focused on composer ownership and user-data durability. It added generation-captured Send/Steer/Follow-up transactions, atomic session-draft persistence and restart recovery, bounded shutdown flush, corruption quarantine, a restore-pending gate, and a persistent Solid composer whose rapid edits cannot create an unbounded IPC promise queue.

A tenth research/audit pass on 2026-08-27 rechecked Pi replacement/interrupt semantics and recent lightweight GUI session/project failures. It made session replacement wait for old-session draft durability, bounded `clear_queue` recovery before cloning it, restored Stop queue text ahead of existing unsent input without destructive overflow, made `ProjectId` durable through an atomic recoverable registry, added an end-to-end project launcher, and exposed the backend-bounded live assistant/tool/Bash projection in the renderer.

An eleventh research/audit pass on 2026-08-27 added bounded current-project session discovery/resume and a file-backed active-branch history reader. Cold history now pages through opaque session-bound cursors without issuing an unbounded initial `get_messages`/`get_entries`, and persisted user entries can drive Pi-native forks. The renderer keeps one bounded history page rather than accumulating a complete transcript.

A twelfth research/audit pass on 2026-08-27 rechecked current Pi model metadata, image routing, compaction, lightweight Pi GUIs, OpenCode attachment failures, and OpenChamber long-session/reconnect reports. Pi Wizard now retains only the narrow declared image-input capability needed by the UI, refuses to submit images when Pi explicitly marks the current model text-only, preserves unknown capability for compatibility, and exposes Pi's native manual compaction rather than implementing a second summarizer.

A thirteenth research/audit pass on 2026-08-27 rechecked current Pi session-stat/compaction semantics plus long-session performance reports from OpenChamber/OpenCode. It added explicit on-demand session usage, useful native compaction result metadata, transactional Git-worktree launch, immutable branch/base/root identity in runtime hydration, and bounded on-demand changed-file/diff review without passive Git polling.

A fourteenth research/audit pass on 2026-08-27 focused on crash recovery and worktree/cwd failures across Pi and lightweight coding harnesses. Worktree creation now durably journals the intended ProjectId/repository/base/branch/path before Git mutation, upgrades the journal after verified creation, conservatively reconciles branch-only/path-only/conflicting states after restart, preserves legitimate descendant commits and dirty task work, and never deletes Git resources automatically. The same pass exposed Pi's native automatic-compaction setting in the GUI with `get_state` reconciliation rather than inventing a local context policy.

## Accepted baseline

- Desktop shell, not an IDE and not a new agent runtime.
- Tauri 2 + Rust/Tokio host.
- SolidJS + TypeScript static renderer.
- Upstream Pi CLI in `--mode rpc` as the main integration boundary.
- One independently owned Pi process per live runtime session.
- Pi session JSONL is the authoritative conversation store.
- Derived indexes/caches are disposable and rebuildable.
- Multi-session orchestration, Pi-native steer/follow-up controls, Git worktree isolation, and bounded change review are core product scope.
- Transcript virtualization, bounded tool output, lazy diff loading, backpressure, and zero passive polling are baseline architecture requirements.
- Project trust, Git isolation, and sandboxing are distinct concepts in the UI and runtime.

## Implemented foundation

- Rust 2024 workspace with a Tauri-independent `pi-wizard-core` crate and thin `src-tauri` composition root.
- Static SolidJS + TypeScript + Vite renderer scaffold with no production application server.
- Centralized validated runtime limits for RPC frames, outbound payloads, streaming text/content blocks, tool previews, UI backlog, active tools, pending RPC/UI requests, diagnostics, failure detail, image attachments, draft size, recovered Stop queues, project/worktree-registry state, Git command/review payloads, retained runtime state, startup RPC readiness, draft flush/debounce, and Stop deadlines.
- UUIDv7-backed opaque project/run/worktree/draft-image identities and string-compatible RPC request identities.
- Strict incremental LF-only JSONL framing with optional CR handling, Unicode-line-separator safety, oversized-frame recovery, and bounded buffering.
- Typed outbound Pi RPC commands and extension UI responses with exact request correlation and outbound byte ceilings, including first-class `steer`, `follow_up`, current capability/session operations, and stable `get_entries(since)` history cursors.
- Bounded validated Pi image payloads with count, per-image decoded-byte, aggregate decoded-byte, MIME/base64, and final outbound RPC ceilings.
- Forward-compatible inbound response/event classification that preserves unknown valid event payloads, plus typed nested assistant-stream parsing by `contentIndex` and direct Bash update correlation by request ID.
- Typed current tool start/update/end, queue-count, session-name, thinking-level, `get_state`, `clear_queue`, and direct-Bash response projections; accumulated tool output is never mistaken for a delta.
- Async bounded RPC reader/writer primitives plus a supervised child-process transport with private stdin/stdout RPC, bounded continuously drained stderr, exact spawn identity, deadline-bounded process-tree termination, and bounded diagnostic EOF finalization. Windows `.cmd` wrappers terminate through the exact captured PID tree; Unix children launch in an app-owned process group.
- Explicit Pi launch specification with canonical working directory, optional canonical session directory, startup model/thinking choices, Pi saved/default trust inheritance or one-run override, independent context-file policy, current direct-Bash `excludeFromContext`, and other typed Pi launch/runtime controls.
- Launch-environment resolver over explicit configuration, desktop-process environment, and a bounded shell-probe result, with one selected environment shared by Pi/Git discovery and spawning. Diagnostics expose executable/provenance metadata without serializing environment values or secrets.
- Active bounded platform shell-environment probing and bounded `pi --version` probing under the exact resolved launch environment. The Tauri host caches environment plus parsed Pi version as one launch profile so diagnostics and actual process startup cannot diverge.
- Canonical project binding primitive with stable `ProjectId`, exact-path verification, missing-path detection, and explicit-only relocation; no silent project fallback/rebinding. A schema-versioned atomic project registry now preserves those IDs across app restarts, quarantines corrupt/oversized/duplicate derived state, and commits registration/relocation to disk before mutating in-memory indexes.
- Separate process lifecycle and agent activity state, enforced through a reducer rather than public field mutation.
- Explicit terminal `Quarantined` state for runs whose OS-process termination cannot be confirmed; quarantined runs cannot accept further activity mutations.
- Authoritative bounded `get_state` reconciliation for live activity, compaction, session identity/name, model, thinking level, native automatic-compaction state, queue modes, and counts so backend recovery does not depend on renderer memory. Session IDs and the complete retained runtime-state projection have explicit byte ceilings, and incremental session-name events cannot bypass the same retained-state budget.
- Session-scoped generation-safe draft state plus a backend session draft owner and bounded atomic persistence worker. Extension `set_editor_text` uses the same owner; first-known Pi session identity adopts a pending-run draft, later session replacement never carries text into another session, restart restores saved text, corruption is quarantined, and Dirty/Saving/Saved/Failed plus bounded persistence detail remain visible. Shutdown bypasses normal debounce and waits only through the configured flush deadline.
- Pending extension interaction ownership by exact request ID and bounded pending RPC request correlation.
- Per-run RPC controller that atomically owns command barriers, pending correlation, writer-failure rollback, live stream projection, direct-Bash ownership, response completion, and runtime-state event application.
- Persistent `RuntimeManager` that owns all live run records/controllers/process handles, request waiters, extension-dialog response ownership, Stop transactions, renderer backlogs, process failures, and global shutdown. It uses separate normal/control queues and requires an explicit active Tokio runtime rather than panicking on an implicit runtime assumption.
- Client-side session-replacement and manual-compaction barriers that close Pi RPC's asynchronous-dispatch race window before corresponding semantic events arrive. Persistent session replacement first forces the old session's current dirty draft through the bounded durability boundary; later commands/composer edits cannot overtake that transaction, and failed/expired flush leaves Pi on the old session.
- Accepted `new_session`/`switch_session`/`fork`/`clone` replacement responses automatically queue authoritative `get_state` reconciliation before the original waiter is released, because the replacement response itself does not establish the new Pi session identity. Extension-cancelled replacements leave the existing binding untouched.
- User-facing Stop transaction that bounds and clears/preserves queues before abort, handles settle-during-clear races, keeps a normally stopped Pi process reusable, escalates rejection/timeouts to exact-process termination, and does not fake an RPC compaction abort that Pi does not expose. Recovered steering/follow-up text is prepended to existing unsent draft text through the same backend draft owner; an oversized merge is reported without destroying the existing draft.
- Semantic RPC response outcomes distinguish protocol rejection, acceptance, and extension-cancelled `new_session`/`switch_session`/`fork`/`clone` operations.
- Bounded backend projection for extension `setStatus`/`setWidget`/`setTitle` state with entry and byte ceilings; transient notifications and draft/editor state remain separate owners.
- Bounded live assistant/tool projections, including ordered text/thinking/tool-call content blocks, sparse-index-safe block ceilings, an aggregate assistant byte ceiling, authoritative block-end replacement, and accumulated tool-output replacement semantics.
- Bounded request-correlated direct-Bash live previews independent from tool-call previews.
- Explicit classification of high-frequency display updates that may be coalesced and must not directly trigger durable app-owned persistence.
- Byte-bounded per-run renderer backlog with keyed replacement for assistant/tool/Bash display frames, accounting for key/frame overhead, display-frame eviction under pressure, and explicit rehydration failure when semantic-only state exceeds capacity.
- Coalesced backend dirty-run signals plus bounded pull-based renderer drains. Ordinary hydration is non-destructive and re-announces any already-pending delivery to a newly subscribed renderer; it never clears another run's transient notification/tool-finished frames. Backlog desynchronization uses an explicit per-run recovery transaction that discards only that run's stale queued frames. Signal loss/lag widens to authoritative hydration rather than raw-event replay or interval polling; IPC enum variants and fields have regression-tested camelCase wire tags.
- Schema-versioned, revisioned hydration schema v6 of runtime semantic state, immutable Git-worktree identity, current durable draft/restore state, backend-derived composer availability/submission state, model image-input capability, capabilities, live assistant/tool/Bash previews, extension UI state/dialogs, and live session-sync cursors. Repeated hydration does not restart a live Pi child.
- Bounded live-history synchronization through `get_entries(since)` with separate append cursor/leaf identity, one request in flight, stale-cursor rejection, and explicit resync-required state rather than fallback to full `get_messages` hydration.
- File-backed bounded active-branch history pages for cold/resumed sessions. Latest/Older/Newer navigation follows persisted `parentId` ancestry, limits line/scan/page/text bytes independently, keeps abandoned branches out of the normal timeline, and binds cursors to the exact Pi session identity. Persisted user entries expose Pi-native fork actions without materializing a whole transcript.
- Tauri owns the runtime manager plus project/worktree registries. Implemented commands cover hydration, explicit per-run UI recovery, bounded event draining, local/worktree/recovered-worktree run startup, durable worktree inspection/reconciliation, on-demand Git review, Pi-native session controls/usage/compaction, draft editing/submission, Stop, extension dialog responses, and Pi environment/version probing. Production composition uses the bundle-scoped app-data `runtime-state` root for independent draft/project/worktree persistence domains. Tauri forwards only normalized dirty/rehydrate wakeups, not raw Pi frames.
- Bounded current-project session discovery/resume now follows Pi's resolved launch environment, `PI_CODING_AGENT_DIR` / `PI_CODING_AGENT_SESSION_DIR`, merged global/project `sessionDir` settings, and default encoded-cwd storage. Discovery filters session headers by canonical cwd even for flat custom directories, reads only bounded head/tail metadata previews, and revalidates a selected JSONL file before launching `--session`.
- Tauri `ExitRequested` is intercepted until the runtime manager completes bounded shutdown of all children owned when shutdown began. Renderer navigation/reload has no process-lifecycle side effect.
- Canonical project paths reuse a durable app-owned `ProjectId` across desktop restarts. Missing stored paths remain detached registrations; corrupt registry state is quarantined rather than rebound to another checkout.
- Solid renderer installs runtime listeners before initial hydration, folds concurrent per-run dirty signals, drains bounded pages without passive polling, carries hydration demand across multi-batch continuation drains, uses request sequencing so older hydration responses cannot overwrite newer state, and widens backend lag/desynchronization to explicit per-run recovery/fresh hydration.
- Solid now exposes an explicit existing-project launch form with Pi inherit/approve/ignore resource-trust choices, local vs inspected-worktree launch, durable worktree recovery inspection/restart, bounded session search/resume, history paging/fork, session naming/clone, discovered model/thinking/slash-command controls, Pi-native automatic/manual compaction and on-demand usage, bounded on-demand Git change review, a backend-owned Send/Steer/Follow-up composer, Stop during work/compaction, visible draft durability/restore errors, and bounded live assistant/thinking/tool/Bash output including truncation markers. Failed draft IPC synchronization is retained locally for explicit retry instead of being overwritten by a later hydration.
- Image drafts support picker, paste, drag/drop, removal, session-scoped persistence/restore, image-only prompts, and generation-safe submission. Raw base64 stays backend-owned and hydration exposes metadata only. Pi's declared model `input` capability is projected as `supportsImages`; explicit text-only models disable new image ingestion and backend submission while preserving existing draft images for removal or a later model switch.
- Renderer `Needs Attention` UI now presents typed Pi extension select/confirm/input/editor dialogs across runs, responds by exact request ID through the priority manager control plane, preserves partially typed local input across same-ID hydration refreshes, and refreshes authoritative state after stale/expired-response rejection.
- Process `Starting` now means the OS child exists but Pi RPC readiness has not yet been proven. A designated startup `get_state` must succeed before `Ready`; its deadline is part of the manager's normal deadline selector, a silent child is terminated/failed, and explicit Stop/application shutdown supersede the startup timer.
- Static renderer Content Security Policy instead of a null CSP.
- Local `quick` and `standard` verification commands.
- Deterministic cross-platform fake-Pi subprocess coverage in addition to in-memory protocol/state fixtures.

## Not implemented yet

- Full visual session tree/navigation. Bounded active-branch history, session search/resume, name, clone, and fork-at-user-entry controls exist, but the complete branch tree is not yet rendered as an inspector.
- Explicit safe Git worktree cleanup/removal, branch integration/merge workflow, and configurable live-run concurrency admission. Several independent live runs, worktree-isolated launch/recovery, dashboard cards, Needs Attention, and bounded Git review are already operational.
- Renderer crash-loop breaker and full recovery screen; hydration/listener recovery primitives now exist.
- Packaging/signing/update behavior.

## Deliberately deferred

- embedded terminal emulator;
- full source editor or file explorer;
- multi-provider harness abstraction beyond Pi;
- remote/mobile clients;
- daemon/background service surviving desktop exit;
- scheduled autonomous jobs;
- custom plugin marketplace;
- application-owned model/provider authentication;
- fake GUI permission modes that Pi itself does not enforce;
- built-in container/VM sandbox implementation until its ownership and cross-platform contract are designed and measured.

Those are not forbidden forever. They are outside the first product boundary so the orchestration core can remain small and measurable.
