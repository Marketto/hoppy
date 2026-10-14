//! Embeds the Hoppy icon (resource id 1, loaded by the tray in platform.rs) and version info
//! into every binary of this package: hoppy.exe and hoppyw.exe (`compile()` emits
//! `rustc-link-arg`, which applies to all bins).
fn main() {
    println!("cargo:rerun-if-changed=../../assets/brand/hoppy.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/brand/hoppy.ico");
        res.set("ProductName", "Hoppy");
        res.set("FileDescription", "Hoppy - send files to nearby devices");
        if let Err(e) = res.compile() {
            // Missing resource compiler must not break the build; the exe just has no icon.
            println!("cargo:warning=could not embed the Windows icon: {e}");
        }
    }
}
