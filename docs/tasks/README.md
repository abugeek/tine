# Tasks — How This Folder Works

Task files are **temporary work packages** for AI agents and contributors. Lasting knowledge belongs in [`../project/`](../project/).
Every task is written so one agent can pick it up cold: it lists what to read, which files it owns, the interfaces it
consumes and provides, and how to prove it is done.

## Assigning work to multiple agents

1. Open [00-roadmap.md](00-roadmap.md). Tasks are grouped into **lanes** (DSP, Windows/APO, Engine/App, UI).
2. A task is **ready** when every task in its `Depends on` row is `done`, meaning **merged into `main`**.
3. Give each agent one ready task. Two tasks in flight must never share a path in their **Owns** row (the roadmap's lane
   layout already guarantees this for tasks that can run at the same time).
4. Use the copy-paste prompts in [AGENT-PROMPTS.md](AGENT-PROMPTS.md): one orchestrator prompt and one worker prompt per task.

## Git workflow (parallel agents)

The repository is the private GitHub repo `tine` under the maintainer's account, created by T01. `main` always builds and passes CI.

| Rule | Detail |
| :--- | :--- |
| One task = one branch = one PR | Branch `task/Txx-short-name` from the latest `origin/main`. |
| One agent = one worktree | Agents never share a working folder: `git worktree add ..\tine-wt\Txx -b task/Txx-short-name origin/main`. The main checkout `D:\projects\rust\tine` belongs to the orchestrator. |
| Commits | Conventional Commits: `feat(dsp): …`, `fix(apo): …`, `docs(tasks): …`, `test(protocol): …`, `ci: …`. Small, focused commits. |
| Stay in your lane | Only touch files under the task's **Owns** (plus your task file and append-only logs). |
| Keep up to date | Before opening the PR (and if `main` moves while you're in review): `git fetch && git rebase origin/main`, then re-run the Definition of done. |
| PR | `gh pr create --base main --title "Txx: <title>" --body-file <summary>`. The body links the task file, lists acceptance criteria with ✓/✗, and pastes the `## Result` numbers. |
| Merge | Only the orchestrator (or the maintainer) merges, with **squash**, after CI is green and the review passes. Workers never merge. |
| Task status | The worker sets `in-progress` in its branch, and `done` plus the move to `tasks/done/` in its final commit. The squash merge makes it official. |
| Contract changes | A change to a frozen contract (`tine-protocol`, the `DspPipeline` API, `UiModel`/`Intent`) needs the orchestrator's OK first and updates all consumers in the same PR. |

## Lifecycle

1. Set `Status: in-progress` and the agent name.
2. Read every file under **Read first**.
3. Work only inside **Owns**. Need a change elsewhere (root `Cargo.toml`, another crate's API)? Stop and note it under
   `## Requests` in the task file; the owner of that path applies it.
4. Tick checklist items as they land. Meet every **Acceptance criterion**; run every **Verification** command.
5. Write a `## Result` section: measured numbers, decisions taken, anything surprising.
6. Fold durable knowledge into `docs/project/`. `06-decisions.md` and `07-risks.md` are **append-only logs** any task may
   append to or update its own rows in; take the next free ID when merging. All other `docs/project/` files are edited only
   when the task changes what they describe.
7. Set `Status: done`, move the file to `tasks/done/`, and open the PR (see Git workflow). The task is done when the PR is merged.

## Status values

`todo` · `in-progress (agent)` · `blocked (reason)` · `review` · `done`

## Task file template

```markdown
# Txx — Title

| Field | Value |
| :--- | :--- |
| Status | todo |
| Lane | A — DSP |
| Depends on | Tyy |
| Unblocks | Tzz |
| Owns | crates/tine-xxx/src/foo/** |
| Size | S (≤ 1 day) · M (2–4 days) · L (≥ 5 days) |

## Goal
## Read first
## Contract  (Provides / Consumes — exact types & functions)
## Steps
## Acceptance criteria
## Verification
## Out of scope
## Pitfalls
```

## Definition of done (all tasks)

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo nextest run --workspace`
- [ ] Task-specific verification commands pass; numbers recorded in `## Result`.
- [ ] No rule in [`09-engineering-standards.md`](../project/09-engineering-standards.md) broken.
- [ ] `docs/project/` updated if a contract, range, decision or risk changed.
