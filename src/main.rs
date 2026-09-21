#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "macos")]
mod macos;
mod ui;
use receive::{diagnostics, favicon, model, thread, worker};

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;

fn main() {
    if std::env::args().any(|arg| arg == "--diagnose") {
        if let Err(error) = diagnostics::run() {
            eprintln!("Diagnostics: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    let data_dir = directories::ProjectDirs::from("dev", "Receive", "Receive")
        .expect("Could not locate the application data directory")
        .data_local_dir()
        .to_path_buf();
    let smoke_test = std::env::args().any(|arg| arg == "--smoke-test");
    let preview = smoke_test || std::env::args().any(|arg| arg == "--preview");
    if smoke_test {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(20));
            eprintln!("UI smoke test timed out before all screens rendered.");
            std::process::exit(1);
        });
    }
    gpui_kit::application()
        .with_assets(ui::assets::Assets)
        .run(move |cx| {
            #[cfg(target_os = "macos")]
            macos::install_icon();
            gpui_kit::init(cx);
            ui::theme::install(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(1260.), px(820.)), cx);
            cx.spawn(async move |cx| {
                cx.open_window(
                    WindowOptions {
                        // Receive draws its own title bar, so the window frame is
                        // as black as the rest of the interface.
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        window_min_size: Some(size(px(1000.), px(650.))),
                        ..TitleBar::window_options()
                    },
                    |window, cx| {
                        let view = cx
                            .new(|cx| ui::Receive::new(data_dir, preview, smoke_test, window, cx));
                        cx.new(|cx| Root::new(view, window, cx))
                    },
                )
                .expect("Could not open Receive");
            })
            .detach();
        });
}
