# Roadmap

Pi Wizard has no standing milestone queue. Current implemented capability is owned by `STATUS.md`; this document only defines how future scope enters the project.

## Near-term direction

Default work should improve the existing Windows desktop product rather than broaden its category.

Priority order:

1. **Daily-use UX** — reduce friction in New Run, model/project selection, session navigation, attention handling, and change review.
2. **Pi compatibility** — follow supported Pi RPC/capability changes without duplicating Pi semantics or credentials.
3. **Reliability and recovery** — preserve exact process, project, worktree, draft, and request ownership across failure/reload/restart paths.
4. **Performance** — keep passive work near zero and keep histories, tool state, IPC, and diffs bounded as real data grows.
5. **Verification** — turn regressions that cross process/persistence/packaging boundaries into deterministic repository-owned tests when practical.

## Entry criteria for new product scope

A new feature belongs in Pi Wizard when all of the following are true:

- it materially improves orchestration of Pi coding work on the personal Windows desktop;
- Pi does not already provide the same user job through a simpler native surface Pi Wizard can expose;
- the feature has one clear owner and does not create a second authority for Pi sessions, credentials, runtime state, or Git identity;
- its passive CPU/process/filesystem cost is understood;
- large-state behavior has an explicit bound or on-demand loading policy;
- failure/recovery behavior can be stated before implementation;
- the repository has a credible verification route for the consequential contract.

Features that fail these criteria remain outside scope until the user explicitly chooses a different product boundary.

## Candidate expansions

These are possible directions, not queued work:

- real container/VM/policy-backed execution profiles;
- integrated terminal;
- file explorer/editor;
- branch integration, commit, and conflict-resolution workflows;
- scheduled autonomous jobs;
- long-lived background daemon;
- remote runtimes or remote clients;
- multi-harness adapters beyond Pi;
- application-owned provider authentication or provider marketplace.

Each candidate requires explicit user demand and a design update before implementation. A Git worktree must never be relabeled as a security sandbox to approximate the first item.

Continuous foreground Supervision is already implemented product scope. **Scheduled autonomous jobs** here means time-triggered or background execution that continues independently of a user-started desktop supervision session.

## Compatibility-driven work

Upstream Pi changes may require work without changing product scope. Treat these as maintenance when they affect supported behavior:

- RPC command/event/schema changes;
- session format or session-directory resolution changes;
- model/thinking/input-capability discovery changes;
- project trust/context/extension semantics;
- Windows npm launcher layout or process behavior;
- provider authentication discovery exposed through Pi.

Compatibility work should preserve Pi as authority and add a narrow adapter or tested fallback only when the installed Pi contract requires it.

## Completion rule

Do not add completed work to this file. Move implemented truth to its owning current document (`STATUS.md`, `DESIGN.md`, `ARCHITECTURE.md`, or `TESTING.md`) and let version control retain chronology.
