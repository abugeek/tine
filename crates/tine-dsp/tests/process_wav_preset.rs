//! End-to-end check of the `--preset` / `--macros` flags of `examples/process_wav`.
//!
//! Runs the built example against the `dynamic_speech` fixture and asserts the
//! reported preset/macros/params evidence lines.

#![allow(missing_docs)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// Locates the compiled `process_wav` example (tests build examples in the
/// active cargo profile; check both common profile directories).
fn example_exe() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = manifest.join("../../target");
    let exe = if cfg!(windows) {
        "process_wav.exe"
    } else {
        "process_wav"
    };
    if let Ok(dir) = std::env::var("CARGO_TARGET_DIR")
        && Path::new(&dir).exists()
    {
        return PathBuf::from(dir).join("examples").join(exe);
    }
    for profile in ["debug", "release"] {
        let p = target.join(profile).join("examples").join(exe);
        if p.exists() {
            return p;
        }
    }
    panic!(
        "process_wav example binary not found under {} (build examples first: cargo test -p tine-dsp)",
        target.display()
    );
}

#[test]
fn preset_dynamic_with_macros_reports_expected_params() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture = manifest.join("tests/fixtures/dynamic_speech.flac");
    assert!(fixture.exists(), "missing fixture {}", fixture.display());
    let out = std::env::temp_dir().join("tine_process_wav_preset_test.wav");
    let _ = std::fs::remove_file(&out);

    let output = Command::new(example_exe())
        .arg(&fixture)
        .arg(&out)
        .args(["--preset", "dynamic"])
        .args(["--macros", "leveler=0.62,purity=0.7"])
        .output()
        .expect("run process_wav example");

    assert!(
        output.status.success(),
        "process_wav failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Preset evidence: the builtin `dynamic` profile with the requested macro nodes.
    assert!(
        stdout.contains("Preset:  dynamic (Dynamic USB / XLR)"),
        "missing preset line in:\n{stdout}"
    );
    assert!(
        stdout.contains("Macros:  purity=0.70 body=0.50 presence=0.50 leveler=0.62"),
        "missing macro line in:\n{stdout}"
    );
    // Params evidence: Dynamic base curve fields (HPF 80 Hz, +2/-3/+3.5 dB EQ)
    // and the mapped macro effects through MacroMapper::apply:
    // purity=0.70  -> denoise_mix 0.5+0.4*0.5=0.70, gate threshold -50+0.4*15=-44.00;
    // leveler=0.62 -> comp_ratio 3.0+0.24*1.75=3.42, comp_thresh -20-0.24*10=-22.40.
    assert!(
        stdout.contains("hpf_freq_hz=80.00"),
        "dynamic base HPF missing in:\n{stdout}"
    );
    assert!(
        stdout.contains("eq_gain_db=[2.00,-3.00,0.00,0.00,3.50]"),
        "dynamic base EQ missing in:\n{stdout}"
    );
    assert!(
        stdout.contains("denoise_mix=0.70"),
        "purity-mapped denoise_mix missing in:\n{stdout}"
    );
    assert!(
        stdout.contains("gate_threshold_db=-44.00"),
        "purity-mapped gate threshold missing in:\n{stdout}"
    );
    assert!(
        stdout.contains("comp_ratio=3.42"),
        "leveler-mapped comp ratio missing in:\n{stdout}"
    );
    assert!(
        stdout.contains("comp_thresh_db=-22.40"),
        "leveler-mapped comp threshold missing in:\n{stdout}"
    );
    assert!(
        stdout.contains("passthrough_latched:  false"),
        "pipeline latched on preset run:\n{stdout}"
    );

    let _ = std::fs::remove_file(&out);
}

#[test]
fn macros_without_preset_is_a_usage_error() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture = manifest.join("tests/fixtures/dynamic_speech.flac");
    let out = std::env::temp_dir().join("tine_process_wav_macros_err.wav");
    let output = Command::new(example_exe())
        .arg(fixture)
        .arg(&out)
        .args(["--macros", "leveler=0.62"])
        .output()
        .expect("run process_wav example");
    assert!(!output.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--macros requires --preset"),
        "unexpected error: {stderr}"
    );
    let _ = std::fs::remove_file(&out);
}
