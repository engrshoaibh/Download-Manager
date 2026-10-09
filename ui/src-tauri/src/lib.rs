// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
fn open_file(path: &str) -> Result<(), String> {
    let p = path.trim().replace('/', "\\");
    if p.is_empty() {
        return Err("Empty path".into());
    }
    #[cfg(target_os = "windows")]
    {
        // `explorer <file>` opens a folder window; use `cmd start` with empty title.
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &p])
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = p;
        Err("open_file is only implemented on Windows".into())
    }
}

#[tauri::command]
fn open_folder(path: &str) -> Result<(), String> {
    let p = path.trim().replace('/', "\\");
    if p.is_empty() {
        return Err("Empty path".into());
    }
    #[cfg(target_os = "windows")]
    {
        let path_obj = std::path::Path::new(&p);
        let is_file = path_obj.is_file();
        if is_file {
            // Single argv so spaces in the path work: /select,"C:\path\file.ext"
            let arg = format!("/select,\"{p}\"");
            std::process::Command::new("explorer")
                .raw_arg(arg)
                .spawn()
                .map_err(|e| e.to_string())?;
        } else {
            let dir = if path_obj.is_dir() {
                path_obj.to_path_buf()
            } else {
                path_obj
                    .parent()
                    .filter(|d| !d.as_os_str().is_empty())
                    .unwrap_or(path_obj)
                    .to_path_buf()
            };
            std::process::Command::new("explorer")
                .arg(&dir)
                .spawn()
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = p;
        Err("open_folder is only implemented on Windows".into())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            use tauri::Manager;
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // Spawn the DM daemon in a background tokio runtime
            std::thread::spawn(|| {
                let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
                rt.block_on(async {
                    if let Err(e) = dm_core::run_daemon().await {
                        eprintln!("Daemon error: {}", e);
                    }
                });
            });

            // Set up System Tray
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::TrayIconBuilder;
            use tauri::Manager;

            let show_i = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            let tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        std::process::exit(0);
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                // Hide the window instead of closing it to keep the app in the tray
                window.hide().unwrap();
                api.prevent_close();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![greet, setup_native_messaging, open_file, open_folder])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[tauri::command]
fn setup_native_messaging(app: tauri::AppHandle) -> Result<String, String> {
    use std::path::PathBuf;

    // We expect dm-native-host.exe to be in the same directory as the executable,
    // or as a sidecar. For simplicity, we assume it's shipped alongside the main executable in release/
    let exe_path = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_dir = exe_path.parent().ok_or("No parent dir for exe")?;

    let native_host_path = exe_dir.join("dm-native-host.exe");
    if !native_host_path.exists() {
        return Err("dm-native-host.exe not found next to the application.".to_string());
    }

    let manifest_path = exe_dir.join("com.highperf.dm.json");

    let manifest_content = format!(
        r#"{{
  "name": "com.highperf.dm",
  "description": "Download Manager Native Host",
  "path": "{}",
  "type": "stdio",
  "allowed_origins": [
    "chrome-extension://*/*"
  ]
}}"#,
        native_host_path.to_string_lossy().replace("\\", "\\\\")
    );

    std::fs::write(&manifest_path, manifest_content).map_err(|e| e.to_string())?;

    // Write to registry
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::*;
        use winreg::RegKey;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let path = "Software\\Google\\Chrome\\NativeMessagingHosts\\com.highperf.dm";
        let (key, _) = hkcu.create_subkey(path).map_err(|e| e.to_string())?;
        key.set_value("", &manifest_path.to_string_lossy().to_string())
            .map_err(|e| e.to_string())?;

        // Also do Edge
        let path_edge = "Software\\Microsoft\\Edge\\NativeMessagingHosts\\com.highperf.dm";
        if let Ok((key_edge, _)) = hkcu.create_subkey(path_edge) {
            let _ = key_edge.set_value("", &manifest_path.to_string_lossy().to_string());
        }
    }

    Ok("Native messaging host configured successfully.".to_string())
}
