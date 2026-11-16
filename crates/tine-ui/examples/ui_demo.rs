//! Standalone `tine-ui` demo running with deterministic fake telemetry and a logging `IntentSink`.

use std::time::{Duration, Instant};

use rune::App;
use tine_ui::fake::FakeTelemetry;
use tine_ui::scenes::make_golden_line;
use tine_ui::{Intent, IntentSink, app_spec};

struct LoggingSink;

impl IntentSink for LoggingSink {
    fn send(&self, i: Intent) {
        eprintln!("[tine-ui demo intent] {i:?}");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let app = App::new(app_spec(LoggingSink, make_golden_line())).args(args);
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
