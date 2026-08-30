# Pi Wizard

Pi Wizard is a personal Windows desktop control surface for the Pi coding harness. It keeps Pi as the agent/runtime authority and adds desktop orchestration, durable app preferences, Git-worktree isolation, bounded session presentation, and change review.

It is not a replacement IDE, a second agent runtime, a general Git client, or a cross-platform/web product.

## What it does

Pi Wizard provides these primary workflows:

- **New Run** — choose a saved project directory, execution root, model/thinking level, and optional initial task, then start one Pi RPC session.
- **Active Runs** — see independent live sessions, their project/worktree identity, state, model, elapsed time, and current high-level activity.
- **Recent Sessions** — find and resume Pi-owned JSONL sessions through bounded project-scoped paging.
- **Needs Attention** — answer backend-owned Pi extension requests by exact run/request identity.
- **Changes** — inspect bounded repository status and paged per-file diffs for a run's immutable execution root.
- **Automation** — run finite saved prompt chains through ordinary Pi sessions under the same live-run ceiling as manual work.
- **Supervision** — keep live runs moving across any selected set of projects with one independent Pi supervisor session. Idle results trigger bounded LLM decisions that may Send the next task, Steer/Follow up active work, or Stop a run that should not continue.

## Core model

| Concept | Meaning |
| --- | --- |
| Project | A saved launch preset bound to one canonical directory. |
| Session | Pi-owned conversation/history stored in Pi JSONL. |
| Run | One live Pi RPC process attached to a session and immutable execution root. |
| Execution root | The project's local checkout or a dedicated Git worktree. |

Projects are convenience presets, not containers for session state. Selecting a project routes launch/session discovery to its canonical directory. Missing or moved project paths become detached and require explicit relocation.

## Model selection

The shared model picker is used by New Run, Automation, and Supervision.

- Models come from Pi's `get_available_models` capability discovery.
- Pi remains the authority for provider authentication and model capability metadata.
- New Run initially selects `opencode-go/muse-spark-1.2-contributor` (Muse Spark 1.2 Contributor).
- The last explicit New Run model selection becomes the durable default, including an explicit return to **Pi default**.
- Models can be favorited; available favorites appear before the ordinary model list across all shared pickers.
- A bounded custom model catalog can store additional provider/model identities and labels without storing credentials.

## Session UX

The run surface separates durable conversation from live execution detail.

- The upper **Conversation** pane shows user prompts and final assistant answers only. Prompts are preserved verbatim; final answers render sanitized Markdown with bounded syntax-highlighted code blocks.
- The lower **Live activity** pane is always present and shows only current execution detail: model reasoning, active tool/output previews, direct command output, and the in-progress answer. Completed reasoning/answer text drops out after the turn settles so it is not duplicated above.
- A persistent live-status label distinguishes an active model turn from idle state and shows a separate advisory when Pi still reports the turn active after an unusually quiet RPC interval.
- Both panes auto-follow only while the user remains at the bottom; scrolling upward disables forced autoscroll.
- Large persisted history remains paged/windowed rather than resident in the renderer.

Supervision cards retain a bounded **Last decision** summary so continuous multi-project supervision remains inspectable without exposing the supervisor's raw protocol output.

Run details also expose Pi-native session HTML export and a bounded cancellable one-shot command control. The command runs in the run's immutable execution root through Pi RPC, is excluded from model context, and is not a persistent terminal emulator. While direct Bash owns that execution root, Pi Wizard blocks overlapping model/session mutations and Close, keeps cancellation available from authoritative hydration after renderer reload, and lets continuous Supervision reconsider the run only after the command releases ownership.

The composer maps directly to Pi semantics: **Send** when idle, **Steer** or **Follow up** while working, and **Stop** through the runtime manager's queue-preserving cancellation path.

## Architecture at a glance

```text
Solid renderer
    |
    | typed Tauri IPC + bounded wakeups/pulls
    v
Tauri desktop host
    |
    v
pi-wizard-core RuntimeManager
    |-- Pi RPC process per live run
    |-- session/catalog/history owners
    |-- project/worktree registries
    |-- draft/preferences persistence
    `-- Git review service
```

The important authority split is:

- **Pi owns** agent behavior, session JSONL, commands, models, extensions, and provider credentials.
- **Pi Wizard owns** process orchestration, app-owned preferences/registries/drafts, bounded projections, Git worktree lifecycle, and review UX.
- **The renderer owns presentation only**; it does not become authoritative for process, request, project, worktree, or durable user-data state.

See `ARCHITECTURE.md` for detailed contracts.

## Durable state

Pi Wizard stores its own recoverable state under `pi-wizard-data`:

- repository builds: `<repo>\pi-wizard-data`
- standalone copied executable: sibling `pi-wizard-data`

This root contains project/worktree registries, prompt chains, custom model profiles, preferences, and session drafts. It is intentionally outside disposable `target` output for repository builds.

Pi session JSONL remains in Pi's own session storage and is never replaced by an app-owned transcript database.

## Windows process model

Pi Wizard is a Windows GUI-subsystem application and must not open a console window.

For standard npm Pi installs, the desktop uses Pi's public `pi.cmd` launcher through an app-owned hidden Windows command wrapper. The wrapper preserves Pi's own launcher behavior, keeps stdin/stdout as the RPC transport, does not inspect npm's internal `node_modules` layout, and opens no console window. `RuntimeManager` owns the wrapper and its descendants as one exact process tree; a Windows kill-on-close Job Object is the abrupt-exit backstop.

## Repository layout

| Path | Purpose |
| --- | --- |
| `crates/pi-wizard-core/` | framework-independent runtime/domain implementation |
| `src-tauri/` | Tauri desktop composition, commands, services, Windows integration |
| `src/` | Solid renderer application and feature surfaces |
| `tools/` | local verification, release checks, and deterministic smokes |
| `pi-wizard-data/` | ignored durable local app state for repository builds |

For contributor routing and invariants, start with `AGENTS.md`.

## Development

Frontend development:

```text
npm run dev
```

Type checking:

```text
npm run check
```

Optimized desktop build:

```text
npm run desktop:build
```

The release executable is produced at `target/release/pi-wizard-desktop.exe`.

## Verification

Repository verification is local and deterministic:

```text
python tools/verify.py quick
python tools/verify.py standard
python tools/verify.py full
```

Use `TESTING.md` to choose the required lane. `full` owns the optimized Windows build, PE GUI-subsystem assertion, scale/startup fixtures, and packaged WebView smoke.

Optional installed-Pi compatibility check:

```text
python tools/smoke_live_pi.py
```

That smoke uses an ephemeral no-session/offline Pi RPC process and does not send a model prompt.

After an optimized desktop build, the packaged-app boundary can also be checked against the actually installed Pi rather than the deterministic fake fixture. In PowerShell:

```text
$env:PI_WIZARD_SMOKE_REAL_PI="1"
$env:PI_WIZARD_REAL_PI_PROMPT="Reply with OK only."
node tools/smoke_packaged_desktop.mjs
```

This launches a disposable copy of the release executable, requires the desktop to resolve Pi through the Windows command wrapper, starts a real persistent Pi run in a disposable project, sends one minimal prompt, and requires the run to enter active work, settle, and remain `Ready`.

## Documentation map

| Question | Authority |
| --- | --- |
| Product and repository orientation | `README.md` |
| Contributor routing and invariants | `AGENTS.md` |
| Implemented capability and known limitations | `STATUS.md` |
| User interaction and information architecture | `DESIGN.md` |
| Runtime/persistence/Git/process contracts | `ARCHITECTURE.md` |
| Verification contract | `TESTING.md` |
| Future scope and entry criteria | `ROADMAP.md` |
| External evidence supporting design constraints | `RESEARCH.md` |

Current docs describe the system in present tense. Version control owns implementation history.
