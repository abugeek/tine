//! Live-only hardware composite of the D-018 routing: the real mic →
//! `VirtualCableProcessor` → the virtual cable sink (T13) driving the
//! T16 visualizer (T21's production stack exactly). Ignored by default
//! (`nextest` skips `#[ignore]`): it needs a capture device and a
//! virtual-cable render sink on the host.

use std::sync::Arc;
use std::time::Duration;

use tine_engine::virtual_cable::VirtualCableRouter;
use tine_engine::visualizer::{InProcessSignals, UiFrame, Visualizer};
use tine_protocol::params::DspParams;
use tine_protocol::sync::{ParamBlock, SpscRing, Triple};
use tine_protocol::telemetry::AudioTelemetryFrame;

/// The routing tap capacity the app uses.
const TAP_N: usize = 16_384;

#[test]
#[ignore = "hardware: needs a live mic and a virtual-cable render sink"]
fn live_router_feeds_the_visualizer_tap() {
    // 1. Real default mic and the virtual cable sink.
    let mic = tine_engine::audio_io::list_inputs()
        .expect("input enumeration")
        .into_iter()
        .find(|d| d.is_default)
        .expect("a default capture device");
    let sink = tine_engine::apo_admin::find_virtual_cable_sink()
        .expect("sink enumeration")
        .expect("a virtual cable sink");
    assert!(sink.is_available, "cable sink active");

    // 2. The app's exact production wiring.
    let params = Arc::new(ParamBlock::new());
    params.store(&DspParams::DEFAULT);
    let telemetry = Arc::new(Triple::new(AudioTelemetryFrame::default()));
    let tap = Arc::new(SpscRing::<TAP_N>::new());
    let router = VirtualCableRouter::start::<TAP_N>(
        &tine_engine::audio_io::DeviceRef::Id(mic.id.clone()),
        &tine_engine::audio_io::DeviceRef::Id(sink.id.clone()),
        Arc::clone(&params),
        Arc::clone(&telemetry),
        Arc::clone(&tap),
    )
    .expect("router start");
    eprintln!("router on mic={} sink={}", mic.name, sink.name);

    // Sanity: the producer side advances.
    std::thread::sleep(Duration::from_millis(500));
    let pre = tap.overruns();
    eprintln!("tap pre-drain overruns={pre} (full until the pump drains)");

    // Instrumentation: pop directly from the ring on this thread.
    let mut probe = vec![0.0_f32; 8192];
    let n = tap.pop_slice(&mut probe);
    eprintln!("direct pop: {n} samples, first few = {:?}", &probe[..n.min(4).min(n)]);

    // 3. The visualizer pump on the shared ring (the child's live path).
    let mut v = Visualizer::start(InProcessSignals {
        telemetry: Arc::clone(&telemetry),
        tap: Arc::clone(&tap),
        rate_hz: mic.default_rate,
    });
    let frames = v.frames();

    // 4. A few seconds of live frames.
    let mut last = UiFrame::default();
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        let mut f = UiFrame::default();
        if frames.latest(&mut f) {
            last = f;
        }
        eprintln!("seq {} bandmax {:.1}", last.telemetry.sequence, band_max(&last));
    }
    eprintln!(
        "final: seq {} bandmax {:.1} ticks visible={}",
        last.telemetry.sequence,
        band_max(&last),
        v.is_running()
    );
    println!("tap overruns after pump: {}", tap.overruns());
    v.stop();
    router.stop();
}

fn band_max(f: &UiFrame) -> f32 {
    f.spectrum.iter().copied().fold(f32::MIN, f32::max)
}
