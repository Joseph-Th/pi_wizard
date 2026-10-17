# Pi Wizard Agent Guide

**BCA policy:** advisory


**Rust agent diagnostics:** advisory. Use bounded diagnostics only for unresolved ownership, test-strength, or macro questions; commands and verification routing: [TESTING.md](TESTING.md).

**Profiles:** Universal, Stateful Application

Use the embedded workspace contract and the task routes below.

<!-- workspace-contract:begin (generated from ../AGENTS.md by tools/sync_agent_context.py; edit the source, not this copy) -->
## Workspace contract

Applies to every agent in every harness. Source, rationale, and evidence: [../AGENTS.md](../AGENTS.md).

The user is a solo developer. Precedence: current request → this contract → project card → standards. This is a safety kernel, not a prescribed workflow. **Hard** rules survive lower-level advice.

### AG-1 Finish the work

Finish the authorized task. On open-ended goals, iterate from evidence until the goal is met or useful in-scope improvements are exhausted; pivot around blockers instead of stopping at them.

### AG-2 Make the calls yourself

Make routine in-scope decisions yourself. Read the project card once, identify the owner and proof route, then work. Ask only when a choice materially changes the deliverable and cannot be inferred.

### AG-3 Write only in your project (hard)

Write only in the requested project. Read-only sibling inspection is allowed when it materially helps. Do not write to the workspace root, parent, or siblings; use project-ignored output or OS temp for scratch.

### AG-4 Done means the user's copy works (hard)

Verify the result in the user's actual copy, including the release, deployment, or running app when applicable. Use the smallest complete project lane and [`REVIEW.md`](../REVIEW.md) for consequential code. Claim only checked results.

### AG-5 Look at what you made

Run or render changed visual, audio, interactive, or published output and inspect the real result against project references and [`STANDARDS_QUALITY.md`](../STANDARDS_QUALITY.md). Tests do not prove appearance.

### AG-6 Real behavior over proxies

Prefer observed product behavior over proxy success. Use tests, harnesses, validators, and metrics as evidence; fix production root causes and proxy divergence rather than weakening gates or hardcoding outcomes.

### AG-7 Answer first, then stop talking

Answer questions directly. Reports state changes, verification limits, and required user action; keep long audit material in project files instead of chat.

### AG-8 Own mistakes; trust the user's evidence

On a reported failure, re-check your work before blaming setup. Try a capability before claiming it is unavailable.

### AG-9 The outcome outranks the process

Skip ceremony and optional diagnostics when they cost more than they protect. Do not skip the proof required by AG-4 through AG-6 or any hard rule.

### AG-10 Leave nothing running

Run automation without disrupting the desktop. Stop task-owned processes, release locks, and remove scratch; never kill or replace the user's program silently.

### AG-11 Git: `main`, commit, push (hard)

Work on `main`; branches or worktrees only when asked. Commit and push verified work unless told not to. Stage task-owned paths explicitly in dirty trees and never discard or sweep in unrelated work.

### AG-12 Current docs describe the present

Update the authority for changed behavior. Keep history and session notes out of current docs and comments.

### AG-13 CI is local; GitHub Actions are banned (hard)

Use repository-owned local verification. Never create, enable, invoke, or push `.github/workflows/`.

### Optional workspace accelerator

Use native navigation first. If cold-start routing is still expensive or ambiguous, run:

```text
python ../tools/agent_context.py brief pi_wizard <task ...>
```

It returns bounded authority, verification, and likely-source evidence. Tool output is evidence, not authority. Use `python ../tools/agent_context.py --help` only when a specific unresolved question justifies a deeper probe. No shared analysis command is a prerequisite to editing.
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
