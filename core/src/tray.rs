//! System tray: closing the window hides it instead of quitting, so the download
//! engine and the local API (used by the browser extension) keep running.
//!
//! The window is shown/hidden through Win32 directly because eframe stops running
//! frames while its window is hidden, so egui viewport commands would never apply.

use std::sync::atomic::{AtomicIsize, Ordering};

static HWND: AtomicIsize = AtomicIsize::new(0);

/// Remember the native window so the tray (and the API) can bring it back.
pub fn set_window(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(handle) = cc.window_handle() {
        if let RawWindowHandle::Win32(h) = handle.as_raw() {
            HWND.store(h.hwnd.get(), Ordering::SeqCst);
        }
    }
}

#[cfg(windows)]
pub fn show_window() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd);
    }
}

/// eframe always shows the window after its first frame, so a hidden start makes the window
/// fully transparent until `finish_hidden_start` hides it; nothing flashes on screen.
#[cfg(windows)]
pub fn begin_hidden_start() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, GWL_EXSTYLE, LWA_ALPHA, WS_EX_LAYERED,
    };
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED as isize);
        SetLayeredWindowAttributes(hwnd, 0, 0, LWA_ALPHA);
    }
}

#[cfg(windows)]
pub fn finish_hidden_start() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_LAYERED};
    hide_window();
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex & !(WS_EX_LAYERED as isize));
    }
}

#[cfg(windows)]
pub fn hide_window() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd != 0 {
        unsafe { ShowWindow(hwnd as windows_sys::Win32::Foundation::HWND, SW_HIDE) };
    }
}

/// GUID the tray icon is registered with, so notifications can target it. Windows ties a
/// tray GUID to the executable's path, so it is derived from that path.
#[cfg(windows)]
static TRAY_GUID: std::sync::OnceLock<u128> = std::sync::OnceLock::new();

#[cfg(windows)]
fn guid_for_exe() -> u128 {
    use sha2::{Digest, Sha256};
    let exe = std::env::current_exe().map(|p| p.display().to_string().to_lowercase()).unwrap_or_default();
    let digest = Sha256::digest(format!("dmpro-tray:{exe}").as_bytes());
    let mut b: [u8; 16] = digest[..16].try_into().unwrap();
    b[6] = (b[6] & 0x0F) | 0x40; // version 4
    b[8] = (b[8] & 0x3F) | 0x80; // RFC 4122 variant
    u128::from_be_bytes(b)
}

#[cfg(windows)]
thread_local! {
    /// The "Check for updates" / "Install update" menu item (UI thread only).
    static UPDATE_ITEM: std::cell::RefCell<Option<tray_icon::menu::MenuItem>> = const { std::cell::RefCell::new(None) };
}

#[cfg(windows)]
static RUNTIME: std::sync::OnceLock<tokio::runtime::Handle> = std::sync::OnceLock::new();

/// Show the update item's state; called when the pointer reaches the icon, before any click.
#[cfg(windows)]
fn refresh_update_item() {
    let status = dm_core::updater::get_status();
    let (text, enabled) = match status.state.as_str() {
        "ready_to_restart" => (format!("Install update {} and restart", status.version.as_deref().unwrap_or("")), true),
        "checking" => ("Checking for updates…".to_string(), false),
        "downloading" => ("Downloading update…".to_string(), false),
        "installing" => ("Installing update…".to_string(), false),
        _ => ("Check for updates".to_string(), true),
    };
    UPDATE_ITEM.with(|item| {
        if let Some(item) = item.borrow().as_ref() {
            item.set_text(text);
            item.set_enabled(enabled);
        }
    });
}

#[cfg(windows)]
fn on_update_clicked() {
    if dm_core::updater::get_status().state == "ready_to_restart" {
        match dm_core::updater::launch_staged_installer() {
            // The installer upgrades in place and starts the new version.
            Ok(()) => exit_now(),
            Err(e) => notify("Update failed", &format!("{e:#}")),
        }
        return;
    }
    let Some(rt) = RUNTIME.get() else { return };
    rt.spawn(async {
        match dm_core::updater::check_and_apply_update().await {
            // A staged update is announced by spawn_update_notifier.
            Ok(true) => {}
            Ok(false) => notify("Download Manager Pro", "You have the latest version."),
            Err(e) => notify("Update check failed", &format!("{e:#}")),
        }
    });
}

/// Remove the icon registered with `guid`: a leftover from a run that ended with
/// `process::exit` (or crashed) would make registering the same GUID fail.
#[cfg(windows)]
fn delete_icon(guid: u128) {
    use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIF_GUID, NIM_DELETE, NOTIFYICONDATAW};
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.uFlags = NIF_GUID;
    nid.guidItem = windows_sys::core::GUID::from_u128(guid);
    unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
}

/// Exit the process without leaving a dead icon in the tray.
#[cfg(windows)]
fn exit_now() -> ! {
    if let Some(guid) = TRAY_GUID.get() {
        delete_icon(*guid);
    }
    std::process::exit(0)
}

/// Show a Windows notification from the tray icon (works from any thread).
#[cfg(windows)]
pub fn notify(title: &str, body: &str) {
    use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIF_GUID, NIF_INFO, NIIF_INFO, NIM_MODIFY, NOTIFYICONDATAW};
    let Some(guid) = TRAY_GUID.get() else { return };
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.uFlags = NIF_INFO | NIF_GUID;
    nid.guidItem = windows_sys::core::GUID::from_u128(*guid);
    nid.dwInfoFlags = NIIF_INFO;
    let copy = |dst: &mut [u16], src: &str| {
        let w: Vec<u16> = src.encode_utf16().take(dst.len() - 1).collect();
        dst[..w.len()].copy_from_slice(&w);
    };
    copy(&mut nid.szInfoTitle, title);
    copy(&mut nid.szInfo, body);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

/// Notify once per version when the background updater has an update ready, since the
/// window (and its "Update - Restart" button) is usually hidden in the tray.
#[cfg(windows)]
pub fn spawn_update_notifier(rt: &tokio::runtime::Handle) {
    rt.spawn(async {
        let mut announced: Option<String> = None;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            let status = dm_core::updater::get_status();
            if status.state == "ready_to_restart" && status.version != announced {
                let v = status.version.clone().unwrap_or_default();
                notify(
                    "Update ready",
                    &format!("Download Manager Pro {v} is ready. Right-click the tray icon and choose Install update, or use Update - Restart in the window."),
                );
                announced = status.version;
            }
        }
    });
}

/// Create the tray icon. Must run on the UI thread, whose message loop drives it.
/// Returns `None` if the tray is unavailable; the caller then lets the window close normally.
#[cfg(windows)]
pub fn install(icon: Option<(u32, u32, Vec<u8>)>, rt: tokio::runtime::Handle) -> Option<tray_icon::TrayIcon> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let _ = RUNTIME.set(rt);
    let open = MenuItem::new("Open Download Manager Pro", true, None);
    let update = MenuItem::new("Check for updates", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[&open, &update, &PredefinedMenuItem::separator(), &quit]).ok()?;

    let (open_id, update_id, quit_id) = (open.id().clone(), update.id().clone(), quit.id().clone());
    UPDATE_ITEM.with(|item| *item.borrow_mut() = Some(update));
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == open_id {
            show_window();
        } else if e.id == update_id {
            on_update_clicked();
        } else if e.id == quit_id {
            exit_now();
        }
    }));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| match e {
        TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
        | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => show_window(),
        TrayIconEvent::Enter { .. } | TrayIconEvent::Move { .. } => refresh_update_item(),
        _ => {}
    }));

    let build = |guid: Option<u128>| {
        let mut builder = TrayIconBuilder::new()
            .with_tooltip("Download Manager Pro")
            .with_menu(Box::new(menu.clone()))
            .with_menu_on_left_click(false);
        if let Some((w, h, rgba)) = icon.clone() {
            if let Ok(icon) = tray_icon::Icon::from_rgba(rgba, w, h) {
                builder = builder.with_icon(icon);
            }
        }
        if let Some(g) = guid {
            builder = builder.with_guid(g);
        }
        builder.build()
    };
    let guid = guid_for_exe();
    delete_icon(guid);
    match build(Some(guid)) {
        Ok(tray) => {
            let _ = TRAY_GUID.set(guid);
            Some(tray)
        }
        // Without the GUID the tray still works; only notifications are lost.
        Err(e) => match build(None) {
            Ok(tray) => {
                eprintln!("Tray icon without notifications ({e})");
                Some(tray)
            }
            Err(e) => {
                eprintln!("Could not create tray icon: {e}");
                None
            }
        },
    }
}

#[cfg(not(windows))]
pub fn show_window() {}

#[cfg(not(windows))]
pub fn hide_window() {}

#[cfg(not(windows))]
pub fn begin_hidden_start() {}

#[cfg(not(windows))]
pub fn finish_hidden_start() {}

#[cfg(not(windows))]
pub fn install(_icon: Option<(u32, u32, Vec<u8>)>, _rt: tokio::runtime::Handle) -> Option<()> {
    None
}

#[cfg(not(windows))]
pub fn notify(_title: &str, _body: &str) {}

#[cfg(not(windows))]
pub fn spawn_update_notifier(_rt: &tokio::runtime::Handle) {}
