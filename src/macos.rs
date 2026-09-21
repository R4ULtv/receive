//! The Dock icon also works when launched directly with `cargo run`.

use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::NSData;

pub fn install_icon() {
    let Some(main_thread) = MainThreadMarker::new() else {
        eprintln!("The application icon must be installed on the main thread.");
        return;
    };
    // Embedded data works independently of the working directory or bundling.
    let data = NSData::with_bytes(include_bytes!("../assets/receive.icns"));
    let Some(icon) = NSImage::initWithData(NSImage::alloc(), &data) else {
        eprintln!("Could not load the Receive application icon.");
        return;
    };
    let application = NSApplication::sharedApplication(main_thread);
    // SAFETY: AppKit is called on its main thread with a valid, non-null NSImage.
    unsafe {
        application.setApplicationIconImage(Some(&icon));
    }
}
