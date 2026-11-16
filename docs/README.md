# Tine Documentation

This folder has two halves with different lifetimes:

| Folder | Lifetime | What goes here |
| :--- | :--- | :--- |
| [`project/`](project/) | **Permanent.** Kept true as the code evolves. | Why Tine exists, who it is for, architecture, DSP reference, decisions, risks, glossary. |
| [`tasks/`](tasks/) | **Temporary.** A task file is deleted or archived once it lands. | Implementation plans and work packages for AI agents and contributors. |

`AGENTS.md` at the repo root is the original specification. Where it disagrees with
[`project/06-decisions.md`](project/06-decisions.md), the decisions log wins (each entry says what changed and why).

## Reading order

1. [project/01-vision.md](project/01-vision.md) — the problem and who we are solving it for
2. [project/02-architecture.md](project/02-architecture.md) — processes, crates, data flow
3. [project/03-dsp-pipeline.md](project/03-dsp-pipeline.md) — the 7-stage chain, parameter ranges
4. [project/04-mic-adaptation.md](project/04-mic-adaptation.md) — archetypes, Auto-Tune, macros
5. [project/05-ui-ux.md](project/05-ui-ux.md) — screens and interaction model
6. [project/06-decisions.md](project/06-decisions.md) — decision log (ADRs)
7. [project/07-risks.md](project/07-risks.md) — known risks and open questions
8. [project/08-glossary.md](project/08-glossary.md) — audio and Windows terms
9. [project/09-engineering-standards.md](project/09-engineering-standards.md) — build baseline, dependency policy, real-time rules, performance budgets, testing
10. [tasks/00-roadmap.md](tasks/00-roadmap.md) — lanes, task index, parallelism

## Rules for keeping docs honest

- A change that alters architecture, a parameter range, or a public contract updates `project/` in the same commit.
- A new irreversible choice gets an entry in `06-decisions.md`.
- A finished task moves to `tasks/done/` (or is deleted) and its lasting knowledge is folded into `project/`.
