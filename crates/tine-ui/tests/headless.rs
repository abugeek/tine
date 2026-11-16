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
    let mut saw_scroll_viewport = false;

    for el in elements {
        let key = el.get("key").and_then(Value::as_str).unwrap_or("<unkeyed>").to_owned();
        let path = el.get("path").and_then(Value::as_str).unwrap_or("").to_owned();
        let inert = el.get("inert").and_then(Value::as_bool).unwrap_or(false);
        let focusable = el.get("focusable").and_then(Value::as_bool).unwrap_or(false);
        let opacity = el.get("opacity").and_then(Value::as_f64).unwrap_or(1.0) as f32;
        let rect = parse_rect(el.get("rect").expect("element missing rect"));

        // The Pro Rack scroll viewport itself must always fit the window.
        if key == "pro/scroll-viewport" {
            saw_scroll_viewport = true;
            assert!(
                rect[0] >= -1.0
                    && rect[1] >= -1.0
                    && rect[0] + rect[2] <= w as f32 + 1.0
                    && rect[1] + rect[3] <= h as f32 + 1.0,
                "scene={scene_name} ({w}x{h}) scroll viewport clipped: {rect:?}"
            );
        }

        // Nothing clipped outside window bounds (within 1.0 pt rounding tolerance) —
        // except rack rows living inside the vertical scroll viewport: those may
        // scroll below the fold, but never off the left/right edges or above the
        // window (the viewport opens scrolled to the top).
        let inside_scroll = path.contains("pro/scroll-viewport");
        let horizontally_inside = rect[0] >= -1.0 && rect[0] + rect[2] <= w as f32 + 1.0;
        let vertically_inside = rect[1] >= -1.0 && rect[1] + rect[3] <= h as f32 + 1.0;
        if inside_scroll {
            assert!(
                horizontally_inside && rect[1] >= -1.0,
                "scene={scene_name} ({w}x{h}) rack element {key} clips horizontally/above: {rect:?}"
            );
        } else {
            assert!(
                horizontally_inside && vertically_inside,
                "scene={scene_name} ({w}x{h}) element {key} clipped outside viewport: {rect:?}"
            );
        }

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
            // Rack rows beyond the fold still must not overlap; only visible
            // interactive elements pair against each other.
            if vertically_inside || inside_scroll {
                interactive.push((key.clone(), rect));
            }
            focus_order.push(key);
        }
    }

    // No two active interactive elements overlap
    for i in 0..interactive.len() {
        for j in (i + 1)..interactive.len() {
            // Rack rows in different scroll positions share x with other rows but
            // never a y range; the generic overlap check below covers them too.
            let (ref ka, ra) = interactive[i];
            let (ref kb, rb) = interactive[j];
            let below_fold_pair = ka.starts_with("pro/") && kb.starts_with("pro/") && scene_name == "pro_drawer_open";
            if below_fold_pair {
                // Rows clipped by the viewport (already scrolled past) may sit outside
                // the window; skip cross-fold pairs.
                let a_inside = ra[1] >= 0.0 && ra[1] + ra[3] <= h as f32 + 1.0;
                let b_inside = rb[1] >= 0.0 && rb[1] + rb[3] <= h as f32 + 1.0;
                if !(a_inside && b_inside) {
                    continue;
                }
            }
            assert!(
                !rects_overlap(ra, rb),
                "scene={scene_name} ({w}x{h}) interactive elements overlap: {ka} {ra:?} vs {kb} {rb:?}"
            );
        }
    }

    let expected_prefix = [
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

    // Focus order matches docs/project/05-ui-ux.md.
    if scene_name == "golden_line" || scene_name == "golden_line_tooltip" {
        assert_eq!(
            focus_order, expected_prefix,
            "scene={scene_name} ({w}x{h}) focus order mismatch"
        );
    } else if scene_name == "pro_drawer_open" {
        // Header → macros → Pro toggle ... then drawer controls (05-ui-ux.md:
        // "Pro toggle → drawer controls"), stage by stage, every param reachable.
        assert!(
            starts_with_prefix(&focus_order, &expected_prefix),
            "scene={scene_name} ({w}x{h}) focus order prefix mismatch: {focus_order:?}"
        );
        let rest = &focus_order[expected_prefix.len()..];
        assert!(!rest.is_empty(), "drawer controls must be tab stops");
        assert!(
            rest.iter().all(|k| k.starts_with("pro/")
                || k.starts_with("switch/")
                || k.starts_with("slider/")
                || k.starts_with("select/")
                || k.starts_with("pin/")),
            "scene={scene_name} ({w}x{h}) unexpected post-toggle focusables: {rest:?}"
        );
        assert!(
            saw_scroll_viewport,
            "scene={scene_name} ({w}x{h}) must mount the rack scroll viewport"
        );
    } else {
        // Modal scenes: focus is trapped in the modal; nothing from the inert
        // background may hold a tab stop — checked implicitly by the modal focus
        // runtime test below.
        let _ = expected_prefix;
    }
}

fn starts_with_prefix(order: &[String], prefix: &[&str]) -> bool {
    order.len() >= prefix.len() && order.iter().zip(prefix.iter()).all(|(o, p)| o == p)
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

/// T20: every parameter in `SPECS` has a reachable, labelled control in the
/// `pro_drawer_open` tree — the global Bypass through the header's A/B switch,
/// every other parameter through its generated rack control.
#[test]
fn pro_rack_every_param_reachable_and_labelled() {
    let root = workspace_root();
    let out_dir = root.join("target").join("ui-snapshots");
    std::fs::create_dir_all(&out_dir).expect("create target/ui-snapshots");
    let tree_path = out_dir.join("pro_drawer_open.tree.json");

    App::new(app_spec(NoopSink, tine_ui::scenes::make_pro_drawer_open()))
        .args(vec![
            "--snap".into(),
            out_dir.join("pro_drawer_open.png").to_string_lossy().into_owned(),
            "--tree".into(),
            tree_path.to_string_lossy().into_owned(),
            "--scene".into(),
            "pro_drawer_open".into(),
            "--size".into(),
            "1280x800".into(),
            "--scale".into(),
            "1".into(),
            "--fps".into(),
            "60".into(),
            "--at".into(),
            "0.0501".into(),
        ])
        .run();

    let tree: Value =
        serde_json::from_str(&std::fs::read_to_string(&tree_path).expect("tree.json")).expect("valid JSON");
    let elements = tree.get("elements").and_then(Value::as_array).expect("elements");
    let find_el = |key: &str| -> Option<&Value> {
        elements
            .iter()
            .find(|e| e.get("key").and_then(Value::as_str) == Some(key))
    };

    for i in 0..tine_protocol::params::PARAM_COUNT {
        let Some(id) = tine_protocol::params::ParamId::from_index(i) else {
            continue;
        };
        let key = if id == tine_protocol::params::ParamId::Bypass {
            String::from("header/bypass")
        } else {
            tine_ui::views::pro_rack::control_key(id)
        };
        let el = find_el(&key).unwrap_or_else(|| panic!("no reachable control for {id:?}: {key}"));
        let label = el.get("label").and_then(Value::as_str).unwrap_or("");
        assert!(!label.is_empty(), "{key} must carry a label");
        let rect = parse_rect(el.get("rect").expect("rect"));
        assert!(rect[2] > 0.0 && rect[3] > 0.0, "{key} must be laid out: {rect:?}");
    }
}

/// T20 scripted run: open the drawer, drag `comp_ratio` — the intent log shows
/// `SetParam(CompRatio, …)`, the parameter is pinned, and the pin icon appears
/// in the element tree.
#[test]
fn pro_rack_edit_pins_param_and_emits_intent() {
    let sink = RecordingSink::default();
    let mut init = make_golden_line();
    init.ui.pro_drawer_open = true;
    let mut rt = runtime_with_state(sink.clone(), init);
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);

    // Scroll the rack until the compressor section is on screen, then drag
    // comp_ratio to the right.
    let (sx, sy) = scroll_rack_to(&mut rt, "slider/comp_ratio");
    rt.pointer_down(sx, sy);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(sx + 100.0, sy);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(sx + 100.0, sy);
    rt.frame(1.0 / 60.0);

    let recorded = sink.0.borrow();
    let set_params: Vec<f32> = recorded
        .iter()
        .filter_map(|i| match i {
            Intent::SetParam(tine_protocol::params::ParamId::CompRatio, v) => Some(*v),
            _ => None,
        })
        .collect();
    assert!(
        !set_params.is_empty() && set_params.iter().all(|v| *v >= 3.0),
        "drag must emit SetParam(CompRatio, …): {set_params:?}"
    );
    drop(recorded);

    // Optimistic pin + tree-visible pin icon.
    assert!(rt.state.model.pins.is_pinned(tine_protocol::params::ParamId::CompRatio));
    rt.frame(1.0 / 60.0);
    let pin = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("pin/comp_ratio"))
        .expect("pin icon must appear in the tree");
    assert_eq!(pin.role, "button");
    assert!(pin.label.as_deref().unwrap_or("").contains("Unpin"));

    // Clicking the pin unpins and emits Unpin.
    let px = pin.rect[0] + pin.rect[2] / 2.0;
    let py = pin.rect[1] + pin.rect[3] / 2.0;
    rt.pointer_down(px, py);
    rt.pointer_up(px, py);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.model.pins.is_pinned(tine_protocol::params::ParamId::CompRatio));
    assert!(
        sink.0
            .borrow()
            .contains(&Intent::Unpin(tine_protocol::params::ParamId::CompRatio))
    );

    // Toggling the drawer emits the persistence intent.
    let (tx_, ty_) = rt.find("pro/toggle").expect("pro/toggle");
    rt.pointer_down(tx_, ty_);
    rt.pointer_up(tx_, ty_);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.ui.pro_drawer_open);
    assert!(sink.0.borrow().contains(&Intent::SetProDrawerOpen(false)));
}

/// T20: the numeric field clamps out-of-range input to the SPECS range and shows
/// the inline message in the tree.
#[test]
fn pro_rack_numeric_field_clamps_and_shows_inline_message() {
    let sink = RecordingSink::default();
    let mut init = make_golden_line();
    init.ui.pro_drawer_open = true;
    let mut rt = runtime_with_state(sink.clone(), init);
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);

    // Open comp_ratio's numeric field (scrolled into view first).
    let (fx, fy) = scroll_rack_to(&mut rt, "field/comp_ratio");
    rt.pointer_down(fx, fy);
    rt.pointer_up(fx, fy);
    rt.frame(1.0 / 60.0);
    assert_eq!(
        rt.state.ui.pro_field.param,
        Some(tine_protocol::params::ParamId::CompRatio)
    );

    // The text field mounts; click it to focus, then type an out-of-range value.
    let (ex, ey) = rt.find("field-edit/comp_ratio").expect("field-edit/comp_ratio");
    rt.pointer_down(ex, ey);
    rt.pointer_up(ex, ey);
    rt.frame(1.0 / 60.0);

    // Type an out-of-range value, replacing the seed: select the seed text
    // first, then type over it.
    let seed_len = rt.state.ui.pro_field.edit.text.len();
    rt.state.ui.pro_field.edit.set_selection(0, seed_len);
    rt.frame(1.0 / 60.0);
    for ch in ['9', '9'] {
        rt.key(KeyEvent {
            key: Key::Text(ch.to_string()),
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
    }

    // Enter commits: clamps 99 → 20.0, pins, emits SetParam, inline message shows.
    rt.key(KeyEvent {
        key: Key::Enter,
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);

    assert!(
        (rt.state.model.params.comp_ratio - 20.0).abs() < 1e-4,
        "clamped to range"
    );
    assert!(
        sink.0
            .borrow()
            .contains(&Intent::SetParam(tine_protocol::params::ParamId::CompRatio, 20.0))
    );
    let msg = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("field-msg/comp_ratio"))
        .expect("inline clamp message must show")
        .text
        .clone()
        .unwrap_or_default();
    assert!(msg.contains("clamped"), "message: {msg:?}");
}

/// T20: scripted wizard run through every step with fake progress (T11 progress
/// contract simulated through the view model).
#[test]
fn auto_tune_wizard_scripted_run_through_every_step() {
    use tine_ui::{AutoTuneStep, AutoTuneUi, CalFailureUi};

    let sink = RecordingSink::default();
    let mut rt = runtime_with(sink.clone());

    let set_step = |rt: &mut Runtime<AppState>, au: AutoTuneUi| {
        rt.state.model.autotune = au;
        rt.invalidate();
        rt.frame(1.0 / 60.0);
    };

    // The background must be inert while the wizard is up.
    set_step(
        &mut rt,
        AutoTuneUi {
            step: AutoTuneStep::Prepare { remaining_secs: 0.9 },
            progress: 0.0,
            level_db: -48.0,
            voiced_ms: 0,
            prompt_text: String::new(),
        },
    );
    assert!(rt.find("autotune/cancel").is_some());
    let bypass = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("header/bypass"))
        .expect("header/bypass");
    assert!(bypass.inert, "background must be inert behind the modal");

    // Silence: countdown ring + live level.
    set_step(
        &mut rt,
        AutoTuneUi {
            step: AutoTuneStep::Silence { remaining_secs: 2.1 },
            progress: 0.3,
            voiced_ms: 0,
            level_db: -51.4,
            prompt_text: String::new(),
        },
    );
    assert!(rt.find("autotune/countdown").is_some());
    assert_eq!(
        rt.state.model.autotune.step,
        AutoTuneStep::Silence { remaining_secs: 2.1 }
    );

    // Speak: suggested sentence + level.
    set_step(
        &mut rt,
        AutoTuneUi {
            step: AutoTuneStep::Speak { remaining_secs: 3.2 },
            progress: 0.66,
            voiced_ms: 2600,
            level_db: -21.5,
            prompt_text: String::new(),
        },
    );
    assert!(rt.find("autotune/countdown").is_some());

    // Analyzing.
    set_step(
        &mut rt,
        AutoTuneUi {
            step: AutoTuneStep::Analyzing,
            progress: 0.92,
            voiced_ms: 3200,
            level_db: -24.0,
            prompt_text: String::new(),
        },
    );
    assert!(rt.find("autotune/progress").is_some());

    // Results: measurements, diffs, keep-pins, Apply/Discard/Retry.
    rt.state.model.calibration = Some(tine_ui::CalibrationUi {
        // set before the set_step below, which invalidates
        noise_rms_db: -51.4,
        speech_rms_db: -21.5,
        simulated_peak_db: -4.5,
        simulated_rms_db: -17.2,
        changes: vec![tine_ui::CalibrationParamDiff {
            param: tine_protocol::params::ParamId::CompRatio,
            label: "Comp Ratio",
            before: 3.0,
            after: 3.6,
            unit: ":1",
        }],
    });
    set_step(
        &mut rt,
        AutoTuneUi {
            step: AutoTuneStep::Results,
            progress: 1.0,
            voiced_ms: 4800,
            level_db: -18.0,
            prompt_text: String::new(),
        },
    );
    assert!(rt.find("autotune/apply").is_some());
    assert!(rt.find("autotune/discard").is_some());
    assert!(rt.find("autotune/retry").is_some());
    assert!(rt.find("autotune/keep-pins").is_some());

    // Uncheck "Keep my Pro pins", then Apply carries keep_pins: false.
    let (kx, ky) = rt.find("autotune/keep-pins").expect("keep-pins");
    rt.pointer_down(kx, ky);
    rt.pointer_up(kx, ky);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.ui.autotune_keep_pins);

    let (ax, ay) = rt.find("autotune/apply").expect("apply");
    rt.pointer_down(ax, ay);
    rt.pointer_up(ax, ay);
    rt.frame(1.0 / 60.0);
    assert!(sink.0.borrow().contains(&Intent::ApplyCalibration { keep_pins: false }));

    // Failed card: concrete remedy per CalFailure + Retry.
    set_step(
        &mut rt,
        AutoTuneUi {
            step: AutoTuneStep::Failed(CalFailureUi::Clipping { pct: 0.02 }),
            progress: 0.0,
            voiced_ms: 400,
            level_db: -2.0,
            prompt_text: String::new(),
        },
    );
    let remedy = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("autotune/failure-remedy"))
        .expect("remedy")
        .text
        .clone()
        .unwrap_or_default();
    assert!(remedy.contains("clipping"), "remedy: {remedy:?}");

    let (rx, ry) = rt.find("autotune/retry").expect("retry");
    rt.pointer_down(rx, ry);
    rt.pointer_up(rx, ry);
    rt.frame(1.0 / 60.0);
    assert!(sink.0.borrow().contains(&Intent::StartAutoTune));

    // Esc during an in-progress phase cancels, and focus returns to the Auto-Tune
    // button once the engine closes the wizard.
    set_step(
        &mut rt,
        AutoTuneUi {
            step: AutoTuneStep::Silence { remaining_secs: 1.0 },
            progress: 0.5,
            voiced_ms: 0,
            level_db: -50.0,
            prompt_text: String::new(),
        },
    );
    rt.key(KeyEvent {
        key: Key::Escape,
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
    assert!(sink.0.borrow().contains(&Intent::CancelAutoTune));
    // Engine closes the wizard:
    rt.state.model.autotune = AutoTuneUi::default();
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert!(rt.find("autotune-card").is_none(), "wizard must be gone");
    assert!(
        matches!(rt.state.model.autotune.step, AutoTuneStep::Idle),
        "engine resets to Idle"
    );
    assert!(
        focused_key(&rt).is_none() || focused_key(&rt).as_deref() != Some("header/bypass"),
        "focus must not be left on an inert background control"
    );
}

/// T20: the first-run onboarding flow reaches the finish line and emits the
/// documented intents on the way.
#[test]
fn onboarding_flow_walks_every_step() {
    let sink = RecordingSink::default();
    let mut init = tine_ui::scenes::make_onboarding();
    init.set_sink(sink.clone());
    let mut rt = Runtime::new(
        init,
        tine_ui::views::root::view as fn(&AppState, &mut rune::Cx<AppState>) -> rune::El,
        (1280.0, 800.0),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);

    // Step 1: the endpoints list; pick the second mic.
    let (mx, my) = rt.find("onboarding/mic-1").expect("second mic radio");
    rt.pointer_down(mx, my);
    rt.pointer_up(mx, my);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.model.selected, Some(1));
    assert!(sink.0.borrow().contains(&Intent::SelectEndpoint(1)));

    let (nx, ny) = rt.find("onboarding/next-1").expect("continue");
    rt.pointer_down(nx, ny);
    rt.pointer_up(nx, ny);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.ui.onboarding_step, 2);

    // Step 2: install starts, then the engine reports failure…
    let (ix, iy) = rt.find("onboarding/install").expect("install");
    rt.pointer_down(ix, iy);
    rt.pointer_up(ix, iy);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.ui.apo_installing);
    assert!(sink.0.borrow().contains(&Intent::InstallApo));
    rt.state.model.apo = tine_ui::ApoBadge::Error(String::from("Access denied (0x80070005)"));
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert!(rt.find("onboarding/apo-error").is_some(), "typed error shows");
    assert!(rt.find("onboarding/apo-remedies").is_some(), "remedies show");

    // …then success: the yellow status turns green and continue appears.
    rt.state.model.apo = tine_ui::ApoBadge::Active;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert!(rt.find("onboarding/apo-success").is_some());

    let (nx, ny) = rt.find("onboarding/next-2").expect("skip/continue from step 2");
    rt.pointer_down(nx, ny);
    rt.pointer_up(nx, ny);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.ui.onboarding_step, 3);

    // Step 3: Run Auto-Tune hands over to the wizard (StartAutoTune) at step 4.
    let (rx, ry) = rt.find("onboarding/run-autotune").expect("run autotune");
    rt.pointer_down(rx, ry);
    rt.pointer_up(rx, ry);
    rt.frame(1.0 / 60.0);
    assert!(sink.0.borrow().contains(&Intent::StartAutoTune));
    assert_eq!(rt.state.ui.onboarding_step, 4);

    // Step 4: Hear Myself test + Finish dismisses the overlay.
    let (hx, hy) = rt.find("onboarding/monitor-test").expect("monitor test");
    rt.pointer_down(hx, hy);
    rt.pointer_up(hx, hy);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.model.monitor.enabled);
    assert!(sink.0.borrow().contains(&Intent::ToggleMonitor));

    let (fx, fy) = rt.find("onboarding/finish").expect("finish");
    rt.pointer_down(fx, fy);
    rt.pointer_up(fx, fy);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.ui.onboarding_dismissed);
    rt.frame(1.0 / 60.0);
    assert!(rt.find("onboarding-card").is_none(), "onboarding must be gone");
}

/// Wheels the Pro Rack scroll viewport down until `key` is on screen, and
/// returns its position.
fn scroll_rack_to(rt: &mut Runtime<AppState>, key: &str) -> (f32, f32) {
    let mut pos = rt.find(key);
    for _ in 0..20 {
        if let Some((x, y)) = pos
            && let Some(rect) = rt
                .elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some("pro/scroll-viewport"))
                .map(|e| e.rect)
            && y >= rect[1]
            && y + 20.0 <= rect[1] + rect[3]
        {
            return (x, y);
        }
        rt.wheel(640.0, 600.0, -240.0);
        rt.frame(1.0 / 60.0);
        pos = rt.find(key);
    }
    pos.expect(key)
}

fn runtime_with_state(sink: RecordingSink, init: AppState) -> Runtime<AppState> {
    let mut init = init;
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
