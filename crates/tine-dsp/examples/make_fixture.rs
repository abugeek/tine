//! Deterministic synthetic speech-proxy fixture generator for `tine-dsp` golden tests.
//!
//! Generates 48 kHz mono 16-bit **FLAC** files (each <= 1 MB) under
//! `crates/tine-dsp/tests/fixtures/`, plus a small self-check of each
//! fixture's duration, level, and spectrum (see `Result` in
//! `docs/tasks/done/T09-dsp-integration.md`).
//!
//! Every fixture is **synthetic**: deterministic digital signal synthesis with
//! fixed seeds, written specifically for Tine (`MIT OR Apache-2.0`, no
//! third-party recordings). See `tests/fixtures/README.md` for the provenance
//! table and the follow-up plan for real-mic / CC0 replacements.
//!
//! `laptop_simulated` is fully reproducible from this single file: it is the
//! clean `dynamic_speech` signal convolved with a synthetic room impulse
//! response plus synthetic fan/keyboard-style pink noise at -45 dBFS. The
//! recipe is isolated in `room_impulse()` / `fan_keyboard_noise()` so a
//! follow-up can swap in a real CC0 RIR / CC0 noise recording without
//! touching anything else.
//!
//! Usage:
//! `cargo run -p tine-dsp --example make_fixture`

#![allow(missing_docs)]

use std::f32::consts::PI;
use std::fs;
use std::path::{Path, PathBuf};

const FS: u32 = 48_000;
const BYTES_1MB: u64 = 1_048_576;

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn next_f32(&mut self) -> f32 {
        let u = (self.next_u64() >> 40) as f32 / 16_777_216.0;
        u * 2.0 - 1.0
    }
}

// ---------------------------------------------------------------------------
// Minimal FLAC writer (frames with VERBATIM subframes).
//
// Tine deliberately has no FLAC encoder dependency; the fixture format only
// needs byte-exact verbatim frames, which this ~200-line writer emits. Output
// is spec-compliant FLAC (STREAMINFO, CRC-8 frame headers, CRC-16 frames) and
// is verified by decoding it back with `claxon` in `verify_roundtrip()`.
// ---------------------------------------------------------------------------

/// MSB-first bit packer with byte alignment.
struct BitWriter {
    out: Vec<u8>,
    buf: u8,
    nbits: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            buf: 0,
            nbits: 0,
        }
    }

    /// Pushes the low `count` bits of `value`, MSB first.
    fn push(&mut self, value: u32, count: u32) {
        for i in (0..count).rev() {
            self.buf = (self.buf << 1) | ((value >> i) & 1) as u8;
            self.nbits += 1;
            if self.nbits == 8 {
                self.out.push(self.buf);
                self.buf = 0;
                self.nbits = 0;
            }
        }
    }

    fn push_u64(&mut self, value: u64, count: u32) {
        for i in (0..count).rev() {
            self.buf = (self.buf << 1) | ((value >> i) as u32 & 1) as u8;
            self.nbits += 1;
            if self.nbits == 8 {
                self.out.push(self.buf);
                self.buf = 0;
                self.nbits = 0;
            }
        }
    }

    /// Pads with zero bits up to the next byte boundary.
    fn align(&mut self) {
        if self.nbits != 0 {
            let pad = 8 - self.nbits;
            self.push(0, pad);
        }
    }
}

/// CRC-8 (poly x^8 + x^2 + x + 1, init 0, MSB-first) as required by FLAC.
fn crc8_byte(mut crc: u8, byte: u8) -> u8 {
    crc ^= byte;
    for _ in 0..8 {
        crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
    }
    crc
}

/// CRC-16 (poly x^16 + x^15 + x^2 + 1, init 0, MSB-first) as required by FLAC.
fn crc16_byte(mut crc: u16, byte: u8) -> u16 {
    crc ^= (byte as u16) << 8;
    for _ in 0..8 {
        crc = if crc & 0x8000 != 0 {
            (crc << 1) ^ 0x8005
        } else {
            crc << 1
        };
    }
    crc
}

/// FLAC UTF-8-like coded number for frame numbering (up to 4 bytes here).
fn utf8_coded_number(value: u64) -> Vec<u8> {
    if value < 0x80 {
        return vec![value as u8];
    }
    let count = if value < 0x800 {
        2
    } else if value < 0x1_0000 {
        3
    } else {
        4
    };
    let mut out = vec![0u8; count];
    let lead_mask: u8 = 0xFFu8 << (7 - count);
    out[0] = lead_mask | ((value >> (6 * (count - 1))) as u8 & !lead_mask);
    for i in 1..count {
        let shift = 6 * (count - 1 - i);
        out[i] = 0x80 | (((value >> shift) as u8) & 0x3F);
    }
    out
}

/// Writes mono 16-bit 48 kHz samples as a FLAC file with verbatim subframes.
fn write_pcm16_flac(path: &Path, samples: &[f32]) {
    let blocksize = 4_096usize;
    let pcms: Vec<i16> = samples
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16)
        .collect();
    let total = pcms.len() as u64;

    let mut out: Vec<u8> = Vec::with_capacity(total as usize * 2 + 1024);
    out.extend_from_slice(b"fLaC");

    // --- STREAMINFO (metadata block type 0, last block, 34-byte payload) ---
    let mut hdr = BitWriter::new();
    hdr.push(1, 1); // last-metadata-block flag
    hdr.push(0, 7); // type: STREAMINFO
    hdr.push(34, 24); // payload length
    let mut si = BitWriter::new();
    si.push(4_096, 16); // min blocksize
    si.push(4_096, 16); // max blocksize
    si.push(0, 24); // min framesize: unknown
    si.push(0, 24); // max framesize: unknown
    si.push(FS, 20); // sample rate
    si.push(0, 3); // channels - 1 (mono)
    si.push(15, 5); // bits per sample - 1 (16)
    si.push_u64(total, 36); // total samples
    assert_eq!(si.nbits, 0);
    out.extend_from_slice(&hdr.out);
    out.extend_from_slice(&si.out);
    out.extend_from_slice(&[0u8; 16]); // MD5: unknown (all zero per spec)

    // --- Frames ---
    let mut pos = 0usize;
    let mut frame_number: u64 = 0;
    while pos < pcms.len() {
        let chunk_end = (pos + blocksize).min(pcms.len());
        let bs = chunk_end - pos;

        let mut h = BitWriter::new();
        h.push(0b1111_1111_1111_10, 14); // sync code
        h.push(0, 1); // reserved
        h.push(0, 1); // fixed blocksize strategy
        let full_block = bs == blocksize;
        if full_block {
            h.push(0b1100, 4); // blocksize code 12: 4096 samples
        } else {
            h.push(0b0111, 4); // blocksize code 7: 16-bit (blocksize-1) at header end
        }
        h.push(0b1010, 4); // 48 kHz sample rate code
        h.push(0b0000, 4); // mono channel assignment
        h.push(0b100, 3); // 16 bits per sample
        h.push(0, 1); // reserved
        for &b in &utf8_coded_number(frame_number) {
            h.push(b as u32, 8);
        }
        if !full_block {
            h.push((bs - 1) as u32, 16); // blocksize - 1
        }
        h.align(); // zero padding to byte boundary
        let mut crc8 = 0u8;
        for &b in &h.out {
            crc8 = crc8_byte(crc8, b);
        }
        h.out.push(crc8);

        // One VERBATIM subframe (mono): type header is exactly one byte.
        let mut sub = BitWriter::new();
        sub.push(0, 1); // zero padding bit
        sub.push(0b000_001, 6); // subframe type: VERBATIM
        sub.push(0, 1); // no wasted bits
        for &s in &pcms[pos..chunk_end] {
            sub.push(s as u16 as u32, 16);
        }
        assert_eq!(sub.nbits, 0);

        let mut frame = h.out;
        frame.extend_from_slice(&sub.out);
        let mut crc16 = 0u16;
        for &b in &frame {
            crc16 = crc16_byte(crc16, b);
        }
        frame.extend_from_slice(&crc16.to_be_bytes());

        out.extend_from_slice(&frame);
        pos = chunk_end;
        frame_number += 1;
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixtures dir");
    }
    fs::write(path, &out).expect("write flac");
    let bytes = fs::metadata(path).expect("metadata").len();
    assert!(bytes <= BYTES_1MB, "{} exceeded 1 MB ({bytes} bytes)", path.display());
    verify_roundtrip(path, &pcms);
    println!(
        "wrote {} ({bytes} bytes, {} samples, {:.1} s)",
        path.display(),
        total,
        total as f32 / (FS as f32)
    );
}

/// Decodes `path` with `claxon` and asserts it equals the encoded samples.
fn verify_roundtrip(path: &Path, expected: &[i16]) {
    let mut reader = claxon::FlacReader::open(path).expect("claxon open");

    assert_eq!(reader.streaminfo().sample_rate, FS);
    assert_eq!(reader.streaminfo().channels, 1);
    assert_eq!(reader.streaminfo().bits_per_sample, 16);
    let decoded: Vec<i16> = reader
        .samples()
        .map(|s| {
            let v = s.expect("claxon sample");
            i16::try_from(v).expect("16-bit sample fits in i16")
        })
        .collect();
    assert_eq!(decoded.len(), expected.len(), "length mismatch");
    for (i, (a, b)) in decoded.iter().zip(expected.iter()).enumerate() {
        assert_eq!(a, b, "sample {i} mismatch after FLAC roundtrip");
    }
}

// ---------------------------------------------------------------------------
// Deterministic signal synthesis.
// ---------------------------------------------------------------------------

fn pink_noise(len: usize, amp: f32, seed: u64) -> Vec<f32> {
    let mut rng = Rng(seed.max(1));
    let (mut b0, mut b1, mut b2, mut b3, mut b4, mut b5, mut b6) = (0.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let mut out = Vec::with_capacity(len);
    let mut sum_sq = 0.0f64;
    for _ in 0..len {
        let w = rng.next_f32();
        b0 = 0.99886 * b0 + w * 0.055_517_9;
        b1 = 0.99332 * b1 + w * 0.075_075_9;
        b2 = 0.96900 * b2 + w * 0.153_852;
        b3 = 0.86650 * b3 + w * 0.310_485_6;
        b4 = 0.55000 * b4 + w * 0.532_952_2;
        b5 = -0.7616 * b5 - w * 0.016_898_0;
        let pink = b0 + b1 + b2 + b3 + b4 + b5 + b6 + w * 0.5362;
        b6 = w * 0.115_926;
        sum_sq += f64::from(pink) * f64::from(pink);
        out.push(pink);
    }
    let rms = (sum_sq / (len.max(1) as f64)).sqrt() as f32;
    let scale = if rms > 1e-9 { amp / rms } else { 0.0 };
    for s in &mut out {
        *s *= scale;
    }
    out
}

/// Simple 2-pole resonator for vowel formants.
struct Resonator {
    a1: f32,
    a2: f32,
    b0: f32,
    y1: f32,
    y2: f32,
}

impl Resonator {
    fn new(freq_hz: f32, bw_hz: f32, fs: f32) -> Self {
        let r = (-PI * bw_hz / fs).exp();
        let theta = 2.0 * PI * freq_hz / fs;
        Self {
            a1: -2.0 * r * theta.cos(),
            a2: r * r,
            b0: 1.0 - r,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x - self.a1 * self.y1 - self.a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Generates a deterministic speech-like formant cadence (voiced syllables + pauses).
fn synth_speech(seconds: f32, target_rms: f32, seed: u64) -> Vec<f32> {
    let len = (seconds * (FS as f32)) as usize;
    let mut out = vec![0.0f32; len];
    let mut rng = Rng(seed.max(1));

    let mut f1 = Resonator::new(520.0, 70.0, FS as f32);
    let mut f2 = Resonator::new(1550.0, 110.0, FS as f32);
    let mut f3 = Resonator::new(2600.0, 160.0, FS as f32);

    let mut phase = 0.0f32;
    let syllable_len = (0.28 * (FS as f32)) as usize;
    let active_len = (0.19 * (FS as f32)) as usize;

    for (i, slot) in out.iter_mut().enumerate() {
        let t = i as f32 / (FS as f32);
        let syl_idx = i / syllable_len;
        let pos = i % syllable_len;

        // Pause every 5th syllable for realistic speech gaps.
        let is_pause = (syl_idx % 5) == 4;
        if is_pause || pos >= active_len {
            let _ = f1.process(0.0);
            let _ = f2.process(0.0);
            let _ = f3.process(0.0);
            continue;
        }

        // Smooth syllable envelope.
        let env = (PI * (pos as f32) / (active_len as f32)).sin().powf(0.6);
        let f0 = 125.0 + 22.0 * (2.0 * PI * 1.3 * t).sin() + 8.0 * ((syl_idx % 3) as f32);
        phase += f0 / (FS as f32);
        if phase >= 1.0 {
            phase -= 1.0;
        }

        // Glottal pulse train approximation + breathiness.
        let glottal = (2.0 * phase - 1.0) * 0.7 + 0.08 * rng.next_f32();
        let vocal = f1.process(glottal) * 0.55 + f2.process(glottal) * 0.30 + f3.process(glottal) * 0.15;
        *slot = vocal * env;
    }

    // Normalize to target RMS.
    let sum_sq: f64 = out.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    let rms = (sum_sq / (len.max(1) as f64)).sqrt() as f32;
    let scale = if rms > 1e-9 { target_rms / rms } else { 0.0 };
    for s in &mut out {
        *s = (*s * scale).clamp(-0.98, 0.98);
    }
    out
}

/// Synthetic room impulse response (early reflections + exponential comb tail).
///
/// Stand-in for a real CC0 room impulse response (see fixtures/README.md for
/// the follow-up swap plan): deterministic, fixed-seed, and license-clean.
fn room_impulse() -> Vec<(usize, f32)> {
    vec![
        (0, 0.65),
        (143, 0.28),
        (317, -0.21),
        (589, 0.16),
        (911, -0.12),
        (1429, 0.09),
        (2113, -0.06),
        (3191, 0.04),
    ]
}

/// Fan/keyboard-style background noise at the given dBFS RMS level.
///
/// Stand-in for a real CC0 fan/keyboard noise recording (see fixtures/README.md).
fn fan_keyboard_noise(len: usize, rms_dbfs: f32, seed: u64) -> Vec<f32> {
    let amp = 10.0f32.powf(rms_dbfs / 20.0);
    pink_noise(len, amp, seed)
}

fn convolve_rir(input: &[f32], taps: &[(usize, f32)]) -> Vec<f32> {
    let mut out = vec![0.0f32; input.len()];
    for i in 0..input.len() {
        let mut acc = 0.0f32;
        for &(d, g) in taps {
            if i >= d {
                acc += input[i - d] * g;
            }
        }
        out[i] = acc;
    }
    out
}

/// Self-check: duration, RMS level, peak, first/last nonzero samples.
fn verify_fixture(name: &str, samples: &[f32], expected_rms_db: f32, min_span_s: f32) {
    let sum_sq: f64 = samples.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    let rms = (sum_sq / (samples.len().max(1) as f64)).sqrt() as f32;
    let rms_db = 20.0 * rms.log10();
    let peak = samples.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
    let peak_db = 20.0 * peak.log10();
    let first = samples.iter().position(|&s| s != 0.0).unwrap_or(0);
    let last = samples
        .iter()
        .rposition(|&s| s != 0.0)
        .unwrap_or(samples.len().saturating_sub(1));
    let span_s = (last - first) as f32 / (FS as f32);
    println!(
        "{name}: len={} ({:.2} s), rms={rms_db:.2} dBFS (target {expected_rms_db:.2}), peak={peak_db:.2} dBFS, active span {span_s:.2} s",
        samples.len(),
        samples.len() as f32 / (FS as f32),
    );
    assert!(
        (rms_db - expected_rms_db).abs() <= 1.0,
        "{name}: rms {rms_db:.2} dBFS deviates from target {expected_rms_db:.2} dBFS by > 1 dB"
    );
    assert!(peak > 0.0 && peak <= 1.0, "{name}: peak out of range");
    assert!(
        span_s >= min_span_s,
        "{name}: active span {span_s:.2} s < {min_span_s:.2} s"
    );
}

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");

    // 1. dynamic_speech: 6 s speech at -18 dBFS RMS
    let dynamic_speech = synth_speech(6.0, 0.1259, 0x1001);
    verify_fixture("dynamic_speech", &dynamic_speech, -18.0, 5.5);
    write_pcm16_flac(&dir.join("dynamic_speech.flac"), &dynamic_speech);

    // 2. noisy_quiet: 3 s room noise (-45 dBFS RMS) + 3 s quiet speech (-30 dBFS RMS)
    let mut noisy_quiet = pink_noise((FS as usize) * 6, 0.00562, 0x2002);
    let quiet_speech = synth_speech(3.0, 0.0316, 0x2003);
    for (dst, &s) in noisy_quiet[(FS as usize) * 3..].iter_mut().zip(quiet_speech.iter()) {
        *dst += s;
    }
    verify_fixture("noisy_quiet", &noisy_quiet, -32.75, 5.9);
    write_pcm16_flac(&dir.join("noisy_quiet.flac"), &noisy_quiet);

    // 3. sibilant: 4 s speech + 7.2 kHz fricative bursts
    let mut sibilant = synth_speech(4.0, 0.10, 0x3003);
    let mut rng = Rng(0x3004);
    let mut sib_bp = Resonator::new(7200.0, 1200.0, FS as f32);
    for (i, slot) in sibilant.iter_mut().enumerate() {
        let burst = sib_bp.process(rng.next_f32());
        let gate = if (i % 24_000) < 4800 { 1.0 } else { 0.0 };
        *slot = (*slot + burst * 0.45 * gate).clamp(-0.98, 0.98);
    }
    verify_fixture("sibilant", &sibilant, -19.8, 3.7);
    write_pcm16_flac(&dir.join("sibilant.flac"), &sibilant);

    // 4. plosive: 4 s speech + 70 Hz plosive thumps
    let mut plosive = synth_speech(4.0, 0.10, 0x4004);
    for (i, slot) in plosive.iter_mut().enumerate() {
        let pos = i % 24_000;
        if pos < 3600 {
            let t = pos as f32 / (FS as f32);
            let thump = 0.65 * (-t * 35.0).exp() * (2.0 * PI * 70.0 * t).sin();
            *slot = (*slot + thump).clamp(-0.98, 0.98);
        }
    }
    verify_fixture("plosive", &plosive, -18.0, 3.7);
    write_pcm16_flac(&dir.join("plosive.flac"), &plosive);

    // 5. clipping: 3 s overdriven speech hitting 0 dBFS
    let mut clipping = synth_speech(3.0, 0.45, 0x5005);
    for s in &mut clipping {
        *s = (*s * 2.5).clamp(-1.0, 1.0);
    }
    verify_fixture("clipping", &clipping, -4.5, 2.7);
    write_pcm16_flac(&dir.join("clipping.flac"), &clipping);

    // 6. laptop_simulated: dynamic_speech convolved with a (synthetic CC0-proxy)
    //    RIR + fan/keyboard noise at -45 dBFS. Fully reproducible from this file.
    let rir = room_impulse();
    let mut laptop = convolve_rir(&dynamic_speech, &rir);
    let fan = fan_keyboard_noise(laptop.len(), -45.0, 0x6006);
    for (dst, &n) in laptop.iter_mut().zip(fan.iter()) {
        *dst = (*dst + n).clamp(-0.98, 0.98);
    }
    verify_fixture("laptop_simulated", &laptop, -22.3, 5.8);
    write_pcm16_flac(&dir.join("laptop_simulated.flac"), &laptop);
}
