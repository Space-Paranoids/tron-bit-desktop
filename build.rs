//! Puts Bit's icon (assets/bit.ico) into the Windows .exe, so Explorer, the
//! taskbar and shortcuts show it. Other systems keep no icon in the program.

fn main() {
    println!("cargo:rerun-if-changed=assets/bit.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/bit.ico");
        res.set("FileDescription", "Bit desktop companion");
        res.set("ProductName", "Bit");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not add the icon to the exe: {e}");
        }
    }
}
