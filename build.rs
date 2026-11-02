//! Embeds the Windows icon (and version info) into the executables.

fn main() {
    println!("cargo:rerun-if-changed=assets/rustty-icon.ico");
    // Gate on TARGET at runtime: build scripts run for the HOST, and
    // cross-checks from Windows to Linux must not run the resource compiler.
    if std::env::var("TARGET").is_ok_and(|t| t.contains("windows")) {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/rustty-icon.ico");
        if let Err(e) = res.compile() {
            eprintln!("failed to embed Windows resources: {e}");
            std::process::exit(1);
        }
    }
}
