//! `mic-record --input <name substring> --seconds N <out.wav>`: records a
//! capture endpoint (e.g. an app's virtual microphone) to a mono f32 WAV,
//! acting as the "Discord" consumer that keeps the app's mic stream alive.
//! Unlike `tine-devhost record-raw`, stream errors are printed and counted
//! instead of aborting the recording.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

fn main() {
    let mut input = String::new();
    let mut seconds = 10.0f64;
    let mut out = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--input" => input = args.next().unwrap_or_default(),
            "--seconds" => seconds = args.next().and_then(|s| s.parse().ok()).unwrap_or(seconds),
            _ => out = Some(a),
        }
    }
    let Some(out) = out else {
        eprintln!("usage: mic-record --input <name> --seconds N out.wav");
        std::process::exit(2);
    };

    let host = cpal::default_host();
    let q = input.to_lowercase();
    let dev = host
        .input_devices()
        .expect("input devices")
        .find(|d| {
            d.description()
                .map(|n| n.name().to_lowercase().contains(&q))
                .unwrap_or(false)
        })
        .unwrap_or_else(|| {
            eprintln!("no input device matching {input:?}");
            std::process::exit(3);
        });
    let name = dev.description().map(|d| d.name().to_owned()).unwrap_or_default();
    let cfg = dev.default_input_config().expect("default input config");
    let ch = cfg.channels() as usize;
    let rate = cfg.sample_rate();
    eprintln!("mic-record: {name} | {rate} Hz {ch} ch {:?}", cfg.sample_format());

    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    let err_tx = mpsc::Sender::clone(&tx);
    let (etx, erx) = mpsc::channel::<String>();
    let _ = err_tx;
    let stream = match cfg.sample_format() {
        cpal::SampleFormat::F32 => dev.build_input_stream::<f32, _, _>(
            cfg.config(),
            move |d: &[f32], _| {
                let _ = tx.send(d.to_vec());
            },
            move |e| {
                let _ = etx.send(e.to_string());
            },
            None,
        ),
        cpal::SampleFormat::I16 => dev.build_input_stream::<i16, _, _>(
            cfg.config(),
            move |d: &[i16], _| {
                let _ = tx.send(d.iter().map(|&s| f32::from(s) / 32768.0).collect());
            },
            move |e| {
                let _ = etx.send(e.to_string());
            },
            None,
        ),
        f => {
            eprintln!("unsupported sample format {f:?}");
            std::process::exit(4);
        }
    }
    .expect("build input stream");
    stream.play().expect("play");

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&out, spec).expect("create wav");
    let start = Instant::now();
    let (mut frames, mut callbacks, mut errors) = (0u64, 0u64, 0u64);
    let mut max_gap = Duration::ZERO;
    let mut last = Instant::now();
    while start.elapsed().as_secs_f64() < seconds {
        if let Ok(buf) = rx.recv_timeout(Duration::from_millis(50)) {
            let now = Instant::now();
            if callbacks > 0 {
                max_gap = max_gap.max(now - last);
            }
            last = now;
            callbacks += 1;
            for f in buf.chunks(ch) {
                let _ = w.write_sample(f.iter().sum::<f32>() / ch as f32);
                frames += 1;
            }
        }
        while let Ok(e) = erx.try_recv() {
            errors += 1;
            eprintln!("mic-record: stream error: {e}");
        }
    }
    drop(stream);
    let _ = w.finalize();
    println!(
        "{{\"device\":\"{}\",\"rate_hz\":{rate},\"frames\":{frames},\"seconds\":{:.3},\"callbacks\":{callbacks},\"errors\":{errors},\"max_callback_gap_ms\":{:.3}}}",
        name.replace('"', "'"),
        start.elapsed().as_secs_f64(),
        max_gap.as_secs_f64() * 1e3
    );
}
