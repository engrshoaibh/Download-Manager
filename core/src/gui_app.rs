use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use eframe::egui::{
    self, Color32, Frame, Margin, Pos2, Rect, RichText, Rounding, Stroke, Vec2,
};
use dm_core::engine::{DownloadJob, DownloadStatus, JobId};
use dm_core::DownloadManager;

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum ThemeMode {
    Light,
    Dark,
}

pub struct ThemeColors {
    pub bg_app: Color32,          // Surface 1: Base background (#0d0d0d)
    pub bg_chrome: Color32,       // Surface 2: Panel/sidebar/toolbar/statusbar (#161616)
    pub bg_raised: Color32,       // Surface 3b: Toolbar buttons/inputs/toolcards (#1a1a1a)
    pub bg_input: Color32,
    pub bg_row_hover: Color32,
    pub bg_row_selected: Color32, // 12% opacity orange tint
    pub border: Color32,          // ~8% white opacity (rgba(255, 255, 255, 20))
    pub border_strong: Color32,
    pub text_main: Color32,       // White/high-contrast (#f8fafc)
    pub text_muted: Color32,      // Muted body text (#b5bec9)
    pub text_dim: Color32,        // Header label gray >= #9A9A9A
    pub text_idle: Color32,       // Dimmed gray for empty/idle values ("0 B", "—") (#5e6673)
    pub accent: Color32,          // Orange (#F7941E) for primary actions only
    pub accent_hover: Color32,
    pub in_progress: Color32,     // Teal/Cyan (#06b6d4 / #22d3ee) for downloading states
    pub success: Color32,         // Saturated bright green (#2ECC71) for complete state
    pub danger: Color32,          // Crimson (#f87171)
    pub warning: Color32,         // Amber (#fbbf24)
    pub probing_blue: Color32,    // Connecting blue (#38bdf8)
    pub progress_track: Color32,  // Visible track (#2a2a2a) so 0% reads as a bar
}

impl ThemeColors {
    pub fn light() -> Self {
        Self {
            bg_app: Color32::from_rgb(244, 245, 247),       // #f4f5f7
            bg_chrome: Color32::from_rgb(255, 255, 255),    // #ffffff
            bg_raised: Color32::from_rgb(243, 244, 246),    // #f3f4f6
            bg_input: Color32::from_rgb(255, 255, 255),     // #ffffff
            bg_row_hover: Color32::from_rgba_unmultiplied(0, 0, 0, 10),
            bg_row_selected: Color32::from_rgba_unmultiplied(79, 70, 229, 26),
            border: Color32::from_rgba_unmultiplied(0, 0, 0, 16),
            border_strong: Color32::from_rgba_unmultiplied(0, 0, 0, 30),
            text_main: Color32::from_rgb(17, 24, 39),       // #111827
            text_muted: Color32::from_rgb(75, 85, 99),      // #4b5563
            text_dim: Color32::from_rgb(107, 114, 128),     // #6b7280
            text_idle: Color32::from_rgb(156, 163, 175),    // #9ca3af
            accent: Color32::from_rgb(79, 70, 229),
            accent_hover: Color32::from_rgb(99, 102, 241),
            in_progress: Color32::from_rgb(99, 102, 241),    // #0891b2
            success: Color32::from_rgb(46, 204, 113),       // #2ECC71
            danger: Color32::from_rgb(239, 68, 68),         // #ef4444
            warning: Color32::from_rgb(245, 158, 11),       // #f59e0b
            probing_blue: Color32::from_rgb(14, 165, 233),  // #0ea5e9
            progress_track: Color32::from_rgb(229, 231, 235), // #e5e7eb
        }
    }

    pub fn dark() -> Self {
        Self {
            bg_app: Color32::from_rgb(11, 14, 23),
            bg_chrome: Color32::from_rgb(14, 18, 29),
            bg_raised: Color32::from_rgb(22, 28, 43),
            bg_input: Color32::from_rgb(16, 20, 32),
            bg_row_hover: Color32::from_rgba_unmultiplied(255, 255, 255, 9),
            bg_row_selected: Color32::from_rgba_unmultiplied(110, 108, 245, 34),
            border: Color32::from_rgba_unmultiplied(148, 163, 210, 16),
            border_strong: Color32::from_rgba_unmultiplied(148, 163, 210, 30),
            text_main: Color32::from_rgb(240, 243, 250),
            text_muted: Color32::from_rgb(158, 168, 192),
            text_dim: Color32::from_rgb(112, 123, 152),
            text_idle: Color32::from_rgb(82, 92, 118),
            accent: Color32::from_rgb(112, 110, 246),
            accent_hover: Color32::from_rgb(134, 132, 250),
            in_progress: Color32::from_rgb(129, 128, 244),
            success: Color32::from_rgb(52, 211, 153),
            danger: Color32::from_rgb(248, 113, 113),
            warning: Color32::from_rgb(251, 191, 36),
            probing_blue: Color32::from_rgb(56, 189, 248),
            progress_track: Color32::from_rgb(34, 41, 60),
        }
    }
}

const KIND_ARCHIVE: Color32 = Color32::from_rgb(245, 158, 11);
const KIND_DOC: Color32 = Color32::from_rgb(96, 165, 250);
const KIND_AUDIO: Color32 = Color32::from_rgb(192, 132, 252);
const KIND_VIDEO: Color32 = Color32::from_rgb(45, 212, 191);
const KIND_PROGRAM: Color32 = Color32::from_rgb(52, 211, 153);
const KIND_OTHER: Color32 = Color32::from_rgb(148, 163, 184);

const ARCHIVE_EXT: &[&str] = &["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "iso"];
const DOC_EXT: &[&str] = &["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "csv", "md"];
const AUDIO_EXT: &[&str] = &["mp3", "flac", "wav", "aac", "ogg", "m4a", "opus"];
const VIDEO_EXT: &[&str] = &["mp4", "mkv", "avi", "mov", "webm", "flv", "m4v"];
const PROGRAM_EXT: &[&str] = &["exe", "msi", "dmg", "apk", "deb", "rpm", "jar"];

/// Glyph + accent color for a file, chosen by extension.
fn file_kind(name: &str) -> (Icon, Color32) {
    let ext = get_extension(name);
    let e = ext.as_str();
    if VIDEO_EXT.contains(&e) {
        (Icon::Play, KIND_VIDEO)
    } else if AUDIO_EXT.contains(&e) {
        (Icon::Music, KIND_AUDIO)
    } else if ARCHIVE_EXT.contains(&e) {
        (Icon::Archive, KIND_ARCHIVE)
    } else if DOC_EXT.contains(&e) {
        (Icon::Doc, KIND_DOC)
    } else if PROGRAM_EXT.contains(&e) {
        (Icon::Apps, KIND_PROGRAM)
    } else {
        (Icon::File, KIND_OTHER)
    }
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum CategoryId {
    All,
    Downloading,
    Completed,
    Waiting,
    Video,
    Archives,
    Documents,
    Music,
    Programs,
}

impl CategoryId {
    pub const ALL: [CategoryId; 9] = [
        CategoryId::All,
        CategoryId::Downloading,
        CategoryId::Completed,
        CategoryId::Waiting,
        CategoryId::Video,
        CategoryId::Archives,
        CategoryId::Documents,
        CategoryId::Music,
        CategoryId::Programs,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            CategoryId::All => "All downloads",
            CategoryId::Downloading => "Downloading",
            CategoryId::Completed => "Completed",
            CategoryId::Waiting => "Queued & paused",
            CategoryId::Video => "Video",
            CategoryId::Archives => "Archives",
            CategoryId::Documents => "Documents",
            CategoryId::Music => "Music",
            CategoryId::Programs => "Programs",
        }
    }

    pub fn icon(&self) -> Icon {
        match self {
            CategoryId::All => Icon::List,
            CategoryId::Downloading => Icon::Download,
            CategoryId::Completed => Icon::Check,
            CategoryId::Waiting => Icon::Clock,
            CategoryId::Video => Icon::Play,
            CategoryId::Archives => Icon::Archive,
            CategoryId::Documents => Icon::Doc,
            CategoryId::Music => Icon::Music,
            CategoryId::Programs => Icon::Apps,
        }
    }

    pub fn section(&self) -> &'static str {
        match self {
            CategoryId::All | CategoryId::Downloading | CategoryId::Completed | CategoryId::Waiting => "LIBRARY",
            _ => "CATEGORIES",
        }
    }
}

pub struct NativeApp {
    manager: Arc<RwLock<DownloadManager>>,
    rt: tokio::runtime::Handle,
    jobs: Vec<DownloadJob>,
    selected_job_id: Option<JobId>,

    // Theme state (Default: Light Mode)
    theme: ThemeMode,

    // Search and category
    category: CategoryId,
    search_query: String,
    sidebar_open: bool,

    // Add Modal State
    show_add_modal: bool,
    add_url: String,
    add_segments: usize,
    add_quality: String,
    add_filename: String,

    // Settings Modal State
    show_settings_modal: bool,
    default_segments: usize,
    default_save_dir: String,

    // About Modal State
    show_about_modal: bool,

    // Properties / Progress Dialog State
    show_properties_modal: bool,
    properties_job_id: Option<JobId>,

    style_applied: bool,
    focus_add_url: bool,
    logo: Option<egui::TextureHandle>,

    // YouTube account cookies (Options)
    yt_cookies_file: String,
    yt_cookies_browser: String,

    // Keeps the tray icon alive; while present, closing the window only hides it.
    tray: Option<Box<dyn std::any::Any>>,
    quitting: bool,
    /// `--background`: hide the window right after eframe first shows it.
    hide_on_start: bool,
}

impl NativeApp {
    pub fn new(manager: Arc<RwLock<DownloadManager>>, rt: tokio::runtime::Handle) -> Self {
        let default_save = dirs::download_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .to_string_lossy()
            .to_string();

        let (yt_cookies_file, yt_cookies_browser) = manager
            .try_read()
            .map(|m| {
                (
                    m.settings.youtube_cookies_file.clone().unwrap_or_default(),
                    m.settings.youtube_cookies_browser.clone().unwrap_or_default(),
                )
            })
            .unwrap_or_default();

        Self {
            manager,
            rt,
            jobs: Vec::new(),
            selected_job_id: None,
            theme: ThemeMode::Dark, // Default: Polished, high-contrast Dark Mode
            category: CategoryId::All,
            search_query: String::new(),
            sidebar_open: true,
            show_add_modal: false,
            add_url: String::new(),
            add_segments: 16,
            add_quality: "best".to_string(),
            add_filename: String::new(),
            show_settings_modal: false,
            default_segments: 16,
            default_save_dir: default_save,
            show_about_modal: false,
            show_properties_modal: false,
            properties_job_id: None,
            style_applied: false,
            focus_add_url: false,
            logo: None,
            yt_cookies_file,
            yt_cookies_browser,
            tray: None,
            quitting: false,
            hide_on_start: false,
        }
    }

    pub fn with_tray(mut self, tray: Option<Box<dyn std::any::Any>>) -> Self {
        self.tray = tray;
        self
    }

    /// Start hidden in the tray. Without a tray the window stays visible so the app is reachable.
    pub fn start_hidden(mut self, hidden: bool) -> Self {
        self.hide_on_start = hidden && self.tray.is_some();
        if self.hide_on_start {
            crate::tray::begin_hidden_start();
        }
        self
    }

    fn refresh_jobs(&mut self) {
        if let Ok(guard) = self.manager.try_read() {
            let mut list: Vec<DownloadJob> = guard.jobs.values().cloned().collect();
            list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            self.jobs = list;
        }
    }

    fn add_download(&mut self) {
        let url = self.add_url.trim().to_string();
        if url.is_empty() {
            return;
        }
        let segments = self.add_segments.clamp(1, 32);
        let quality = if self.add_quality.is_empty() { None } else { Some(self.add_quality.clone()) };
        let filename_override = if self.add_filename.trim().is_empty() { None } else { Some(self.add_filename.trim().to_string()) };
        let save_dir = if self.default_save_dir.trim().is_empty() {
            dirs::download_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .to_string_lossy()
                .to_string()
        } else {
            self.default_save_dir.clone()
        };

        let mgr = Arc::clone(&self.manager);
        self.rt.spawn(async move {
            let job_res = {
                let mut guard = mgr.write().await;
                guard.add_download(
                    url,
                    save_dir,
                    Some(segments),
                    None,
                    None,
                    None,
                    filename_override,
                    None,
                    None,
                    quality,
                    None,
                )
            };

            if let Ok(job) = job_res {
                let client = {
                    let guard = mgr.read().await;
                    Arc::clone(&guard.http_client)
                };
                let user_agent = job.user_agent.clone();
                let cookies = job.cookies.clone();
                let referer = job.referer.clone();
                tokio::spawn(async move {
                    if let Err(e) = DownloadManager::run_download(
                        mgr.clone(),
                        job.clone(),
                        client,
                        user_agent,
                        cookies,
                        referer,
                    ).await {
                        tracing::error!("Download {} failed: {:#}", job.id, e);
                        let mut m = mgr.write().await;
                        m.mark_failed(&job.id, format!("{e:#}"));
                    }
                });
            }
        });

        self.add_url.clear();
        self.add_filename.clear();
        self.show_add_modal = false;
    }

    fn pause_selected(&mut self) {
        if let Some(id) = self.selected_job_id {
            let mgr = Arc::clone(&self.manager);
            self.rt.spawn(async move {
                let mut guard = mgr.write().await;
                guard.pause(&id);
            });
        }
    }

    fn resume_selected(&mut self) {
        if let Some(id) = self.selected_job_id {
            let mgr = Arc::clone(&self.manager);
            self.rt.spawn(async move {
                for _ in 0..50 {
                    if !mgr.read().await.previous_run_active(&id) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }

                let job_and_client = {
                    let mut guard = mgr.write().await;
                    guard.begin_resume(&id).ok().map(|job| (job, Arc::clone(&guard.http_client)))
                };

                if let Some((job, client)) = job_and_client {
                    let user_agent = job.user_agent.clone();
                    let cookies = job.cookies.clone();
                    let referer = job.referer.clone();
                    tokio::spawn(async move {
                        if let Err(e) = DownloadManager::run_download(
                            mgr.clone(),
                            job.clone(),
                            client,
                            user_agent,
                            cookies,
                            referer,
                        ).await {
                            tracing::error!("Resume {} failed: {:#}", job.id, e);
                            let mut m = mgr.write().await;
                            m.mark_failed(&job.id, format!("{e:#}"));
                        }
                    });
                }
            });
        }
    }

    fn delete_selected(&mut self, delete_file: bool) {
        if let Some(id) = self.selected_job_id {
            let mgr = Arc::clone(&self.manager);
            self.rt.spawn(async move {
                let mut guard = mgr.write().await;
                guard.cancel(&id, delete_file);
            });
            self.selected_job_id = None;
        }
    }

    fn open_file(&self, path: &str) {
        let _ = open::that(path);
    }

    fn open_folder(&self, path: &str) {
        let p = std::path::Path::new(path);
        let dir = if p.is_file() {
            p.parent().unwrap_or(p)
        } else {
            p
        };
        let _ = open::that(dir);
    }

    fn matches_category(&self, job: &DownloadJob, cat: CategoryId) -> bool {
        let ext = get_extension(&job.filename);
        let e = ext.as_str();
        match cat {
            CategoryId::All => true,
            CategoryId::Downloading => matches!(job.status, DownloadStatus::Downloading | DownloadStatus::Probing),
            CategoryId::Completed => job.status == DownloadStatus::Completed,
            CategoryId::Waiting => matches!(job.status, DownloadStatus::Queued | DownloadStatus::Paused | DownloadStatus::Failed),
            CategoryId::Video => VIDEO_EXT.contains(&e),
            CategoryId::Archives => ARCHIVE_EXT.contains(&e),
            CategoryId::Documents => DOC_EXT.contains(&e),
            CategoryId::Music => AUDIO_EXT.contains(&e),
            CategoryId::Programs => PROGRAM_EXT.contains(&e),
        }
    }
}

fn get_extension(name: &str) -> String {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

fn format_speed(bps: f64) -> String {
    if bps <= 0.0 {
        "—".to_string()
    } else {
        format!("{}/s", format_bytes(bps as u64))
    }
}

const APP_ICON_PNG: &[u8] = include_bytes!("../assets/app-icon.png");

/// Decode the bundled app icon to RGBA pixels (width, height, bytes).
pub fn app_icon_rgba() -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory_with_format(APP_ICON_PNG, image::ImageFormat::Png).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some((w, h, img.into_raw()))
}

struct TableCols {
    name: Rect,
    progress: Rect,
    speed: Option<Rect>,
    eta: Option<Rect>,
}

/// Lay out table columns right-to-left so the name column absorbs spare width;
/// speed and ETA drop out as the window narrows.
fn table_columns(area: Rect, compact: bool, narrow: bool) -> TableCols {
    let gap = 16.0;
    let eta_w = if compact { 0.0 } else { 96.0 };
    let speed_w = if narrow { 0.0 } else { 110.0 };
    let prog_w = if narrow { 100.0 } else if compact { 150.0 } else { 200.0 };
    let mut right = area.right();
    let mut take = |w: f32| {
        let r = Rect::from_min_max(Pos2::new(right - w, area.top()), Pos2::new(right, area.bottom()));
        right -= w + gap;
        r
    };
    let eta = (eta_w > 0.0).then(|| take(eta_w));
    let speed = (speed_w > 0.0).then(|| take(speed_w));
    let progress = take(prog_w);
    let name = Rect::from_min_max(area.min, Pos2::new((right + gap).max(area.left() + 80.0), area.bottom()));
    TableCols { name, progress, speed, eta }
}

fn format_eta_human(eta_opt: Option<f64>) -> String {
    match eta_opt {
        Some(eta) if eta.is_finite() && eta > 0.0 && eta < 86400.0 * 30.0 => {
            let t = eta as u64;
            let (h, m, s) = (t / 3600, (t % 3600) / 60, t % 60);
            if h > 0 { format!("{}h {}m", h, m) } else if m > 0 { format!("{}m {}s", m, s) } else { format!("{}s", s) }
        }
        _ => "—".to_string(),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Icon {
    Plus,
    Download,
    Check,
    Clock,
    List,
    Play,
    Pause,
    Archive,
    Doc,
    Music,
    Apps,
    File,
    Folder,
    Info,
    Trash,
    Sun,
    Moon,
    Search,
    Inbox,
}

/// Vector icon drawn with the painter (crisp at any scale, theme-colored).
fn paint_icon(painter: &egui::Painter, center: Pos2, size: f32, icon: Icon, color: Color32) {
    use egui::Shape;
    let u = size * 0.5;
    let st = Stroke::new((size / 10.0).clamp(1.3, 2.0), color);
    let p = |x: f32, y: f32| Pos2::new(center.x + x * u, center.y + y * u);
    let line = |pts: &[(f32, f32)]| Shape::line(pts.iter().map(|&(x, y)| p(x, y)).collect(), st);
    let closed = |pts: &[(f32, f32)]| Shape::closed_line(pts.iter().map(|&(x, y)| p(x, y)).collect(), st);
    match icon {
        Icon::Plus => {
            painter.add(line(&[(0.0, -0.7), (0.0, 0.7)]));
            painter.add(line(&[(-0.7, 0.0), (0.7, 0.0)]));
        }
        Icon::Download => {
            painter.add(line(&[(0.0, -0.85), (0.0, 0.3)]));
            painter.add(line(&[(-0.45, -0.15), (0.0, 0.3), (0.45, -0.15)]));
            painter.add(line(&[(-0.85, 0.35), (-0.85, 0.85), (0.85, 0.85), (0.85, 0.35)]));
        }
        Icon::Check => {
            painter.circle_stroke(center, 0.85 * u, st);
            painter.add(line(&[(-0.4, 0.02), (-0.08, 0.34), (0.45, -0.28)]));
        }
        Icon::Clock => {
            painter.circle_stroke(center, 0.85 * u, st);
            painter.add(line(&[(0.0, -0.5), (0.0, 0.0), (0.35, 0.22)]));
        }
        Icon::List => {
            for y in [-0.55_f32, 0.0, 0.55] {
                painter.circle_filled(p(-0.7, y), st.width * 0.7, color);
                painter.add(line(&[(-0.3, y), (0.85, y)]));
            }
        }
        Icon::Play => {
            painter.add(Shape::convex_polygon(vec![p(-0.35, -0.6), p(-0.35, 0.6), p(0.65, 0.0)], color, Stroke::NONE));
        }
        Icon::Pause => {
            painter.rect_filled(Rect::from_min_max(p(-0.55, -0.6), p(-0.15, 0.6)), Rounding::same(1.0), color);
            painter.rect_filled(Rect::from_min_max(p(0.15, -0.6), p(0.55, 0.6)), Rounding::same(1.0), color);
        }
        Icon::Archive => {
            painter.rect_stroke(Rect::from_min_max(p(-0.85, -0.7), p(0.85, -0.25)), Rounding::same(2.0), st);
            painter.add(line(&[(-0.7, -0.25), (-0.7, 0.75), (0.7, 0.75), (0.7, -0.25)]));
            painter.add(line(&[(-0.25, 0.1), (0.25, 0.1)]));
        }
        Icon::Doc => {
            painter.add(line(&[(0.25, -0.85), (-0.6, -0.85), (-0.6, 0.85), (0.6, 0.85), (0.6, -0.5), (0.25, -0.85), (0.25, -0.5), (0.6, -0.5)]));
            painter.add(line(&[(-0.3, 0.05), (0.3, 0.05)]));
            painter.add(line(&[(-0.3, 0.45), (0.3, 0.45)]));
        }
        Icon::Music => {
            painter.circle_filled(p(-0.4, 0.55), 0.3 * u, color);
            painter.circle_filled(p(0.55, 0.35), 0.3 * u, color);
            painter.add(line(&[(-0.12, 0.55), (-0.12, -0.7), (0.83, -0.9), (0.83, 0.35)]));
        }
        Icon::Apps => {
            for (x, y) in [(-0.8, -0.8), (0.1, -0.8), (-0.8, 0.1), (0.1, 0.1)] {
                painter.rect_stroke(Rect::from_min_max(p(x, y), p(x + 0.7, y + 0.7)), Rounding::same(1.5), st);
            }
        }
        Icon::File => {
            painter.add(closed(&[(0.0, -0.85), (0.8, 0.0), (0.0, 0.85), (-0.8, 0.0)]));
        }
        Icon::Folder => {
            painter.add(closed(&[(-0.85, -0.6), (-0.2, -0.6), (0.05, -0.3), (0.85, -0.3), (0.85, 0.7), (-0.85, 0.7)]));
        }
        Icon::Info => {
            painter.circle_stroke(center, 0.85 * u, st);
            painter.circle_filled(p(0.0, -0.4), st.width * 0.75, color);
            painter.add(line(&[(0.0, -0.08), (0.0, 0.5)]));
        }
        Icon::Trash => {
            painter.add(line(&[(-0.8, -0.5), (0.8, -0.5)]));
            painter.add(line(&[(-0.25, -0.5), (-0.25, -0.8), (0.25, -0.8), (0.25, -0.5)]));
            painter.add(line(&[(-0.6, -0.5), (-0.5, 0.85), (0.5, 0.85), (0.6, -0.5)]));
        }
        Icon::Sun => {
            painter.circle_stroke(center, 0.3 * size, st);
            for i in 0..8 {
                let a = (i as f32 * 45.0).to_radians();
                painter.add(line(&[(0.68 * a.cos(), 0.68 * a.sin()), (0.92 * a.cos(), 0.92 * a.sin())]));
            }
        }
        Icon::Moon => {
            let mut pts: Vec<Pos2> = Vec::new();
            for i in 0..=24 {
                let a = (45.0 + 270.0 * i as f32 / 24.0).to_radians();
                pts.push(p(0.8 * a.cos(), 0.8 * a.sin()));
            }
            for i in 0..=16 {
                let a = (272.0 - 184.0 * i as f32 / 16.0).to_radians();
                pts.push(p(0.55 + 0.57 * a.cos(), 0.57 * a.sin()));
            }
            painter.add(Shape::line(pts, st));
        }
        Icon::Search => {
            painter.circle_stroke(p(-0.15, -0.15), 0.6 * u, st);
            painter.add(line(&[(0.3, 0.3), (0.85, 0.85)]));
        }
        Icon::Inbox => {
            painter.add(line(&[(-0.9, 0.1), (-0.55, -0.7), (0.55, -0.7), (0.9, 0.1), (0.9, 0.8), (-0.9, 0.8), (-0.9, 0.1)]));
            painter.add(line(&[(-0.9, 0.1), (-0.3, 0.1), (-0.2, 0.4), (0.2, 0.4), (0.3, 0.1), (0.9, 0.1)]));
        }
    }
}

/// Button with an optional vector icon and label. Shows a hand cursor when enabled.
fn icon_text_button(
    ui: &mut egui::Ui,
    icon: Icon,
    text: &str,
    fg: Color32,
    fill: Color32,
    hover_fill: Color32,
    stroke: Stroke,
    height: f32,
) -> egui::Response {
    let enabled = ui.is_enabled();
    let fg = if enabled { fg } else { fg.gamma_multiply(0.4) };
    let font = egui::FontId::proportional(12.5);
    let galley = (!text.is_empty()).then(|| ui.painter().layout_no_wrap(text.to_string(), font, fg));
    let icon_s = 14.0;
    let width = match &galley {
        Some(g) => 12.0 + icon_s + 7.0 + g.size().x + 14.0,
        None => height,
    };
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), sense);
    let bg = if enabled && resp.hovered() { hover_fill } else { fill };
    ui.painter().rect(rect, Rounding::same(8.0), bg, stroke);
    match galley {
        Some(g) => {
            paint_icon(ui.painter(), Pos2::new(rect.left() + 12.0 + icon_s * 0.5, rect.center().y), icon_s, icon, fg);
            let pos = Pos2::new(rect.left() + 12.0 + icon_s + 7.0, rect.center().y - g.size().y * 0.5);
            ui.painter().galley(pos, g, fg);
        }
        None => paint_icon(ui.painter(), rect.center(), icon_s + 2.0, icon, fg),
    }
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

fn icon_button(ui: &mut egui::Ui, c: &ThemeColors, icon: Icon, tip: &str) -> egui::Response {
    icon_text_button(ui, icon, "", c.text_muted, c.bg_raised, c.bg_row_hover, Stroke::new(1.0_f32, c.border), 32.0).on_hover_text(tip)
}

fn chip_button(ui: &mut egui::Ui, c: &ThemeColors, icon: Icon, text: &str) -> egui::Response {
    icon_text_button(ui, icon, text, c.text_main, c.bg_raised, c.bg_row_hover, Stroke::new(1.0_f32, c.border), 30.0)
}

fn status_info(status: &DownloadStatus, c: &ThemeColors) -> (&'static str, Color32) {
    match status {
        DownloadStatus::Downloading => ("Downloading", c.in_progress),
        DownloadStatus::Probing => ("Connecting…", c.probing_blue),
        DownloadStatus::Queued => ("Queued", c.warning),
        DownloadStatus::Paused => ("Paused", c.text_dim),
        DownloadStatus::Completed => ("Complete", c.success),
        DownloadStatus::Failed => ("Failed", c.danger),
        DownloadStatus::Cancelled => ("Stopped", c.text_dim),
    }
}

fn job_progress(job: &DownloadJob) -> f32 {
    if job.total_bytes > 0 {
        (job.downloaded_bytes as f32 / job.total_bytes as f32).clamp(0.0, 1.0)
    } else if job.status == DownloadStatus::Completed {
        1.0
    } else {
        0.0
    }
}

/// Slim rounded progress bar with an inline percentage label.
fn progress_bar(ui: &mut egui::Ui, frac: f32, fill: Color32, c: &ThemeColors) {
    let width = ui.available_width().max(40.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 8.0), egui::Sense::hover());
    let bar = rect;
    let painter = ui.painter();
    painter.rect_filled(bar, Rounding::same(3.0), c.progress_track);
    if frac > 0.0 {
        let mut filled = bar;
        filled.set_width((bar.width() * frac).max(6.0));
        painter.rect_filled(filled, Rounding::same(3.0), fill);
    }
}

fn apply_style(ctx: &egui::Context) {
    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 5.0);
        s.spacing.interact_size.y = 26.0;
        s.spacing.slider_width = 200.0;
        s.interaction.selectable_labels = false;
    });
}

impl eframe::App for NativeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.refresh_jobs();
        ctx.request_repaint_after(Duration::from_millis(250));

        // Closing the window sends the app to the tray; downloads and the browser API keep running.
        // eframe makes the window visible after the first frame; hide it on the next one.
        if self.hide_on_start {
            if self.style_applied {
                // Not the first frame any more: eframe has already shown the window.
                self.hide_on_start = false;
                crate::tray::finish_hidden_start();
            } else {
                ctx.request_repaint();
            }
        }

        if self.tray.is_some() && !self.quitting && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            crate::tray::hide_window();
        }

        if self.logo.is_none() {
            if let Some((w, h, px)) = app_icon_rgba() {
                let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
                self.logo = Some(ctx.load_texture("app_logo", img, egui::TextureOptions::LINEAR));
            }
        }

        if !self.style_applied {
            apply_style(ctx);
            self.style_applied = true;
        }

        // ── Keyboard shortcuts ─────────────────────────────────────────
        let any_modal = self.show_add_modal || self.show_settings_modal || self.show_about_modal || self.show_properties_modal;
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::N)) {
            self.show_add_modal = true;
            self.focus_add_url = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_add_modal = false;
            self.show_settings_modal = false;
            self.show_about_modal = false;
            self.show_properties_modal = false;
        }
        if !any_modal && !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
            self.delete_selected(false);
        }

        // ── Active Theme Setup ─────────────────────────────────────────
        let c = match self.theme {
            ThemeMode::Light => ThemeColors::light(),
            ThemeMode::Dark => ThemeColors::dark(),
        };

        let mut visuals = match self.theme {
            ThemeMode::Light => egui::Visuals::light(),
            ThemeMode::Dark => egui::Visuals::dark(),
        };

        visuals.panel_fill = c.bg_app;
        visuals.window_fill = c.bg_chrome;
        visuals.faint_bg_color = c.bg_raised;
        visuals.extreme_bg_color = c.progress_track; // Unfilled progress bar track (#2a2a2a) reads clearly
        visuals.override_text_color = Some(c.text_main);

        visuals.widgets.noninteractive.bg_fill = c.progress_track;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, c.border);
        visuals.widgets.inactive.bg_fill = c.bg_raised;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, c.border);
        visuals.widgets.hovered.bg_fill = c.bg_row_hover;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, c.border_strong);
        visuals.widgets.active.bg_fill = c.bg_row_selected;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, c.accent);
        visuals.widgets.open.bg_fill = c.bg_input;
        visuals.selection.bg_fill = c.bg_row_selected;
        visuals.selection.stroke = Stroke::new(1.0_f32, c.accent_hover);
        let r = Rounding::same(6.0);
        visuals.widgets.noninteractive.rounding = r;
        visuals.widgets.inactive.rounding = r;
        visuals.widgets.hovered.rounding = r;
        visuals.widgets.active.rounding = r;
        visuals.widgets.open.rounding = r;
        visuals.window_shadow.blur = 24.0;
        visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
        ctx.set_visuals(visuals);

        // ── Responsive breakpoints ─────────────────────────────────────
        let screen_w = ctx.screen_rect().width();
        let narrow = screen_w < 700.0;
        let compact = screen_w < 920.0;
        let show_sidebar = self.sidebar_open && screen_w >= 760.0;

        // Precompute selection states
        let (has_selection, is_active, is_paused, is_done, selected_path) = {
            if let Some(job) = self.selected_job_id.and_then(|id| self.jobs.iter().find(|j| j.id == id)) {
                let active = matches!(job.status, DownloadStatus::Downloading | DownloadStatus::Probing | DownloadStatus::Queued);
                let paused = matches!(job.status, DownloadStatus::Paused | DownloadStatus::Failed);
                let done = job.status == DownloadStatus::Completed;
                (true, active, paused, done, Some(job.save_path.clone()))
            } else {
                (false, false, false, false, None)
            }
        };

        // ── 1. TITLEBAR ────────────────────────────────────────────────
        egui::TopBottomPanel::top("app_titlebar")
            .frame(Frame::none().fill(c.bg_chrome).stroke(Stroke::new(1.0_f32, c.border)).inner_margin(Margin::symmetric(16.0_f32, 10.0_f32)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let add_label = if narrow { "" } else { "Add URL" };
                    if icon_text_button(ui, Icon::Plus, add_label, Color32::WHITE, c.accent, c.accent_hover, Stroke::NONE, 32.0)
                        .on_hover_text("Add URL (Ctrl+N)")
                        .clicked()
                    {
                        self.show_add_modal = true;
                        self.focus_add_url = true;
                    }
                    ui.add_space(8.0);

                    egui::menu::bar(ui, |ui| {
                        ui.menu_button("File", |ui| {
                            if ui.button("Add URL…   Ctrl+N").clicked() {
                                self.show_add_modal = true;
                                self.focus_add_url = true;
                                ui.close_menu();
                            }
                            if ui.button("Options…").clicked() {
                                self.show_settings_modal = true;
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("Exit").on_hover_text("Quit and stop all downloads").clicked() {
                                self.quitting = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                ui.close_menu();
                            }
                        });
                        ui.menu_button("View", |ui| {
                            if ui.button("Toggle sidebar").clicked() {
                                self.sidebar_open = !self.sidebar_open;
                                ui.close_menu();
                            }
                        });
                        ui.menu_button("Help", |ui| {
                            if ui.button("About").clicked() {
                                self.show_about_modal = true;
                                ui.close_menu();
                            }
                        });
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let update = dm_core::updater::get_status();
                        if update.state == "ready_to_restart" {
                            let label = format!("Update {} - Restart", update.version.as_deref().unwrap_or(""));
                            if icon_text_button(ui, Icon::Download, &label, Color32::WHITE, c.success, c.success.gamma_multiply(0.85), Stroke::NONE, 32.0)
                                .on_hover_text("Install the downloaded update and restart")
                                .clicked()
                                && dm_core::updater::launch_staged_installer().is_ok()
                            {
                                self.quitting = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            ui.add_space(6.0);
                        }
                        let theme_icon = if self.theme == ThemeMode::Light { Icon::Moon } else { Icon::Sun };
                        if icon_button(ui, &c, theme_icon, "Toggle light/dark theme").clicked() {
                            self.theme = if self.theme == ThemeMode::Light { ThemeMode::Dark } else { ThemeMode::Light };
                        }
                    });
                });
            });

        // ── 2. STATUS BAR ──────────────────────────────────────────────
        egui::TopBottomPanel::bottom("app_statusbar")
            .frame(Frame::none().fill(c.bg_chrome).stroke(Stroke::new(1.0_f32, c.border)).inner_margin(Margin::symmetric(16.0_f32, 8.0_f32)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let active_count = self.jobs.iter().filter(|j| matches!(j.status, DownloadStatus::Downloading | DownloadStatus::Probing)).count();
                    let completed_count = self.jobs.iter().filter(|j| j.status == DownloadStatus::Completed).count();
                    let total_speed: f64 = self.jobs.iter().filter(|j| j.status == DownloadStatus::Downloading).map(|j| j.speed_bps).sum();

                    let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 3.5, c.success);
                    ui.label(RichText::new("Rust engine").size(11.5).color(c.text_muted));
                    if !narrow {
                        ui.label(RichText::new("·").color(c.text_idle));
                        ui.label(RichText::new(format!("{} active", active_count)).size(11.5).color(c.text_dim));
                        ui.label(RichText::new("·").color(c.text_idle));
                        ui.label(RichText::new(format!("{} complete", completed_count)).size(11.5).color(c.text_dim));
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if total_speed > 0.0 {
                            ui.label(RichText::new(format!("↓ {}", format_speed(total_speed))).size(11.5).strong().color(c.success));
                        } else {
                            ui.label(RichText::new("Idle").size(11.5).color(c.text_idle));
                        }
                        if !narrow {
                            ui.label(RichText::new(format!("{} total", self.jobs.len())).size(11.5).color(c.text_dim));
                        }
                    });
                });
            });

        // ── 3. SIDEBAR ─────────────────────────────────────────────────
        if show_sidebar {
            egui::SidePanel::left("app_sidebar")
                .resizable(false)
                .exact_width(if compact { 196.0 } else { 232.0 })
                .frame(Frame::none().fill(c.bg_chrome).stroke(Stroke::new(1.0_f32, c.border)).inner_margin(Margin::symmetric(12.0_f32, 16.0_f32)))
                .show(ctx, |ui| {
                    let mut section = "";
                    for cat in CategoryId::ALL {
                        if cat.section() != section {
                            section = cat.section();
                            ui.add_space(if section == "LIBRARY" { 0.0 } else { 18.0 });
                            ui.label(RichText::new(section).size(10.5).strong().color(c.text_dim));
                            ui.add_space(6.0);
                        }
                        let count = self.jobs.iter().filter(|j| self.matches_category(j, cat)).count();
                        let active = self.category == cat;
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.0), egui::Sense::click());
                        let bg = if active {
                            c.bg_row_selected
                        } else if resp.hovered() {
                            c.bg_row_hover
                        } else {
                            Color32::TRANSPARENT
                        };
                        ui.painter().rect_filled(rect, Rounding::same(9.0), bg);
                        let fg = if active { c.accent } else { c.text_muted };
                        paint_icon(ui.painter(), Pos2::new(rect.left() + 21.0, rect.center().y), 16.0, cat.icon(), fg);
                        ui.painter().text(Pos2::new(rect.left() + 40.0, rect.center().y), egui::Align2::LEFT_CENTER, cat.label(), egui::FontId::proportional(13.5), if active { c.text_main } else { fg });
                        ui.painter().text(Pos2::new(rect.right() - 12.0, rect.center().y), egui::Align2::RIGHT_CENTER, count.to_string(), egui::FontId::proportional(12.0), if active { c.accent } else { c.text_dim });
                        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                        if resp.clicked() {
                            self.category = cat;
                        }
                        ui.add_space(2.0);
                    }
                });
        }

        // ── 4. MAIN CONTENT ────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(Frame::none().fill(c.bg_app).inner_margin(Margin::symmetric(if narrow { 12.0_f32 } else { 24.0_f32 }, 20.0_f32)))
            .show(ctx, |ui| {
                // Header: title · search · primary action
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.category.label()).size(if narrow { 17.0 } else { 20.0 }).strong().color(c.text_main));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let search_w = if narrow { 120.0 } else if compact { 170.0 } else { 240.0 };
                        Frame::none()
                            .fill(c.bg_raised)
                            .stroke(Stroke::new(1.0_f32, c.border_strong))
                            .rounding(Rounding::same(10.0))
                            .inner_margin(Margin::symmetric(10.0, 7.0))
                            .show(ui, |ui| {
                                let (ir, _) = ui.allocate_exact_size(Vec2::splat(14.0), egui::Sense::hover());
                                paint_icon(ui.painter(), ir.center(), 14.0, Icon::Search, c.text_dim);
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.search_query)
                                        .desired_width(search_w)
                                        .hint_text(RichText::new("Search downloads…").color(c.text_dim))
                                        .frame(false),
                                );
                            });
                    });
                });

                // Contextual actions for the selected download
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    ui.add_enabled_ui(has_selection, |ui| {
                        if is_paused && chip_button(ui, &c, Icon::Play, "Resume").clicked() {
                            self.resume_selected();
                        }
                        if is_active && chip_button(ui, &c, Icon::Pause, "Pause").clicked() {
                            self.pause_selected();
                        }
                        if is_done && chip_button(ui, &c, Icon::File, "Open file").clicked() {
                            if let Some(ref p) = selected_path { self.open_file(p); }
                        }
                        if chip_button(ui, &c, Icon::Folder, "Folder").clicked() {
                            if let Some(ref p) = selected_path { self.open_folder(p); }
                        }
                        if chip_button(ui, &c, Icon::Info, "Details").clicked() {
                            self.properties_job_id = self.selected_job_id;
                            self.show_properties_modal = true;
                        }
                        if chip_button(ui, &c, Icon::Trash, "Remove").clicked() {
                            self.delete_selected(false);
                        }
                    });
                    if !has_selection {
                        ui.label(RichText::new("Select a download to see its actions").size(12.0).color(c.text_idle));
                    }
                });
                ui.add_space(8.0);

                let filtered: Vec<DownloadJob> = self.jobs.iter().filter(|j| {
                    let match_cat = self.matches_category(j, self.category);
                    let match_query = if self.search_query.trim().is_empty() {
                        true
                    } else {
                        let q = self.search_query.to_lowercase();
                        j.filename.to_lowercase().contains(&q) || j.url.to_lowercase().contains(&q)
                    };
                    match_cat && match_query
                }).cloned().collect();

                if filtered.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space((ui.available_height() * 0.28).max(20.0));
                        let searching = !self.search_query.trim().is_empty();
                        let (ir, _) = ui.allocate_exact_size(Vec2::splat(44.0), egui::Sense::hover());
                        paint_icon(ui.painter(), ir.center(), 44.0, Icon::Inbox, c.text_idle);
                        ui.add_space(8.0);
                        ui.label(RichText::new(if searching { "No downloads match your search" } else { "Nothing here yet" }).size(16.0).strong().color(c.text_main));
                        ui.add_space(4.0);
                        ui.label(RichText::new(if searching { "Try a different filename or URL" } else { "Add a URL or press Ctrl+N to start a download" }).size(13.0).color(c.text_muted));
                    });
                    return;
                }

                // Column header
                let hdr_h = 30.0;
                let (hdr, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), hdr_h), egui::Sense::hover());
                let hdr_inner = hdr.shrink2(Vec2::new(14.0, 0.0));
                let cols = table_columns(hdr_inner, compact, narrow);
                let hfont = egui::FontId::proportional(10.5);
                ui.painter().text(Pos2::new(cols.name.left() + 48.0, hdr.center().y), egui::Align2::LEFT_CENTER, "FILE NAME", hfont.clone(), c.text_dim);
                ui.painter().text(Pos2::new(cols.progress.left(), hdr.center().y), egui::Align2::LEFT_CENTER, "PROGRESS", hfont.clone(), c.text_dim);
                if let Some(r) = cols.speed {
                    ui.painter().text(Pos2::new(r.left(), hdr.center().y), egui::Align2::LEFT_CENTER, "SPEED", hfont.clone(), c.text_dim);
                }
                if let Some(r) = cols.eta {
                    ui.painter().text(Pos2::new(r.left(), hdr.center().y), egui::Align2::LEFT_CENTER, "TIME LEFT", hfont, c.text_dim);
                }
                ui.painter().line_segment([hdr.left_bottom(), hdr.right_bottom()], Stroke::new(1.0_f32, c.border_strong));

                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for job in &filtered {
                        let is_selected = Some(job.id) == self.selected_job_id;
                        let (row_rect, row_resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 66.0), egui::Sense::click());

                        let row_resp = row_resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                        if row_resp.clicked() {
                            self.selected_job_id = Some(job.id);
                        }
                        if row_resp.double_clicked() {
                            self.properties_job_id = Some(job.id);
                            self.show_properties_modal = true;
                        }
                        if row_resp.secondary_clicked() {
                            self.selected_job_id = Some(job.id);
                        }
                        row_resp.clone().context_menu(|ui| {
                            let done = job.status == DownloadStatus::Completed;
                            let running = matches!(job.status, DownloadStatus::Downloading | DownloadStatus::Probing | DownloadStatus::Queued);
                            let resumable = matches!(job.status, DownloadStatus::Paused | DownloadStatus::Failed);
                            self.selected_job_id = Some(job.id);
                            if ui.add_enabled(done, egui::Button::new("Open File")).clicked() {
                                self.open_file(&job.save_path);
                                ui.close_menu();
                            }
                            if ui.button("Open Folder").clicked() {
                                self.open_folder(&job.save_path);
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.add_enabled(resumable, egui::Button::new("Resume")).clicked() {
                                self.resume_selected();
                                ui.close_menu();
                            }
                            if ui.add_enabled(running, egui::Button::new("Pause")).clicked() {
                                self.pause_selected();
                                ui.close_menu();
                            }
                            if ui.button("Copy URL").clicked() {
                                ui.ctx().copy_text(job.url.clone());
                                ui.close_menu();
                            }
                            if ui.button("Properties").clicked() {
                                self.properties_job_id = Some(job.id);
                                self.show_properties_modal = true;
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("Remove from list").clicked() {
                                self.delete_selected(false);
                                ui.close_menu();
                            }
                            if ui.add(egui::Button::new(RichText::new("Delete with file").color(c.danger))).clicked() {
                                self.delete_selected(true);
                                ui.close_menu();
                            }
                        });

                        let bg = if is_selected {
                            c.bg_row_selected
                        } else if row_resp.hovered() {
                            c.bg_row_hover
                        } else {
                            Color32::TRANSPARENT
                        };
                        ui.painter().rect_filled(row_rect.shrink2(Vec2::new(0.0, 1.0)), Rounding::same(8.0), bg);
                        ui.painter().line_segment([row_rect.left_bottom(), row_rect.right_bottom()], Stroke::new(1.0_f32, c.border));

                        let inner = row_rect.shrink2(Vec2::new(14.0, 0.0));
                        let cols = table_columns(inner, compact, narrow);
                        let cell = |ui: &mut egui::Ui, rect: Rect| {
                            ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(rect)
                                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                            )
                        };

                        // Name cell: icon tile + filename + subtitle
                        let (glyph, kind_color) = file_kind(&job.filename);
                        let (status_str, _) = status_info(&job.status, &c);
                        let mut name_ui = cell(ui, cols.name);
                        let (tile, _) = name_ui.allocate_exact_size(Vec2::splat(36.0), egui::Sense::hover());
                        name_ui.painter().rect_filled(tile, Rounding::same(9.0), c.bg_raised);
                        name_ui.painter().rect_stroke(tile, Rounding::same(9.0), Stroke::new(1.0_f32, c.border));
                        paint_icon(name_ui.painter(), tile.center(), 17.0, glyph, kind_color);
                        name_ui.add_space(6.0);
                        name_ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.add(egui::Label::new(RichText::new(&job.filename).size(14.0).strong().color(c.text_main)).truncate());
                            let size = if job.total_bytes > 0 { format_bytes(job.total_bytes) } else if job.downloaded_bytes > 0 { format_bytes(job.downloaded_bytes) } else { "Unknown size".to_string() };
                            let sub = match job.status {
                                DownloadStatus::Completed => format!("{} · Download complete", size),
                                DownloadStatus::Downloading => format!("{} · {} connections", size, job.segment_count.max(job.segments.len())),
                                DownloadStatus::Failed => format!("{} · {}", size, job.error.clone().unwrap_or_else(|| "Failed".into())),
                                _ => format!("{} · {}", size, status_str),
                            };
                            ui.add(egui::Label::new(RichText::new(sub).size(11.5).color(c.text_dim)).truncate());
                        });

                        // Progress cell
                        let progress = job_progress(job);
                        let mut prog_ui = cell(ui, cols.progress);
                        match job.status {
                            DownloadStatus::Completed => {
                                prog_ui.label(RichText::new("Complete").size(13.0).strong().color(c.success));
                            }
                            DownloadStatus::Failed => {
                                prog_ui.label(RichText::new("Failed").size(13.0).strong().color(c.danger));
                            }
                            _ => {
                                let bar_col = match job.status {
                                    DownloadStatus::Paused | DownloadStatus::Cancelled => c.text_idle,
                                    DownloadStatus::Queued => c.warning,
                                    _ => c.in_progress,
                                };
                                prog_ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 3.0;
                                    let label = if matches!(job.status, DownloadStatus::Probing) { "Connecting…".to_string() } else { format!("{:.1}%", progress * 100.0) };
                                    ui.label(RichText::new(label).size(13.0).strong().color(c.text_main));
                                    progress_bar(ui, progress, bar_col, &c);
                                });
                            }
                        }

                        if let Some(r) = cols.speed {
                            let mut u = cell(ui, r);
                            if job.speed_bps > 0.0 && job.status == DownloadStatus::Downloading {
                                u.label(RichText::new(format_speed(job.speed_bps)).size(13.0).strong().color(c.success));
                            } else {
                                u.label(RichText::new("—").size(13.0).color(c.text_idle));
                            }
                        }
                        if let Some(r) = cols.eta {
                            let mut u = cell(ui, r);
                            let eta = if job.status == DownloadStatus::Downloading { format_eta_human(job.eta_seconds) } else { "—".to_string() };
                            let col = if eta == "—" { c.text_idle } else { c.text_muted };
                            u.label(RichText::new(eta).size(13.0).color(col));
                        }
                    }
                });
            });

        // ── 7. ADD DOWNLOAD MODAL (Clean card & generous padding) ──────
        if self.show_add_modal {
            egui::Window::new("Add Download")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .frame(Frame::none().fill(c.bg_chrome).stroke(Stroke::new(1.0_f32, c.border_strong)).rounding(Rounding::same(14.0_f32)).inner_margin(Margin::same(24.0_f32)))
                .show(ctx, |ui| {
                    ui.set_width(480.0_f32);
                    ui.heading(RichText::new("Add Download URL").size(18.0).strong().color(c.text_main));
                    ui.add_space(14.0_f32);

                    ui.label(RichText::new("Address:").size(12.0).strong().color(c.text_muted));
                    ui.add_space(4.0_f32);
                    let url_resp = ui.add(
                        egui::TextEdit::singleline(&mut self.add_url)
                            .desired_width(460.0_f32)
                            .hint_text("https://... or YouTube video URL")
                    );
                    if self.focus_add_url {
                        url_resp.request_focus();
                        self.focus_add_url = false;
                    }
                    if url_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.add_download();
                    }
                    ui.add_space(10.0_f32);

                    ui.label(RichText::new("Save As (Optional):").size(12.0).strong().color(c.text_muted));
                    ui.add_space(4.0_f32);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.add_filename)
                            .desired_width(460.0_f32)
                            .hint_text("Leave blank to auto-detect from server")
                    );
                    ui.add_space(12.0_f32);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Connections:").size(12.0).strong().color(c.text_muted));
                        ui.add_space(8.0_f32);
                        ui.add(egui::Slider::new(&mut self.add_segments, 1..=32));
                    });
                    ui.add_space(10.0_f32);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("YouTube Quality:").size(12.0).strong().color(c.text_muted));
                        ui.add_space(8.0_f32);
                        egui::ComboBox::from_id_salt("modal_quality")
                            .selected_text(&self.add_quality)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.add_quality, "best".into(), "Best Available");
                                ui.selectable_value(&mut self.add_quality, "1080".into(), "1080p Full HD");
                                ui.selectable_value(&mut self.add_quality, "720".into(), "720p HD");
                                ui.selectable_value(&mut self.add_quality, "audio".into(), "Audio Only (MP3)");
                            });
                    });

                    ui.add_space(20.0_f32);
                    ui.horizontal(|ui| {
                        if ui.add(
                            egui::Button::new(RichText::new("Start Download").size(13.0).strong().color(Color32::WHITE))
                                .fill(c.accent)
                                .rounding(Rounding::same(8.0_f32))
                                .min_size(Vec2::new(130.0_f32, 36.0_f32))
                        ).clicked() {
                            self.add_download();
                        }
                        ui.add_space(8.0_f32);
                        if ui.add(
                            egui::Button::new(RichText::new("Cancel").size(13.0).color(c.text_muted))
                                .fill(c.bg_raised)
                                .rounding(Rounding::same(8.0_f32))
                                .min_size(Vec2::new(80.0_f32, 36.0_f32))
                        ).clicked() {
                            self.show_add_modal = false;
                        }
                    });
                });
        }

        // ── 8. DETAILS MODAL ──────────────────────────────────────────
        if self.show_properties_modal {
            if let Some(prop_id) = self.properties_job_id {
                let job_opt = self.jobs.iter().find(|j| j.id == prop_id).cloned();
                if let Some(job) = job_opt {
                    let is_active = matches!(job.status, DownloadStatus::Downloading | DownloadStatus::Probing | DownloadStatus::Queued);
                    let is_paused = matches!(job.status, DownloadStatus::Paused | DownloadStatus::Failed);
                    let (status_text, status_col) = status_info(&job.status, &c);
                    let progress = job_progress(&job);
                    let bar_col = if job.status == DownloadStatus::Completed { c.success } else { c.in_progress };
                    let width = (screen_w - 48.0).clamp(300.0, 540.0);

                    egui::Window::new("Download Details")
                        .title_bar(false)
                        .collapsible(false)
                        .resizable(false)
                        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                        .frame(Frame::none().fill(c.bg_chrome).stroke(Stroke::new(1.0_f32, c.border_strong)).rounding(Rounding::same(16.0_f32)).inner_margin(Margin::same(22.0_f32)))
                        .show(ctx, |ui| {
                            ui.set_width(width);

                            // Title row
                            let (glyph, kind_color) = file_kind(&job.filename);
                            ui.horizontal(|ui| {
                                let (tile, _) = ui.allocate_exact_size(Vec2::splat(40.0), egui::Sense::hover());
                                ui.painter().rect_filled(tile, Rounding::same(10.0), c.bg_raised);
                                ui.painter().rect_stroke(tile, Rounding::same(10.0), Stroke::new(1.0_f32, c.border));
                                paint_icon(ui.painter(), tile.center(), 19.0, glyph, kind_color);
                                ui.add_space(4.0);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.add(egui::Label::new(RichText::new(&job.filename).size(16.0).strong().color(c.text_main)).truncate());
                                    ui.label(RichText::new(status_text).size(12.0).strong().color(status_col));
                                });
                            });
                            ui.add_space(18.0);

                            // Progress
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("{:.1}%", progress * 100.0)).size(26.0).strong().color(c.text_main));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let total = if job.total_bytes > 0 { format_bytes(job.total_bytes) } else { "Unknown".to_string() };
                                    ui.label(RichText::new(format!("{} of {}", format_bytes(job.downloaded_bytes), total)).size(12.5).color(c.text_muted));
                                });
                            });
                            ui.add_space(6.0);
                            progress_bar(ui, progress, bar_col, &c);
                            ui.add_space(16.0);

                            // Stat tiles
                            let tile_w = (width - 2.0 * 10.0) / 3.0;
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                let rate = if job.status == DownloadStatus::Downloading && job.speed_bps > 0.0 { format_speed(job.speed_bps) } else { "—".to_string() };
                                let eta = if job.status == DownloadStatus::Downloading { format_eta_human(job.eta_seconds) } else { "—".to_string() };
                                let size = if job.total_bytes > 0 { format_bytes(job.total_bytes) } else { "Unknown".to_string() };
                                let rate_col = if rate == "—" { c.text_idle } else { c.success };
                                let eta_col = if eta == "—" { c.text_idle } else { c.text_main };
                                for (title, value, col) in [
                                    ("SIZE", size, c.text_main),
                                    ("SPEED", rate, rate_col),
                                    ("TIME LEFT", eta, eta_col),
                                ] {
                                    Frame::none()
                                        .fill(c.bg_raised)
                                        .stroke(Stroke::new(1.0_f32, c.border))
                                        .rounding(Rounding::same(10.0))
                                        .inner_margin(Margin::symmetric(12.0, 10.0))
                                        .show(ui, |ui| {
                                            ui.set_width(tile_w - 24.0);
                                            ui.spacing_mut().item_spacing.y = 3.0;
                                            ui.label(RichText::new(title).size(10.0).strong().color(c.text_dim));
                                            ui.label(RichText::new(value).size(14.0).strong().color(col));
                                        });
                                }
                            });
                            ui.add_space(12.0);

                            // Source & location
                            Frame::none()
                                .fill(c.bg_raised)
                                .stroke(Stroke::new(1.0_f32, c.border))
                                .rounding(Rounding::same(10.0))
                                .inner_margin(Margin::symmetric(14.0, 12.0))
                                .show(ui, |ui| {
                                    ui.set_width(width - 28.0);
                                    for (key, value) in [("Source", job.url.as_str()), ("Save to", job.save_path.as_str())] {
                                        ui.horizontal(|ui| {
                                            ui.add_sized([64.0, 20.0], egui::Label::new(RichText::new(key).size(12.0).color(c.text_dim)));
                                            let text_w = (ui.available_width() - 76.0).max(60.0);
                                            ui.add_sized([text_w, 20.0], egui::Label::new(RichText::new(value).size(12.5).color(c.text_main)).truncate())
                                                .on_hover_text(value);
                                            if chip_button(ui, &c, Icon::File, "Copy").clicked() {
                                                ui.ctx().copy_text(value.to_string());
                                            }
                                        });
                                    }
                                    if let Some(ref err_msg) = job.error {
                                        ui.add_space(4.0);
                                        ui.label(RichText::new(format!("Error: {}", err_msg)).size(12.0).color(c.danger));
                                    }
                                });

                            ui.add_space(18.0);
                            ui.horizontal(|ui| {
                                if is_active {
                                    if icon_text_button(ui, Icon::Pause, "Pause", Color32::WHITE, c.accent, c.accent_hover, Stroke::NONE, 34.0).clicked() {
                                        self.selected_job_id = Some(job.id);
                                        self.pause_selected();
                                    }
                                } else if is_paused {
                                    if icon_text_button(ui, Icon::Play, "Resume", Color32::WHITE, c.accent, c.accent_hover, Stroke::NONE, 34.0).clicked() {
                                        self.selected_job_id = Some(job.id);
                                        self.resume_selected();
                                    }
                                }
                                if icon_text_button(ui, Icon::Folder, "Open folder", c.text_main, c.bg_raised, c.bg_row_hover, Stroke::new(1.0_f32, c.border), 34.0).clicked() {
                                    self.open_folder(&job.save_path);
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if icon_text_button(ui, Icon::Check, "Close", c.text_muted, Color32::TRANSPARENT, c.bg_row_hover, Stroke::new(1.0_f32, c.border), 34.0).clicked() {
                                        self.show_properties_modal = false;
                                    }
                                });
                            });
                        });
                }
            }
        }

        // ── 9. SETTINGS MODAL ─────────────────────────────────────────
        if self.show_settings_modal {
            egui::Window::new("Options")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .frame(Frame::none().fill(c.bg_chrome).stroke(Stroke::new(1.0_f32, c.border_strong)).rounding(Rounding::same(14.0_f32)).inner_margin(Margin::same(24.0_f32)))
                .show(ctx, |ui| {
                    ui.set_width(460.0_f32);
                    ui.heading(RichText::new("General Options").size(18.0).strong().color(c.text_main));
                    ui.add_space(14.0_f32);

                    ui.label(RichText::new("Default Parallel Connections:").size(12.5).strong().color(c.text_muted));
                    ui.add_space(4.0_f32);
                    ui.add(egui::Slider::new(&mut self.default_segments, 1..=32));
                    ui.add_space(12.0_f32);

                    ui.label(RichText::new("Default Download Folder:").size(12.5).strong().color(c.text_muted));
                    ui.add_space(4.0_f32);
                    ui.add(egui::TextEdit::singleline(&mut self.default_save_dir).desired_width(440.0_f32));
                    ui.add_space(16.0_f32);

                    ui.label(RichText::new("YouTube sign-in (fixes \"confirm you're not a bot\"):").size(12.5).strong().color(c.text_muted));
                    ui.add_space(4.0_f32);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.yt_cookies_file)
                            .desired_width(440.0_f32)
                            .hint_text("Path to a cookies.txt file (recommended)"),
                    );
                    ui.add_space(6.0_f32);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("or read cookies from:").size(12.0).color(c.text_dim));
                        egui::ComboBox::from_id_salt("yt_cookie_browser")
                            .selected_text(if self.yt_cookies_browser.is_empty() { "None" } else { self.yt_cookies_browser.as_str() })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.yt_cookies_browser, String::new(), "None");
                                for b in ["firefox", "chrome", "edge", "brave"] {
                                    ui.selectable_value(&mut self.yt_cookies_browser, b.to_string(), b);
                                }
                            });
                    });
                    ui.add_space(20.0_f32);

                    if ui.add(
                        egui::Button::new(RichText::new("Save & Close").size(13.0).strong().color(Color32::WHITE))
                            .fill(c.accent)
                            .rounding(Rounding::same(8.0_f32))
                            .min_size(Vec2::new(120.0_f32, 36.0_f32))
                    ).clicked() {
                        self.add_segments = self.default_segments;
                        let file = self.yt_cookies_file.trim().to_string();
                        let browser = self.yt_cookies_browser.trim().to_string();
                        let mgr = Arc::clone(&self.manager);
                        self.rt.spawn(async move {
                            let mut guard = mgr.write().await;
                            let mut settings = guard.settings.clone();
                            settings.youtube_cookies_file = (!file.is_empty()).then_some(file);
                            settings.youtube_cookies_browser = (!browser.is_empty()).then_some(browser);
                            if let Err(e) = guard.update_settings(settings) {
                                tracing::warn!("Could not save options: {e:#}");
                            }
                        });
                        self.show_settings_modal = false;
                    }
                });
        }

        // ── 10. ABOUT MODAL ───────────────────────────────────────────
        if self.show_about_modal {
            egui::Window::new("About Download Manager Pro")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .frame(Frame::none().fill(c.bg_chrome).stroke(Stroke::new(1.0_f32, c.border_strong)).rounding(Rounding::same(14.0_f32)).inner_margin(Margin::same(24.0_f32)))
                .show(ctx, |ui| {
                    ui.set_width(400.0_f32);
                    ui.heading(RichText::new("⚡ Download Manager Pro").size(18.0).strong().color(c.accent));
                    ui.add_space(4.0_f32);
                    ui.label(RichText::new(concat!("Version ", env!("CARGO_PKG_VERSION"), " (Native GPU Edition)")).size(12.0).color(c.text_muted));
                    ui.add_space(10.0_f32);
                    ui.label(RichText::new("A blazing-fast, lightweight IDM alternative built in native Rust.").color(c.text_main));
                    ui.add_space(4.0_f32);
                    ui.label(RichText::new("Features dynamic segmentation, YouTube video/audio extraction, and browser extension interception.").color(c.text_dim));
                    ui.add_space(18.0_f32);
                    if ui.add(
                        egui::Button::new(RichText::new("OK").size(13.0).strong().color(Color32::WHITE))
                            .fill(c.accent)
                            .rounding(Rounding::same(8.0_f32))
                            .min_size(Vec2::new(80.0_f32, 34.0_f32))
                    ).clicked() {
                        self.show_about_modal = false;
                    }
                });
        }
    }
}
