# Pi Wizard Agent Guide

**BCA policy:** advisory


**Rust agent diagnostics:** advisory. Use bounded diagnostics only for unresolved ownership, test-strength, or macro questions; commands and verification routing: [TESTING.md](TESTING.md).

**Profiles:** Universal, Stateful Application

Use the embedded workspace contract and the task routes below.

<!-- workspace-contract:begin (generated from ../AGENTS.md by tools/sync_agent_context.py; edit the source, not this copy) -->
## Workspace contract

Applies to every agent in every harness. Source, rationale, and evidence: [../AGENTS.md](../AGENTS.md).

The user is a solo developer. Precedence: current request → this contract → project card → standards. **Hard** rules survive lower-level workflow advice.

### AG-1 Finish the work

Finish the authorized task; plans, progress, or deferred notes are not delivery. On open-ended or hill-climbing goals, keep iterating—observe, diagnose, fix, verify—until the goal is met or in-scope improvements are exhausted; if one track is blocked, move to the next unblocked work in scope. Delegate only bounded read-only work when authorized.

### AG-2 Make the calls yourself

Make in-scope decisions yourself; state consequential choices briefly and continue. Do not ask permission for routine verification, bounded evaluations, or the next in-scope step; ask only when a choice changes the deliverable and cannot be inferred. Read the project card once, then resolve authority, owner, and proof. Expand only for unresolved scope or boundaries. Links and profiles are lookups; research and artifact work starts at the product workflow.

### AG-3 Stay in your project (hard)

Work only in the requested project. Do not create top-level directories or write to the workspace root or project parent. Put scratch work in the project's ignored output (normally `target/agent-output/`) or OS temp.

### AG-4 Done means the user's copy works (hard)

Verify every requirement against the user's actual copy. Refresh and check the affected release, deployment, or running app; source edits alone do not update it. Docs-only work verifies docs and routes. Use the smallest complete project lane and [`REVIEW.md`](../REVIEW.md) for consequential code. Claim only checked results; report what remains unverified.

### AG-5 Look at what you made

Run or render changed visual, audio, interactive, or published output; inspect it against [`STANDARDS_QUALITY.md`](../STANDARDS_QUALITY.md) and project references. Use isolated workbench or multi-angle views where detail matters, compare directly against references at matching angles and delivery scale, fix defects before reporting, and report rendered evidence. Internal prose needs only readability and route review. Tests do not prove appearance.

### AG-6 Real behavior over proxies

Observe user-experienced behavior. Tests, harnesses, validators, and metrics are evidence for hill climbing, not the goal. Use their findings to diagnose and fix systemic root causes in production owners; repair proxy/product divergence, and never weaken gates, hardcode outcomes, suppress warnings, or retry until green. Simulations stay causal and parameterized (STANDARDS_QUALITY.md QUAL-3).

### AG-7 Answer first, then stop talking

Answer questions directly; they are not permission to act. Reports state changes, verification limits, and required user action. Put long audits or research in a project file. No lectures or revisiting dismissed topics.

### AG-8 Own mistakes; trust the user's evidence

On a user-reported failure, re-check your work before blaming setup; own and fix mistakes. Try a tool, file, or capability before claiming it is unavailable.

### AG-9 The outcome outranks the process

Skip ceremony, optional diagnostics, or intermediate process when they cost more than they protect, and state what you skipped. Do not use this rule to skip the smallest complete verification lane (AG-4), applicable inspection of user-visible output (AG-5), or real-behavior evidence (AG-6). Hard rules still apply.

### AG-10 Leave nothing running

Run automation headlessly or in the background without popping up extra terminal or GUI windows that disrupt the desktop. Stop task-owned processes, servers, watchers, and terminals; release locks and remove scratch. Keep deliverables and the app the user should run. Never kill or replace the user's program silently. Built software cleans up child processes on exit.

### AG-11 Git: `main`, commit, push (hard)

Work on `main`; branches or worktrees only when asked. Commit and push verified work unless told not to. In a dirty shared tree, stage task-owned paths explicitly; broad add/commit-all modes MUST NOT sweep in unrelated changes. Never revert, stash, overwrite, or discard others' changes. Shared progress MAY be included only when deliberately selected, reviewed, and verified as complete; otherwise leave it untouched and report it.

### AG-12 Current docs describe the present

Update the single authority for changed behavior. Keep history, session notes, and changelog prose out of current docs/comments. Plans and design intent do not prove capability.

### AG-13 CI is local; GitHub Actions are banned (hard)

Use repository-owned local verification. Never create, enable, invoke, or push `.github/workflows/`; remove existing workflows while keeping their local equivalent.

### Shared workspace tools

Shared portfolio tools are optional accelerators available from this project root through `../tools/`; using them does not authorize edits outside this project. Skip them when native model/harness navigation already provides equivalent evidence or ordinary source inspection is cheaper. No shared analysis command is a prerequisite to editing. When useful:

```text
python ../tools/agent_context.py brief pi_wizard <task ...>
python ../tools/agent_context.py --help
python ../tools/agent_context.py diff-impact pi_wizard
```

`agent_context.py` provides `brief`, `context`, `impact`, `diff-impact`, `outline`, `structure`, `architecture`, `backends`, `audit`, `rule`, and `exemplar`. `brief` returns copyable optional next commands for the current working directory. `exemplar` surfaces non-normative cross-project prior art; adapt ideas locally unless a real shared contract justifies shared code.
For Cargo projects, `python ../tools/rust_agent_tools.py check` verifies the shared Rust diagnostics; use them only as routed by this card's Rust diagnostics marker and `TESTING.md`.
<!-- workspace-contract:end -->

## Scope

Pi Wizard is a personal **Windows desktop** control surface for the Pi coding harness.

Optimize for Windows desktop reliability, Pi integration, Git/worktree correctness, durable state, recovery, performance, and daily-use UX. Do not treat macOS/Linux delivery, browser/web deployment, app-store distribution, signing, public release certification, remote clients, or general IDE features as project gaps unless the user explicitly expands scope.

## Authorities

- `README.md` owns product orientation and the repository map.
- `STATUS.md` owns current implemented capability and limitations.
- `DESIGN.md` owns product and UI behavior.
- `ARCHITECTURE.md` owns runtime, persistence, process, Git, and performance contracts.
- `TESTING.md` owns verification lanes and focused fixtures.
- `ROADMAP.md` owns future scope. `RESEARCH.md` is supporting evidence, not current product authority.

Use the change-routing table below to select the owning subsystem and companion proof. Version control owns implementation history.

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
