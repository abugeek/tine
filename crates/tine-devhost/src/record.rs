//! Non-RT WAV file writer fed by a lock-free `SpscRing`.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use hound::{SampleFormat, WavSpec, WavWriter};
use tine_dsp::primitives::db::lin_to_db;
use tine_protocol::sync::SpscRing;

/// Ring capacity for WAV recording tap (must be a power of two).
pub const RECORD_RING_CAP: usize = 32_768;

/// Summary metrics collected while writing a WAV recording.
#[derive(Clone, Copy, Debug)]
pub struct WavStats {
    /// Total mono samples written.
    pub samples: u64,
    /// Peak amplitude in dBFS.
    pub peak_db: f32,
    /// Integrated RMS level in dBFS.
    pub rms_db: f32,
}

/// Background WAV recorder draining mono `f32` samples from an [`SpscRing`].
pub struct WavRecorder {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<Result<WavStats, String>>>,
}

impl WavRecorder {
    /// Spawns a non-RT thread that drains `ring` and writes 32-bit float mono WAV to `path`.
    pub fn start(path: &Path, sample_rate: u32, ring: Arc<SpscRing<RECORD_RING_CAP>>) -> Result<Self, String> {
        let spec = WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let writer = WavWriter::create(path, spec).map_err(|e| format!("failed to create {}: {e}", path.display()))?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);

        let handle = thread::Builder::new()
            .name("tine-wav-writer".to_owned())
            .spawn(move || writer_loop(writer, &ring, &stop_thread))
            .map_err(|e| format!("failed to spawn wav writer thread: {e}"))?;

        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }

    /// Signals the writer thread to flush remaining samples, finalize the WAV header, and return recording stats.
    pub fn finish(mut self) -> Result<WavStats, String> {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            handle.join().map_err(|_| "wav writer thread panicked".to_owned())?
        } else {
            Ok(WavStats {
                samples: 0,
                peak_db: -120.0,
                rms_db: -120.0,
            })
        }
    }
}

fn writer_loop(
    mut writer: WavWriter<BufWriter<File>>,
    ring: &SpscRing<RECORD_RING_CAP>,
    stop: &AtomicBool,
) -> Result<WavStats, String> {
    let mut buf = [0.0f32; 2048];
    let mut total_samples = 0u64;
    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;

    loop {
        let n = ring.pop_slice(&mut buf);
        if n > 0 {
            for &s in buf.iter().take(n) {
                writer.write_sample(s).map_err(|e| format!("wav write error: {e}"))?;
                let abs = s.abs();
                if abs > peak {
                    peak = abs;
                }
                sum_sq += f64::from(s) * f64::from(s);
            }
            total_samples = total_samples.saturating_add(n as u64);
            continue;
        }

        if stop.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }

    writer.finalize().map_err(|e| format!("wav finalize error: {e}"))?;
    let rms = if total_samples > 0 {
        (sum_sq / (total_samples as f64)).sqrt() as f32
    } else {
        0.0
    };
    Ok(WavStats {
        samples: total_samples,
        peak_db: lin_to_db(peak),
        rms_db: lin_to_db(rms),
    })
}
