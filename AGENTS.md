# Pi Wizard Agent Guide

**BCA policy:** advisory

Follow workspace `../AGENTS.md` and portfolio `../STANDARDS.md` first. Pi Wizard applies the Universal and Stateful Application profiles.

## Scope

Pi Wizard is a personal **Windows desktop** control surface for the Pi coding harness.

Optimize for Windows desktop reliability, Pi integration, Git/worktree correctness, durable state, recovery, performance, and daily-use UX. Do not treat macOS/Linux delivery, browser/web deployment, app-store distribution, signing, public release certification, remote clients, or general IDE features as project gaps unless the user explicitly expands scope.

## Cold-start route

1. Inspect Git state and preserve unrelated work.
2. Read `README.md` for the product and repository map.
3. Read `STATUS.md` for current implemented capability and known limitations.
4. Read only the authority needed for the task:
   - product/UI behavior: `DESIGN.md`
   - runtime, persistence, process, Git, and performance contracts: `ARCHITECTURE.md`
   - verification obligations: `TESTING.md`
   - future scope: `ROADMAP.md`
   - external evidence for a design decision: `RESEARCH.md`
5. Identify the owning source subsystem and the narrowest existing test before editing.

`RESEARCH.md` is evidence, not current product authority. Version control owns implementation history.

## Repository map

| Area | Owns |
| --- | --- |
| `crates/pi-wizard-core/` | Pi protocol, runtime state, process-independent orchestration rules, persistence primitives, session/history/catalog logic, worktree and Git-review contracts |
| `src-tauri/src/app/` | desktop composition, startup, portable-state wiring, environment/profile ownership |
| `src-tauri/src/commands/` | typed Tauri command adapters |
| `src-tauri/src/services/` | desktop orchestration services such as Automation and Supervision |
| `src-tauri/src/platform/` | Windows-specific process/lifecycle integration |
| `src/app/` | application shell and top-level renderer state composition |
| `src/features/` | user-facing workflow surfaces and bounded projections |
| `src/lib/`, `src/types/`, `src/styles/` | shared renderer utilities, wire types, and styling |
| `tools/` | repository verification, release checks, and deterministic smoke fixtures |

## Non-negotiable invariants

### Authority

- Pi owns agent execution semantics, model/provider capability, commands, extensions, and authoritative session JSONL.
- Pi Wizard owns subprocess orchestration, bounded projections, app-owned preferences/registries/drafts, Git worktrees, and review UX.
- The renderer is never authoritative for process lifecycle, request identity, project/worktree identity, trust decisions, durable drafts, or pending extension interactions.
- `crates/pi-wizard-core` remains independent of Tauri and renderer-framework types.

### Runtime and process lifecycle

- Process lifecycle and agent activity are separate state axes; Pi `agent_settled` does not mean the process exited.
- A live run has one immutable canonical execution root. Navigation cannot retarget it.
- The execution root has one Pi Wizard mutation owner at a time. An accepted idle Prompt owns the pre-`agent_start` handoff; active direct Bash excludes overlapping model/session mutation and Close until it completes or is cancelled. Read-only probes/export and Bash cancellation may remain available.
- Stop preserves recoverable queued user text before aborting. Unconfirmed termination becomes `Quarantined`; such a run cannot accept further RPC writes or be shown as safely stopped.
- Process termination targets the exact owned process identity/tree. Never kill by executable name or wildcard.
- Windows script launchers must be normalized into an app-owned hidden wrapper with exact process-tree ownership. Standard npm `pi.cmd`/`.bat` shims are invoked through the system command interpreter with `CREATE_NO_WINDOW`; package-internal Node/CLI paths are not inferred. Unsupported script hosts fail before live spawn.

### Sessions and user data

- Pi JSONL is the only authoritative transcript store.
- Live synchronization prefers stable incremental Pi entry cursors; cold history remains bounded and file-backed.
- Drafts are session-scoped backend-owned user data with generation-safe persistence. Renderer lifetime is not draft lifetime.
- App-owned registries, preferences, catalogs, and recovery journals are bounded, schema-versioned, and recoverable. Corruption in one app-owned domain must not hide or modify Pi JSONL or unrelated state.
- Durable app state lives under the portable `pi-wizard-data` root. Generated build output is disposable; user state is not.

### Projects, Git, and trust

- A registered project is a stable app ID bound to one canonical directory. Missing/moved paths become detached and require explicit relocation.
- Worktree creation binds an explicit base commit, branch, and path as a recoverable transaction. Do not infer a default branch or reuse/pool live worktrees.
- Git worktrees provide checkout isolation, not security isolation.
- Pi project-resource trust and context-file loading are independent policies. `--no-approve` does not disable `AGENTS.md`/`CLAUDE.md`.

### Bounds and passive work

- Streaming token/tool updates are transient hot state and must not trigger durable writes.
- Do not eagerly load complete large transcripts, tool logs, or diffs.
- Large Git review work stays outside the renderer and is loaded on demand.
- Passive UI must not introduce periodic Git/session/filesystem polling. Prefer semantic invalidation and explicit refresh.
- Image and other bounded payloads are revalidated at the backend boundary even when the renderer already validated them.

## Change routing and companion work

| Change | Primary owner | Required companion checks |
| --- | --- | --- |
| Pi RPC command/event semantics | core RPC/controller | protocol fixtures, wire tests, compatibility behavior |
| Runtime lifecycle/backpressure | core runtime manager/process owner | lifecycle tests, Stop/shutdown tests, hydration projection |
| Durable app-owned schema | owning persistence module | schema bump/migration, bounds, corruption fixture, docs |
| Project identity | project registry | canonical-path tests, relocation/detached behavior |
| Worktree lifecycle | worktree service/registry | real Git fixture, recovery/cleanup invariants |
| Session catalog/history | session read-model owners | large-history bounds, cursor/stale behavior |
| Git review | Git review service | binary/large diff/cursor/cancellation fixtures |
| Desktop IPC surface | Tauri command adapter + renderer caller | command registration/surface contract, typed wire shape |
| Model/thinking behavior | Pi capability discovery + model preference owner | fake-Pi catalog, preference persistence, packaged selector smoke |
| Product interaction policy | `DESIGN.md` + feature surface | accessibility contract and relevant renderer behavior |
| Packaging/process behavior | Tauri/platform owners | `full` verification and packaged WebView/PE checks |

## Documentation and comments

Current documentation and production comments follow portfolio standards Sections 3.4, 3.8, and 29:

- describe current behavior in present tense;
- put the contract before rationale or evidence;
- keep one documentary owner for mutable facts and link to it elsewhere;
- remove implementation diaries, incident narratives, superseded designs, and milestone history from current docs;
- comments explain non-obvious ownership, ordering, invariants, external constraints, or why a simpler-looking approach is invalid;
- comments do not restate syntax, narrate bug history, preserve obsolete approaches, or embed one-off measurements/debugging stories;
- compatibility comments state the current external constraint and the condition under which the special handling applies.

When implementation evidence changes a current contract, update the owning document in the same change. Use version control for history.

## Verification

Repository-local verification is authoritative; GitHub Actions are prohibited by workspace policy.

- `python tools/verify.py quick` — ordinary core/renderer changes.
- `python tools/verify.py standard` — routine cross-surface or desktop-host changes.
- `python tools/verify.py full` — persistence/schema, packaging, process, large-history/diff, startup, or release-boundary changes.

See `TESTING.md` for exact lane contents and focused fixtures.
