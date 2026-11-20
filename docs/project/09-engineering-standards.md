# 09 — Engineering Standards

The fundamentals every line of Tine code follows from day one. A task is not done if it breaks one of these.
Reviewers (human or agent) check against this file.

## 1. Toolchain & build baseline

| Item | Value | Why |
| :--- | :--- | :--- |
| Rust | **1.99.1** stable, pinned in `rust-toolchain.toml` | Same as Rune's pinned toolchain; reproducible builds. |
| Edition / resolver | **2024** / **3** | Current edition; resolver 3 is MSRV-aware. |
| Target | `x86_64-pc-windows-msvc` | Rune + `windows` crate first-class; no MinGW `dlltool` issues. |
| CPU baseline | `-C target-cpu=x86-64-v2` (`.cargo/config.toml`) | SSE4.2/POPCNT: every Windows 11 machine has it, and it unlocks better auto-vectorization. No AVX runtime dispatch in v0.1. |
| Release profile | `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `debug = "line-tables-only"`, `split-debuginfo = "packed"` (PDBs shipped as release assets, not installed) | Small, fast binaries with symbolized crash reports. |
| `perf` profile | inherits release, `incremental = false` | Benchmarks only run on this profile. |
| Panic strategy | `unwind` (default) everywhere | Standard Rust unwinding across the workspace. |

## 2. Dependency policy

- **Latest stable at the time of adding**, declared once in root `[workspace.dependencies]`; crates use `dep.workspace = true`.
  Current baseline (checked 2026-11-16) lives in [`AGENTS.md` §2](../../AGENTS.md#approved-dependencies-cargotoml).
- `default-features = false` on every dependency, then enable only what is used.
- Every new dependency needs: a reason in the PR, a license on the allow-list (`deny.toml`), no duplicate major versions (`cargo deny check bans`), and no RT-path usage unless it is proven allocation-free under test.
- Prefer **writing small, hot, well-understood code ourselves** (biquads, lock-free triple buffer/ring, smoothers) over pulling a crate: we need `repr(C)` shared-memory versions anyway, we control allocation and inlining, and there is one tested implementation (D-009, D-010).
- Dependabot (or Renovate) weekly for Cargo + GitHub Actions; `cargo deny check advisories` in CI.
- Rune is pinned by `rev`; bumping it is its own PR that re-runs headless snapshots and T03's memory checks:
  1. Update `rev` on `rune` in `[workspace.dependencies]` and on every entry in `[patch.crates-io]` in root `Cargo.toml` (and `AGENTS.md` §2).
  2. Inspect Rune's `Cargo.toml` and `rust-toolchain.toml` at the new `rev` for any changed `[patch.crates-io]` entries or toolchain version.
  3. Run `cargo update -p rune` and verify `cargo tree -i winit` resolves to the new `rev`.
  4. Run `cargo deny check`, headless UI snapshots (`tine-ui`), and T03's memory/lifecycle checks.

## 3. Real-time code rules (DSP, APO, audio callbacks)

1. **No allocation, locks, syscalls, logging, I/O, or panics** after `prepare()`. Enforced with `assert_no_alloc` in every RT test and `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::arithmetic_side_effects)]` in RT modules (arithmetic lint for integer index math only — allow per-fn for float math with a comment).
2. **Static dispatch only.** `DspPipeline` is a concrete struct of concrete stages; no `dyn`, no `Box<dyn Stage>` in the hot path.
3. **Block processing, stage by stage.** Each stage processes the whole block (`&mut [f32]`) before the next, so loops stay tight and auto-vectorize. Per-sample state (smoothers, envelopes) lives in local variables inside the loop and is written back once.
4. **Coefficients off the per-sample path.** Smooth the *parameters* per sample; recompute biquad coefficients at most every 32 samples, and only when the smoothed value moved by more than ε. Design coefficients in `f64`, run filters in `f32`.
5. **No transcendental math per sample** in hot loops unless benchmarked: precompute `exp` coefficients; dB↔linear in the detector uses a validated fast approximation (max error < 0.01 dB) only if the bench proves the win.
6. **Bounds checks eliminated**, not suppressed: `chunks_exact`, iterators, `zip`, fixed-size arrays, power-of-two ring masks. No `get_unchecked` without a benchmark and a `// SAFETY:` comment.
7. **Denormals**: FTZ/DAZ set on RT entry (`DenormalGuard`), restored on exit.
8. **Fixed latency**: total latency never changes at runtime (toggles cross-fade, never re-plumb).
9. **Always finite output**: NaN/Inf guard at the end of the chain → reset + pass-through latch.
10. Memory is preallocated as `Box<[f32]>` sized for `max_block` (and 192 kHz for time-based buffers), allocated in `prepare()`.

## 4. Concurrency rules

- One lock-free implementation per pattern (`tine-protocol::sync`): `Triple<T>` (latest-value), `SpscRing` (stream), `ParamBlock` (generation-stamped params). Each works both in-process and inside a shared-memory mapping.
- Every atomic `Ordering` other than `Relaxed` has a comment naming the paired operation.
- All three primitives have **loom** model tests and a 10⁷-iteration stress test; `miri` runs on `tine-protocol` non-loom tests in CI.
- No `Mutex`/`RwLock` anywhere the UI thread could wait on audio, or vice versa.

## 5. Unsafe policy

- `unsafe` allowed only in: `tine-protocol::{sync, shm}`, `tine-engine::win` (Win32/COM), `tine-dsp::rt` (FTZ intrinsics), and `tine-app` Win32 wrappers.
- `clippy::undocumented_unsafe_blocks = "deny"`; every block has `// SAFETY:` stating the invariant.
- FFI/COM entry points: `catch_unwind`; never let a panic cross an FFI boundary.

## 6. Error handling & logging

- Libraries return typed errors (`thiserror` 2). No `anyhow` in libraries. `tine-app` maps errors to user messages.
- `tracing` for logs in non-RT code only; `tine-app` installs an in-house size-rotating file sink in `%LOCALAPPDATA%\Tine\logs\tine.log` (7 files × 2 MiB, D-025).
- Audio RT callbacks never log; they report state through atomic flags/telemetry, which `tine.exe` logs.

## 7. Performance budgets (CI-enforced where marked ✓)

| Budget | Limit | Where measured |
| :--- | :--- | :--- |
| DSP, 480 samples, all stages, 48 kHz ✓ | < 150 µs hard, < 60 µs target | `cargo bench -p tine-dsp` (T05/T09) |
| Allocations in any RT path ✓ | 0 | `assert_no_alloc` tests |
| `tine.exe` idle, window closed | < 0.5 % CPU, < 12 MB working set (< 4 MB private bytes, hard 20 MB), 0 GPU memory | T03, T22 (D-017, R-09) |
| `tine.exe --ui` window open, 60 Hz | < 3 % CPU, frame build+encode < 4 ms | T19 |
| Cold start to tray | < 150 ms | T17 |
| Window open (closed → first frame) | < 250 ms | T03 |
| Added latency | ≤ 21.6 ms at 48 kHz (1036 samples, per D-019) | T09 |
| `tine.exe` size ✓ | < 30 MiB (release, no `tooling`; D-037) | CI + release artifact check |

## 8. Testing pyramid

| Layer | Tool | Required for |
| :--- | :--- | :--- |
| Unit + property | `cargo nextest`, `proptest` | every crate |
| Concurrency models | `loom` | `tine-protocol::sync` |
| UB check | `miri` | `tine-protocol` |
| DSP measurement | in-repo harness (sweeps, FFT response, impulse latency) | every stage |
| Golden audio regression | processed-fixture metrics (peak, RMS, spectrum bands) with tolerances, not byte hashes | `tine-dsp` |
| Fuzz / property | `cargo fuzz` / `proptest` on `DspPipeline::process` + param inputs | `tine-dsp` |
| Headless UI | Rune `--snap`/`--tree` + Rust checks | `tine-ui` |
| Soak / system | `tine soak` | release |

CI runs on `windows-latest`: fmt, clippy (`-D warnings`), nextest, loom (nightly job), miri (nightly job), deny, bench budget check, headless UI scenes, artifact size check.

## 9. Code style & hygiene

- `rustfmt.toml`: `max_width = 120`, `imports_granularity = "Crate"`, `group_imports = "StdExternalCrate"` (format with the pinned toolchain).
- Lints in `[workspace.lints]`: `clippy::pedantic` warn (documented allow-list), `missing_docs` warn on library crates, `unsafe_op_in_unsafe_fn` deny.
- Public items documented; each crate has a top-level `//!` explaining its role and its RT/non-RT boundary.
- Commits: Conventional Commits (`feat(dsp): …`), one task per PR, PR description links the task file.
- No dead code, no commented-out code, no TODOs without an issue link.

## 10. Parallel-agent workflow

- Each task file declares **Owns** (paths it may edit). Two tasks in flight never own the same path.
- Root `Cargo.toml`, `deny.toml`, `.cargo/config.toml`, CI workflows are owned by T01; later tasks request changes through their PR description instead of editing them concurrently. All workspace dependencies are declared up front in T01, so later tasks only touch their own crate's `Cargo.toml`.
- Contracts first: `tine-protocol` (T04) and the `DspPipeline` API skeleton (T05) land before the lanes fan out, so every lane codes against stable interfaces.
- Changing a contract after it lands requires updating every consumer in the same PR, plus `docs/project/`.
