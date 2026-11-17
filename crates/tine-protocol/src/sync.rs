//! Lock-free `#[repr(C)]` concurrency primitives usable both in-process and inside
//! shared memory mappings.
//!
//! Every primitive is position-independent (no pointers or `usize`), panic-free,
//! allocation-free, and verified with `loom` and stress tests.

use core::mem::MaybeUninit;

use bytemuck::Pod;

#[cfg(not(all(test, loom)))]
use crate::params::{Band, EQ_BAND_COUNT, FilterType};
use crate::params::{DspParams, PARAM_COUNT, ParamId};

#[cfg(all(test, loom))]
mod atomic {
    pub(crate) use loom::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};
}

#[cfg(not(all(test, loom)))]
mod atomic {
    pub(crate) use core::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};
}

use atomic::{AtomicU32, AtomicU64, Ordering, fence};

#[repr(transparent)]
pub(crate) struct SyncCell<T>(core::cell::UnsafeCell<T>);

impl<T> SyncCell<T> {
    #[inline]
    pub(crate) const fn new(val: T) -> Self {
        Self(core::cell::UnsafeCell::new(val))
    }

    #[inline]
    pub(crate) unsafe fn with<R>(&self, f: impl FnOnce(*const T) -> R) -> R {
        f(self.0.get())
    }

    #[inline]
    pub(crate) unsafe fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R {
        f(self.0.get())
    }
}

/// Helper for storing an `f32` inside an [`AtomicU32`] with `Relaxed` ordering.
#[repr(transparent)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    /// Creates a new [`AtomicF32`] initialized to `val`.
    #[cfg(not(all(test, loom)))]
    #[must_use]
    pub const fn new(val: f32) -> Self {
        Self(AtomicU32::new(val.to_bits()))
    }

    /// Creates a new [`AtomicF32`] initialized to `val`.
    #[cfg(all(test, loom))]
    #[must_use]
    pub fn new(val: f32) -> Self {
        Self(AtomicU32::new(val.to_bits()))
    }

    /// Loads the current `f32` value (`Relaxed`).
    #[inline]
    #[must_use]
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    /// Stores a new `f32` value (`Relaxed`).
    #[inline]
    pub fn store(&self, val: f32) {
        self.0.store(val.to_bits(), Ordering::Relaxed);
    }
}

/// 64-byte cache-line-aligned wrapper to prevent false sharing between producer and consumer indices.
#[repr(C, align(64))]
pub struct CachePadded<T>(pub T);

// ---------------------------------------------------------------------------
// ParamBlock
// ---------------------------------------------------------------------------

/// Seqlock-stamped parameter block for lock-free single-writer -> single/multi-reader updates.
///
/// Every parameter is stored as a single 32-bit atomic (`f32::to_bits` for floats/bools,
/// `u32` discriminant for [`FilterType`]). Even `generation` values denote a quiescent
/// snapshot; odd values denote an in-progress write. Readers verify `g1 == g2` (and even)
/// around field reads so a torn cross-parameter snapshot is never returned.
#[repr(C)]
pub struct ParamBlock {
    generation: AtomicU64,
    values: [AtomicU32; PARAM_COUNT],
}

// SAFETY: All fields are atomics (`AtomicU64`, `AtomicU32`), which are `Send + Sync`.
unsafe impl Sync for ParamBlock {}
// SAFETY: All fields are atomics (`AtomicU64`, `AtomicU32`), which are `Send + Sync`.
unsafe impl Send for ParamBlock {}

impl Default for ParamBlock {
    fn default() -> Self {
        Self::new()
    }
}

impl ParamBlock {
    /// Creates a [`ParamBlock`] initialized with [`DspParams::DEFAULT`] and `generation = 2`.
    #[must_use]
    pub fn new() -> Self {
        let block = Self {
            generation: AtomicU64::new(2),
            values: core::array::from_fn(|_| AtomicU32::new(0)),
        };
        block.write_fields(&DspParams::DEFAULT);
        block
    }

    /// Initializes a [`ParamBlock`] in-place at `ptr` with `initial` parameters and `generation = 2`.
    ///
    /// # Safety
    /// `ptr` must be non-null, properly aligned for `ParamBlock`, and point to valid writable memory.
    pub unsafe fn init_in_place(ptr: *mut Self, initial: &DspParams) {
        // SAFETY: Caller guarantees `ptr` is valid, aligned, and writable.
        unsafe {
            core::ptr::write_bytes(ptr, 0, 1);
            let block = &*ptr;
            block.write_fields(initial);
            // Release pairs with reader's Acquire load of `generation`.
            block.generation.store(2, Ordering::Release);
        }
    }

    fn write_fields(&self, p: &DspParams) {
        let set_u32 = |id: ParamId, bits: u32| {
            if let Some(slot) = self.values.get(id.index()) {
                slot.store(bits, Ordering::Relaxed);
            }
        };
        let set_bool = |id: ParamId, b: bool| {
            set_u32(id, if b { 1.0f32.to_bits() } else { 0.0f32.to_bits() });
        };
        let set_f32 = |id: ParamId, v: f32| {
            set_u32(id, v.to_bits());
        };

        set_bool(ParamId::Bypass, p.bypass);
        set_f32(ParamId::InputGainDb, p.input_gain_db);
        #[cfg(not(all(test, loom)))]
        {
            set_bool(ParamId::HpfEnabled, p.hpf_enabled);
            set_f32(ParamId::HpfFreqHz, p.hpf_freq_hz);
            set_f32(ParamId::DenoiseMix, p.denoise_mix);
            set_bool(ParamId::GateEnabled, p.gate_enabled);
            set_f32(ParamId::GateThresholdDb, p.gate_threshold_db);
            set_f32(ParamId::GateRatio, p.gate_ratio);
            set_f32(ParamId::GateHysteresisDb, p.gate_hysteresis_db);
            set_f32(ParamId::GateAttackMs, p.gate_attack_ms);
            set_f32(ParamId::GateReleaseMs, p.gate_release_ms);

            for i in 0..EQ_BAND_COUNT {
                if let Some(b) = Band::new(i as u8) {
                    set_u32(ParamId::EqType(b), p.eq_type.get(i).copied().unwrap_or_default() as u32);
                    set_f32(ParamId::EqFreqHz(b), p.eq_freq_hz.get(i).copied().unwrap_or(1000.0));
                    set_f32(ParamId::EqGainDb(b), p.eq_gain_db.get(i).copied().unwrap_or(0.0));
                    set_f32(ParamId::EqQ(b), p.eq_q.get(i).copied().unwrap_or(1.0));
                }
            }

            set_bool(ParamId::DeesserEnabled, p.deesser_enabled);
            set_f32(ParamId::DeesserFreqHz, p.deesser_freq_hz);
            set_f32(ParamId::DeesserThreshDb, p.deesser_thresh_db);
            set_f32(ParamId::DeesserAmountDb, p.deesser_amount_db);
            set_bool(ParamId::CompEnabled, p.comp_enabled);
            set_f32(ParamId::CompThreshDb, p.comp_thresh_db);
            set_f32(ParamId::CompRatio, p.comp_ratio);
            set_f32(ParamId::CompAttackMs, p.comp_attack_ms);
            set_f32(ParamId::CompReleaseMs, p.comp_release_ms);
            set_f32(ParamId::CompKneeDb, p.comp_knee_db);
            set_f32(ParamId::CompMakeupDb, p.comp_makeup_db);
            set_bool(ParamId::CompAutoMakeup, p.comp_auto_makeup);
            set_f32(ParamId::LimiterCeilingDb, p.limiter_ceiling_db);
            set_bool(ParamId::LimiterTruePeak, p.limiter_true_peak);
        }
    }

    /// Publishes a new [`DspParams`] snapshot using a seqlock-style generation protocol.
    ///
    /// Single-writer precondition.
    pub fn store(&self, p: &DspParams) {
        let prev = self.generation.load(Ordering::Relaxed);
        let odd = prev.wrapping_add(1) | 1;
        self.generation.store(odd, Ordering::Relaxed);
        // Release fence pairs with `load_if_changed`'s Acquire fence so `odd` is
        // ordered before all field writes.
        fence(Ordering::Release);
        self.write_fields(p);
        let even = odd.wrapping_add(1);
        // Release pairs with `load_if_changed`'s Acquire load of `generation`.
        self.generation.store(even, Ordering::Release);
    }

    /// Loads parameters into `out` (clamped) if `generation != *last_gen`.
    ///
    /// Uses a bounded seqlock retry loop so a torn cross-parameter snapshot is never
    /// written to `out` and a stalled writer cannot block a real-time reader.
    /// Returns `true` if `out` was updated, `false` on the fast unchanged path or if
    /// a concurrent write did not settle within the bounded retry budget.
    #[inline]
    pub fn load_if_changed(&self, last_gen: &mut u64, out: &mut DspParams) -> bool {
        #[cfg(all(test, loom))]
        const MAX_RETRIES: usize = 4;
        #[cfg(not(all(test, loom)))]
        const MAX_RETRIES: usize = 8;

        for _ in 0..MAX_RETRIES {
            // Acquire pairs with `store`'s Release store to `generation`.
            let g1 = self.generation.load(Ordering::Acquire);
            if g1 == *last_gen {
                return false;
            }
            if (g1 & 1) != 0 {
                core::hint::spin_loop();
                continue;
            }
            let candidate = self.read_fields_clamped();
            // Acquire fence pairs with `store`'s Release fence so field loads
            // cannot be reordered after the post-read `generation` check.
            fence(Ordering::Acquire);
            let g2 = self.generation.load(Ordering::Relaxed);
            if g1 == g2 {
                *out = candidate;
                *last_gen = g1;
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    fn read_fields_clamped(&self) -> DspParams {
        let get_u32 = |id: ParamId| -> u32 { self.values.get(id.index()).map_or(0, |a| a.load(Ordering::Relaxed)) };
        let get_bool = |id: ParamId| -> bool { f32::from_bits(get_u32(id)) >= 0.5 };
        let get_f32 = |id: ParamId| -> f32 { f32::from_bits(get_u32(id)) };

        #[cfg(all(test, loom))]
        let loaded = DspParams {
            bypass: get_bool(ParamId::Bypass),
            input_gain_db: get_f32(ParamId::InputGainDb),
            ..DspParams::DEFAULT
        };

        #[cfg(not(all(test, loom)))]
        let mut loaded = DspParams {
            bypass: get_bool(ParamId::Bypass),
            input_gain_db: get_f32(ParamId::InputGainDb),
            hpf_enabled: get_bool(ParamId::HpfEnabled),
            hpf_freq_hz: get_f32(ParamId::HpfFreqHz),
            denoise_mix: get_f32(ParamId::DenoiseMix),
            gate_enabled: get_bool(ParamId::GateEnabled),
            gate_threshold_db: get_f32(ParamId::GateThresholdDb),
            gate_ratio: get_f32(ParamId::GateRatio),
            gate_hysteresis_db: get_f32(ParamId::GateHysteresisDb),
            gate_attack_ms: get_f32(ParamId::GateAttackMs),
            gate_release_ms: get_f32(ParamId::GateReleaseMs),
            eq_type: [FilterType::Peaking; EQ_BAND_COUNT],
            eq_freq_hz: [1000.0; EQ_BAND_COUNT],
            eq_gain_db: [0.0; EQ_BAND_COUNT],
            eq_q: [1.0; EQ_BAND_COUNT],
            deesser_enabled: get_bool(ParamId::DeesserEnabled),
            deesser_freq_hz: get_f32(ParamId::DeesserFreqHz),
            deesser_thresh_db: get_f32(ParamId::DeesserThreshDb),
            deesser_amount_db: get_f32(ParamId::DeesserAmountDb),
            comp_enabled: get_bool(ParamId::CompEnabled),
            comp_thresh_db: get_f32(ParamId::CompThreshDb),
            comp_ratio: get_f32(ParamId::CompRatio),
            comp_attack_ms: get_f32(ParamId::CompAttackMs),
            comp_release_ms: get_f32(ParamId::CompReleaseMs),
            comp_knee_db: get_f32(ParamId::CompKneeDb),
            comp_makeup_db: get_f32(ParamId::CompMakeupDb),
            comp_auto_makeup: get_bool(ParamId::CompAutoMakeup),
            limiter_ceiling_db: get_f32(ParamId::LimiterCeilingDb),
            limiter_true_peak: get_bool(ParamId::LimiterTruePeak),
        };

        #[cfg(not(all(test, loom)))]
        for i in 0..EQ_BAND_COUNT {
            if let Some(b) = Band::new(i as u8) {
                if let Some(slot) = loaded.eq_type.get_mut(i) {
                    *slot = FilterType::from_u32(get_u32(ParamId::EqType(b))).unwrap_or_default();
                }
                if let Some(slot) = loaded.eq_freq_hz.get_mut(i) {
                    *slot = get_f32(ParamId::EqFreqHz(b));
                }
                if let Some(slot) = loaded.eq_gain_db.get_mut(i) {
                    *slot = get_f32(ParamId::EqGainDb(b));
                }
                if let Some(slot) = loaded.eq_q.get_mut(i) {
                    *slot = get_f32(ParamId::EqQ(b));
                }
            }
        }

        loaded.clamped()
    }
}

// ---------------------------------------------------------------------------
// Triple<T>
// ---------------------------------------------------------------------------

const TRIPLE_DIRTY_BIT: u32 = 0b100;
const TRIPLE_IDX_MASK: u32 = 0b011;
#[cfg(all(debug_assertions, not(loom)))]
const GUARD_WRITER_BIT: u32 = 1 << 0;
#[cfg(all(debug_assertions, not(loom)))]
const GUARD_READER_BIT: u32 = 1 << 1;

/// Lock-free, wait-free triple buffer for latest-value telemetry.
///
/// Single-writer / single-reader is a documented precondition (checked with `debug_assert!`
/// in debug builds). Usable both on the heap and inside shared memory.
#[repr(C)]
pub struct Triple<T: Copy + Pod> {
    slots: [SyncCell<T>; 3],
    /// Bits `0..2`: middle slot index (`0..3`). Bit `2`: dirty flag (`1` = unpublished frame waiting).
    state: AtomicU32,
    write_idx: AtomicU32,
    read_idx: AtomicU32,
    debug_guard: AtomicU32,
}

// SAFETY: `T: Pod` implies `Send + Sync` (Wait: `Pod` requires `Copy + 'static`, and `Triple`
// enforces single-writer/single-reader synchronization via `state` atomics).
unsafe impl<T: Copy + Pod + Send> Send for Triple<T> {}
// SAFETY: Synchronized via `state: AtomicU32` AcqRel swaps between single writer and single reader.
unsafe impl<T: Copy + Pod + Send> Sync for Triple<T> {}

impl<T: Copy + Pod + Default> Default for Triple<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: Copy + Pod> Triple<T> {
    /// Creates a new [`Triple`] with all slots initialized to `initial`.
    #[must_use]
    pub fn new(initial: T) -> Self {
        Self {
            slots: [SyncCell::new(initial), SyncCell::new(initial), SyncCell::new(initial)],
            state: AtomicU32::new(2), // middle = 2, dirty = 0
            write_idx: AtomicU32::new(0),
            read_idx: AtomicU32::new(1),
            debug_guard: AtomicU32::new(0),
        }
    }

    /// Initializes a [`Triple`] in-place at `ptr` with zeroed `T` slots.
    ///
    /// # Safety
    /// `ptr` must be non-null, properly aligned for `Triple<T>`, and point to valid writable memory.
    pub unsafe fn init_in_place(ptr: *mut Self) {
        // SAFETY: `T: Pod` allows zeroed bit-pattern (`Zeroable`), and caller guarantees `ptr` is valid.
        unsafe {
            core::ptr::write_bytes(ptr, 0, 1);
            let t = &*ptr;
            t.write_idx.store(0, Ordering::Relaxed);
            t.read_idx.store(1, Ordering::Relaxed);
            // Release pairs with any subsequent Acquire/AcqRel on `state`.
            t.state.store(2, Ordering::Release);
        }
    }

    /// Publishes `v` to the triple buffer, overwriting any unconsumed previous frame.
    ///
    /// # Precondition
    /// Must only be called from a single writer at a time.
    #[inline]
    pub fn publish(&self, v: &T) {
        #[cfg(all(debug_assertions, not(loom)))]
        {
            let prev_g = self.debug_guard.fetch_or(GUARD_WRITER_BIT, Ordering::Relaxed);
            debug_assert_eq!(
                prev_g & GUARD_WRITER_BIT,
                0,
                "Triple::publish called concurrently by multiple writers"
            );
        }

        let w = self.write_idx.load(Ordering::Relaxed);
        if let Some(slot) = self.slots.get((w & TRIPLE_IDX_MASK) as usize) {
            // SAFETY: Single writer exclusively owns `write_idx` until `state.swap` below.
            unsafe {
                slot.with_mut(|ptr| *ptr = *v);
            }
        }

        // AcqRel: Release publishes `slots[w]` to the reader; Acquire pairs with reader's
        // AcqRel swap so we don't reuse the returned slot before the reader finishes reading it.
        let prev = self
            .state
            .swap(TRIPLE_DIRTY_BIT | (w & TRIPLE_IDX_MASK), Ordering::AcqRel);
        self.write_idx.store(prev & TRIPLE_IDX_MASK, Ordering::Relaxed);

        #[cfg(all(debug_assertions, not(loom)))]
        {
            self.debug_guard.fetch_and(!GUARD_WRITER_BIT, Ordering::Relaxed);
        }
    }

    /// Copies the latest published frame into `out` if a new frame is available.
    ///
    /// Returns `true` if `out` was updated with a new frame, `false` otherwise.
    ///
    /// # Precondition
    /// Must only be called from a single reader at a time.
    #[inline]
    pub fn latest(&self, out: &mut T) -> bool {
        #[cfg(all(debug_assertions, not(loom)))]
        {
            let prev_g = self.debug_guard.fetch_or(GUARD_READER_BIT, Ordering::Relaxed);
            debug_assert_eq!(
                prev_g & GUARD_READER_BIT,
                0,
                "Triple::latest called concurrently by multiple readers"
            );
        }

        let s = self.state.load(Ordering::Relaxed);
        if (s & TRIPLE_DIRTY_BIT) == 0 {
            #[cfg(all(debug_assertions, not(loom)))]
            {
                self.debug_guard.fetch_and(!GUARD_READER_BIT, Ordering::Relaxed);
            }
            return false;
        }

        let r = self.read_idx.load(Ordering::Relaxed) & TRIPLE_IDX_MASK;
        // AcqRel: Acquire pairs with writer's Release swap to see `slots[new_r]`;
        // Release hands our old `read_idx` slot back to the writer.
        let prev = self.state.swap(r, Ordering::AcqRel);
        let new_r = prev & TRIPLE_IDX_MASK;
        self.read_idx.store(new_r, Ordering::Relaxed);

        if let Some(slot) = self.slots.get(new_r as usize) {
            // SAFETY: Single reader exclusively owns `new_r` after swapping it out of `state`.
            unsafe {
                slot.with(|ptr| *out = *ptr);
            }
        }

        #[cfg(all(debug_assertions, not(loom)))]
        {
            self.debug_guard.fetch_and(!GUARD_READER_BIT, Ordering::Relaxed);
        }

        true
    }
}

// ---------------------------------------------------------------------------
// SpscRing<N>
// ---------------------------------------------------------------------------

/// Wait-free, single-producer / single-consumer ring buffer for `f32` audio tap samples.
///
/// `N` must be a non-zero power of two. `head` and `tail` are placed on separate 64-byte
/// cache lines to avoid false sharing.
#[repr(C)]
pub struct SpscRing<const N: usize> {
    head: CachePadded<AtomicU64>,
    tail: CachePadded<AtomicU64>,
    overruns: AtomicU64,
    debug_guard: AtomicU32,
    _pad: u32,
    data: [SyncCell<f32>; N],
}

// SAFETY: Synchronized via `head` (producer Release -> consumer Acquire) and `tail` (consumer Release -> producer Acquire).
unsafe impl<const N: usize> Send for SpscRing<N> {}
// SAFETY: Synchronized via `head` and `tail` atomics between single producer and single consumer.
unsafe impl<const N: usize> Sync for SpscRing<N> {}

impl<const N: usize> Default for SpscRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> SpscRing<N> {
    const _ASSERT_POW2: () = {
        assert!(N > 0, "SpscRing capacity must be non-zero");
        assert!(N.is_power_of_two(), "SpscRing capacity must be a power of two");
    };

    /// Creates an empty [`SpscRing`].
    #[must_use]
    pub fn new() -> Self {
        let () = Self::_ASSERT_POW2;
        Self {
            head: CachePadded(AtomicU64::new(0)),
            tail: CachePadded(AtomicU64::new(0)),
            overruns: AtomicU64::new(0),
            debug_guard: AtomicU32::new(0),
            _pad: 0,
            data: core::array::from_fn(|_| SyncCell::new(0.0)),
        }
    }

    /// Initializes an [`SpscRing`] in-place at `ptr` (zeroing the buffer).
    ///
    /// # Safety
    /// `ptr` must be non-null, properly aligned for `SpscRing<N>`, and point to valid writable memory.
    pub unsafe fn init_in_place(ptr: *mut Self) {
        let () = Self::_ASSERT_POW2;
        // SAFETY: Caller guarantees `ptr` is valid, aligned, and writable; all-zero bits is valid for `f32` and atomics.
        unsafe {
            core::ptr::write_bytes(ptr, 0, 1);
        }
    }

    /// Pushes as many samples from `s` as fit into the ring without blocking.
    ///
    /// Returns the number of samples written. Any samples that do not fit are dropped
    /// and counted in [`SpscRing::overruns`].
    ///
    /// # Precondition
    /// Must only be called from a single producer at a time.
    #[inline]
    pub fn push_slice(&self, s: &[f32]) -> usize {
        let () = Self::_ASSERT_POW2;
        #[cfg(all(debug_assertions, not(loom)))]
        {
            let prev_g = self.debug_guard.fetch_or(GUARD_WRITER_BIT, Ordering::Relaxed);
            debug_assert_eq!(
                prev_g & GUARD_WRITER_BIT,
                0,
                "SpscRing::push_slice called concurrently by multiple producers"
            );
        }

        let head = self.head.0.load(Ordering::Relaxed);
        // Acquire pairs with `pop_slice`'s Release store to `tail`.
        let tail = self.tail.0.load(Ordering::Acquire);
        let used = head.wrapping_sub(tail) as usize;
        let free = N.saturating_sub(used);
        let to_write = s.len().min(free);

        if to_write < s.len() {
            self.overruns.fetch_add((s.len() - to_write) as u64, Ordering::Relaxed);
        }

        let mask = N - 1;
        for (i, &sample) in s.iter().take(to_write).enumerate() {
            let idx = (head as usize).wrapping_add(i) & mask;
            if let Some(slot) = self.data.get(idx) {
                // SAFETY: `idx` is in `[head, head + free)`, which is disjoint from consumer's `[tail, head)`.
                unsafe {
                    slot.with_mut(|ptr| *ptr = sample);
                }
            }
        }

        // Release pairs with `pop_slice`'s Acquire load of `head`.
        self.head.0.store(head.wrapping_add(to_write as u64), Ordering::Release);

        #[cfg(all(debug_assertions, not(loom)))]
        {
            self.debug_guard.fetch_and(!GUARD_WRITER_BIT, Ordering::Relaxed);
        }

        to_write
    }

    /// Pops up to `d.len()` samples into `d` without blocking.
    ///
    /// Returns the number of samples read.
    ///
    /// # Precondition
    /// Must only be called from a single consumer at a time.
    #[inline]
    pub fn pop_slice(&self, d: &mut [f32]) -> usize {
        let () = Self::_ASSERT_POW2;
        #[cfg(all(debug_assertions, not(loom)))]
        {
            let prev_g = self.debug_guard.fetch_or(GUARD_READER_BIT, Ordering::Relaxed);
            debug_assert_eq!(
                prev_g & GUARD_READER_BIT,
                0,
                "SpscRing::pop_slice called concurrently by multiple consumers"
            );
        }

        let tail = self.tail.0.load(Ordering::Relaxed);
        // Acquire pairs with `push_slice`'s Release store to `head`.
        let head = self.head.0.load(Ordering::Acquire);
        let avail = head.wrapping_sub(tail) as usize;
        let to_read = d.len().min(avail);

        let mask = N - 1;
        for (i, dst) in d.iter_mut().take(to_read).enumerate() {
            let idx = (tail as usize).wrapping_add(i) & mask;
            if let Some(slot) = self.data.get(idx) {
                // SAFETY: `idx` is in `[tail, head)`, which was published by producer's Release store to `head`.
                unsafe {
                    slot.with(|ptr| *dst = *ptr);
                }
            }
        }

        // Release pairs with `push_slice`'s Acquire load of `tail`.
        self.tail.0.store(tail.wrapping_add(to_read as u64), Ordering::Release);

        #[cfg(all(debug_assertions, not(loom)))]
        {
            self.debug_guard.fetch_and(!GUARD_READER_BIT, Ordering::Relaxed);
        }

        to_read
    }

    /// Returns the cumulative number of dropped samples due to ring full conditions.
    #[inline]
    #[must_use]
    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }

    /// Initializes a `MaybeUninit<Self>` in-place.
    pub fn init_uninit(slot: &mut MaybeUninit<Self>) -> &Self {
        // SAFETY: `slot.as_mut_ptr()` is valid, aligned, and writable.
        unsafe {
            Self::init_in_place(slot.as_mut_ptr());
            slot.assume_init_ref()
        }
    }
}

#[cfg(all(test, loom))]
mod loom {
    use core::sync::atomic::Ordering;

    use loom::sync::Arc;
    use loom::thread;

    use super::{ParamBlock, SpscRing, Triple};
    use crate::params::DspParams;
    use crate::telemetry::AudioTelemetryFrame;

    #[test]
    fn param_block_publishes_latest() {
        let mut builder = loom::model::Builder::new();
        builder.preemption_bound = Some(2);
        builder.check(|| {
            let block = Arc::new(ParamBlock::new());
            let b_writer = Arc::clone(&block);

            let writer = thread::spawn(move || {
                let mut p = DspParams::DEFAULT;
                p.input_gain_db = 12.0;
                p.bypass = true;
                b_writer.store(&p);
            });

            let mut last_gen = 0u64;
            let mut out = DspParams::DEFAULT;
            if block.load_if_changed(&mut last_gen, &mut out) && out.bypass {
                assert!((out.input_gain_db - 12.0).abs() < f32::EPSILON);
            }

            assert!(writer.join().is_ok());

            // After writer finishes, a subsequent load must observe the final values.
            let _ = block.load_if_changed(&mut last_gen, &mut out);
            assert!((out.input_gain_db - 12.0).abs() < f32::EPSILON);
            assert!(out.bypass);
        });
    }

    #[test]
    fn triple_never_tears_and_delivers_latest() {
        loom::model(|| {
            let triple = Arc::new(Triple::new(AudioTelemetryFrame::default()));
            let t_writer = Arc::clone(&triple);

            let writer = thread::spawn(move || {
                for seq in 1..=2u64 {
                    let frame = AudioTelemetryFrame {
                        sequence: seq,
                        flags: (seq as u32) ^ 0xA5A5_5A5A,
                        input_rms_db: -(seq as f32),
                        output_rms_db: -(seq as f32) * 2.0,
                        ..AudioTelemetryFrame::default()
                    };
                    t_writer.publish(&frame);
                }
            });

            let mut last_seq = 0u64;
            let mut out = AudioTelemetryFrame::default();
            for _ in 0..3 {
                if triple.latest(&mut out) {
                    assert!(out.sequence >= last_seq);
                    assert_eq!(out.flags, (out.sequence as u32) ^ 0xA5A5_5A5A);
                    assert_eq!(out.input_rms_db, -(out.sequence as f32));
                    assert_eq!(out.output_rms_db, -(out.sequence as f32) * 2.0);
                    last_seq = out.sequence;
                }
            }

            assert!(writer.join().is_ok());

            if triple.latest(&mut out) {
                assert!(out.sequence >= last_seq);
                assert_eq!(out.flags, (out.sequence as u32) ^ 0xA5A5_5A5A);
                last_seq = out.sequence;
            }
            assert_eq!(last_seq, 2);
            let _ = Ordering::Relaxed;
        });
    }

    #[test]
    fn spsc_ring_preserves_fifo_order() {
        loom::model(|| {
            let ring = Arc::new(SpscRing::<4>::new());
            let r_prod = Arc::clone(&ring);

            let producer = thread::spawn(move || {
                let mut sent = 0usize;
                let data = [1.0f32, 2.0, 3.0];
                while sent < data.len() {
                    sent += r_prod.push_slice(&data[sent..]);
                    loom::thread::yield_now();
                }
            });

            let mut received = [0.0f32; 3];
            let mut got = 0usize;
            for _ in 0..6 {
                got += ring.pop_slice(&mut received[got..]);
                if got == 3 {
                    break;
                }
                loom::thread::yield_now();
            }

            producer.join().unwrap();
            while got < 3 {
                got += ring.pop_slice(&mut received[got..]);
            }
            assert_eq!(received, [1.0, 2.0, 3.0]);
        });
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    extern crate std;

    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering as StdOrdering};
    use std::thread;

    use super::{Ordering, ParamBlock};
    use crate::params::DspParams;

    #[test]
    fn param_block_never_returns_torn_snapshot() {
        let mut a = DspParams::DEFAULT;
        a.bypass = false;
        a.input_gain_db = -12.0;
        a.comp_thresh_db = -30.0;
        a.limiter_ceiling_db = -3.0;

        let mut b = DspParams::DEFAULT;
        b.bypass = true;
        b.input_gain_db = 12.0;
        b.comp_thresh_db = -6.0;
        b.limiter_ceiling_db = -0.5;

        let block = Arc::new(ParamBlock::new());
        block.store(&a);

        let done = Arc::new(AtomicBool::new(false));
        let b_writer = Arc::clone(&block);
        let done_writer = Arc::clone(&done);

        let writer = thread::spawn(move || {
            for i in 0..50_000u32 {
                if i % 2 == 0 {
                    b_writer.store(&b);
                } else {
                    b_writer.store(&a);
                }
            }
            done_writer.store(true, StdOrdering::Release);
        });

        let mut last_gen = 0u64;
        let mut out = DspParams::DEFAULT;
        while !done.load(StdOrdering::Acquire) {
            if block.load_if_changed(&mut last_gen, &mut out) {
                let is_a = !out.bypass
                    && (out.input_gain_db - (-12.0)).abs() < f32::EPSILON
                    && (out.comp_thresh_db - (-30.0)).abs() < f32::EPSILON
                    && (out.limiter_ceiling_db - (-3.0)).abs() < f32::EPSILON;
                let is_b = out.bypass
                    && (out.input_gain_db - 12.0).abs() < f32::EPSILON
                    && (out.comp_thresh_db - (-6.0)).abs() < f32::EPSILON
                    && (out.limiter_ceiling_db - (-0.5)).abs() < f32::EPSILON;
                assert!(is_a || is_b, "observed torn ParamBlock snapshot: {out:?}");
            }
        }

        assert!(writer.join().is_ok());
    }

    #[test]
    fn param_block_odd_generation_is_non_blocking() {
        let block = ParamBlock::new();
        let mut last_gen = 0u64;
        let mut out = DspParams::DEFAULT;
        assert!(block.load_if_changed(&mut last_gen, &mut out));

        // Simulate a stalled writer holding an odd seqlock sequence.
        block.generation.store(last_gen.wrapping_add(1), Ordering::Release);
        let sentinel = 7.5f32;
        out.input_gain_db = sentinel;
        assert!(!block.load_if_changed(&mut last_gen, &mut out));
        assert!((out.input_gain_db - sentinel).abs() < f32::EPSILON);
    }
}
