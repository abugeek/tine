//! Offline processor for listening to and inspecting `DspPipeline` output.
//!
//! Accepts WAV and FLAC input (stereo is downmixed to mono; no resampling - the
//! pipeline runs at the file's native rate) and writes 32-bit float mono WAV.
//!
//! Usage:
//! `cargo run -p tine-dsp --example process_wav -- in.flac out.wav [--params file.json] [--set comp_ratio=4]`

#![allow(missing_docs)]

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

use tine_dsp::primitives::db::lin_to_db;
use tine_dsp::{DspPipeline, MeterSink, NoTap};
use tine_protocol::params::{DspParams, FilterType, ParamId, SPECS};
use tine_protocol::telemetry::{AudioTelemetryFrame, flags};

#[derive(Default)]
struct SummaryMeters {
    frames: usize,
    max_gate_gr_db: f32,
    max_comp_gr_db: f32,
    max_deesser_gr_db: f32,
    input_clipped: bool,
    passthrough_latched: bool,
}

impl MeterSink for SummaryMeters {
    fn publish(&mut self, f: &AudioTelemetryFrame) {
        self.frames += 1;
        if f.gate_attenuation_db.abs() > self.max_gate_gr_db.abs() {
            self.max_gate_gr_db = f.gate_attenuation_db;
        }
        if f.comp_gain_reduction_db.abs() > self.max_comp_gr_db.abs() {
            self.max_comp_gr_db = f.comp_gain_reduction_db;
        }
        if f.deesser_reduction_db.abs() > self.max_deesser_gr_db.abs() {
            self.max_deesser_gr_db = f.deesser_reduction_db;
        }
        if (f.flags & flags::INPUT_CLIPPED) != 0 {
            self.input_clipped = true;
        }
        if (f.flags & flags::PASSTHROUGH_LATCHED) != 0 {
            self.passthrough_latched = true;
        }
    }
}

fn find_param_id(key: &str) -> Option<ParamId> {
    SPECS.iter().find(|s| s.key == key).map(|s| s.id)
}

fn parse_filter_type(s: &str) -> Option<f32> {
    let ft = match s.to_ascii_lowercase().as_str() {
        "low_shelf" | "lowshelf" => FilterType::LowShelf,
        "high_shelf" | "highshelf" => FilterType::HighShelf,
        "peaking" => FilterType::Peaking,
        "notch" => FilterType::Notch,
        "low_pass" | "lowpass" => FilterType::LowPass,
        "high_pass" | "highpass" => FilterType::HighPass,
        "band_pass" | "bandpass" => FilterType::BandPass,
        _ => return None,
    };
    Some(ft.as_f32())
}

fn parse_param_value(raw: &str) -> Result<f32, String> {
    if raw.eq_ignore_ascii_case("true") {
        return Ok(1.0);
    }
    if raw.eq_ignore_ascii_case("false") {
        return Ok(0.0);
    }
    if let Some(ft) = parse_filter_type(raw) {
        return Ok(ft);
    }
    raw.parse::<f32>()
        .map_err(|_| format!("invalid parameter value: {raw}"))
}

fn apply_json_params(params: &mut DspParams, path: &PathBuf) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let val: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("invalid JSON in {}: {e}", path.display()))?;
    let obj = val
        .as_object()
        .ok_or_else(|| format!("expected JSON object in {}", path.display()))?;
    for (k, v) in obj {
        let Some(id) = find_param_id(k) else {
            return Err(format!("unknown parameter key in JSON: {k}"));
        };
        let num = match v {
            serde_json::Value::Bool(b) => {
                if *b {
                    1.0
                } else {
                    0.0
                }
            }
            serde_json::Value::Number(n) => n.as_f64().ok_or_else(|| format!("non-finite number for {k}"))? as f32,
            serde_json::Value::String(s) => parse_param_value(s)?,
            _ => return Err(format!("unsupported JSON value type for {k}")),
        };
        params.set(id, num);
    }
    Ok(())
}

fn read_wav_mono(path: &PathBuf) -> Result<(Vec<f32>, u32), String> {
    // FLAC input (the repo's fixture format); anything else is read as WAV.
    let is_flac = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("flac"));
    if is_flac {
        let mut reader = claxon::FlacReader::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        let spec = reader.streaminfo();
        let ch = spec.channels as usize;
        let max = 2.0f64.powf(f64::from(spec.bits_per_sample) - 1.0);
        let raw: Vec<f32> = reader
            .samples()
            .map(|s| {
                s.map(|v| (f64::from(v) / max) as f32)
                    .map_err(|e| format!("sample error: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mono = if ch <= 1 {
            raw
        } else {
            raw.chunks_exact(ch)
                .map(|frame| frame.iter().sum::<f32>() / (ch as f32))
                .collect()
        };
        return Ok((mono, spec.sample_rate));
    }

    let mut reader = hound::WavReader::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let spec = reader.spec();
    let ch = spec.channels as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| s.map_err(|e| format!("sample error: {e}")))
            .collect::<Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| (v as f32) / max).map_err(|e| format!("sample error: {e}")))
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    let mono = if ch <= 1 {
        raw
    } else {
        raw.chunks_exact(ch)
            .map(|frame| frame.iter().sum::<f32>() / (ch as f32))
            .collect()
    };
    Ok((mono, spec.sample_rate))
}

fn write_wav_mono(path: &PathBuf, samples: &[f32], sample_rate: u32) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|e| format!("create dir {}: {e}", parent.display()))?;
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|e| format!("create {}: {e}", path.display()))?;
    for &s in samples {
        writer.write_sample(s).map_err(|e| format!("write sample: {e}"))?;
    }
    writer
        .finalize()
        .map_err(|e| format!("finalize {}: {e}", path.display()))
}

fn compute_peak_rms_db(samples: &[f32]) -> (f32, f32) {
    if samples.is_empty() {
        return (-120.0, -120.0);
    }
    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;
    for &s in samples {
        let a = s.abs();
        if a > peak {
            peak = a;
        }
        sum_sq += f64::from(s) * f64::from(s);
    }
    let rms = (sum_sq / (samples.len() as f64)).sqrt() as f32;
    (lin_to_db(peak), lin_to_db(rms))
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() < 2 {
        return Err(
            "Usage: process_wav <in.flac|in.wav> <out.wav> [--params file.json] [--set key=value]...".to_owned(),
        );
    }

    let in_path = PathBuf::from(&args[0]);
    let out_path = PathBuf::from(&args[1]);
    let mut params = DspParams::DEFAULT;

    let mut i = 2usize;
    while i < args.len() {
        match args[i].as_str() {
            "--params" => {
                let p = args.get(i + 1).ok_or("--params requires a file path")?;
                apply_json_params(&mut params, &PathBuf::from(p))?;
                i += 2;
            }
            "--set" => {
                let kv = args.get(i + 1).ok_or("--set requires key=value")?;
                let (k, v) = kv
                    .split_once('=')
                    .ok_or_else(|| format!("--set expects key=value, got: {kv}"))?;
                let id = find_param_id(k).ok_or_else(|| format!("unknown parameter key: {k}"))?;
                let val = parse_param_value(v)?;
                params.set(id, val);
                i += 2;
            }
            other => return Err(format!("unexpected argument: {other}")),
        }
    }
    params = params.clamped();

    let (input, sample_rate) = read_wav_mono(&in_path)?;
    let (in_peak_db, in_rms_db) = compute_peak_rms_db(&input);

    let mut output = input;
    let mut pipe = DspPipeline::new(sample_rate as f32, 480);
    let mut meters = SummaryMeters::default();
    for chunk in output.chunks_mut(480) {
        pipe.process(chunk, &params, &mut meters, &mut NoTap);
    }

    let (out_peak_db, out_rms_db) = compute_peak_rms_db(&output);
    let latency_samples = pipe.latency_samples();
    let latency_ms = (latency_samples as f32) * 1000.0 / (sample_rate as f32);

    write_wav_mono(&out_path, &output, sample_rate)?;

    println!(
        "Input:   {} ({} Hz, {} samples)",
        in_path.display(),
        sample_rate,
        output.len()
    );
    println!("Output:  {}", out_path.display());
    println!("Latency: {latency_samples} samples ({latency_ms:.2} ms)");
    println!("Meters:");
    println!("  input_peak_db:        {in_peak_db:.2} dBFS");
    println!("  input_rms_db:         {in_rms_db:.2} dBFS");
    println!("  output_peak_db:       {out_peak_db:.2} dBFS");
    println!("  output_rms_db:        {out_rms_db:.2} dBFS");
    println!("  max_gate_gr_db:       {:.2} dB", meters.max_gate_gr_db);
    println!("  max_deesser_gr_db:    {:.2} dB", meters.max_deesser_gr_db);
    println!("  max_comp_gr_db:       {:.2} dB", meters.max_comp_gr_db);
    println!("  input_clipped:        {}", meters.input_clipped);
    println!("  passthrough_latched:  {}", meters.passthrough_latched);

    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        process::exit(1);
    }
}
