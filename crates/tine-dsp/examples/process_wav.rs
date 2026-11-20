//! Offline processor for listening to and inspecting `DspPipeline` output.
//!
//! Accepts WAV and FLAC input (stereo is downmixed to mono; no resampling - the
//! pipeline runs at the file's native rate) and writes 32-bit float mono WAV.
//!
//! Usage:
//! `cargo run -p tine-dsp --example process_wav -- in.flac out.wav [--preset dynamic] [--macros leveler=0.62,purity=0.7] [--params file.json] [--set comp_ratio=4]`
//!
//! `--preset <id>` starts from the built-in archetype base curve (`dynamic`,
//! `condenser`, `headset`, `laptop`) with its default Golden Line macro
//! positions; `--macros` (comma-separated `purity=..,body=..,presence=..,leveler=..`,
//! each `0.0..=1.0`) moves the macro nodes on top of it. Both go through
//! `tine_dsp::presets::builtin` + `MacroMapper::apply` (no calibration, no pins);
//! order matters: `--preset`/`--macros` first, then `--params`/`--set` overrides.

#![allow(missing_docs)]

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

use tine_dsp::macros::MacroMapper;
use tine_dsp::primitives::db::lin_to_db;
use tine_dsp::{DspPipeline, MeterSink, NoTap, presets};
use tine_protocol::params::{DenoiseEngine, DspParams, FilterType, ParamId, SPECS};
use tine_protocol::presets::{Archetype, MacroPositions, PinSet};
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

/// Resolves a built-in preset by its stable id (`dynamic|condenser|headset|laptop`).
fn preset_by_id(id: &str) -> Result<&'static presets::PresetDef, String> {
    let archetype = match id.trim().to_ascii_lowercase().as_str() {
        "dynamic" => Archetype::Dynamic,
        "condenser" => Archetype::Condenser,
        "headset" => Archetype::Headset,
        "laptop" => Archetype::Laptop,
        _ => {
            return Err(format!(
                "unknown preset id: {id} (expected dynamic|condenser|headset|laptop)"
            ));
        }
    };
    Ok(presets::builtin(archetype))
}

/// Parses `--macros` (`purity=0.7,leveler=0.62`) on top of the preset's defaults.
fn parse_macros(spec: &str, base: MacroPositions) -> Result<MacroPositions, String> {
    let mut m = base;
    for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (name, value) = part
            .split_once('=')
            .ok_or_else(|| format!("--macros expects name=value (0.0..=1.0), got: {part}"))?;
        let v: f32 = value
            .trim()
            .parse()
            .map_err(|_| format!("invalid macro position for {name}: {value}"))?;
        if !v.is_finite() || !(0.0..=1.0).contains(&v) {
            return Err(format!("macro position for {name} out of range 0.0..=1.0: {v}"));
        }
        match name.trim().to_ascii_lowercase().as_str() {
            "purity" => m.purity = v,
            "body" => m.body = v,
            "presence" => m.presence = v,
            "leveler" => m.leveler = v,
            _ => {
                return Err(format!(
                    "unknown macro name: {name} (expected purity|body|presence|leveler)"
                ));
            }
        }
    }
    Ok(m)
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() < 2 {
        return Err(
            "Usage: process_wav <in.flac|in.wav> <out.wav> [--preset dynamic|condenser|headset|laptop] [--macros purity=0.7,leveler=0.62] [--params file.json] [--set key=value]...".to_owned(),
        );
    }

    let in_path = PathBuf::from(&args[0]);
    let out_path = PathBuf::from(&args[1]);
    let mut params = DspParams::DEFAULT;
    let mut preset_used: Option<&'static presets::PresetDef> = None;
    let mut macros_used: Option<MacroPositions> = None;
    let mut denoise_engine = DenoiseEngine::Rnnoise;

    let mut i = 2usize;
    while i < args.len() {
        match args[i].as_str() {
            "--denoiser" => {
                let raw = args
                    .get(i + 1)
                    .ok_or("--denoiser requires an engine (rnnoise|deepfilter|off)")?;
                denoise_engine = DenoiseEngine::from_name(raw)
                    .ok_or_else(|| format!("unknown denoiser engine: {raw} (expected rnnoise|deepfilter|off)"))?;
                i += 2;
            }
            "--preset" => {
                let id = args
                    .get(i + 1)
                    .ok_or("--preset requires an archetype id (dynamic|condenser|headset|laptop)")?;
                let preset = preset_by_id(id)?;
                params = MacroMapper::apply(preset, None, preset.macro_defaults, PinSet::EMPTY, &params);
                preset_used = Some(preset);
                macros_used = Some(preset.macro_defaults);
                i += 2;
            }
            "--macros" => {
                let spec = args
                    .get(i + 1)
                    .ok_or("--macros requires name=value pairs, e.g. purity=0.7,leveler=0.62")?;
                let preset = preset_used.ok_or("--macros requires --preset <id> first")?;
                let macros = parse_macros(spec, preset.macro_defaults)?;
                params = MacroMapper::apply(preset, None, macros, PinSet::EMPTY, &params);
                macros_used = Some(macros);
                i += 2;
            }
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
    let mut pipe = DspPipeline::for_engine(sample_rate as f32, 480, denoise_engine);
    let mut meters = SummaryMeters::default();
    for chunk in output.chunks_mut(480) {
        pipe.process(chunk, &params, &mut meters, &mut NoTap);
    }

    let (out_peak_db, out_rms_db) = compute_peak_rms_db(&output);
    let in_crest_db = in_peak_db - in_rms_db;
    let out_crest_db = out_peak_db - out_rms_db;
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
    println!(
        "A/B:     raw (RMS {in_rms_db:+.2} dBFS, peak {in_peak_db:+.2} dBFS, crest {in_crest_db:.2} dB) -> tine (RMS {out_rms_db:+.2} dBFS, peak {out_peak_db:+.2} dBFS, crest {out_crest_db:.2} dB | delta RMS {:+.2} dB, peak {:+.2} dB)",
        out_rms_db - in_rms_db,
        out_peak_db - in_peak_db,
    );
    if let Some(preset) = preset_used {
        let m = macros_used.unwrap_or(preset.macro_defaults);
        println!("Preset:  {} ({})", preset.id, preset.name);
        println!(
            "Macros:  purity={:.2} body={:.2} presence={:.2} leveler={:.2}",
            m.purity, m.body, m.presence, m.leveler
        );
        println!(
            "Params:  input_gain_db={:.2} hpf_freq_hz={:.2} denoise_mix={:.2} gate_threshold_db={:.2} eq_gain_db=[{:.2},{:.2},{:.2},{:.2},{:.2}] deesser_enabled={} deesser_amount_db={:.2} comp_thresh_db={:.2} comp_ratio={:.2}",
            params.input_gain_db,
            params.hpf_freq_hz,
            params.denoise_mix,
            params.gate_threshold_db,
            params.eq_gain_db[0],
            params.eq_gain_db[1],
            params.eq_gain_db[2],
            params.eq_gain_db[3],
            params.eq_gain_db[4],
            params.deesser_enabled,
            params.deesser_amount_db,
            params.comp_thresh_db,
            params.comp_ratio
        );
    }
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
