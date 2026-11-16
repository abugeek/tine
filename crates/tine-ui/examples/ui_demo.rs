//! Standalone `tine-ui` demo running with deterministic fake telemetry and a logging `IntentSink`.
//!
//! Also reports the Rune frame profile: mean build+encode over the last window of
//! drawn frames and the 60 Hz share of one core it implies (T19's budget: < 4 ms
//! at 1280x800, < 3 % CPU at 60 Hz).

use std::cell::RefCell;
use std::time::{Duration, Instant};

use rune::{App, FrameOutcome};
use tine_ui::fake::FakeTelemetry;
use tine_ui::scenes::make_golden_line;
use tine_ui::{Intent, IntentSink, app_spec};

struct LoggingSink;

impl IntentSink for LoggingSink {
    fn send(&self, i: Intent) {
        eprintln!("[tine-ui demo intent] {i:?}");
    }
}

thread_local! {
    /// Build+encode times of the drawn frames since the last profile line (UI thread only).
    static FRAME_SAMPLES: RefCell<Vec<f64>> = const { RefCell::new(Vec::new()) };
}

const REPORT_EVERY: usize = 5 * 60; // samples → one stderr line each 5 s at 60 Hz

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let app = App::new(app_spec(LoggingSink, make_golden_line()))
        .args(args)
        .on_frame_encoded(|f| {
            if f.outcome() == FrameOutcome::Drawn {
                let ms = f.update_ms().unwrap_or(0.0) + f.encode_ms().unwrap_or(0.0);
                FRAME_SAMPLES.with_borrow_mut(|s| {
                    s.push(ms);
                    if s.len() >= REPORT_EVERY {
                        let mean = s.iter().sum::<f64>() / s.len() as f64;
                        let max = s.iter().copied().fold(0.0_f64, f64::max);
                        let share = mean / (1000.0 / 60.0) * 100.0;
                        eprintln!(
                            "[tine-ui demo profile] frames={} mean build+encode={mean:.3} ms max={max:.3} ms ≈ {share:.1}% of one core at 60 Hz",
                            s.len()
                        );
                        s.clear();
                    }
                });
            }
        });
    let handle = app.handle();

    std::thread::Builder::new()
        .name("tine-ui-demo-feeder".to_owned())
        .spawn(move || {
            let start = Instant::now();
            let mut tel = FakeTelemetry::default();
            loop {
                if handle.is_visible() {
                    let t = start.elapsed().as_secs_f32();
                    let next_tel = tel.clone();
                    handle.update(move |s, _| {
                        let mut g = next_tel;
                        s.live = g.sample(t, s.model.bypass);
                    });
                    // Advance generator deterministically outside the closure too
                    let _ = tel.sample(t, false);
                }
                std::thread::sleep(Duration::from_millis(16));
            }
        })
        .ok();

    app.run();
}
