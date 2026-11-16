//! T03 spike: Rune windowless app, tray, GPU release, single-instance, and memory measurement.

mod single_instance;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rune::tok::{self, radius, space, type_};
use rune::{
    App, AppSpec, Cx, El, Key, KeyEvent, Tray, TrayEvent, TrayIcon, TrayItem, TraySpec, Tx, col, el, row, text,
};

struct SpikeState {
    meter: f32,
    ticks: u64,
    bypass: bool,
    tray: Option<Arc<Tray>>,
}

impl Default for SpikeState {
    fn default() -> Self {
        Self {
            meter: 0.0,
            ticks: 0,
            bypass: false,
            tray: None,
        }
    }
}

fn build_tray_menu(bypass: bool) -> Vec<TrayItem> {
    let bypass_label = if bypass { "Bypass ✓" } else { "Bypass ✗" };
    vec![
        TrayItem::action("open", "Open Tine"),
        TrayItem::action("bypass", bypass_label),
        TrayItem::Separator,
        TrayItem::action("quit", "Quit"),
    ]
}

fn make_tray_icon() -> TrayIcon {
    const W: u32 = 16;
    const H: u32 = 16;
    let mut rgba = vec![0u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let idx = ((y * W + x) * 4) as usize;
            // Simple tuning-fork-like silhouette in white/cyan
            let is_prong_l = (4..=5).contains(&x) && (2..=9).contains(&y);
            let is_prong_r = (10..=11).contains(&x) && (2..=9).contains(&y);
            let is_bridge = (4..=11).contains(&x) && (9..=10).contains(&y);
            let is_stem = (7..=8).contains(&x) && (10..=14).contains(&y);
            if is_prong_l || is_prong_r || is_bridge || is_stem {
                rgba[idx] = 0x38;
                rgba[idx + 1] = 0xBD;
                rgba[idx + 2] = 0xF8;
                rgba[idx + 3] = 0xFF;
            }
        }
    }
    TrayIcon::rgba(W, H, rgba)
}

fn button(key: &'static str, label: &'static str, t: tok::Theme, f: impl Fn(&mut SpikeState, &mut Tx) + 'static) -> El {
    row()
        .key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .h(30.0)
        .px(space::S3)
        .center()
        .radius(radius::MD)
        .fill(t.card)
        .border(1.0, t.card_border)
        .on_press_tx(f)
        .child(text(label).font(type_::LABEL).color(t.text))
}

fn view(s: &SpikeState, cx: &mut Cx<SpikeState>) -> El {
    let t = tok::theme(cx.system_dark());
    let meter = s.meter.clamp(0.0, 1.0);
    let track_w = 340.0_f32;
    let bar_w = (track_w * meter).max(4.0);
    let db = if meter <= 0.0001 { -60.0 } else { 20.0 * meter.log10() };
    let status_text = if s.bypass {
        "Processing: BYPASSED"
    } else {
        "Processing: ACTIVE"
    };

    col()
        .key("root")
        .w_full()
        .h_full()
        .fill(t.bg)
        .px(space::S6)
        .py(space::S6)
        .gap(space::S3)
        .child(
            text("Tine — Rune Lifecycle Spike")
                .key("title")
                .font(type_::TITLE)
                .color(t.text),
        )
        .child(
            text(status_text)
                .key("status")
                .font(type_::BODY)
                .color(t.text_secondary),
        )
        .child(
            text(format!("Input level: {db:.1} dBFS  (tick {})", s.ticks))
                .key("meter-readout")
                .font(type_::BODY)
                .color(t.text_secondary),
        )
        .child(
            el().key("meter-track")
                .w(track_w)
                .h(12.0)
                .radius(6.0)
                .fill(t.card)
                .border(1.0, t.card_border)
                .child(el().key("meter-bar").w(bar_w).h(12.0).radius(6.0).fill(if s.bypass {
                    t.text_secondary
                } else {
                    tok::accent(0, cx.system_dark())
                })),
        )
        .child(
            row()
                .key("actions")
                .gap(space::S2)
                .child(button("toggle-bypass", "Toggle Bypass", t, |s, _| {
                    s.bypass = !s.bypass;
                    if let Some(tray) = &s.tray {
                        tray.set_menu(build_tray_menu(s.bypass));
                    }
                }))
                .child(button("close-window", "Close Window", t, |_, tx| tx.close_window())),
        )
}

fn on_key(_: &mut SpikeState, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match ev.key {
        Key::Command('w') => tx.close_window(),
        Key::Command('q') => tx.quit(),
        _ => return false,
    }
    true
}

fn is_headless_cli(args: &[String]) -> bool {
    args.iter().any(|a| {
        matches!(
            a.as_str(),
            "--snap" | "--tree" | "--list-scenes" | "--script" | "--trace" | "--video" | "--out" | "--strip"
        )
    })
}

fn run_ui_child(metrics: bool, t_start: Instant) {
    let mut frame_count = 0u64;
    let mut last_frame = Instant::now();
    let app = App::new(AppSpec::<SpikeState> {
        scenes: &[
            ("default", |_| {}),
            ("active", |s| {
                s.meter = 0.72;
                s.ticks = 42;
                s.bypass = false;
            }),
            ("bypassed", |s| {
                s.meter = 0.35;
                s.ticks = 84;
                s.bypass = true;
            }),
        ],
        on_key: Some(on_key),
        ..AppSpec::new("Tine Spike", (460.0, 260.0), SpikeState::default, view)
    })
    .args(Vec::new())
    .on_window(move |_| {
        if metrics {
            eprintln!(
                "metric:ui_child_open_ms={:.2}",
                t_start.elapsed().as_secs_f64() * 1000.0
            );
        }
    })
    .on_frame_drawn(move |_| {
        if metrics {
            let now = Instant::now();
            let dt_us = now.duration_since(last_frame).as_secs_f64() * 1_000_000.0;
            last_frame = now;
            frame_count += 1;
            if frame_count.is_multiple_of(60) {
                eprintln!("metric:frame_dt_us={dt_us:.1} frames={frame_count}");
            }
        }
    });

    let handle = app.handle();
    std::thread::Builder::new()
        .name("tine-meter-feeder".to_owned())
        .spawn(move || {
            let mut phase = 0.0_f32;
            loop {
                phase = (phase + 0.08) % std::f32::consts::TAU;
                let fake_meter = 0.5 + 0.45 * phase.sin();
                handle.update(move |s, _| {
                    s.ticks = s.ticks.wrapping_add(1);
                    s.meter = fake_meter;
                });
                std::thread::sleep(Duration::from_millis(16));
            }
        })
        .ok();

    app.run();
}

enum ParentMsg {
    Open,
    ToggleBypass,
    Quit,
}

fn run_parent_shell(metrics: bool, t_start: Instant, child_cycles: Option<u32>) {
    let instance_guard = match single_instance::acquire_or_notify() {
        single_instance::AcquireResult::Primary(guard) => guard,
        single_instance::AcquireResult::SecondaryNotified => return,
    };

    let (tx, rx) = std::sync::mpsc::channel::<ParentMsg>();

    let tray_tx = tx.clone();
    let tray = match Tray::new(
        TraySpec::new("dev.tine.spike", "Tine Spike", make_tray_icon())
            .tooltip("Tine Spike")
            .menu(build_tray_menu(false)),
        move |ev| match ev {
            TrayEvent::Activate { .. } => {
                let _ = tray_tx.send(ParentMsg::Open);
            }
            TrayEvent::Menu(id) => match id.as_str() {
                "open" => {
                    let _ = tray_tx.send(ParentMsg::Open);
                }
                "bypass" => {
                    let _ = tray_tx.send(ParentMsg::ToggleBypass);
                }
                "quit" => {
                    let _ = tray_tx.send(ParentMsg::Quit);
                }
                _ => {}
            },
            _ => {}
        },
    ) {
        Ok(t) => {
            if metrics {
                eprintln!(
                    "metric:cold_start_tray_ms={:.2}",
                    t_start.elapsed().as_secs_f64() * 1000.0
                );
            }
            Some(t)
        }
        Err(e) => {
            if metrics {
                eprintln!(
                    "metric:cold_start_tray_ms={:.2} tray_ok=false err={e}",
                    t_start.elapsed().as_secs_f64() * 1000.0
                );
            }
            None
        }
    };

    let pipe_tx = tx.clone();
    single_instance::spawn_pipe_listener(instance_guard.pipe_name.clone(), move || {
        let _ = pipe_tx.send(ParentMsg::Open);
    });

    if let Some(n) = child_cycles
        && let Ok(exe) = std::env::current_exe()
    {
        std::thread::Builder::new()
            .name("tine-child-cycle-bench".to_owned())
            .spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                for i in 0..n {
                    let t0 = Instant::now();
                    let mut cmd = std::process::Command::new(&exe);
                    cmd.arg("--ui");
                    if metrics {
                        cmd.arg("--metrics");
                    }
                    if let Ok(mut child) = cmd.spawn() {
                        std::thread::sleep(Duration::from_millis(180));
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    if i == 0 && metrics {
                        eprintln!("metric:child_cycle_0_ms={:.2}", t0.elapsed().as_secs_f64() * 1000.0);
                    }
                    if (i + 1) % 10 == 0 {
                        eprintln!("metric:child_cycle_done={}/{}", i + 1, n);
                    }
                }
                eprintln!("metric:all_child_cycles_done={n}");
            })
            .ok();
    }

    let mut bypass = false;
    let mut child: Option<std::process::Child> = None;

    while let Ok(msg) = rx.recv() {
        match msg {
            ParentMsg::Open => {
                let alive = child.as_mut().is_some_and(|c| c.try_wait().ok().flatten().is_none());
                if !alive && let Ok(exe_path) = std::env::current_exe() {
                    let mut cmd = std::process::Command::new(exe_path);
                    cmd.arg("--ui");
                    if metrics {
                        cmd.arg("--metrics");
                    }
                    child = cmd.spawn().ok();
                }
            }
            ParentMsg::ToggleBypass => {
                bypass = !bypass;
                if let Some(t) = &tray {
                    t.set_menu(build_tray_menu(bypass));
                }
            }
            ParentMsg::Quit => {
                if let Some(mut c) = child.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                break;
            }
        }
    }
}

fn main() {
    let t_start = Instant::now();
    let raw_args: Vec<String> = std::env::args().collect();
    let headless = is_headless_cli(&raw_args);
    let metrics = raw_args.iter().any(|a| a == "--metrics");
    let start_open = raw_args.iter().any(|a| a == "--open");
    let ui_mode = raw_args.iter().any(|a| a == "--ui");
    let parent_mode = raw_args.iter().any(|a| a == "--parent");
    let cycles: Option<u32> = raw_args
        .windows(2)
        .find(|w| w[0] == "--cycles")
        .and_then(|w| w[1].parse().ok());
    let child_cycles: Option<u32> = raw_args
        .windows(2)
        .find(|w| w[0] == "--child-cycles")
        .and_then(|w| w[1].parse().ok());

    if ui_mode {
        run_ui_child(metrics, t_start);
        return;
    }
    if parent_mode {
        run_parent_shell(metrics, t_start, child_cycles);
        return;
    }

    let mut filtered_args: Vec<String> = Vec::with_capacity(raw_args.len());
    let mut skip_next = false;
    for arg in &raw_args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--cycles" {
            skip_next = true;
            continue;
        }
        if matches!(arg.as_str(), "--metrics" | "--open") {
            continue;
        }
        filtered_args.push(arg.clone());
    }

    let instance_guard = if headless {
        None
    } else {
        match single_instance::acquire_or_notify() {
            single_instance::AcquireResult::Primary(guard) => Some(guard),
            single_instance::AcquireResult::SecondaryNotified => return,
        }
    };

    let tray_holder: Arc<std::sync::Mutex<Option<Arc<Tray>>>> = Arc::new(std::sync::Mutex::new(None));
    let tray_for_init = Arc::clone(&tray_holder);

    let first_open_req = Arc::new(std::sync::Mutex::new(None::<Instant>));
    let first_open_done = Arc::new(AtomicBool::new(false));

    let req_on_win = Arc::clone(&first_open_req);
    let done_on_win = Arc::clone(&first_open_done);

    let mut frame_count = 0u64;
    let mut last_frame = Instant::now();

    let mut app = App::new(AppSpec::<SpikeState> {
        scenes: &[
            ("default", |_| {}),
            ("active", |s| {
                s.meter = 0.72;
                s.ticks = 42;
                s.bypass = false;
            }),
            ("bypassed", |s| {
                s.meter = 0.35;
                s.ticks = 84;
                s.bypass = true;
            }),
        ],
        on_key: Some(on_key),
        ..AppSpec::new("Tine Spike", (460.0, 260.0), SpikeState::default, view)
    })
    .args(filtered_args)
    .init(move || {
        let tray = tray_for_init.lock().ok().and_then(|g| g.clone());
        SpikeState {
            tray,
            ..SpikeState::default()
        }
    })
    .keep_running()
    .release_gpu_when_closed()
    .on_window(move |_| {
        if metrics
            && let Ok(guard) = req_on_win.lock()
            && let Some(t0) = *guard
        {
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            let kind = if done_on_win.swap(true, Ordering::AcqRel) {
                "reopen"
            } else {
                "first_open"
            };
            eprintln!("metric:{kind}_window_ms={ms:.2}");
        }
    })
    .on_close(|_, _| {})
    .on_frame_drawn(move |_| {
        if metrics {
            let now = Instant::now();
            let dt_us = now.duration_since(last_frame).as_secs_f64() * 1_000_000.0;
            last_frame = now;
            frame_count += 1;
            if frame_count.is_multiple_of(60) {
                eprintln!("metric:frame_dt_us={dt_us:.1} frames={frame_count}");
            }
        }
    });

    if !start_open {
        app = app.windowless();
    }

    let handle = app.handle();

    if !headless {
        let tray_handle = handle.clone();
        let tray_req = Arc::clone(&first_open_req);
        let tray = Tray::new(
            TraySpec::new("dev.tine.spike", "Tine Spike", make_tray_icon())
                .tooltip("Tine Spike")
                .menu(build_tray_menu(false)),
            move |ev| match ev {
                TrayEvent::Activate { .. } => {
                    if let Ok(mut g) = tray_req.lock() {
                        *g = Some(Instant::now());
                    }
                    tray_handle.open();
                }
                TrayEvent::Menu(id) => match id.as_str() {
                    "open" => {
                        if let Ok(mut g) = tray_req.lock() {
                            *g = Some(Instant::now());
                        }
                        tray_handle.open();
                    }
                    "bypass" => {
                        tray_handle.update(|s, _| {
                            s.bypass = !s.bypass;
                            if let Some(t) = &s.tray {
                                t.set_menu(build_tray_menu(s.bypass));
                            }
                        });
                    }
                    "quit" => {
                        tray_handle.quit();
                    }
                    _ => {}
                },
                _ => {}
            },
        );
        if let Ok(t) = tray {
            if let Ok(mut g) = tray_holder.lock() {
                *g = Some(Arc::new(t));
            }
            if metrics {
                eprintln!(
                    "metric:cold_start_tray_ms={:.2}",
                    t_start.elapsed().as_secs_f64() * 1000.0
                );
            }
        }

        if let Some(guard) = &instance_guard {
            let pipe_handle = handle.clone();
            let pipe_req = Arc::clone(&first_open_req);
            single_instance::spawn_pipe_listener(guard.pipe_name.clone(), move || {
                if let Ok(mut g) = pipe_req.lock() {
                    *g = Some(Instant::now());
                }
                pipe_handle.open();
            });
        }

        // Background 60 Hz fake meter feeder: only updates while window is visible.
        let meter_handle = handle.clone();
        std::thread::Builder::new()
            .name("tine-meter-feeder".to_owned())
            .spawn(move || {
                let mut phase = 0.0_f32;
                loop {
                    if meter_handle.is_visible() {
                        phase = (phase + 0.08) % std::f32::consts::TAU;
                        let fake_meter = 0.5 + 0.45 * phase.sin();
                        meter_handle.update(move |s, _| {
                            s.ticks = s.ticks.wrapping_add(1);
                            s.meter = fake_meter;
                        });
                        std::thread::sleep(Duration::from_millis(16));
                    } else {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
            })
            .ok();
    }

    if let Some(n) = cycles {
        let cycle_handle = handle.clone();
        let cycle_req = Arc::clone(&first_open_req);
        std::thread::Builder::new()
            .name("tine-cycle-bench".to_owned())
            .spawn(move || {
                std::thread::sleep(Duration::from_millis(500));
                for i in 0..n {
                    if let Ok(mut g) = cycle_req.lock() {
                        *g = Some(Instant::now());
                    }
                    cycle_handle.open();
                    let wait_start = Instant::now();
                    while !cycle_handle.is_visible() && wait_start.elapsed() < Duration::from_secs(2) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    std::thread::sleep(Duration::from_millis(120));
                    cycle_handle.close();
                    let close_start = Instant::now();
                    while cycle_handle.is_visible() && close_start.elapsed() < Duration::from_secs(2) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    std::thread::sleep(Duration::from_millis(80));
                    if (i + 1) % 10 == 0 {
                        eprintln!("metric:cycle_done={}/{}", i + 1, n);
                    }
                }
                eprintln!("metric:all_cycles_done={n}");
            })
            .ok();
    }

    app.run();
}
