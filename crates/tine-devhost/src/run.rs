//! Subcommand implementations for `tine-devhost`.

use std::io::BufRead;
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use hound::{SampleFormat, WavSpec, WavWriter};
use tine_dsp::{DspPipeline, MeterSink, TapSink};
use tine_engine::audio_io::{DeviceRef, list_inputs, list_outputs, open_capture, open_playback};
use tine_protocol::params::{DspParams, FilterType, ParamId, SPECS};
use tine_protocol::presets::{Preset, PresetParams};
use tine_protocol::sync::{ParamBlock, SpscRing, Triple};
use tine_protocol::telemetry::{AudioTelemetryFrame, flags};

use crate::cli::{LatencyArgs, RecordRawArgs, RunArgs};
use crate::record::{RECORD_RING_CAP, WavRecorder};

/// Ring capacity between capture and playback (32768 samples ≈ 682 ms @ 48 kHz).
const AUDIO_RING_CAP: usize = 32_768;

/// `MeterSink` publishing ~60 Hz frames into a lock-free `Triple`.
struct TripleMeterSink {
    triple: Arc<Triple<AudioTelemetryFrame>>,
}

impl MeterSink for TripleMeterSink {
    #[inline]
    fn publish(&mut self, f: &AudioTelemetryFrame) {
        self.triple.publish(f);
    }
}

/// Optional `TapSink` pushing post-DSP samples into a recording `SpscRing`.
struct OptionalRingTap {
    ring: Option<Arc<SpscRing<RECORD_RING_CAP>>>,
}

impl TapSink for OptionalRingTap {
    #[inline]
    fn push(&mut self, post: &[f32]) {
        if let Some(ring) = &self.ring {
            let _ = ring.push_slice(post);
        }
    }
}

/// Lists all audio input and output devices.
pub fn cmd_list() -> Result<(), String> {
    let inputs = list_inputs().map_err(|e| e.to_string())?;
    println!("=== Input Devices ({}) ===", inputs.len());
    for d in &inputs {
        let def = if d.is_default { " [default]" } else { "" };
        println!(
            "  {}{} | id={} | {} Hz, {} ch",
            d.name, def, d.id, d.default_rate, d.channels
        );
    }

    let outputs = list_outputs().map_err(|e| e.to_string())?;
    println!("\n=== Output Devices ({}) ===", outputs.len());
    for d in &outputs {
        let def = if d.is_default { " [default]" } else { "" };
        println!(
            "  {}{} | id={} | {} Hz, {} ch",
            d.name, def, d.id, d.default_rate, d.channels
        );
    }

    Ok(())
}

/// Runs live capture -> `DspPipeline` -> playback with interactive stdin control and 1 Hz meters.
pub fn cmd_run(args: &RunArgs) -> Result<(), String> {
    let mut params = match &args.params_path {
        Some(path) => load_params(path)?,
        None => DspParams::DEFAULT,
    };

    let param_block = Arc::new(ParamBlock::new());
    param_block.store(&params);

    let telemetry_triple = Arc::new(Triple::new(AudioTelemetryFrame::default()));
    let audio_ring = Arc::new(SpscRing::<AUDIO_RING_CAP>::new());

    let raw_ring = args
        .record_raw
        .as_ref()
        .map(|_| Arc::new(SpscRing::<RECORD_RING_CAP>::new()));
    let proc_ring = args
        .record_processed
        .as_ref()
        .map(|_| Arc::new(SpscRing::<RECORD_RING_CAP>::new()));

    let in_ref = DeviceRef::parse(&args.input);
    let out_ref = DeviceRef::parse(&args.output);

    let capture_params = Arc::clone(&param_block);
    let capture_telemetry = Arc::clone(&telemetry_triple);
    let capture_raw_ring = raw_ring.clone();
    let capture_proc_ring = proc_ring.clone();

    // Pre-initialize pipeline inside a closure after we know sample rate?
    // Since open_capture negotiates rate, we lazily initialize DspPipeline on first block
    // OR we query device default rate first. Wait: allocating in the first callback would violate
    // RT rules, so let's query the device's rate first or allocate at 48 kHz?
    // Better: resolve rate via list_inputs / default, or prepare DspPipeline at negotiated rate!
    // Wait: open_capture knows the negotiated rate before starting the stream, or we can
    // inspect list_inputs() to get the device's default_rate and pass want_rate = Some(rate).
    // Even simpler: let's pass `None` to `open_capture` and check if we know the rate before.
    // Actually, `DspPipeline::new` needs `sample_rate`. If we pick 48_000 (or device's default_rate),
    // let's look up the device's default_rate from `list_inputs()`!
    let target_rate = find_input_rate(&in_ref).unwrap_or(48_000);
    let mut pipeline = DspPipeline::new(target_rate as f32, 4096);
    let mut dsp_params = params;
    let mut last_gen = 0u64;
    let mut meter_sink = TripleMeterSink {
        triple: capture_telemetry,
    };
    let mut tap_sink = OptionalRingTap {
        ring: capture_proc_ring,
    };

    let capture = open_capture(&in_ref, Some(target_rate), Arc::clone(&audio_ring), move |block| {
        if let Some(raw) = &capture_raw_ring {
            let _ = raw.push_slice(block);
        }
        capture_params.load_if_changed(&mut last_gen, &mut dsp_params);
        pipeline.process(block, &dsp_params, &mut meter_sink, &mut tap_sink);
    })
    .map_err(|e| e.to_string())?;

    let actual_in_rate = capture.sample_rate();
    let playback = open_playback(&out_ref, Arc::clone(&audio_ring), actual_in_rate).map_err(|e| e.to_string())?;

    println!(
        "Running live DSP: input={} Hz ({} ch) -> output={} Hz ({} ch)",
        actual_in_rate,
        capture.channels(),
        playback.sample_rate(),
        playback.channels()
    );
    println!("Commands: `set <key> <value>`, `macro <name> <v>`, `bypass`, `stats`, `quit`");

    let raw_recorder = match (&args.record_raw, raw_ring) {
        (Some(path), Some(ring)) => Some(WavRecorder::start(path, actual_in_rate, ring)?),
        _ => None,
    };
    let proc_recorder = match (&args.record_processed, proc_ring) {
        (Some(path), Some(ring)) => Some(WavRecorder::start(path, actual_in_rate, ring)?),
        _ => None,
    };

    let (tx, rx) = mpsc::channel::<String>();
    thread::Builder::new()
        .name("tine-devhost-stdin".to_owned())
        .spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .map_err(|e| format!("failed to spawn stdin thread: {e}"))?;

    let mut frame = AudioTelemetryFrame::default();
    let mut last_meter_print = Instant::now();

    loop {
        if capture.has_error() {
            eprintln!("ERROR: capture stream lost or errored (device unplugged?)");
            break;
        }
        if playback.has_error() {
            eprintln!("ERROR: playback stream lost or errored (device unplugged?)");
            break;
        }

        while let Ok(line) = rx.try_recv() {
            if handle_stdin_command(&line, &mut params, &param_block, &capture, &playback) {
                drop(capture);
                drop(playback);
                finalize_recorders(raw_recorder, proc_recorder)?;
                return Ok(());
            }
        }

        if last_meter_print.elapsed() >= Duration::from_secs(1) {
            last_meter_print = Instant::now();
            if telemetry_triple.latest(&mut frame) {
                let bypass_tag = if (frame.flags & flags::BYPASS) != 0 {
                    " [BYPASS]"
                } else {
                    ""
                };
                let latch_tag = if (frame.flags & flags::PASSTHROUGH_LATCHED) != 0 {
                    " [LATCHED]"
                } else {
                    ""
                };
                println!(
                    "in: {:>5.1}/{:>5.1} dBFS | out: {:>5.1}/{:>5.1} dBFS | gate: {:>4.1} dB | deess: {:>4.1} dB | comp: {:>4.1} dB{}{}",
                    frame.input_rms_db,
                    frame.input_peak_db,
                    frame.output_rms_db,
                    frame.output_peak_db,
                    frame.gate_attenuation_db,
                    frame.deesser_reduction_db,
                    frame.comp_gain_reduction_db,
                    bypass_tag,
                    latch_tag,
                );
            }
        }

        thread::sleep(Duration::from_millis(20));
    }

    drop(capture);
    drop(playback);
    finalize_recorders(raw_recorder, proc_recorder)?;
    Ok(())
}

fn finalize_recorders(raw: Option<WavRecorder>, proc_rec: Option<WavRecorder>) -> Result<(), String> {
    if let Some(rec) = raw {
        let n = rec.finish()?;
        println!("Saved raw WAV ({n} samples).");
    }
    if let Some(rec) = proc_rec {
        let n = rec.finish()?;
        println!("Saved processed WAV ({n} samples).");
    }
    Ok(())
}

fn handle_stdin_command(
    line: &str,
    params: &mut DspParams,
    param_block: &ParamBlock,
    capture: &tine_engine::audio_io::CaptureStream,
    playback: &tine_engine::audio_io::PlaybackStream,
) -> bool {
    let parts: Vec<&str> = line.trim_start_matches('\u{feff}').split_whitespace().collect();
    let Some(&cmd) = parts.first() else {
        return false;
    };

    match cmd {
        "quit" | "q" | "exit" => return true,
        "bypass" => {
            params.bypass = !params.bypass;
            param_block.store(params);
            println!("bypass = {}", params.bypass);
        }
        "stats" => {
            let cs = capture.stats();
            let ps = playback.stats();
            println!(
                "capture : callbacks={} max_us={} overruns={} err={}",
                cs.callbacks,
                cs.max_callback_us,
                cs.overruns,
                capture.has_error()
            );
            println!(
                "playback: callbacks={} max_us={} underruns={} err={}",
                ps.callbacks,
                ps.max_callback_us,
                ps.underruns,
                playback.has_error()
            );
        }
        "macro" => {
            println!("`macro` requires T10 MacroMapper (not yet merged); use `set <key> <value>` instead.");
        }
        "set" => {
            if parts.len() < 3 {
                println!("usage: set <key> <value>");
                return false;
            }
            let key = parts[1];
            let val_str = parts[2];
            let Some(spec) = SPECS.iter().find(|s| s.key == key) else {
                println!("unknown parameter key `{key}`");
                return false;
            };
            match parse_param_value(spec.id, val_str) {
                Ok(val) => {
                    params.set(spec.id, val);
                    *params = params.clamped();
                    param_block.store(params);
                    println!("{key} = {}", params.get(spec.id));
                }
                Err(msg) => println!("invalid value `{val_str}` for `{key}`: {msg}"),
            }
        }
        other => println!("unknown command `{other}` (expected set, macro, bypass, stats, quit)"),
    }

    false
}

fn parse_param_value(id: ParamId, raw: &str) -> Result<f32, String> {
    if matches!(
        id,
        ParamId::Bypass
            | ParamId::HpfEnabled
            | ParamId::GateEnabled
            | ParamId::DeesserEnabled
            | ParamId::CompEnabled
            | ParamId::CompAutoMakeup
            | ParamId::LimiterTruePeak
    ) {
        return match raw.to_ascii_lowercase().as_str() {
            "true" | "on" | "1" => Ok(1.0),
            "false" | "off" | "0" => Ok(0.0),
            _ => raw.parse::<f32>().map_err(|e| e.to_string()),
        };
    }

    if matches!(id, ParamId::EqType(_)) {
        return match raw.to_ascii_lowercase().as_str() {
            "low_shelf" | "lowshelf" => Ok(FilterType::LowShelf.as_f32()),
            "high_shelf" | "highshelf" => Ok(FilterType::HighShelf.as_f32()),
            "peaking" => Ok(FilterType::Peaking.as_f32()),
            "notch" => Ok(FilterType::Notch.as_f32()),
            "low_pass" | "lowpass" => Ok(FilterType::LowPass.as_f32()),
            "high_pass" | "highpass" => Ok(FilterType::HighPass.as_f32()),
            "band_pass" | "bandpass" => Ok(FilterType::BandPass.as_f32()),
            _ => raw.parse::<f32>().map_err(|e| e.to_string()),
        };
    }

    raw.parse::<f32>().map_err(|e| e.to_string())
}

fn find_input_rate(id: &DeviceRef) -> Option<u32> {
    let inputs = list_inputs().ok()?;
    match id {
        DeviceRef::Default => inputs.iter().find(|d| d.is_default).map(|d| d.default_rate),
        DeviceRef::Id(target) => inputs
            .iter()
            .find(|d| d.id == *target || d.id.ends_with(target))
            .map(|d| d.default_rate),
        DeviceRef::Name(q) => {
            let ql = q.to_ascii_lowercase();
            inputs
                .iter()
                .find(|d| d.name.to_ascii_lowercase() == ql)
                .or_else(|| inputs.iter().find(|d| d.name.to_ascii_lowercase().contains(&ql)))
                .map(|d| d.default_rate)
        }
    }
    .filter(|&r| r > 0)
}

fn load_params(path: &Path) -> Result<DspParams, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    if let Ok(preset) = serde_json::from_str::<Preset>(&text) {
        preset
            .validate()
            .map_err(|e| format!("invalid preset {}: {e}", path.display()))?;
        let mut params = DspParams::DEFAULT;
        preset.params.apply_to(&mut params);
        return Ok(params.clamped());
    }

    if let Ok(pp) = serde_json::from_str::<PresetParams>(&text) {
        pp.validate()
            .map_err(|e| format!("invalid params {}: {e}", path.display()))?;
        let mut params = DspParams::DEFAULT;
        pp.apply_to(&mut params);
        return Ok(params.clamped());
    }

    Err(format!(
        "failed to parse {} as Preset or PresetParams JSON",
        path.display()
    ))
}

/// Captures raw unprocessed microphone audio to a mono 32-bit float WAV file.
pub fn cmd_record_raw(args: &RecordRawArgs) -> Result<(), String> {
    let in_ref = DeviceRef::parse(&args.input);
    let ring = Arc::new(SpscRing::<RECORD_RING_CAP>::new());
    let ring_cap = Arc::clone(&ring);

    let capture = open_capture(&in_ref, args.rate, ring, |_| {}).map_err(|e| e.to_string())?;
    let sample_rate = capture.sample_rate();
    let target_samples = ((f64::from(sample_rate) * f64::from(args.seconds)) as usize).max(1);

    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(&args.out_path, spec)
        .map_err(|e| format!("failed to create {}: {e}", args.out_path.display()))?;

    println!(
        "Recording raw audio for {:.1}s at {} Hz -> {}",
        args.seconds,
        sample_rate,
        args.out_path.display()
    );

    let mut written = 0usize;
    let mut buf = [0.0f32; 2048];
    let start = Instant::now();
    let timeout = Duration::from_secs_f32(args.seconds + 3.0);

    while written < target_samples {
        if capture.has_error() {
            return Err("capture stream errored during record-raw".to_owned());
        }
        if start.elapsed() > timeout {
            return Err("timed out waiting for capture samples".to_owned());
        }
        let need = (target_samples - written).min(buf.len());
        let n = ring_cap.pop_slice(&mut buf[..need]);
        if n > 0 {
            for &s in &buf[..n] {
                writer.write_sample(s).map_err(|e| e.to_string())?;
            }
            written += n;
        } else {
            thread::sleep(Duration::from_millis(5));
        }
    }

    drop(capture);
    writer.finalize().map_err(|e| e.to_string())?;
    println!(
        "Done: wrote {written} samples ({:.2}s).",
        written as f64 / f64::from(sample_rate)
    );
    Ok(())
}

/// Measures round-trip loopback latency by emitting a short impulse and detecting its return.
pub fn cmd_latency(args: &LatencyArgs) -> Result<(), String> {
    let in_ref = DeviceRef::parse(&args.input);
    let out_ref = DeviceRef::parse(&args.output);

    let play_ring = Arc::new(SpscRing::<AUDIO_RING_CAP>::new());
    let cap_ring = Arc::new(SpscRing::<AUDIO_RING_CAP>::new());

    let rate = find_input_rate(&in_ref).unwrap_or(48_000);
    let capture = open_capture(&in_ref, Some(rate), Arc::clone(&cap_ring), |_| {}).map_err(|e| e.to_string())?;
    let in_rate = capture.sample_rate();
    let playback = open_playback(&out_ref, Arc::clone(&play_ring), in_rate).map_err(|e| e.to_string())?;

    // Let streams settle for 200 ms.
    let silence = vec![0.0f32; (in_rate / 5) as usize];
    play_ring.push_slice(&silence);
    thread::sleep(Duration::from_millis(250));

    // Drain pre-impulse capture.
    let mut drain = [0.0f32; 2048];
    while cap_ring.pop_slice(&mut drain) > 0 {}

    // Emit a 1-sample full-scale impulse followed by silence.
    let mut impulse = vec![0.0f32; (f64::from(in_rate) * f64::from(args.seconds)) as usize];
    if let Some(first) = impulse.first_mut() {
        *first = 0.95;
    }
    let send_time = Instant::now();
    play_ring.push_slice(&impulse);

    // Capture for `args.seconds`.
    let mut recorded = Vec::with_capacity(impulse.len());
    let deadline = Duration::from_secs_f32(args.seconds);
    while send_time.elapsed() < deadline {
        if capture.has_error() || playback.has_error() {
            return Err("stream error during latency test".to_owned());
        }
        let n = cap_ring.pop_slice(&mut drain);
        if n > 0 {
            recorded.extend_from_slice(&drain[..n]);
        } else {
            thread::sleep(Duration::from_millis(2));
        }
    }

    drop(capture);
    drop(playback);

    // Find peak sample in recorded buffer.
    let mut max_idx = 0usize;
    let mut max_val = 0.0f32;
    for (i, &s) in recorded.iter().enumerate() {
        let a = s.abs();
        if a > max_val {
            max_val = a;
            max_idx = i;
        }
    }

    if max_val < 0.02 {
        println!("No loopback impulse detected (peak={max_val:.4}). Connect output to input for hardware loopback.");
    } else {
        let ms = (max_idx as f64 / f64::from(in_rate)) * 1000.0;
        println!(
            "Measured round-trip loopback latency: {max_idx} samples ({ms:.2} ms @ {in_rate} Hz, peak={max_val:.3})"
        );
    }

    Ok(())
}
