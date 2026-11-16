//! Build script declaring custom `cfg(loom)` for `cargo::rustc-check-cfg`.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(loom)");
}
