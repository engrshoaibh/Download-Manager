use std::sync::Arc;
use tokio::sync::RwLock;

use eframe::egui::Vec2;
use dm_core::{api, updater, youtube, DownloadManager};

mod gui_app;
use gui_app::NativeApp;

fn main() -> eframe::Result<()> {
    // 1. Initialize background Tokio runtime for network engine and daemon API
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime");

    let _enter = rt.enter();

    updater::cleanup_bak();

    // Load or initialize manager
    let manager = DownloadManager::load_or_new();
    let manager = Arc::new(RwLock::new(manager));

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
    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Download Manager Pro")
            .with_inner_size(Vec2::new(1050.0, 680.0))
            .with_min_inner_size(Vec2::new(800.0, 480.0))
            .with_active(true)
            .with_drag_and_drop(true),
        ..Default::default()
    };

    let rt_handle = rt.handle().clone();
    eframe::run_native(
        "Download Manager Pro",
        native_options,
        Box::new(move |_cc| Ok(Box::new(NativeApp::new(manager, rt_handle)))),
    )
}
