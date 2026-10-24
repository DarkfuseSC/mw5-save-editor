fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/app.ico")
            .set("ProductName", "MW5 Save Editor")
            .set("FileDescription", "MechWarrior 5: Mercenaries save editor and repair tool")
            .set("LegalCopyright", "MIT License")
            .set("OriginalFilename", "MW5SaveEditor.exe");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon resource not embedded: {}", e);
        }
    }
}
