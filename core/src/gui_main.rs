// Release builds are a GUI app: no console window behind it on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use tokio::sync::RwLock;

use eframe::egui::Vec2;
use dm_core::{api, updater, youtube, DownloadManager};

mod gui_app;
mod tray;
use gui_app::NativeApp;

/// If the app is already running (likely hidden in the tray), ask it to show its window.
/// Returns true when that instance took over, so this launch should just exit.
fn activate_running_instance() -> bool {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let addr = ([127, 0, 0, 1], 7373).into();
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let req = "POST /window/show HTTP/1.1\r\nHost: 127.0.0.1:7373\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut resp = String::new();
    let _ = stream.read_to_string(&mut resp);
    resp.contains("\"shown\":true")
}

fn main() -> eframe::Result<()> {
    if activate_running_instance() {
        return Ok(());
    }

    // 1. Initialize background Tokio runtime for network engine and daemon API
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime");

    let _enter = rt.enter();

    updater::cleanup_bak();
    // Updates wait for the user to click "Restart" instead of closing the app mid-download.
    updater::require_user_confirmation();

    // Load or initialize manager
    let manager = DownloadManager::load_or_new();
    let manager = Arc::new(RwLock::new(manager));

    // A new download from the browser, or a second launch, brings the window back from the tray.
    dm_core::set_job_added_hook(|_| tray::show_window());
    dm_core::set_show_window_hook(tray::show_window);

    // Spawn tool checks and schedulers in background without blocking GUI window launch
    rt.spawn(async {
        let _ = youtube::verify_tools_at_startup().await;
        youtube::spawn_ytdlp_update_scheduler(youtube::YTDLP_UPDATE_INTERVAL);
        updater::spawn_auto_updater(std::time::Duration::from_secs(24 * 3600));
    });

    // Spawn REST + WebSocket API daemon on 127.0.0.1:7373 in background (for browser extensions)
    let api_mgr = Arc::clone(&manager);
    rt.spawn(async move {
        if let Err(e) = api::serve(api_mgr).await {
            eprintln!("API daemon server error: {e}");
        }
    });

    // 2. Configure native eframe / egui window
    let mut viewport = eframe::egui::ViewportBuilder::default();
    if let Some((width, height, rgba)) = gui_app::app_icon_rgba() {
        viewport = viewport.with_icon(eframe::egui::IconData { rgba, width, height });
    }
    let native_options = eframe::NativeOptions {
        viewport: viewport
            .with_title("Download Manager Pro")
            .with_inner_size(Vec2::new(1100.0, 700.0))
            .with_min_inner_size(Vec2::new(520.0, 420.0))
            .with_active(true)
            .with_drag_and_drop(true),
        ..Default::default()
    };

    let rt_handle = rt.handle().clone();
    eframe::run_native(
        "Download Manager Pro",
        native_options,
        Box::new(move |cc| {
            tray::set_window(cc);
            let tray = tray::install(gui_app::app_icon_rgba()).map(|t| Box::new(t) as Box<dyn std::any::Any>);
            Ok(Box::new(NativeApp::new(manager, rt_handle).with_tray(tray)))
        }),
    )
}
