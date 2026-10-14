//! Embeds the Hoppy icon and version info into hoppy.exe (shown by "Send to → Hoppy").
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
