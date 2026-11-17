# Contributing to Tine

Thanks for helping! Tine is a real-time audio project with a few unusual
rules, so please read this short guide before your first PR. The deep
reference docs live in [`AGENTS.md`](AGENTS.md) and
[`docs/project/`](docs/project/) — they bind human and AI contributors
alike.

## The short version

1. Work in issues / task-shaped PRs. Small, focused, reviewable.
2. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo nextest run --workspace` all green before you push.
3. Never break the real-time guarantees (below). Never add a dependency
   without checking `deny.toml` and the dependency direction.
4. License: **inbound = outbound** — MIT OR Apache-2.0, no CLA. Your
   contributions land under the same dual license as the project.

## How work is organized

Tine's work is tracked in task files under [`docs/tasks/`](docs/tasks/)
(lanes: DSP, Windows/routing, Engine/App, UI, ship), and lasting knowledge
in [`docs/project/`](docs/project/) — the decisions log
([`06-decisions.md`](docs/project/06-decisions.md)) is the project's memory
and takes precedence over older prose. If you're picking up an existing task
file, follow its **Steps** and **Acceptance criteria**; if you're
proposing something new, open an issue first so a task file (and lane
ownership) can be agreed.

## Real-time rules (the ones that get a PR rejected)

Every audio-callback path (`DspPipeline::process`, the engine's capture and
cable-output callbacks) must be **allocation-free, lock-free and
panic-free**:

- no `Mutex`/`RwLock`, `Vec::push`, `Box::new`, logging, or syscalls in the
  callback;
- RT crates deny `clippy::{unwrap_used, expect_used, panic, indexing_slicing}`;
- zero-allocation is enforced in tests with `assert_no_alloc`;
- a fault must latch to bypass/pass-through, never take audio down.

DSP status lives in cabinets: parameters cross threads through the
lock-free `Triple`/`SpscRing` primitives in `tine-protocol` (plain
`repr(C)` shared-memory structs — don't add `std` deps to `tine-protocol`).
`tine-dsp` must stay platform-neutral (no `windows`/`cpal`/`serde_json`
imports — CI enforces the dependency direction).

## Adding / bumping dependencies

- Prefer writing small, hot, well-understood code ourselves (D-009, D-010) —
  every new crate needs a justification in the PR, a license on the
  `deny.toml` allow-list (no GPL — it would viral-infect the binary), and no
  duplicate major versions (`cargo deny check bans`).
- **Rune** (the UI framework) is pre-release and pinned to a `rev` with
  mirrored `[patch.crates-io]` entries. Bump it only in a dedicated PR:
  update the rev *and* the four patch lines together, then re-run the
  headless UI tests (`cargo test -p tine-ui`, re-bless PNGs with
  `UPDATE_SNAPSHOTS=1` only when intentional) and the lifecycle checks.

## UI contribution notes

All window content comes from the plain view-model types in
`tine-ui::model` — engine types stay behind one adapter module per contract.
Headless scene tests render the UI to PNG + an accessibility tree and gate
overlap, contrast (≥ 4.5:1) and focus order; run them locally, they don't
need a GPU.

## Releases

Releases are built from a `v*` tag by
[`.github/workflows/release.yml`](.github/workflows/release.yml) (draft
Release; publishing and code signing are maintainer actions — see the
runbook). Don't bump versions inside feature PRs; the workspace version and
`CHANGELOG.md` move together, in the release PR.

## Reporting problems

Bugs: use the bug-report issue template and attach the output of
`tine diag` (privacy-safe: no audio, hashed device ids). New microphone
archetype requests: the preset-request template — real recordings (48 kHz
raw, a few seconds each: speech / quiet room / sibilants) help enormously.

## Security

See [SECURITY.md](SECURITY.md) — please don't open public issues for
anything security-sensitive.
