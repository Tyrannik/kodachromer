// Embed the icon and version info into the Windows executable.
fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        res.set("ProductName", "Kodachromer");
        res.set("FileDescription", "Kodachromer - film emulation for Leica M9 DNGs");
        res.set("LegalCopyright", "GPL-3.0-or-later");
        res.compile().expect("compiling Windows resources (icon)");
    }
}
