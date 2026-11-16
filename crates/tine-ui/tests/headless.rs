//! Headless verification suite for `tine-ui`:
//! - Renders every scene at 1280x800 and 960x640 to PNG (`target/ui-snapshots/`) and dumps tree JSON.
//! - Asserts no overlapping interactive elements, no parent clipping, valid `focusable`+`role`+`label`,
//!   exact Tab focus order per `docs/project/05-ui-ux.md`, and WCAG >= 4.5:1 contrast.
//! - Compares rendered PNGs against `tests/snapshots/*.png` (`UPDATE_SNAPSHOTS=1` blesses).
//! - Exercises optimistic UI + `IntentSink` via `rune::Runtime`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rune::{App, Env, FrameOutcome, Key, KeyEvent, Mods, Runtime};
use serde_json::Value;
use tine_ui::scenes::{SCENES, make_golden_line};
use tine_ui::theme::{TineTheme, contrast_ratio};
use tine_ui::{AppState, Intent, IntentSink, Macro, NoopSink, app_spec, view};

const SIZES: [(u32, u32); 2] = [(1280, 800), (960, 640)];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is under crates/tine-ui")
        .to_path_buf()
}

fn read_png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    if bytes.len() < 24 || bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Some((w, h))
}

fn rects_overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    const EPS: f32 = 0.5;
    let ax2 = a[0] + a[2];
    let ay2 = a[1] + a[3];
    let bx2 = b[0] + b[2];
    let by2 = b[1] + b[3];
    a[0] + EPS < bx2 && ax2 - EPS > b[0] && a[1] + EPS < by2 && ay2 - EPS > b[1]
}

fn parse_rect(v: &Value) -> [f32; 4] {
    let arr = v.as_array().expect("rect must be array");
    [
        arr[0].as_f64().expect("x") as f32,
        arr[1].as_f64().expect("y") as f32,
        arr[2].as_f64().expect("w") as f32,
        arr[3].as_f64().expect("h") as f32,
    ]
}

fn assert_tree_invariants(scene_name: &str, w: u32, h: u32, tree: &Value) {
    let elements = tree
        .get("elements")
        .and_then(Value::as_array)
        .expect("tree.json must have an 'elements' array");

    let mut interactive: Vec<(String, [f32; 4])> = Vec::new();
    let mut focus_order: Vec<String> = Vec::new();

    for el in elements {
        let key = el.get("key").and_then(Value::as_str).unwrap_or("<unkeyed>").to_owned();
        let inert = el.get("inert").and_then(Value::as_bool).unwrap_or(false);
        let focusable = el.get("focusable").and_then(Value::as_bool).unwrap_or(false);
        let opacity = el.get("opacity").and_then(Value::as_f64).unwrap_or(1.0) as f32;
        let rect = parse_rect(el.get("rect").expect("element missing rect"));

        // Nothing clipped outside window bounds (within 1.0 pt rounding tolerance)
        assert!(
            rect[0] >= -1.0
                && rect[1] >= -1.0
                && rect[0] + rect[2] <= w as f32 + 1.0
                && rect[1] + rect[3] <= h as f32 + 1.0,
            "scene={scene_name} ({w}x{h}) element {key} clipped outside viewport: {rect:?}"
        );

        if focusable && !inert && opacity > 0.0 {
            let role = el.get("role").and_then(Value::as_str).unwrap_or("");
            let label = el.get("label").and_then(Value::as_str).unwrap_or("");
            assert!(
                !role.is_empty(),
                "scene={scene_name} focusable element {key} missing role"
            );
            assert!(
                !label.is_empty(),
                "scene={scene_name} focusable element {key} missing label"
            );
            interactive.push((key.clone(), rect));
            focus_order.push(key);
        }
    }

    // No two active interactive elements overlap
    for i in 0..interactive.len() {
        for j in (i + 1)..interactive.len() {
            let (ref ka, ra) = interactive[i];
            let (ref kb, rb) = interactive[j];
            assert!(
                !rects_overlap(ra, rb),
                "scene={scene_name} ({w}x{h}) interactive elements overlap: {ka} {ra:?} vs {kb} {rb:?}"
            );
        }
    }

    // Focus order matches docs/project/05-ui-ux.md
    if scene_name == "golden_line" {
        let expected = [
            "header/mic-selector",
            "header/arch-dynamic",
            "header/arch-condenser",
            "header/arch-headset",
            "header/arch-laptop",
            "header/autotune",
            "header/monitor",
            "header/bypass",
            "macro/purity",
            "macro/body",
            "macro/presence",
            "macro/leveler",
            "pro/toggle",
        ];
        assert_eq!(
            focus_order, expected,
            "scene={scene_name} ({w}x{h}) focus order mismatch"
        );
    } else if scene_name == "golden_line_tooltip" {
        // Same control surface as golden_line, plus the (non-focusable) tooltip card,
        // which must not overlap any interactive element.
        let expected = [
            "header/mic-selector",
            "header/arch-dynamic",
            "header/arch-condenser",
            "header/arch-headset",
            "header/arch-laptop",
            "header/autotune",
            "header/monitor",
            "header/bypass",
            "macro/purity",
            "macro/body",
            "macro/presence",
            "macro/leveler",
            "pro/toggle",
        ];
        assert_eq!(
            focus_order, expected,
            "scene={scene_name} ({w}x{h}) focus order mismatch"
        );
    } else if scene_name == "pro_drawer_open" {
        let expected = [
            "header/mic-selector",
            "header/arch-dynamic",
            "header/arch-condenser",
            "header/arch-headset",
            "header/arch-laptop",
            "header/autotune",
            "header/monitor",
            "header/bypass",
            "macro/purity",
            "macro/body",
            "macro/presence",
            "macro/leveler",
            "pro/toggle",
            "pro/unpin-all",
        ];
        assert_eq!(
            focus_order, expected,
            "scene={scene_name} ({w}x{h}) focus order mismatch"
        );
    }
}

#[test]
fn headless_scenes_render_and_pass_checks() {
    let root = workspace_root();
    let out_dir = root.join("target").join("ui-snapshots");
    let golden_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("snapshots");
    std::fs::create_dir_all(&out_dir).expect("create target/ui-snapshots");
    std::fs::create_dir_all(&golden_dir).expect("create tests/snapshots");

    let update_snapshots = std::env::var("UPDATE_SNAPSHOTS").ok().as_deref() == Some("1");

    for &(scene_name, _) in SCENES {
        for &(w, h) in &SIZES {
            let png_name = format!("{scene_name}_{w}x{h}.png");
            let png_path = out_dir.join(&png_name);
            let tree_path = out_dir.join(format!("{scene_name}_{w}x{h}.tree.json"));
            let size_arg = format!("{w}x{h}");

            App::new(app_spec(NoopSink, AppState::default()))
                .args(vec![
                    "--snap".into(),
                    png_path.to_string_lossy().into_owned(),
                    "--tree".into(),
                    tree_path.to_string_lossy().into_owned(),
                    "--scene".into(),
                    scene_name.into(),
                    "--size".into(),
                    size_arg,
                    "--scale".into(),
                    "1".into(),
                    // Three frames: Cx::layout reads settle after the first layout.
                    "--fps".into(),
                    "60".into(),
                    "--at".into(),
                    "0.0501".into(),
                ])
                .run();

            let actual_png = std::fs::read(&png_path).expect("rendered PNG must exist");
            let dims = read_png_dimensions(&actual_png).expect("valid PNG header");
            assert_eq!(dims, (w, h), "PNG dimensions mismatch for {png_name}");

            let tree_text = std::fs::read_to_string(&tree_path).expect("tree.json must exist");
            let tree: Value = serde_json::from_str(&tree_text).expect("valid tree JSON");
            assert_tree_invariants(scene_name, w, h, &tree);

            let golden_path = golden_dir.join(&png_name);
            if update_snapshots {
                std::fs::write(&golden_path, &actual_png).expect("write golden snapshot");
            } else {
                let expected_png = std::fs::read(&golden_path).unwrap_or_else(|_| {
                    panic!(
                        "missing golden snapshot {}; run with UPDATE_SNAPSHOTS=1",
                        golden_path.display()
                    )
                });
                assert_eq!(
                    read_png_dimensions(&expected_png),
                    Some((w, h)),
                    "golden PNG dimension mismatch for {png_name}"
                );
                assert!(
                    actual_png == expected_png,
                    "snapshot mismatch for {png_name} (compare {} vs {}; run UPDATE_SNAPSHOTS=1 to bless)",
                    png_path.display(),
                    golden_path.display()
                );
            }
        }
    }
}

#[test]
fn theme_contrast_meets_wcag_aa() {
    let t = TineTheme::dark();
    for bg in [t.bg, t.surface, t.surface_raised] {
        assert!(contrast_ratio(t.text, bg) >= 4.5);
        assert!(contrast_ratio(t.text_dim, bg) >= 4.5);
    }
    assert!(contrast_ratio(t.text_on_accent, t.accent) >= 4.5);
}

#[derive(Clone, Default)]
struct RecordingSink(Rc<RefCell<Vec<Intent>>>);

impl IntentSink for RecordingSink {
    fn send(&self, i: Intent) {
        self.0.borrow_mut().push(i);
    }
}

fn runtime_with(sink: RecordingSink) -> Runtime<AppState> {
    let mut init = make_golden_line();
    init.set_sink(sink);
    Runtime::new(
        init,
        view,
        (1280.0, 800.0),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    )
}

fn element_label(rt: &Runtime<AppState>, key: &str) -> String {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .and_then(|e| e.label)
        .unwrap_or_default()
}

fn focused_key(rt: &Runtime<AppState>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

#[test]
fn interactions_update_optimistically_and_emit_intents() {
    let sink = RecordingSink::default();
    let mut rt = runtime_with(sink.clone());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);

    // Click A/B bypass button.
    let (bx, by) = rt.find("header/bypass").expect("header/bypass");
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.model.bypass);

    // Space with no focused control also toggles A/B.
    rt.key(KeyEvent {
        key: Key::Text(" ".into()),
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.model.bypass);

    let recorded = sink.0.borrow();
    assert_eq!(
        recorded.iter().filter(|i| **i == Intent::ToggleBypass).count(),
        2,
        "click + Space must both toggle bypass"
    );
}

#[test]
fn mic_selector_opens_native_dropdown_and_picks_endpoint() {
    let sink = RecordingSink::default();
    let mut rt = runtime_with(sink.clone());
    rt.frame(0.0);

    let (mx, my) = rt.find("header/mic-selector").expect("header/mic-selector");
    rt.pointer_down(mx, my);
    rt.pointer_up(mx, my);
    rt.frame(1.0 / 60.0);

    let menu = rt.popup_menu().expect("mic dropdown menu requested").clone();
    let second_id = "{0.0.1.00000000}.{headset-boom}";
    assert!(menu.find(second_id).is_some(), "dropdown must list every endpoint");
    assert!(menu.find(second_id).is_some_and(rune::PopupItem::is_enabled));

    rt.resolve_popup(Some(second_id)).expect("pick resolves");
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.model.selected, Some(1));
    assert!(sink.0.borrow().contains(&Intent::SelectEndpoint(1)));
}

#[test]
fn drag_body_up_100px_is_monotonic_and_updates_label() {
    let sink = RecordingSink::default();
    let start_body = make_golden_line().model.macros.body;
    let mut rt = runtime_with(sink.clone());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);

    let (mx, my) = rt.find("macro/body").expect("macro/body");
    rt.pointer_down(mx, my);
    rt.frame(1.0 / 60.0);

    // Drag up 100 px in ten steps: upward (negative dy) raises the macro.
    for k in 1..=10_u16 {
        rt.pointer_move(mx, my - f32::from(k) * 10.0);
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_up(mx, my - 100.0);
    rt.frame(1.0 / 60.0);

    let seq: Vec<f32> = sink
        .0
        .borrow()
        .iter()
        .filter_map(|i| match i {
            Intent::SetMacro(Macro::Body, v) => Some(*v),
            _ => None,
        })
        .collect();
    assert!(seq.len() >= 10, "drag must emit SetMacro per move: {seq:?}");
    assert!(
        seq.windows(2).all(|w| w[1] >= w[0]),
        "SetMacro(Body) sequence must be monotonic: {seq:?}"
    );
    let expected = (start_body + 100.0 * 0.004).min(1.0);
    assert!((rt.state.model.macros.body - expected).abs() < 1e-4);
    assert_eq!(element_label(&rt, "macro/body"), format!("Body {expected:.2}"));
}

#[test]
fn double_click_resets_macro_to_preset_default() {
    let sink = RecordingSink::default();
    let mut rt = runtime_with(sink.clone());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);

    let (mx, my) = rt.find("macro/presence").expect("macro/presence");
    rt.pointer_down(mx, my);
    rt.pointer_up(mx, my);
    rt.frame(1.0 / 60.0);
    rt.pointer_down(mx, my);
    rt.pointer_up(mx, my);
    rt.frame(1.0 / 60.0);

    assert!(
        (rt.state.model.macros.presence - 0.5).abs() < 1e-4,
        "double-click must reset to the preset default"
    );
    assert!(
        sink.0
            .borrow()
            .iter()
            .any(|i| matches!(i, Intent::SetMacro(Macro::Presence, v) if (*v - 0.5).abs() < 1e-4))
    );
}

/// Keyboard-only run: Tab walks the documented focus order, arrows nudge the focused
/// macro (±0.01, Shift ±0.1), and Space toggles A/B even with a node focused.
#[test]
fn keyboard_only_focus_order_walk() {
    let sink = RecordingSink::default();
    let mut rt = runtime_with(sink.clone());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(focused_key(&rt), None, "nothing focused before the first Tab");

    let expected = [
        "header/mic-selector",
        "header/arch-dynamic",
        "header/arch-condenser",
        "header/arch-headset",
        "header/arch-laptop",
        "header/autotune",
        "header/monitor",
        "header/bypass",
        "macro/purity",
        "macro/body",
        "macro/presence",
        "macro/leveler",
        "pro/toggle",
    ];
    for key in expected {
        rt.key(KeyEvent {
            key: Key::Tab,
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
        assert_eq!(focused_key(&rt).as_deref(), Some(key), "focus order mismatch at {key}");
    }

    // Wraps back to the first focusable.
    rt.key(KeyEvent {
        key: Key::Tab,
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("header/mic-selector"));

    // Keyboard all the way to macro/body, then nudge with arrows.
    for _ in 0..9 {
        rt.key(KeyEvent {
            key: Key::Tab,
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(focused_key(&rt).as_deref(), Some("macro/body"));

    let before = rt.state.model.macros.body;
    rt.key(KeyEvent {
        key: Key::Up,
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
    assert!(
        (rt.state.model.macros.body - (before + 0.01)).abs() < 1e-4,
        "arrow = ±0.01"
    );

    rt.key(KeyEvent {
        key: Key::Up,
        mods: Mods {
            shift: true,
            ..Mods::default()
        },
    });
    rt.frame(1.0 / 60.0);
    assert!(
        (rt.state.model.macros.body - (before + 0.11)).abs() < 1e-4,
        "Shift+arrow = ±0.1 ({})",
        rt.state.model.macros.body
    );

    // Space with a node focused still toggles A/B: nodes take no Space of their own.
    let bypass_before = rt.state.model.bypass;
    rt.key(KeyEvent {
        key: Key::Text(" ".into()),
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
    assert_ne!(rt.state.model.bypass, bypass_before);
    assert_eq!(
        focused_key(&rt).as_deref(),
        Some("macro/body"),
        "focus stays on the node"
    );
}

/// `--script` CLI run: dragging `macro/body` up 100 px must end with a tree label
/// that reads the new value.
#[test]
fn scripted_drag_updates_tree_label_via_cli() {
    let root = workspace_root();
    let out_dir = root.join("target").join("ui-snapshots");
    std::fs::create_dir_all(&out_dir).expect("create target/ui-snapshots");
    let script_path = out_dir.join("t19_drag_body_up.json");
    let tree_path = out_dir.join("t19_drag_body_up.tree.json");
    std::fs::write(
        &script_path,
        r#"[{"t": 0.1, "type": "drag", "key": "macro/body", "dx": 0.0, "dy": -100.0, "dur": 0.3}]"#,
    )
    .expect("write script");

    App::new(app_spec(NoopSink, make_golden_line()))
        .args(vec![
            "--scene".into(),
            "golden_line".into(),
            "--size".into(),
            "1280x800".into(),
            "--scale".into(),
            "1".into(),
            "--fps".into(),
            "60".into(),
            "--script".into(),
            script_path.to_string_lossy().into_owned(),
            "--tree".into(),
            tree_path.to_string_lossy().into_owned(),
        ])
        .run();

    let tree_text = std::fs::read_to_string(&tree_path).expect("tree.json must exist");
    let tree: Value = serde_json::from_str(&tree_text).expect("valid tree JSON");
    let elements = tree.get("elements").and_then(Value::as_array).expect("elements");
    let label = elements
        .iter()
        .find(|e| e.get("key").and_then(Value::as_str) == Some("macro/body"))
        .and_then(|e| e.get("label"))
        .and_then(Value::as_str)
        .expect("macro/body label");
    assert_eq!(label, "Body 1.00", "dragged node must read its new value");
}

/// Rune frame profile at 1280x800: mean build + encode across the drawn frames.
#[test]
fn headless_frame_build_and_encode_budget() {
    let samples: Rc<RefCell<Vec<f64>>> = Rc::new(RefCell::new(Vec::new()));
    let recorded = samples.clone();

    App::new(app_spec(NoopSink, make_golden_line()))
        .args(vec![
            "--scene".into(),
            "golden_line".into(),
            "--size".into(),
            "1280x800".into(),
            "--scale".into(),
            "1".into(),
            "--fps".into(),
            "60".into(),
            "--frames".into(),
            "120".into(),
        ])
        .on_frame_encoded(move |f| {
            if f.outcome() == FrameOutcome::Drawn {
                let ms = f.update_ms().unwrap_or(0.0) + f.encode_ms().unwrap_or(0.0);
                recorded.borrow_mut().push(ms);
            }
        })
        .run();

    let v = samples.borrow();
    assert!(v.len() >= 60, "must have drawn frames: {v:?}");
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    let max = v.iter().copied().fold(0.0_f64, f64::max);
    eprintln!(
        "tine-ui frame profile @1280x800 (dev build): frames={} mean build+encode={mean:.3} ms max={max:.3} ms",
        v.len()
    );
    assert!(mean < 4.0, "mean build+encode {mean:.3} ms must stay under 4 ms");
}
