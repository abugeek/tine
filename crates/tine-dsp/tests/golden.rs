//! Golden audio regression tests for `DspPipeline`.
//!
//! Processes each fixture in `tests/fixtures/*.flac` with neutral parameters and
//! with each stage isolated, then compares acoustic metrics (peak, RMS, crest,
//! 32-band log spectrum, GR stats) against `tests/golden/<fixture>.json` within
//! ±0.5 dB. Set `UPDATE_GOLDEN=1` to regenerate the golden JSON files.

#![allow(missing_docs)]

mod common;

use std::env;
use std::fs;
use std::path::PathBuf;

use realfft::RealFftPlanner;
use serde_json::{Map, Value, json};
use tine_dsp::primitives::db::{DB_FLOOR, lin_to_db};
use tine_dsp::{DspPipeline, MeterSink, NoTap};
use tine_protocol::params::DspParams;
use tine_protocol::telemetry::AudioTelemetryFrame;

const TOLERANCE_DB: f32 = 0.5;
const BAND_FLOOR_DB: f32 = -70.0;

#[derive(Default)]
#[allow(clippy::struct_field_names)]
struct MaxMeters {
    max_gate_gr_db: f32,
    max_comp_gr_db: f32,
    max_deesser_gr_db: f32,
}

impl MeterSink for MaxMeters {
    fn publish(&mut self, f: &AudioTelemetryFrame) {
        if f.gate_attenuation_db.abs() > self.max_gate_gr_db.abs() {
            self.max_gate_gr_db = f.gate_attenuation_db;
        }
        if f.comp_gain_reduction_db.abs() > self.max_comp_gr_db.abs() {
            self.max_comp_gr_db = f.comp_gain_reduction_db;
        }
        if f.deesser_reduction_db.abs() > self.max_deesser_gr_db.abs() {
            self.max_deesser_gr_db = f.deesser_reduction_db;
        }
    }
}

#[derive(Clone, Debug)]
#[allow(clippy::struct_field_names)]
struct GoldenMetrics {
    peak_db: f32,
    rms_db: f32,
    crest_db: f32,
    bands_db: [f32; 32],
    max_gate_gr_db: f32,
    max_comp_gr_db: f32,
    max_deesser_gr_db: f32,
}

impl GoldenMetrics {
    fn to_json(&self) -> Value {
        json!({
            "peak_db": self.peak_db,
            "rms_db": self.rms_db,
            "crest_db": self.crest_db,
            "bands_db": self.bands_db.to_vec(),
            "max_gate_gr_db": self.max_gate_gr_db,
            "max_comp_gr_db": self.max_comp_gr_db,
            "max_deesser_gr_db": self.max_deesser_gr_db,
        })
    }

    fn from_json(v: &Value) -> Self {
        let bands_vec = v["bands_db"].as_array().expect("bands_db array");
        assert_eq!(bands_vec.len(), 32);
        let mut bands_db = [DB_FLOOR; 32];
        for (dst, item) in bands_db.iter_mut().zip(bands_vec.iter()) {
            *dst = item.as_f64().expect("band f64") as f32;
        }
        Self {
            peak_db: v["peak_db"].as_f64().expect("peak_db") as f32,
            rms_db: v["rms_db"].as_f64().expect("rms_db") as f32,
            crest_db: v["crest_db"].as_f64().expect("crest_db") as f32,
            bands_db,
            max_gate_gr_db: v["max_gate_gr_db"].as_f64().expect("max_gate_gr_db") as f32,
            max_comp_gr_db: v["max_comp_gr_db"].as_f64().expect("max_comp_gr_db") as f32,
            max_deesser_gr_db: v["max_deesser_gr_db"].as_f64().expect("max_deesser_gr_db") as f32,
        }
    }
}

/// Computes 32 log-spaced spectrum bands (20 Hz .. 20 kHz) in dBFS via averaged 4096-pt FFT frames.
fn spectrum_32_bands_db(samples: &[f32], fs: f32) -> [f32; 32] {
    let fft_len = 4096usize;
    let hop = 2048usize;
    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(fft_len);
    let mut scratch_in = vec![0.0f32; fft_len];
    let mut spectrum = r2c.make_output_vec();
    let mut accum_pow = vec![0.0f64; spectrum.len()];
    let mut frames = 0usize;

    let window: Vec<f32> = (0..fft_len)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * (i as f32) / ((fft_len - 1) as f32)).cos()))
        .collect();

    let mut pos = 0usize;
    while pos + fft_len <= samples.len() {
        for ((dst, &src), &w) in scratch_in.iter_mut().zip(&samples[pos..pos + fft_len]).zip(&window) {
            *dst = src * w;
        }
        r2c.process(&mut scratch_in, &mut spectrum).expect("fft");
        let scale = 2.0 / (fft_len as f64);
        for (acc, c) in accum_pow.iter_mut().zip(spectrum.iter()) {
            let mag = f64::from(c.norm()) * scale;
            *acc += mag * mag;
        }
        frames += 1;
        pos += hop;
    }

    if frames == 0 {
        return [DB_FLOOR; 32];
    }

    let bin_hz = f64::from(fs) / (fft_len as f64);
    let f_min = 20.0f64;
    let f_max = 20_000.0f64;
    let ratio = (f_max / f_min).powf(1.0 / 32.0);
    let mut bands = [DB_FLOOR; 32];

    for (b, slot) in bands.iter_mut().enumerate() {
        let b_i32 = i32::try_from(b).unwrap_or(0);
        let lo = f_min * ratio.powi(b_i32);
        let hi = f_min * ratio.powi(b_i32 + 1);
        let bin_lo = ((lo / bin_hz).floor() as usize).max(1);
        let bin_hi = ((hi / bin_hz).ceil() as usize).min(accum_pow.len() - 1).max(bin_lo);

        let mut band_pow = 0.0f64;
        for &p in &accum_pow[bin_lo..=bin_hi] {
            band_pow += p / (frames as f64);
        }
        let rms = band_pow.sqrt() as f32;
        *slot = lin_to_db(rms);
    }

    bands
}

fn bare_params() -> DspParams {
    let mut p = DspParams::DEFAULT;
    p.hpf_enabled = false;
    p.denoise_mix = 0.0;
    p.gate_enabled = false;
    p.deesser_enabled = false;
    p.comp_enabled = false;
    p.limiter_ceiling_db = -0.1;
    p
}

fn profiles() -> Vec<(&'static str, DspParams)> {
    let neutral = DspParams::DEFAULT;

    let mut stage1_hpf = bare_params();
    stage1_hpf.hpf_enabled = true;
    stage1_hpf.hpf_freq_hz = 100.0;

    let mut stage2_cleanup = bare_params();
    stage2_cleanup.denoise_mix = 0.8;
    stage2_cleanup.gate_enabled = true;
    stage2_cleanup.gate_threshold_db = -45.0;

    let mut stage3_eq = bare_params();
    stage3_eq.eq_gain_db[0] = 3.0;
    stage3_eq.eq_gain_db[1] = -3.0;
    stage3_eq.eq_gain_db[4] = 3.0;

    let mut stage4_deesser = bare_params();
    stage4_deesser.deesser_enabled = true;
    stage4_deesser.deesser_freq_hz = 6800.0;
    stage4_deesser.deesser_thresh_db = -28.0;
    stage4_deesser.deesser_amount_db = 8.0;

    let mut stage5_comp = bare_params();
    stage5_comp.comp_enabled = true;
    stage5_comp.comp_thresh_db = -20.0;
    stage5_comp.comp_ratio = 4.0;

    let mut stage6_limiter = bare_params();
    stage6_limiter.input_gain_db = 6.0;
    stage6_limiter.limiter_ceiling_db = -3.0;

    vec![
        ("neutral", neutral),
        ("stage1_hpf", stage1_hpf),
        ("stage2_cleanup", stage2_cleanup),
        ("stage3_eq", stage3_eq),
        ("stage4_deesser", stage4_deesser),
        ("stage5_comp", stage5_comp),
        ("stage6_limiter", stage6_limiter),
    ]
}

fn measure_fixture(input: &[f32], fs: u32, params: &DspParams) -> GoldenMetrics {
    let mut buf = input.to_vec();
    let mut pipe = DspPipeline::new(fs as f32, 480);
    let mut meters = MaxMeters::default();
    for chunk in buf.chunks_mut(480) {
        pipe.process(chunk, params, &mut meters, &mut NoTap);
    }
    assert!(!pipe.is_passthrough_latched(), "unexpected passthrough latch");
    assert!(buf.iter().all(|s| s.is_finite()), "non-finite output");

    // Skip initial pipeline latency when measuring steady-state audio metrics.
    let latency = pipe.latency_samples() as usize;
    let tail = if buf.len() > latency { &buf[latency..] } else { &buf[..] };

    GoldenMetrics {
        peak_db: common::peak_db(tail),
        rms_db: common::rms_db(tail),
        crest_db: common::crest_db(tail),
        bands_db: spectrum_32_bands_db(tail, fs as f32),
        max_gate_gr_db: meters.max_gate_gr_db,
        max_comp_gr_db: meters.max_comp_gr_db,
        max_deesser_gr_db: meters.max_deesser_gr_db,
    }
}

fn assert_metrics_close(fixture: &str, profile: &str, actual: &GoldenMetrics, expected: &GoldenMetrics) {
    let check = |metric: &str, a: f32, e: f32| {
        assert!(
            (a - e).abs() <= TOLERANCE_DB,
            "[{fixture}/{profile}] {metric} mismatch: actual={a:.3} dB, expected={e:.3} dB (tol=±{TOLERANCE_DB} dB)"
        );
    };
    check("peak_db", actual.peak_db, expected.peak_db);
    check("rms_db", actual.rms_db, expected.rms_db);
    check("crest_db", actual.crest_db, expected.crest_db);
    check("max_gate_gr_db", actual.max_gate_gr_db, expected.max_gate_gr_db);
    check("max_comp_gr_db", actual.max_comp_gr_db, expected.max_comp_gr_db);
    check(
        "max_deesser_gr_db",
        actual.max_deesser_gr_db,
        expected.max_deesser_gr_db,
    );

    for (b, (&a, &e)) in actual.bands_db.iter().zip(expected.bands_db.iter()).enumerate() {
        let a_clamped = a.max(BAND_FLOOR_DB);
        let e_clamped = e.max(BAND_FLOOR_DB);
        assert!(
            (a_clamped - e_clamped).abs() <= TOLERANCE_DB,
            "[{fixture}/{profile}] band[{b}] mismatch: actual={a_clamped:.3} dB, expected={e_clamped:.3} dB (tol=±{TOLERANCE_DB} dB)"
        );
    }
}

#[test]
fn golden_regression_all_fixtures() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixtures_dir = manifest_dir.join("tests/fixtures");
    let golden_dir = manifest_dir.join("tests/golden");
    let update = env::var("UPDATE_GOLDEN").as_deref() == Ok("1");

    let mut fixture_paths: Vec<PathBuf> = fs::read_dir(&fixtures_dir)
        .expect("read fixtures dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("flac"))
        .collect();
    fixture_paths.sort();
    assert!(
        !fixture_paths.is_empty(),
        "no .flac fixtures found in {}",
        fixtures_dir.display()
    );

    if update {
        fs::create_dir_all(&golden_dir).expect("create golden dir");
    }

    let profs = profiles();
    for fixture_path in fixture_paths {
        let name = fixture_path.file_stem().and_then(|s| s.to_str()).expect("stem");
        let (input, fs) = common::read_flac_mono(&fixture_path);
        let golden_path = golden_dir.join(format!("{name}.json"));

        if update {
            let mut map = Map::new();
            for (prof_name, params) in &profs {
                let metrics = measure_fixture(&input, fs, params);
                map.insert((*prof_name).to_owned(), metrics.to_json());
            }
            let text = serde_json::to_string_pretty(&Value::Object(map)).expect("serialize golden");
            fs::write(&golden_path, format!("{text}\n")).expect("write golden json");
        } else {
            let text = fs::read_to_string(&golden_path).unwrap_or_else(|e| {
                panic!(
                    "missing golden file {}: {e} (run with UPDATE_GOLDEN=1)",
                    golden_path.display()
                )
            });
            let root: Value = serde_json::from_str(&text).expect("parse golden json");
            for (prof_name, params) in &profs {
                let actual = measure_fixture(&input, fs, params);
                let expected = GoldenMetrics::from_json(&root[*prof_name]);
                assert_metrics_close(name, prof_name, &actual, &expected);
                if *prof_name == "neutral" {
                    assert!(
                        actual.peak_db <= -1.0 + 0.05,
                        "[{name}/neutral] peak {:.3} dBFS exceeded -1.0 dBFS ceiling",
                        actual.peak_db
                    );
                }
            }
        }
    }
}
