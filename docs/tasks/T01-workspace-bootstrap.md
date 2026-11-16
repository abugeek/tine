# T01 — Workspace, Toolchain, CI, Dependency Baseline

| Field | Value |
| :--- | :--- |
| Status | todo |
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
- [ ] `git init`; `.gitignore` (`/target`, `*.pdb` outside releases, `/docs/tasks/done/` stays tracked); `.gitattributes` (`*.wav binary`, `*.flac binary`, `* text=auto eol=lf`).
- [ ] License files per D-015: `LICENSE-MIT`, `LICENSE-APACHE` (copyright line "Copyright (c) 2026 The Tine Contributors"); `license = "MIT OR Apache-2.0"` in `[workspace.package]`.
- [ ] `rust-toolchain.toml`: channel `1.99.1`, components `rustfmt`, `clippy`, `rust-src`; target `x86_64-pc-windows-msvc`.
- [ ] `.cargo/config.toml`: `[target.x86_64-pc-windows-msvc] rustflags = ["-C", "target-cpu=x86-64-v2"]`. CI scripts live in the workflow YAML (no xtask crate).
- [ ] Root `Cargo.toml`: `resolver = "3"`, `members = ["crates/*", "spikes/*"]`, `default-members = ["crates/*"]` (spikes T02/T03 build only when named), `[workspace.package]` (edition 2024, rust-version 1.99, license, repository), `[workspace.dependencies]` exactly as [`AGENTS.md` §2](../../AGENTS.md#approved-dependencies-cargotoml) with `default-features = false`.
- [ ] Rune patches: copy Rune's `[patch.crates-io]` entries (`wgpu-hal`, `winit`, `parley_engine`, `accesskit_windows`; skip macOS-only ones if cargo allows) pointing at `{ git = "https://github.com/anomalyco/rune", rev = "631e8a6…" }`. Verify with `cargo tree -i winit` that the patched version is used. Document the procedure for bumping Rune in `docs/project/09-engineering-standards.md` §2.
- [ ] Rune features: `default-features = false`, enable what the UI needs (start with `accessibility`, `embedded-fonts`; add `svg` only if icons need it). `tooling` only through a `tine-app` feature `ui-tooling` that is on in dev/CI and off in release.
- [ ] `[workspace.lints]`: rust `unsafe_op_in_unsafe_fn = "deny"`, `missing_docs = "warn"`; clippy `pedantic` warn + allow-list, `undocumented_unsafe_blocks = "deny"`, `dbg_macro = "deny"`, `todo = "deny"`, `unwrap_used = "warn"`. RT crates (`tine-dsp`, `tine-apo`, `tine-protocol`) add crate-level `deny` for `unwrap_used`, `expect_used`, `panic`, `indexing_slicing` (tests exempt via `cfg_attr(test, allow(...))`).
- [ ] Profiles: `release` (opt 3, fat LTO, cgu 1, `debug = "line-tables-only"`, `split-debuginfo = "packed"`), `perf` (inherits release, `incremental = false`), `dev` (`debug = "line-tables-only"`; `[profile.dev.package."*"] opt-level = 2` so audio code is listenable in debug).
- [ ] Crates (stub `lib.rs`/`main.rs` with `//!` role docs): `tine-protocol` (lib, `no_std` + `std`/`serde` features), `tine-dsp` (lib, `no_std` + `std` feature), `tine-apo` (`crate-type = ["cdylib", "rlib"]`), `tine-engine` (lib), `tine-ui` (lib), `tine-app` (bin `tine`, `windows_subsystem = "windows"` in release with an attach-console path for CLI), `tine-devhost` (bin).
- [ ] **Module skeleton**: create every module from the [module map](../project/02-architecture.md#module-map-t01-creates-every-module-as-an-empty-stub-so-tasks-own-disjoint-files) as an empty file with a `//!` doc line, declared in each crate's `lib.rs`. Later tasks then never edit a shared `lib.rs` (only T05/T18/T17 touch their crate root, and they run first in their crate).
- [ ] **Crate manifests**: write each crate's `Cargo.toml` per the [crate manifest plan](../project/02-architecture.md#crate-manifest-plan-t01-writes-these-later-tasks-change-only-their-own-crate), including the `windows` features. Verify every feature name exists in `windows` 0.62.
- [ ] Enforce the dependency direction: `tine-dsp` has no `windows`/`cpal`/`serde_json`/`rune` (dev-dependencies excepted); `tine-ui` has no `tine-engine`/`cpal`/`windows` (it may use `tine-dsp`); `tine-apo` has no `cpal`/`rune`/`tine-engine`. Add a CI step using `cargo tree -e normal -p <crate>` + grep to fail on violations.
- [ ] `deny.toml`: licenses allow-list (MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, BSL-1.0, CC0-1.0, MPL-2.0 *only for Rune's transitive deps if any*), bans on duplicate majors (warn), advisories deny, sources allow the Rune git repo only.
- [ ] CI `.github/workflows/ci.yml` on `windows-latest` with `Swatinem/rust-cache`: fmt · clippy · nextest · deny · dependency-direction check · `cargo build --profile release` + artifact size report. Nightly jobs (scheduled): `miri` on `tine-protocol`, loom tests (`RUSTFLAGS="--cfg loom"`), benches with budget gate (added by T05).
- [ ] `.github/dependabot.yml` for cargo + github-actions, weekly.
- [ ] `crates/tine-dsp/tests/fixtures/README.md` placeholder explaining fixture licensing rules (filled by T09).

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
