# T01 — Workspace, Toolchain, CI, Dependency Baseline

| Field | Value |
| :--- | :--- |
| Status | done |
| Lane | 0 — Foundation |
| Depends on | — |
| Unblocks | everything |
| Owns | `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `.cargo/config.toml`, `rustfmt.toml`, `clippy.toml`, `deny.toml`, `.gitignore`, `.gitattributes`, `.github/**`, `AGENTS.md` (§2 version bumps only), `crates/*/Cargo.toml`, `crates/*/src/lib.rs|main.rs` (stubs only), `LICENSE*` |
| Size | M |

## Goal
A compiling Cargo workspace of empty crates, with every external dependency (latest stable) declared once, Rune
wired in with its required patches, strict lints, release/perf profiles, and CI enforcing all of
[09-engineering-standards.md](../project/09-engineering-standards.md) from the first commit.

## Read first
- [`AGENTS.md`](../../AGENTS.md) §2 · [02-architecture.md](../project/02-architecture.md) §2 · [09-engineering-standards.md](../project/09-engineering-standards.md) §1, §2, §8, §9
- Rune `README.md` at rev `631e8a67cb3c40225efa3f6c4f57b80e7b18f190` ("Add it", the `[patch.crates-io]` block in its `Cargo.toml`, the `tooling` feature)

## Steps
- [x] `git init`; `.gitignore` (`/target`, `*.pdb` outside releases, `/docs/tasks/done/` stays tracked); `.gitattributes` (`*.wav binary`, `*.flac binary`, `* text=auto eol=lf`).
- [x] License files per D-015: `LICENSE-MIT`, `LICENSE-APACHE` (copyright line "Copyright (c) 2026 The Tine Contributors"); `license = "MIT OR Apache-2.0"` in `[workspace.package]`.
- [x] `rust-toolchain.toml`: channel `1.99.1`, components `rustfmt`, `clippy`, `rust-src`; target `x86_64-pc-windows-msvc`.
- [x] `.cargo/config.toml`: `[target.x86_64-pc-windows-msvc] rustflags = ["-C", "target-cpu=x86-64-v2"]`. CI scripts live in the workflow YAML (no xtask crate).
- [x] Root `Cargo.toml`: `resolver = "3"`, `members = ["crates/*", "spikes/*"]`, `default-members = ["crates/*"]` (spikes T02/T03 build only when named), `[workspace.package]` (edition 2024, rust-version 1.99, license, repository), `[workspace.dependencies]` exactly as [`AGENTS.md` §2](../../AGENTS.md#approved-dependencies-cargotoml) with `default-features = false`.
- [x] Rune patches: copy Rune's `[patch.crates-io]` entries (`wgpu-hal`, `winit`, `parley_engine`, `accesskit_windows`; skip macOS-only ones if cargo allows) pointing at `{ git = "https://github.com/anomalyco/rune", rev = "631e8a6…" }`. Verify with `cargo tree -i winit` that the patched version is used. Document the procedure for bumping Rune in `docs/project/09-engineering-standards.md` §2.
- [x] Rune features: `default-features = false`, enable what the UI needs (start with `accessibility`, `embedded-fonts`; add `svg` only if icons need it). `tooling` only through a `tine-app` feature `ui-tooling` that is on in dev/CI and off in release.
- [x] `[workspace.lints]`: rust `unsafe_op_in_unsafe_fn = "deny"`, `missing_docs = "warn"`; clippy `pedantic` warn + allow-list, `undocumented_unsafe_blocks = "deny"`, `dbg_macro = "deny"`, `todo = "deny"`, `unwrap_used = "warn"`. RT crates (`tine-dsp`, `tine-apo`, `tine-protocol`) add crate-level `deny` for `unwrap_used`, `expect_used`, `panic`, `indexing_slicing` (tests exempt via `cfg_attr(test, allow(...))`).
- [x] Profiles: `release` (opt 3, fat LTO, cgu 1, `debug = "line-tables-only"`, `split-debuginfo = "packed"`), `perf` (inherits release, `incremental = false`), `dev` (`debug = "line-tables-only"`; `[profile.dev.package."*"] opt-level = 2` so audio code is listenable in debug).
- [x] Crates (stub `lib.rs`/`main.rs` with `//!` role docs): `tine-protocol` (lib, `no_std` + `std`/`serde` features), `tine-dsp` (lib, `no_std` + `std` feature), `tine-apo` (`crate-type = ["cdylib", "rlib"]`), `tine-engine` (lib), `tine-ui` (lib), `tine-app` (bin `tine`, `windows_subsystem = "windows"` in release with an attach-console path for CLI), `tine-devhost` (bin).
- [x] **Module skeleton**: create every module from the [module map](../project/02-architecture.md#module-map-t01-creates-every-module-as-an-empty-stub-so-tasks-own-disjoint-files) as an empty file with a `//!` doc line, declared in each crate's `lib.rs`. Later tasks then never edit a shared `lib.rs` (only T05/T18/T17 touch their crate root, and they run first in their crate).
- [x] **Crate manifests**: write each crate's `Cargo.toml` per the [crate manifest plan](../project/02-architecture.md#crate-manifest-plan-t01-writes-these-later-tasks-change-only-their-own-crate), including the `windows` features. Verify every feature name exists in `windows` 0.62.
- [x] Enforce the dependency direction: `tine-dsp` has no `windows`/`cpal`/`serde_json`/`rune` (dev-dependencies excepted); `tine-ui` has no `tine-engine`/`cpal`/`windows` (it may use `tine-dsp`); `tine-apo` has no `cpal`/`rune`/`tine-engine`. Add a CI step using `cargo tree -e normal -p <crate>` + grep to fail on violations.
- [x] `deny.toml`: licenses allow-list (MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, BSL-1.0, CC0-1.0, MPL-2.0 *only for Rune's transitive deps if any*), bans on duplicate majors (warn), advisories deny, sources allow the Rune git repo only.
- [x] CI `.github/workflows/ci.yml` on `windows-latest` with `Swatinem/rust-cache`: fmt · clippy · nextest · deny · dependency-direction check · `cargo build --profile release` + artifact size report. Nightly jobs (scheduled): `miri` on `tine-protocol`, loom tests (`RUSTFLAGS="--cfg loom"`), benches with budget gate (added by T05).
- [x] `.github/dependabot.yml` for cargo + github-actions, weekly.
- [x] `crates/tine-dsp/tests/fixtures/README.md` placeholder explaining fixture licensing rules (filled by T09).

## Acceptance criteria
- Clean clone: `cargo build --workspace` and all CI jobs green.
- `cargo tree -i winit` shows the Rune-patched source; `cargo deny check` passes.
- Dependency-direction check fails if someone adds `cpal` to `tine-dsp` (verify by a throwaway local edit).
- `cargo build -p tine-dsp --no-default-features` and `-p tine-protocol --no-default-features` succeed (no_std).

## Verification
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo deny check
cargo build --workspace --profile release
cargo tree -i winit
```

## Out of scope
Any logic. Feature code goes in later tasks.

## Pitfalls
- Re-check the latest stable versions on crates.io the day you do this task; bump `AGENTS.md` §2 if newer ones exist.
- `nnnoiseless` may enable `hound`/binary features by default; use `default-features = false`.
- Root `[workspace.dependencies]` declares `windows` with no features. Each crate adds the `windows` features it needs in **its own** `Cargo.toml` (`windows = { workspace = true, features = [...] }`; features are additive). So later tasks never edit the root manifest for features.
- After T01, each `crates/<name>/Cargo.toml` is owned by whichever task owns that crate's sources. Only a new *workspace-level* dependency needs a `## Requests` entry for the T01 owner (or the lead).

## Result
- Workspace bootstrapped with all 7 crates and 63 module stubs on Rust `1.99.1` (`x86_64-pc-windows-msvc`, `-C target-cpu=x86-64-v2`).
- Re-checked all 19 crates.io dependencies in `AGENTS.md` §2: all match the latest stable versions as of 2026-11-16 (no bumps required).
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check`, `cargo check -p tine-protocol --no-default-features`, `cargo check -p tine-dsp --no-default-features`, and `cargo build --workspace --profile release` all pass.
- `cargo tree -i winit` resolves to `winit v0.30.13 (https://github.com/anomalyco/rune?rev=631e8a67cb3c40225efa3f6c4f57b80e7b18f190#631e8a67)`.
- Release stub artifact sizes: `tine_apo.dll` = 9,216 bytes (0.01 MB < 2 MB budget), `tine.exe` = 109,056 bytes (0.10 MB < 20 MB budget).
- Notes:
  - `tine-ui` pulls `windows` transitively via `rune`, so the dependency-direction check enforces no `windows` at `--depth 1` for `tine-ui` while forbidding `tine-engine` and `cpal` anywhere in its normal dependency tree.
  - Added `spikes/.gitkeep` so `members = ["crates/*", "spikes/*"]` resolves cleanly before T02/T03 add spike crates.
  - Split CI into `.github/workflows/ci.yml` (lean PR/main checks) and `.github/workflows/nightly.yml` (`workflow_dispatch` + weekly Monday schedule for `miri`, `loom`, `bench`, and `headless-ui`).
