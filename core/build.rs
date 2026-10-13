// Embed the app icon into the Windows executables (Explorer, taskbar, shortcuts).
fn main() {
    println!("cargo:rerun-if-changed=assets/app-icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/app-icon.ico");
        res.set("ProductName", "Download Manager Pro");
        res.set("FileDescription", "Download Manager Pro");
        if let Err(e) = res.compile() {
            // A missing resource compiler must not break the build; the exe just keeps the default icon.
            println!("cargo:warning=could not embed Windows icon: {e}");
        }
    }
}
