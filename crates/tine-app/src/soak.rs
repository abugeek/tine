//! `tine soak --minutes N --report <file>`: run the engine for a fixed window
//! and record CPU, RSS, private bytes, and engine liveliness, writing a JSON
//! report (QA soak evidence, T22 rehearsal).
//!
//! Runs either inside the app (measuring itself while the controller serves)
//! or standalone in the CLI (a local controller on a temp profile store, so
//! the measurement never touches persisted user state).

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::GetCurrentProcess;

/// Sample cadence while soaking.
pub const SAMPLE_PERIOD: Duration = Duration::from_secs(1);

/// One measurement row.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    /// Seconds since soak start.
    pub t_s: f64,
    /// Process CPU (kernel+user) percent of one core over the last period.
    pub cpu_pct: f64,
    /// Working set bytes.
    pub working_set: u64,
    /// Private bytes.
    pub private_bytes: u64,
    /// Live controller (telemetry generational sequence) evidence — 0 when
    /// no publisher is linked (normal under D-018 until T16/T21 wire the
    /// router).
    pub telemetry_sequence: u64,
}

/// CPU + memory sampler over the current process.
pub struct Soak {
    started: Instant,
    last_kernel_us: u64,
    last_user_us: u64,
    last_at: Instant,
    samples: Vec<Sample>,
}

impl Soak {
    /// New sampler; the first baseline is taken immediately.
    #[must_use]
    pub fn new() -> Self {
        let (k, u) = cpu_times_us();
        Self {
            started: Instant::now(),
            last_kernel_us: k,
            last_user_us: u,
            last_at: Instant::now(),
            samples: Vec::new(),
        }
    }

    /// Takes one sample now and stores it.
    #[must_use]
    pub fn sample(&mut self, telemetry_sequence: u64) -> Sample {
        let now = Instant::now();
        let (k, u) = cpu_times_us();
        let period_s = self.last_at.elapsed().as_secs_f64().max(1e-3);
        let cpu_pct = (k.saturating_sub(self.last_kernel_us) + u.saturating_sub(self.last_user_us)) as f64
                / 10_000.0 // µs → ms
                / period_s
            * 100.0;
        let (working_set, private_bytes) = memory_bytes();
        let s = Sample {
            t_s: self.started.elapsed().as_secs_f64(),
            cpu_pct,
            working_set,
            private_bytes,
            telemetry_sequence,
        };
        self.samples.push(s);
        self.last_kernel_us = k;
        self.last_user_us = u;
        self.last_at = now;
        s
    }

    /// All samples so far.
    #[must_use]
    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    /// Wall time since the soak began.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

impl Default for Soak {
    fn default() -> Self {
        Self::new()
    }
}

fn cpu_times_us() -> (u64, u64) {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle; all pointers are
    // valid FILETIMEs for the duration of the call.
    unsafe {
        let proc = GetCurrentProcess();
        let _ = windows::Win32::System::Threading::GetProcessTimes(
            proc,
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        );
    }
    (time_100ns_to_us(kernel), time_100ns_to_us(user))
}

fn time_100ns_to_us(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime)) / 10
}

fn memory_bytes() -> (u64, u64) {
    // `cb` set in the initializer: the struct must describe its own size.
    let cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb,
        ..PROCESS_MEMORY_COUNTERS_EX::default()
    };
    // SAFETY: the current process pseudo-handle is valid, and `counters` is a
    // `PROCESS_MEMORY_COUNTERS_EX` with `cb` set to its own size (the API
    // accepts the wider struct for the plain view).
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            cb,
        )
    };
    if ok.is_ok() {
        (counters.WorkingSetSize as u64, counters.PrivateUsage as u64)
    } else {
        (0, 0)
    }
}

/// Builds the JSON report document.
#[must_use]
pub fn report_json(s: &Soak, notes: &Value) -> Value {
    let samples: Vec<Value> = s
        .samples()
        .iter()
        .map(|x| {
            json!({
                "t_s": round2(x.t_s),
                "cpu_pct": round2(x.cpu_pct),
                "working_set_mb": round2(x.working_set as f64 / 1e6),
                "private_mb": round2(x.private_bytes as f64 / 1e6),
                "telemetry_sequence": x.telemetry_sequence,
            })
        })
        .collect();
    let avg_cpu = s.samples().iter().map(|x| x.cpu_pct).sum::<f64>() / s.samples().len().max(1) as f64;
    let max_ws = s.samples().iter().map(|x| x.working_set).max().unwrap_or(0);
    let max_priv = s.samples().iter().map(|x| x.private_bytes).max().unwrap_or(0);
    let max_cpu = s.samples().iter().map(|x| x.cpu_pct).fold(0.0, f64::max);
    json!({
        "tool": "tine soak",
        "tine_version": crate::version::tine_version(),
        "duration_s": round2(s.elapsed().as_secs_f64()),
        "sample_count": s.samples().len(),
        "summary": {
            "avg_cpu_pct": round2(avg_cpu),
            "max_cpu_pct": round2(max_cpu),
            "max_working_set_mb": round2(max_ws as f64 / 1e6),
            "max_private_mb": round2(max_priv as f64 / 1e6),
        },
        "samples": samples,
        "notes": notes,
    })
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Writes a report document to `path` (pretty JSON).
///
/// # Errors
/// Propagates io errors from creating the parent dir or writing the file.
pub fn write_report(path: &Path, report: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        serde_json::to_vec_pretty(report).unwrap_or_else(|_| b"{}".to_vec()),
    )
}

/// Samples the process in the foreground until the window elapses (CLI mode).
///
/// # Panics
/// Never: running inside the process it measures; all failures degrade to
/// zeroed samples.
pub fn run_blocking(
    minutes: u64,
    report_path: &Path,
    notes: &Value,
    telemetry_seq: &(dyn Fn() -> u64 + Send),
) -> Value {
    let mut soak = Soak::new();
    let deadline = Duration::from_secs(minutes.saturating_mul(60));
    while soak.elapsed() < deadline && !crate::console::interrupted() {
        std::thread::sleep(SAMPLE_PERIOD);
        let _ = soak.sample(telemetry_seq());
    }
    let report = report_json(&soak, notes);
    let _ = write_report(report_path, &report);
    report
}

#[cfg(test)]
mod tests {
    use super::{Soak, report_json, write_report};
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn samples_carry_time_and_memory() {
        let mut s = Soak::new();
        std::thread::sleep(Duration::from_millis(20));
        let sample = s.sample(42);
        assert!(sample.t_s >= 0.0);
        assert!(sample.cpu_pct >= 0.0);
        assert_eq!(sample.telemetry_sequence, 42);
        let report = report_json(&s, &json!({"where": "test"}));
        let samples = report
            .get("samples")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(samples.len(), 1);
        assert_eq!(
            samples[0].get("telemetry_sequence").and_then(serde_json::Value::as_u64),
            Some(42)
        );
        assert!(
            report
                .get("summary")
                .and_then(|s| s.get("max_working_set_mb"))
                .is_some()
        );
    }

    #[test]
    fn report_writes_pretty_json_to_disk() {
        let mut s = Soak::new();
        let _ = s.sample(0);
        let report = report_json(&s, &json!({"where": "test"}));
        let dir = std::env::temp_dir().join(format!("tine-soak-{}", std::process::id()));
        let file = dir.join("soak.json");
        write_report(&file, &report).unwrap_or(());
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        assert!(text.starts_with('{'));
        assert!(text.contains("\"sample_count\""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
