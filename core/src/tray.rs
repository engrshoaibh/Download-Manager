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

/// Create the tray icon. Must run on the UI thread, whose message loop drives it.
/// Returns `None` if the tray is unavailable; the caller then lets the window close normally.
#[cfg(windows)]
pub fn install(icon: Option<(u32, u32, Vec<u8>)>) -> Option<tray_icon::TrayIcon> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let open = MenuItem::new("Open Download Manager Pro", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[&open, &PredefinedMenuItem::separator(), &quit]).ok()?;

    let (open_id, quit_id) = (open.id().clone(), quit.id().clone());
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == open_id {
            show_window();
        } else if e.id == quit_id {
            std::process::exit(0);
        }
    }));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| match e {
        TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
        | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => show_window(),
        _ => {}
    }));

    let mut builder = TrayIconBuilder::new()
        .with_tooltip("Download Manager Pro")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false);
    if let Some((w, h, rgba)) = icon {
        if let Ok(icon) = tray_icon::Icon::from_rgba(rgba, w, h) {
            builder = builder.with_icon(icon);
        }
    }
    match builder.build() {
        Ok(tray) => Some(tray),
        Err(e) => {
            eprintln!("Could not create tray icon: {e}");
            None
        }
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
pub fn install(_icon: Option<(u32, u32, Vec<u8>)>) -> Option<()> {
    None
}
