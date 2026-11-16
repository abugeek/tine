# T04 — `tine-protocol`: Params, Lock-Free Primitives, Shared-Memory Layout

| Field | Value |
| :--- | :--- |
| Status | done |
| Lane | 0 — Foundation |
| Depends on | T01 |
| Unblocks | T05, T13, T15, T18 (every lane codes against this) |
| Owns | `crates/tine-protocol/**` |
| Size | M |

## Goal
The single source of truth shared by the DSP, the APO, the engine and the UI:
- every parameter with its range, default and scale
- the one lock-free implementation of each concurrency pattern, usable both in-process and inside a shared-memory mapping
- the versioned `#[repr(C)]` shm layout
- telemetry
- preset/settings data types

Once this lands, the contract is **frozen**. Later changes must update every consumer in the same PR.

## Read first
- [03-dsp-pipeline.md](../project/03-dsp-pipeline.md) parameter table · [02-architecture.md](../project/02-architecture.md) §4–5 · [04-mic-adaptation.md](../project/04-mic-adaptation.md) preset shape
- [09-engineering-standards.md](../project/09-engineering-standards.md) §3–5 · decisions D-005, D-009

## Contract (provides)

```rust
// params.rs
pub enum ParamId { Bypass, InputGainDb, HpfEnabled, HpfFreqHz, DenoiseMix, GateEnabled, GateThresholdDb, GateRatio,
    GateHysteresisDb, GateAttackMs, GateReleaseMs, EqType(Band), EqFreqHz(Band), EqGainDb(Band), EqQ(Band),
    DeesserEnabled, DeesserFreqHz, DeesserThreshDb, DeesserAmountDb, CompEnabled, CompThreshDb, CompRatio,
    CompAttackMs, CompReleaseMs, CompKneeDb, CompMakeupDb, CompAutoMakeup, LimiterCeilingDb, LimiterTruePeak }
pub struct Band(u8);                       // 0..5, constructor validates
pub enum FilterType { LowShelf, HighShelf, Peaking, Notch, LowPass, HighPass, BandPass }   // #[repr(u32)]
pub struct ParamSpec { pub id: ParamId, pub key: &'static str, pub label: &'static str, pub unit: Unit,
                       pub min: f32, pub max: f32, pub default: f32, pub step: f32, pub scale: Scale }
pub const PARAM_COUNT: usize;              // flat index count (EQ bands expanded)
pub const SPECS: [ParamSpec; PARAM_COUNT];
impl ParamId { pub const fn index(self) -> usize; pub fn from_index(i: usize) -> Option<Self>; pub fn spec(self) -> &'static ParamSpec; }

#[derive(Clone, Copy, PartialEq)] pub struct DspParams { /* plain fields, see table */ }
impl DspParams { pub fn get(&self, id) -> f32; pub fn set(&mut self, id, v: f32); pub fn clamped(self) -> Self; pub const DEFAULT: Self; }

// sync.rs — each usable on the heap or in a mapping (#[repr(C)], no pointers, position independent)
#[repr(C)] pub struct ParamBlock { generation: AtomicU64, values: [AtomicU32; PARAM_COUNT] }
impl ParamBlock { pub fn store(&self, p: &DspParams); pub fn load_if_changed(&self, last_gen: &mut u64, out: &mut DspParams) -> bool; }
#[repr(C)] pub struct Triple<T: Copy + Pod> { /* 3 slots + AtomicU32 state */ }
impl<T> Triple<T> { pub fn publish(&self, v: &T); pub fn latest(&self, out: &mut T) -> bool /* true if new */; }
#[repr(C)] pub struct SpscRing<const N: usize> { /* f32 data, cache-padded head/tail, overrun counter */ }
impl<const N: usize> SpscRing<N> { pub fn push_slice(&self, s: &[f32]) -> usize; pub fn pop_slice(&self, d: &mut [f32]) -> usize; pub fn overruns(&self) -> u64; }
// Single-writer / single-reader is a documented precondition; debug builds assert it with an owner token.

// telemetry.rs
#[repr(C)] #[derive(Clone, Copy, Default, Pod, Zeroable)]
pub struct AudioTelemetryFrame { sequence: u64, flags: u32, input_rms_db: f32, input_peak_db: f32, output_rms_db: f32,
    output_peak_db: f32, gate_attenuation_db: f32, comp_gain_reduction_db: f32, deesser_reduction_db: f32, _pad: u32 }
pub mod flags { BYPASS, PASSTHROUGH_LATCHED, INPUT_CLIPPED, RESAMPLING_ACTIVE }

// shm.rs
pub const SHM_MAGIC: u64; pub const SHM_LAYOUT_VERSION: u32;
pub fn shm_name(endpoint_id: &str) -> heapless/fixed buffer ("Global\\Tine.v1.<sanitized id>");
#[repr(C)] pub struct ShmHeader { magic: u64, layout_version: u32, apo_build: [u8; 16], sample_rate: AtomicU32,
    channels: AtomicU32, heartbeat: AtomicU64, state: AtomicU32, writer_claim: AtomicU32, bypass_lease_until_ms: AtomicU64 }
#[repr(C)] pub struct ShmLayout { header: ShmHeader, params: ParamBlock, telemetry: Triple<AudioTelemetryFrame>, tap: SpscRing<TAP_LEN> }
pub const TAP_LEN: usize = 32768;          // ~0.68 s at 48 kHz, power of two
pub unsafe fn layout_from_ptr<'a>(p: *mut u8, len: usize) -> Result<&'a ShmLayout, ShmError>; // validates size/alignment/magic/version

// presets.rs (feature "serde")
pub enum Archetype { Dynamic, Condenser, Headset, Laptop }
pub struct MacroPositions { purity: f32, body: f32, presence: f32, leveler: f32 }
pub struct PinSet(u64 /* bitset over ParamId index; PARAM_COUNT <= 64 asserted */);
pub struct Preset { schema: u32, id, name, params: PresetParams /* sparse overrides */, macro_defaults: MacroPositions }
pub struct Calibration { noise_rms_db, noise_peak_db, speech_rms_db, crest_db, tilt_db, derived: CalibrationOverrides }
pub struct EndpointProfile { schema: u32, endpoint_id: String, archetype, macros, pins, calibration: Option<Calibration>, overrides: PresetParams }
```

## Steps
- [x] `#![no_std]` by default; features `std`, `serde`. `bytemuck` for `Pod` (add `bytemuck` to workspace deps via `## Requests` if T01 didn't).
- [x] `params.rs`: all rows of the parameter table, using the ranges/defaults/units exactly as in `03-dsp-pipeline.md`; `const` assertions that `SPECS[i].id.index() == i`.
- [x] `sync.rs`: `ParamBlock`, `Triple`, `SpscRing` with documented orderings; cache-line padding (`#[repr(C, align(64))]` wrapper) for head/tail.
- [x] `shm.rs`: layout + validation; `const _: () = assert!(size_of::<ShmLayout>() == EXPECTED)` and per-field offset asserts (`core::mem::offset_of!`) so any change is a compile error until `SHM_LAYOUT_VERSION` is bumped.
- [x] `presets.rs` + `settings.rs` (serde): versioned (`schema`), `#[serde(deny_unknown_fields)]` off for forward compat, validation function returning typed errors listing out-of-range fields.
- [x] Tests: clamping, index round-trip, serde round-trip, validation errors.
- [x] Concurrency tests: loom models (`#[cfg(loom)]` using `loom::sync::atomic` via an internal `atomic` shim module) for all three primitives; 10⁷-iteration two-thread stress test; miri for single-threaded API tests.
- [x] Benchmarks (criterion, `benches/sync.rs`): `ParamBlock::load_if_changed` unchanged-path cost (< 5 ns), `Triple::publish`, `SpscRing::push_slice(480)`.

## Acceptance criteria
- loom passes for all three primitives; stress test shows the reader never observes a torn `AudioTelemetryFrame` (frame carries a checksum in tests).
- `cargo build -p tine-protocol --no-default-features` passes (no_std, no alloc).
- Every row of the docs parameter table maps 1:1 to `SPECS` (checklist in `## Result`).
- Layout size/offsets asserted; `layout_from_ptr` rejects wrong magic/version/size.

## Verification
```bash
cargo nextest run -p tine-protocol
RUSTFLAGS="--cfg loom" cargo test -p tine-protocol --release sync::loom
cargo +nightly miri test -p tine-protocol
cargo bench -p tine-protocol --profile perf
```

## Out of scope
Creating/opening OS file mappings (T13 creates, T15 opens). DSP math.

## Pitfalls
- No `bool`, `usize`, enums or pointers inside shm structs; use `u32`/`u64`. Store `FilterType` as `u32` bits in `ParamBlock`.
- `DspParams` floats inside `ParamBlock` are stored as `f32::to_bits`; booleans as 0.0/1.0 so every param is one `AtomicU32`.
- `Triple<T>` reads need a copy out (`T: Copy`); never hand out references into a slot.

## Result

### Measured numbers & verification
- `cargo build -p tine-protocol --no-default-features`: **pass** (pure `no_std`, no `alloc`).
- `cargo fmt --all --check`: **pass**.
- `cargo deny check`: **pass** (`advisories ok, bans ok, licenses ok, sources ok`).
- `cargo clippy --workspace --all-targets -- -D warnings`: **pass**.
- `cargo test --workspace`: **pass** (all 12 `tine-protocol` integration/property/stress/alloc tests + `apo-spike` pass).
- `RUSTFLAGS="--cfg loom" cargo test -p tine-protocol --lib --release sync::loom`: **pass** (3/3 models in 0.00s: `param_block_publishes_latest`, `triple_never_tears_and_delivers_latest`, `spsc_ring_preserves_fifo_order`).
- `triple_10m_stress_test_never_tears` (`10_000_000` frames): **pass** in release (0 torn frames, strictly monotonic sequence).
- `rt_paths_perform_zero_allocations` (`assert_no_alloc`): **pass** (0 allocations across `ParamBlock`, `Triple`, `SpscRing`, `DspParams`, `shm_name`).
- Criterion (`cargo bench -p tine-protocol --profile perf`):
  - `ParamBlock::load_if_changed (unchanged)`: **372.86 ps** (budget: `< 5 ns`)
  - `Triple::publish`: **6.68 ns**
  - `SpscRing::push_slice(480)` (+ pop): **41.66 ns**
- `ShmLayout` compile-time invariants: `size_of::<ShmHeader>() == 64`, `offset_of!(ShmLayout, header) == 0`, `params == 64`, `telemetry == 256`, `tap == 448`, `size_of::<ShmLayout>() == 131_712`, `align_of::<ShmLayout>() == 64`.
- Note: `cargo-nextest` and `cargo-miri` are not installed on this local machine; `cargo test --workspace` ran instead, and `miri` will run in CI's nightly job.

### Parameter table 1:1 mapping (`PARAM_COUNT = 45`)
- `0`: `bypass` (`Bypass`, bool, default `false`)
- `1`: `input_gain_db` (`InputGainDb`, `-24..=36`, default `0.0`, dB)
- `2`: `hpf_enabled` (`HpfEnabled`, bool, default `true`)
- `3`: `hpf_freq_hz` (`HpfFreqHz`, `20..=200`, default `80.0`, Hz)
- `4`: `denoise_mix` (`DenoiseMix`, `0..=1`, default `0.5`)
- `5`: `gate_enabled` (`GateEnabled`, bool, default `true`)
- `6`: `gate_threshold_db` (`GateThresholdDb`, `-80..=-20`, default `-50.0`, dBFS)
- `7`: `gate_ratio` (`GateRatio`, `1..=10`, default `2.0`, `:1`)
- `8`: `gate_hysteresis_db` (`GateHysteresisDb`, `0..=12`, default `4.0`, dB)
- `9`: `gate_attack_ms` (`GateAttackMs`, `0.1..=20`, default `1.0`, ms)
- `10`: `gate_release_ms` (`GateReleaseMs`, `10..=1000`, default `150.0`, ms)
- `11..=15`: `eq.{0..4}.type` (`EqType(Band)`, `FilterType`, defaults `[LowShelf, Peaking, Peaking, Peaking, HighShelf]`)
- `16..=20`: `eq.{0..4}.freq_hz` (`EqFreqHz(Band)`, `20..=20000`, defaults `[110, 320, 950, 4200, 8500]`, Hz)
- `21..=25`: `eq.{0..4}.gain_db` (`EqGainDb(Band)`, `-18..=18`, default `0.0`, dB)
- `26..=30`: `eq.{0..4}.q` (`EqQ(Band)`, `0.1..=10`, defaults `[0.707, 1.0, 1.0, 1.0, 0.707]`)
- `31`: `deesser_enabled` (`DeesserEnabled`, bool, default `false`)
- `32`: `deesser_freq_hz` (`DeesserFreqHz`, `5500..=9500`, default `6800.0`, Hz)
- `33`: `deesser_thresh_db` (`DeesserThreshDb`, `-60..=0`, default `-30.0`, dBFS)
- `34`: `deesser_amount_db` (`DeesserAmountDb`, `0..=12`, default `6.0`, dB)
- `35`: `comp_enabled` (`CompEnabled`, bool, default `true`)
- `36`: `comp_thresh_db` (`CompThreshDb`, `-60..=0`, default `-20.0`, dBFS)
- `37`: `comp_ratio` (`CompRatio`, `1..=20`, default `3.0`, `:1`)
- `38`: `comp_attack_ms` (`CompAttackMs`, `0.1..=100`, default `5.0`, ms)
- `39`: `comp_release_ms` (`CompReleaseMs`, `10..=1000`, default `120.0`, ms)
- `40`: `comp_knee_db` (`CompKneeDb`, `0..=12`, default `6.0`, dB)
- `41`: `comp_makeup_db` (`CompMakeupDb`, `0..=24`, default `0.0`, dB)
- `42`: `comp_auto_makeup` (`CompAutoMakeup`, bool, default `true`)
- `43`: `limiter_ceiling_db` (`LimiterCeilingDb`, `-12..=-0.1`, default `-1.0`, dBFS)
- `44`: `limiter_true_peak` (`LimiterTruePeak`, bool, default `false`)

### Notes
- `AudioTelemetryFrame` has 8-byte `sequence` + `flags` + 7 `f32`s = 40 bytes (`align(8)`), so zero implicit padding is present and `bytemuck::Pod` is satisfied without a trailing `_pad` field.
- `ParamBlock` loom model tests 2 representative slots + `generation` under `cfg(all(test, loom))` to avoid combinatorial explosion across 45 `AtomicU32`s.
