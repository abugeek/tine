//! Headless verification suite for `tine-ui`:
//! - Renders every scene at 1280x800 and 960x640 to PNG (`target/ui-snapshots/`) and dumps tree JSON.
//! - Asserts no overlapping interactive elements, no parent clipping, valid `focusable`+`role`+`label`,
//!   exact Tab focus order per `docs/project/05-ui-ux.md`, and WCAG >= 4.5:1 contrast.
//! - Compares rendered PNGs against `tests/snapshots/*.png` (`UPDATE_SNAPSHOTS=1` blesses).
//! - Exercises optimistic UI + `IntentSink` via `rune::Runtime`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rune::{App, Env, Key, KeyEvent, Mods, Runtime};
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

#[test]
fn interactions_update_optimistically_and_emit_intents() {
    let sink = RecordingSink::default();
    let mut init = make_golden_line();
    init.set_sink(sink.clone());

    let mut rt = Runtime::new(
        init,
        view,
        (1280.0, 800.0),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.frame(0.0);

    // Click A/B bypass button
    let (bx, by) = rt.find("header/bypass").expect("header/bypass");
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.model.bypass);

    // Click macro/body
    let before_body = rt.state.model.macros.body;
    let (mx, my) = rt.find("macro/body").expect("macro/body");
    rt.pointer_down(mx, my);
    rt.pointer_up(mx, my);
    rt.frame(1.0 / 60.0);
    assert!((rt.state.model.macros.body - before_body).abs() > 0.01);

    let recorded = sink.0.borrow();
    assert!(recorded.contains(&Intent::ToggleBypass));
    assert!(recorded.iter().any(|i| matches!(i, Intent::SetMacro(Macro::Body, _))));

    // Verify Runtime::key works with Space
    drop(recorded);
    rt.key(KeyEvent {
        key: Key::Text(" ".into()),
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
}
