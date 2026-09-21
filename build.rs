//! Embeds the Windows application icon, so Receive is itself in Explorer, the
//! taskbar, and Alt-Tab rather than the default executable icon.
//!
//! `assets/receive.ico` is `assets/receive.svg` rendered at 16 through 256 pixels. Nothing
//! is embedded on other platforms, and a machine without the Windows SDK's
//! `rc.exe` warns rather than failing the build: the icon is presentation, not
//! something the application needs to run.

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/receive.ico");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/receive.ico");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=Could not embed the application icon: {error}");
        }
    }
}
