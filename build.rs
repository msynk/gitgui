// Embeds an application manifest (Common Controls v6, per-monitor DPI v2, UTF-8 code page)
// through linker arguments, so no resource compiler or extra crate is required.
fn main() {
    let target = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target == "msvc" {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("app.manifest").display());
        println!("cargo:rerun-if-changed=app.manifest");
    }
}
