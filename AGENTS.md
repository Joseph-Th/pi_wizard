# Pi Wizard Agent Guide

**BCA policy:** advisory


**Rust agent diagnostics:** advisory. Use bounded diagnostics only for unresolved ownership, test-strength, or macro questions; commands and verification routing: [TESTING.md](TESTING.md).

**Profiles:** Universal, Stateful Application

Use the embedded workspace contract and the task routes below.

<!-- workspace-contract:begin (generated from ../AGENTS.md by tools/sync_agent_context.py; edit the source, not this copy) -->
## Workspace contract

Applies to every agent in every harness. Source, rationale, and evidence: [../AGENTS.md](../AGENTS.md).

The user is a solo developer. Precedence: the user's current request → this contract → the project card → standards references. **Hard** rules cannot be waived by lower-level workflow advice.

### AG-1 Finish the work

Finish the authorized task; a plan, progress update, or deferred note is not delivery. Delegate only bounded, read-only work when delegation is authorized.

### AG-2 Make the calls yourself

Make design and implementation decisions within scope; state consequential choices briefly and continue. Ask only when the answer changes the result and cannot be inferred. Read the project card once, then the task's authority, owner, and proof route. Begin when those are clear; expand for unclear scope or crossed boundaries. Links and profiles are lookups, not a recursive reading list. Use product workflows before internals for research or artifact authoring.

### AG-3 Stay in your project (hard)

Identify the requested project before working. Do not create top-level directories or write output to the workspace root or a project's parent. Scratch work belongs in the project's ignored output location (normally `target/agent-output/`) or OS temp.

### AG-4 Done means the user's copy works (hard)

Check every requested requirement against the actual result. Refresh and verify the affected release binary, deployed site, or running application the user uses; source edits alone do not update it. Documentation-only work verifies the delivered documents and routes. Use the smallest complete project verification lane, and [`REVIEW.md`](../REVIEW.md) for consequential code changes. Claim only what you checked; report unverified requirements.

### AG-5 Look at what you made

Render or run changed visual, audio, interactive, or published output and inspect it against the applicable [`STANDARDS_QUALITY.md`](../STANDARDS_QUALITY.md) rubric and references. Inspect individual assets at useful scales and angles, verify every requested file, and fix defects. Include rendered evidence in the report. Internal prose needs readability and route review only, not a publication workflow. Tests do not prove appearance.

### AG-6 Real behavior over proxies

Observe the behavior the user experiences. A passing test, harness, validator, or metric is evidence, not the goal. Repair harness/product divergence; never pass a gate by weakening it, hardcoding outcomes, suppressing warnings, or retrying until green. Simulations use causal, parameterized systems (STANDARDS_QUALITY.md QUAL-3).

### AG-7 Answer first, then stop talking

Answer questions directly; do not treat them as permission to act. Reports state what changed, verification and its limits, and any required user action. Put long audits or research in a project file and link it. No lectures or revisiting dismissed topics.

### AG-8 Own mistakes; trust the user's evidence

When the user reports a failure, re-check your work before blaming their setup. Own mistakes plainly and fix them. Try a tool, file, or capability before claiming it is unavailable.

### AG-9 The outcome outranks the process

If a procedure harms the requested result or costs more than it protects, favor the result and briefly state what you skipped. This does not waive hard rules.

### AG-10 Leave nothing running

Stop task-owned processes, servers, watchers, and terminals; release locks and remove your scratch output. Keep requested deliverables and the updated application the user is meant to run. Never kill or replace the user's program without saying so. Built software must clean up its own child processes on exit.

### AG-11 Git: `main`, commit, push (hard)

Work on `main`; create branches or worktrees only when asked. Commit and push verified work unless the user says not to. Preserve others' changes: never revert, stash, or discard them. Include sound shared progress when appropriate; leave clearly broken unrelated work out and report it.

### AG-12 Current docs describe the present

Update the single authority for changed behavior. Keep history, session notes, and changelogs out of current docs and comments. Plans and design intent do not prove implemented capability.

### AG-13 CI is local; GitHub Actions are banned (hard)

Use repository-owned local build, test, check, and audit commands. Never create, enable, invoke, or push `.github/workflows/`; remove existing workflow files while retaining their local verification equivalent.
<!-- workspace-contract:end -->

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
- Global durable app state lives under the portable `pi-wizard-data` root, except project-scoped prompt-chain definitions. Prompt chains live at `<project>/.pi-wizard/prompt-chains.json` under the exact registered canonical project and must never fall back to AppData/global state. Generated build output is disposable; user state is not.

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

Current documentation and production comments follow `STANDARDS.md` DOC-3, DOC-4, DOC-8, and SCOPE-7.

When implementation evidence changes a current contract, update the owning document in the same change. Use version control for history.

## Verification

Repository-local verification is authoritative.

- `python tools/verify.py quick` — ordinary core/renderer changes.
- `python tools/verify.py standard` — routine cross-surface or desktop-host changes.
- `python tools/verify.py full` — persistence/schema, packaging, process, large-history/diff, startup, or release-boundary changes.

See `TESTING.md` for exact lane contents and focused fixtures.
