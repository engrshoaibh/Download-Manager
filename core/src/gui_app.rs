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
    pub bg_card: Color32,         // Surface 3: Row/card container background (#1f1f1f)
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
    pub badge_bg: Color32,        // Badge pill background (#2a2a2a)
    pub badge_text: Color32,      // Badge pill text color
}

impl ThemeColors {
    pub fn light() -> Self {
        Self {
            bg_app: Color32::from_rgb(244, 245, 247),       // #f4f5f7
            bg_chrome: Color32::from_rgb(255, 255, 255),    // #ffffff
            bg_card: Color32::from_rgb(255, 255, 255),      // #ffffff
            bg_raised: Color32::from_rgb(243, 244, 246),    // #f3f4f6
            bg_input: Color32::from_rgb(255, 255, 255),     // #ffffff
            bg_row_hover: Color32::from_rgba_unmultiplied(0, 0, 0, 10),
            bg_row_selected: Color32::from_rgba_unmultiplied(247, 148, 30, 28),
            border: Color32::from_rgba_unmultiplied(0, 0, 0, 16),
            border_strong: Color32::from_rgba_unmultiplied(0, 0, 0, 30),
            text_main: Color32::from_rgb(17, 24, 39),       // #111827
            text_muted: Color32::from_rgb(75, 85, 99),      // #4b5563
            text_dim: Color32::from_rgb(107, 114, 128),     // #6b7280
            text_idle: Color32::from_rgb(156, 163, 175),    // #9ca3af
            accent: Color32::from_rgb(247, 148, 30),        // #F7941E
            accent_hover: Color32::from_rgb(255, 162, 54),
            in_progress: Color32::from_rgb(8, 145, 178),    // #0891b2
            success: Color32::from_rgb(46, 204, 113),       // #2ECC71
            danger: Color32::from_rgb(239, 68, 68),         // #ef4444
            warning: Color32::from_rgb(245, 158, 11),       // #f59e0b
            probing_blue: Color32::from_rgb(14, 165, 233),  // #0ea5e9
            progress_track: Color32::from_rgb(229, 231, 235), // #e5e7eb
            badge_bg: Color32::from_rgb(229, 231, 235),       // #e5e7eb
            badge_text: Color32::from_rgb(75, 85, 99),
        }
    }

    pub fn dark() -> Self {
        Self {
            // Level 1: Base background (#0d0d0d)
            bg_app: Color32::from_rgb(13, 13, 13),
            // Level 2: Panel/sidebar/toolbar/statusbar background (#161616)
            bg_chrome: Color32::from_rgb(22, 22, 22),
            // Level 3: Row/card container background (#1f1f1f)
            bg_card: Color32::from_rgb(31, 31, 31),
            // Tactile toolbar buttons / search / modal inputs (#1a1a1a)
            bg_raised: Color32::from_rgb(26, 26, 26),
            bg_input: Color32::from_rgb(18, 18, 18),
            // Row states: 12% opacity orange tint for selected row, gentle semi-transparent hover
            bg_row_hover: Color32::from_rgba_unmultiplied(255, 255, 255, 14),   // ~5.5% white overlay (never washes out text)
            bg_row_selected: Color32::from_rgba_unmultiplied(247, 148, 30, 31), // 12% orange tint
            // Softer, smoothed divider borders (~5% and ~9% white opacity for smooth transition)
            border: Color32::from_rgba_unmultiplied(255, 255, 255, 12),
            border_strong: Color32::from_rgba_unmultiplied(255, 255, 255, 24),
            // Typography WCAG AA high-contrast
            text_main: Color32::from_rgb(248, 250, 252),    // White/high-contrast (#f8fafc)
            text_muted: Color32::from_rgb(186, 194, 205),   // Legible secondary text (#bac2cd)
            text_dim: Color32::from_rgb(154, 154, 154),     // Column header gray >= #9A9A9A
            text_idle: Color32::from_rgb(94, 102, 115),     // Dimmed idle values ("0 B", "—")
            // Orange (#F7941E) exclusively for primary actions & active states
            accent: Color32::from_rgb(247, 148, 30),        // #F7941E
            accent_hover: Color32::from_rgb(255, 162, 54),
            // Secondary palette: Teal/cyan for in-progress states
            in_progress: Color32::from_rgb(6, 182, 212),    // #06b6d4 Vibrant Teal/Cyan
            // Brighter, saturated green (#2ECC71) for complete state legible against dark background
            success: Color32::from_rgb(46, 204, 113),       // #2ECC71
            danger: Color32::from_rgb(248, 113, 113),       // #f87171 Crimson
            warning: Color32::from_rgb(251, 191, 36),       // #fbbf24 Amber
            probing_blue: Color32::from_rgb(56, 189, 248),  // #38bdf8 Cyan-Blue
            // Progress bars: visible unfilled track color (#2a2a2a)
            progress_track: Color32::from_rgb(42, 42, 42),  // #2a2a2a
            // Category badges: subtle pill background (#2a2a2a)
            badge_bg: Color32::from_rgb(42, 42, 42),        // #2a2a2a
            badge_text: Color32::from_rgb(209, 213, 219),   // #d1d5db
        }
    }
}

// Category colors matching ui/src/index.css
const KIND_COMPRESSED: Color32 = Color32::from_rgb(245, 158, 11);
const KIND_DOCUMENTS: Color32 = Color32::from_rgb(59, 130, 246);
const KIND_MUSIC: Color32 = Color32::from_rgb(168, 85, 247);
const KIND_VIDEO: Color32 = Color32::from_rgb(236, 72, 153);
const KIND_PROGRAMS: Color32 = Color32::from_rgb(16, 185, 129);
const KIND_GENERAL: Color32 = Color32::from_rgb(107, 114, 128);

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum CategoryId {
    All,
    Unfinished,
    Finished,
    Compressed,
    Documents,
    Music,
    Video,
    Programs,
    General,
}

impl CategoryId {
    pub fn label(&self) -> &'static str {
        match self {
            CategoryId::All => "All Downloads",
            CategoryId::Unfinished => "Unfinished",
            CategoryId::Finished => "Finished",
            CategoryId::Compressed => "Compressed",
            CategoryId::Documents => "Documents",
            CategoryId::Music => "Music",
            CategoryId::Video => "Video",
            CategoryId::Programs => "Programs",
            CategoryId::General => "General",
        }
    }

    pub fn dot_color(&self) -> Color32 {
        match self {
            CategoryId::All => KIND_GENERAL,
            CategoryId::Unfinished => Color32::from_rgb(240, 138, 44),
            CategoryId::Finished => Color32::from_rgb(16, 185, 129),
            CategoryId::Compressed => KIND_COMPRESSED,
            CategoryId::Documents => KIND_DOCUMENTS,
            CategoryId::Music => KIND_MUSIC,
            CategoryId::Video => KIND_VIDEO,
            CategoryId::Programs => KIND_PROGRAMS,
            CategoryId::General => KIND_GENERAL,
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
}

impl NativeApp {
    pub fn new(manager: Arc<RwLock<DownloadManager>>, rt: tokio::runtime::Handle) -> Self {
        let default_save = dirs::download_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .to_string_lossy()
            .to_string();

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
        }
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

    fn stop_all(&mut self) {
        let mgr = Arc::clone(&self.manager);
        self.rt.spawn(async move {
            let mut guard = mgr.write().await;
            let active_ids: Vec<JobId> = guard
                .jobs
                .iter()
                .filter(|(_, j)| matches!(j.status, DownloadStatus::Downloading | DownloadStatus::Probing | DownloadStatus::Queued))
                .map(|(id, _)| *id)
                .collect();
            for id in active_ids {
                guard.pause(&id);
            }
        });
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
        match cat {
            CategoryId::All => true,
            CategoryId::Unfinished => matches!(job.status, DownloadStatus::Downloading | DownloadStatus::Probing | DownloadStatus::Queued | DownloadStatus::Paused | DownloadStatus::Failed),
            CategoryId::Finished => job.status == DownloadStatus::Completed,
            CategoryId::Compressed => {
                let ext = get_extension(&job.filename);
                ["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "iso"].contains(&ext.as_str())
            }
            CategoryId::Documents => {
                let ext = get_extension(&job.filename);
                ["pdf", "doc", "docx", "xls", "xlsx", "ppt", "txt", "csv", "md"].contains(&ext.as_str())
            }
            CategoryId::Music => {
                let ext = get_extension(&job.filename);
                ["mp3", "flac", "wav", "aac", "ogg", "m4a", "opus"].contains(&ext.as_str())
            }
            CategoryId::Video => {
                let ext = get_extension(&job.filename);
                ["mp4", "mkv", "avi", "mov", "webm", "flv", "m4v"].contains(&ext.as_str())
            }
            CategoryId::Programs => {
                let ext = get_extension(&job.filename);
                ["exe", "msi", "dmg", "apk", "deb", "rpm", "jar"].contains(&ext.as_str())
            }
            CategoryId::General => true,
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

fn format_eta(eta_opt: Option<f64>) -> String {
    if let Some(eta) = eta_opt {
        if eta.is_finite() && eta > 0.0 && eta < 86400.0 * 30.0 {
            let total_secs = eta as u64;
            let h = total_secs / 3600;
            let m = (total_secs % 3600) / 60;
            let s = total_secs % 60;
            if h > 0 {
                format!("{:02}:{:02}:{:02}", h, m, s)
            } else {
                format!("{:02}:{:02}", m, s)
            }
        } else {
            "—".to_string()
        }
    } else {
        "—".to_string()
    }
}

impl eframe::App for NativeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.refresh_jobs();
        ctx.request_repaint_after(Duration::from_millis(250));

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
        ctx.set_visuals(visuals);

        // Precompute selection states
        let (has_selection, is_active, is_paused, is_done, selected_path) = {
            if let Some(id) = self.selected_job_id {
                if let Some(job) = self.jobs.iter().find(|j| j.id == id) {
                    let active = matches!(job.status, DownloadStatus::Downloading | DownloadStatus::Probing | DownloadStatus::Queued);
                    let paused = matches!(job.status, DownloadStatus::Paused | DownloadStatus::Failed);
                    let done = job.status == DownloadStatus::Completed;
                    (true, active, paused, done, Some(job.save_path.clone()))
                } else {
                    (false, false, false, false, None)
                }
            } else {
                (false, false, false, false, None)
            }
        };

        // ── 1. TITLEBAR (height: 48px, generous spacing) ──────────────
        egui::TopBottomPanel::top("app_titlebar")
            .frame(Frame::none().fill(c.bg_chrome).stroke(Stroke::new(1.0_f32, c.border)).inner_margin(Margin::symmetric(16.0_f32, 10.0_f32)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("☰").size(15.0)).frame(false)).clicked() {
                        self.sidebar_open = !self.sidebar_open;
                    }
                    ui.add_space(8.0_f32);
                    // Use clean white/high-contrast for logo text to reserve orange exclusively for primary actions
                    ui.label(RichText::new("⚡ Download Manager Pro").size(14.0).strong().color(c.text_main));

                    ui.add_space(32.0_f32);
                    ui.horizontal(|ui| {
                        let search_frame = Frame::none()
                            .fill(c.bg_raised)
                            .stroke(Stroke::new(1.0_f32, c.border))
                            .rounding(Rounding::same(16.0_f32))
                            .inner_margin(Margin::symmetric(12.0_f32, 5.0_f32));
                        search_frame.show(ui, |ui| {
                            ui.label(RichText::new("🔍").size(12.0).color(c.text_muted));
                            ui.add_space(4.0_f32);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.search_query)
                                    .desired_width(280.0_f32)
                                    .hint_text(RichText::new("Search downloads...").color(c.text_dim))
                                    .frame(false),
                            );
                        });
                    });

                    // Right window controls: Theme Toggle & Options
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(
                            egui::Button::new(RichText::new(if self.theme == ThemeMode::Light { "🌙 Dark" } else { "☀ Light" }).size(12.0))
                                .rounding(Rounding::same(6.0_f32))
                                .fill(c.bg_raised)
                        ).clicked() {
                            self.theme = if self.theme == ThemeMode::Light { ThemeMode::Dark } else { ThemeMode::Light };
                        }

                        ui.add_space(8.0_f32);

                        if ui.add(
                            egui::Button::new(RichText::new("⚙ Options").size(12.0))
                                .rounding(Rounding::same(6.0_f32))
                                .fill(c.bg_raised)
                        ).clicked() {
                            self.show_settings_modal = true;
                        }
                    });
                });
            });

        // ── 2. MENUBAR (height: 32px, clean border) ────────────────────
        egui::TopBottomPanel::top("app_menubar")
            .frame(Frame::none().fill(c.bg_chrome).stroke(Stroke::new(1.0_f32, c.border)).inner_margin(Margin::symmetric(12.0_f32, 4.0_f32)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    egui::menu::bar(ui, |ui| {
                        ui.menu_button("File", |ui| {
                            if ui.button("Add URL… (Ctrl+N)").clicked() {
                                self.show_add_modal = true;
                                ui.close_menu();
                            }
                            if ui.button("Options…").clicked() {
                                self.show_settings_modal = true;
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("Exit").clicked() {
                                std::process::exit(0);
                            }
                        });

                        ui.menu_button("Downloads", |ui| {
                            if ui.add_enabled(has_selection && is_done, egui::Button::new("Open File")).clicked() {
                                if let Some(ref p) = selected_path { self.open_file(p); }
                                ui.close_menu();
                            }
                            if ui.add_enabled(has_selection, egui::Button::new("Open Folder")).clicked() {
                                if let Some(ref p) = selected_path { self.open_folder(p); }
                                ui.close_menu();
                            }
                            if ui.add_enabled(has_selection, egui::Button::new("Properties")).clicked() {
                                self.properties_job_id = self.selected_job_id;
                                self.show_properties_modal = true;
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.add_enabled(has_selection && is_paused, egui::Button::new("Resume")).clicked() {
                                self.resume_selected();
                                ui.close_menu();
                            }
                            if ui.add_enabled(has_selection && is_active, egui::Button::new("Stop")).clicked() {
                                self.pause_selected();
                                ui.close_menu();
                            }
                            if ui.button("Stop All").clicked() {
                                self.stop_all();
                                ui.close_menu();
                            }
                            if ui.add_enabled(has_selection, egui::Button::new("Delete")).clicked() {
                                self.delete_selected(false);
                                ui.close_menu();
                            }
                        });

                        ui.menu_button("Help", |ui| {
                            if ui.button("About Download Manager Pro").clicked() {
                                self.show_about_modal = true;
                                ui.close_menu();
                            }
                        });
                    });
                });
            });

        // ── 3. TOOLBAR (height: 68px, comfortable spacing & paddings) ──
        egui::TopBottomPanel::top("app_toolbar")
            .frame(Frame::none().fill(c.bg_chrome).stroke(Stroke::new(1.0_f32, c.border)).inner_margin(Margin::symmetric(16.0_f32, 10.0_f32)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let add_btn = ui.add(
                        egui::Button::new(RichText::new("➕\nAdd URL").strong().color(Color32::WHITE))
                            .fill(c.accent)
                            .rounding(Rounding::same(8.0_f32))
                            .min_size(Vec2::new(76.0_f32, 48.0_f32))
                    );
                    if add_btn.clicked() {
                        self.show_add_modal = true;
                    }

                    ui.add_space(8.0_f32);

                    let resume_btn = ui.add_enabled(
                        has_selection && is_paused,
                        egui::Button::new(RichText::new("▶\nResume").size(11.5).color(c.text_main))
                            .fill(c.bg_raised)
                            .stroke(Stroke::new(1.0_f32, c.border))
                            .rounding(Rounding::same(8.0_f32))
                            .min_size(Vec2::new(72.0_f32, 48.0_f32))
                    );
                    if resume_btn.clicked() {
                        self.resume_selected();
                    }

                    ui.add_space(4.0_f32);

                    let stop_btn = ui.add_enabled(
                        has_selection && is_active,
                        egui::Button::new(RichText::new("⏸\nStop").size(11.5).color(c.text_main))
                            .fill(c.bg_raised)
                            .stroke(Stroke::new(1.0_f32, c.border))
                            .rounding(Rounding::same(8.0_f32))
                            .min_size(Vec2::new(72.0_f32, 48.0_f32))
                    );
                    if stop_btn.clicked() {
                        self.pause_selected();
                    }

                    ui.add_space(4.0_f32);

                    if ui.add(
                        egui::Button::new(RichText::new("⏹\nStop All").size(11.5).color(c.text_main))
                            .fill(c.bg_raised)
                            .stroke(Stroke::new(1.0_f32, c.border))
                            .rounding(Rounding::same(8.0_f32))
                            .min_size(Vec2::new(72.0_f32, 48.0_f32))
                    ).clicked() {
                        self.stop_all();
                    }

                    ui.add_space(4.0_f32);

                    let del_btn = ui.add_enabled(
                        has_selection,
                        egui::Button::new(RichText::new("🗑\nDelete").size(11.5).color(c.text_main))
                            .fill(c.bg_raised)
                            .stroke(Stroke::new(1.0_f32, c.border))
                            .rounding(Rounding::same(8.0_f32))
                            .min_size(Vec2::new(72.0_f32, 48.0_f32))
                    );
                    if del_btn.clicked() {
                        self.delete_selected(false);
                    }

                    ui.add_space(4.0_f32);

                    let prop_btn = ui.add_enabled(
                        has_selection,
                        egui::Button::new(RichText::new("ℹ\nProperties").size(11.5).color(c.text_main))
                            .fill(c.bg_raised)
                            .stroke(Stroke::new(1.0_f32, c.border))
                            .rounding(Rounding::same(8.0_f32))
                            .min_size(Vec2::new(76.0_f32, 48.0_f32))
                    );
                    if prop_btn.clicked() {
                        self.properties_job_id = self.selected_job_id;
                        self.show_properties_modal = true;
                    }

                    ui.add_space(8.0_f32);
                    ui.separator();
                    ui.add_space(8.0_f32);

                    if ui.add(
                        egui::Button::new(RichText::new("⚙\nOptions").size(11.5).color(c.text_main))
                            .fill(c.bg_raised)
                            .stroke(Stroke::new(1.0_f32, c.border))
                            .rounding(Rounding::same(8.0_f32))
                            .min_size(Vec2::new(72.0_f32, 48.0_f32))
                    ).clicked() {
                        self.show_settings_modal = true;
                    }
                });
            });

        // ── 4. STATUSBAR (height: 36px, clean badges & metrics) ───────
        egui::TopBottomPanel::bottom("app_statusbar")
            .frame(Frame::none().fill(c.bg_chrome).stroke(Stroke::new(1.0_f32, c.border)).inner_margin(Margin::symmetric(16.0_f32, 8.0_f32)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (dot_rect, _) = ui.allocate_exact_size(Vec2::new(8.0_f32, 8.0_f32), egui::Sense::hover());
                    ui.painter().circle_filled(dot_rect.center(), 4.0_f32, c.success);
                    ui.add_space(4.0_f32);
                    ui.label(RichText::new("Engine Active").size(12.0).strong().color(c.text_muted));
                    ui.add_space(12.0_f32);
                    ui.separator();
                    ui.add_space(12.0_f32);

                    let active_count = self.jobs.iter().filter(|j| matches!(j.status, DownloadStatus::Downloading | DownloadStatus::Probing)).count();
                    let completed_count = self.jobs.iter().filter(|j| j.status == DownloadStatus::Completed).count();
                    let total_speed: f64 = self.jobs.iter().filter(|j| j.status == DownloadStatus::Downloading).map(|j| j.speed_bps).sum();
                    let total_downloaded: u64 = self.jobs.iter().map(|j| j.downloaded_bytes).sum();

                    ui.label(RichText::new("Speed: ").size(12.0).color(c.text_dim));
                    if total_speed > 0.0 {
                        ui.label(RichText::new(format_speed(total_speed)).strong().color(c.in_progress));
                    } else {
                        ui.label(RichText::new("—").color(c.text_idle));
                    }
                    ui.add_space(12.0_f32);
                    ui.separator();
                    ui.add_space(12.0_f32);

                    ui.label(RichText::new(format!("Active: {}", active_count)).size(12.0).color(if active_count > 0 { c.text_main } else { c.text_idle }));
                    ui.add_space(8.0_f32);
                    ui.separator();
                    ui.add_space(8.0_f32);
                    ui.label(RichText::new(format!("Complete: {}", completed_count)).size(12.0).color(if completed_count > 0 { c.text_main } else { c.text_idle }));
                    ui.add_space(8.0_f32);
                    ui.separator();
                    ui.add_space(8.0_f32);
                    let downloaded_str = if total_downloaded > 0 { format_bytes(total_downloaded) } else { "0 B".to_string() };
                    ui.label(RichText::new("Downloaded: ").size(12.0).color(c.text_dim));
                    ui.label(RichText::new(downloaded_str).size(12.0).color(if total_downloaded > 0 { c.text_main } else { c.text_idle }));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(format!("{} total downloads", self.jobs.len())).size(11.5).color(c.text_dim));
                    });
                });
            });

        // ── 5. CATEGORIES SIDEBAR (width: 220px, light & well-spaced) ─
        if self.sidebar_open {
            egui::SidePanel::left("app_category_tree")
                .min_width(220.0_f32)
                .max_width(240.0_f32)
                .frame(Frame::none().fill(c.bg_chrome).stroke(Stroke::new(1.0_f32, c.border)).inner_margin(Margin::symmetric(14.0_f32, 16.0_f32)))
                .show(ctx, |ui| {
                    ui.label(RichText::new("CATEGORIES").size(10.5).strong().color(c.text_dim));
                    ui.add_space(10.0_f32);

                    let categories = [
                        CategoryId::All,
                        CategoryId::Unfinished,
                        CategoryId::Finished,
                        CategoryId::Compressed,
                        CategoryId::Documents,
                        CategoryId::Music,
                        CategoryId::Video,
                        CategoryId::Programs,
                        CategoryId::General,
                    ];

                    for cat in categories {
                        let count = self.jobs.iter().filter(|j| self.matches_category(j, cat)).count();
                        let is_active = self.category == cat;

                        let item_bg = if is_active { c.bg_row_selected } else { Color32::TRANSPARENT };
                        let item_frame = Frame::none().fill(item_bg).rounding(Rounding::same(8.0_f32)).inner_margin(Margin::symmetric(10.0_f32, 8.0_f32));

                        item_frame.show(ui, |ui| {
                            let resp = ui.horizontal(|ui| {
                                let (dot_rect, _) = ui.allocate_exact_size(Vec2::new(8.0_f32, 8.0_f32), egui::Sense::hover());
                                ui.painter().rect_filled(dot_rect, Rounding::same(2.0_f32), cat.dot_color());
                                ui.add_space(6.0_f32);

                                let label_color = if is_active { c.text_main } else { c.text_muted };
                                ui.label(RichText::new(cat.label()).size(13.0).strong().color(label_color));

                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    // Visual badge pill (#2a2a2a in dark mode)
                                    let badge_bg = if is_active { c.accent } else { c.badge_bg };
                                    let badge_text = if is_active { Color32::WHITE } else { c.badge_text };
                                    let badge_frame = Frame::none()
                                        .fill(badge_bg)
                                        .stroke(Stroke::new(1.0_f32, c.border))
                                        .rounding(Rounding::same(99.0_f32))
                                        .inner_margin(Margin::symmetric(7.0_f32, 2.0_f32));
                                    badge_frame.show(ui, |ui| {
                                        ui.label(RichText::new(format!("{}", count)).size(10.5).color(badge_text));
                                    });
                                });
                            }).response;

                            if resp.interact(egui::Sense::click()).clicked() {
                                self.category = cat;
                            }
                        });
                        ui.add_space(3.0_f32);
                    }
                });
        }

        // ── 6. CENTRAL DOWNLOAD TABLE (Clean grid spacing) ────────────
        egui::CentralPanel::default()
            .frame(Frame::none().fill(c.bg_app).inner_margin(Margin::symmetric(12.0_f32, 10.0_f32)))
            .show(ctx, |ui| {
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
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("📂").size(42.0).color(c.text_dim));
                            ui.add_space(10.0_f32);
                            ui.label(RichText::new("No downloads in this category").size(16.0).strong().color(c.text_main));
                            ui.add_space(4.0_f32);
                            ui.label(RichText::new("Add a URL from the toolbar or press Ctrl+N").size(13.0).color(c.text_muted));
                            ui.add_space(16.0_f32);
                            if ui.add(
                                egui::Button::new(RichText::new("➕ Add URL").size(13.0).strong().color(Color32::WHITE))
                                    .fill(c.accent)
                                    .rounding(Rounding::same(8.0_f32))
                                    .min_size(Vec2::new(120.0_f32, 36.0_f32))
                            ).clicked() {
                                self.show_add_modal = true;
                            }
                        });
                    });
                    return;
                }

                // Table Container Card (Level 3 Elevation: #1f1f1f)
                let table_container = Frame::none()
                    .fill(c.bg_card)
                    .stroke(Stroke::new(1.0_f32, c.border))
                    .rounding(Rounding::same(10.0_f32))
                    .inner_margin(Margin::same(0.0_f32));

                table_container.show(ui, |ui| {
                    // Table Header
                    let header_frame = Frame::none()
                        .fill(c.bg_raised)
                        .stroke(Stroke::new(1.0_f32, c.border))
                        .rounding(Rounding { nw: 10.0_f32, ne: 10.0_f32, sw: 0.0_f32, se: 0.0_f32 })
                        .inner_margin(Margin::symmetric(14.0_f32, 10.0_f32));

                    header_frame.show(ui, |ui| {
                        ui.columns(7, |cols| {
                            cols[0].label(RichText::new("NAME").size(11.0).strong().color(c.text_dim));
                            cols[1].label(RichText::new("SIZE").size(11.0).strong().color(c.text_dim));
                            cols[2].label(RichText::new("STATUS").size(11.0).strong().color(c.text_dim));
                            cols[3].label(RichText::new("PROGRESS").size(11.0).strong().color(c.text_dim));
                            cols[4].label(RichText::new("TRANSFER RATE").size(11.0).strong().color(c.text_dim));
                            cols[5].label(RichText::new("TIME LEFT").size(11.0).strong().color(c.text_dim));
                            cols[6].label(RichText::new("SAVE TO").size(11.0).strong().color(c.text_dim));
                        });
                    });

                    // Table Rows
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for job in &filtered {
                            let is_selected = Some(job.id) == self.selected_job_id;

                            let (row_rect, row_resp) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 44.0_f32),
                                egui::Sense::click(),
                            );

                            // Entire row selection & double-click interactions
                            if row_resp.clicked() {
                                self.selected_job_id = Some(job.id);
                            }
                            if row_resp.double_clicked() {
                                self.properties_job_id = Some(job.id);
                                self.show_properties_modal = true;
                            }

                            // Dynamic row background: 12% orange tint if selected, hover, or transparent
                            let row_bg = if is_selected {
                                c.bg_row_selected
                            } else if row_resp.hovered() {
                                c.bg_row_hover
                            } else {
                                Color32::TRANSPARENT
                            };

                            // Draw row background and bottom border
                            ui.painter().rect_filled(row_rect, Rounding::ZERO, row_bg);
                            ui.painter().line_segment(
                                [
                                    Pos2::new(row_rect.left(), row_rect.bottom()),
                                    Pos2::new(row_rect.right(), row_rect.bottom()),
                                ],
                                Stroke::new(1.0_f32, c.border),
                            );

                            // 2-3px solid orange left-border accent stripe for selected row
                            if is_selected {
                                ui.painter().rect_filled(
                                    Rect::from_min_size(row_rect.min, Vec2::new(3.0_f32, row_rect.height())),
                                    Rounding::ZERO,
                                    c.accent,
                                );
                            }

                            // Render columns inside the row bounds
                            let mut row_ui = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(row_rect.shrink2(Vec2::new(14.0_f32, 0.0_f32)))
                                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                            );

                            row_ui.columns(7, |cols| {
                                cols[0].vertical_centered_justified(|col| {
                                    // High contrast white text on selected row (never solid orange background)
                                    col.add(
                                        egui::Label::new(
                                            RichText::new(&job.filename)
                                                .strong()
                                                .color(c.text_main)
                                        ).truncate()
                                    );
                                });

                                let size_text = if job.total_bytes > 0 {
                                    format!("{}/{}", format_bytes(job.downloaded_bytes), format_bytes(job.total_bytes))
                                } else if job.downloaded_bytes > 0 {
                                    format_bytes(job.downloaded_bytes)
                                } else {
                                    "—".to_string()
                                };
                                let size_color = if job.downloaded_bytes > 0 { c.text_muted } else { c.text_idle };
                                cols[1].label(RichText::new(size_text).size(12.0).color(size_color));

                                // Status: teal/cyan for in-progress, bright green for complete, neutral gray for paused/stopped
                                let (status_str, status_color) = match job.status {
                                    DownloadStatus::Downloading => ("Downloading", c.in_progress),
                                    DownloadStatus::Probing => ("Connecting…", c.probing_blue),
                                    DownloadStatus::Queued => ("Queued", c.warning),
                                    DownloadStatus::Paused => ("Paused", c.text_dim),
                                    DownloadStatus::Completed => ("Complete", c.success),
                                    DownloadStatus::Failed => ("Failed", c.danger),
                                    DownloadStatus::Cancelled => ("Stopped", c.text_dim),
                                };
                                let status_resp = cols[2].label(RichText::new(status_str).size(12.0).strong().color(status_color));
                                if job.status == DownloadStatus::Failed {
                                    if let Some(ref err) = job.error {
                                        status_resp.on_hover_text(format!("Reason: {}", err));
                                    }
                                }

                                let progress = if job.total_bytes > 0 {
                                    (job.downloaded_bytes as f32 / job.total_bytes as f32).clamp(0.0, 1.0)
                                } else if job.status == DownloadStatus::Completed {
                                    1.0
                                } else {
                                    0.0
                                };
                                let bar_fill = if job.status == DownloadStatus::Completed {
                                    c.success // Saturated bright green (#2ECC71)
                                } else {
                                    c.in_progress // Secondary teal/cyan (#06b6d4)
                                };
                                cols[3].add(
                                    egui::ProgressBar::new(progress)
                                        .show_percentage()
                                        .fill(bar_fill)
                                );

                                let speed_text = if job.speed_bps > 0.0 { format_speed(job.speed_bps) } else { "—".to_string() };
                                let speed_color = if job.speed_bps > 0.0 { c.text_main } else { c.text_idle };
                                cols[4].label(RichText::new(speed_text).size(12.0).strong().color(speed_color));

                                let eta_str = format_eta(job.eta_seconds);
                                let has_valid_eta = eta_str != "—";
                                let eta_color = if has_valid_eta { c.text_muted } else { c.text_idle };
                                cols[5].label(RichText::new(eta_str).size(12.0).color(eta_color));

                                let save_display = std::path::Path::new(&job.save_path)
                                    .file_name()
                                    .and_then(|f| f.to_str())
                                    .unwrap_or(&job.save_path);
                                cols[6].label(RichText::new(save_display).size(12.0).color(c.text_dim));
                            });
                        }
                    });
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
                    ui.add(
                        egui::TextEdit::singleline(&mut self.add_url)
                            .desired_width(460.0_f32)
                            .hint_text("https://... or YouTube video URL")
                    );
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

        // ── 8. PROPERTIES MODAL (Download Details & Segment Breakdown) 
        if self.show_properties_modal {
            if let Some(prop_id) = self.properties_job_id {
                let job_opt = self.jobs.iter().find(|j| j.id == prop_id).cloned();
                if let Some(job) = job_opt {
                    let is_active = matches!(job.status, DownloadStatus::Downloading | DownloadStatus::Probing | DownloadStatus::Queued);
                    let is_paused = matches!(job.status, DownloadStatus::Paused | DownloadStatus::Failed);

                    egui::Window::new("Download Details")
                        .collapsible(false)
                        .resizable(false)
                        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                        .frame(Frame::none().fill(c.bg_chrome).stroke(Stroke::new(1.0_f32, c.border_strong)).rounding(Rounding::same(14.0_f32)).inner_margin(Margin::same(24.0_f32)))
                        .show(ctx, |ui| {
                            ui.set_width(580.0_f32);
                            ui.heading(RichText::new(&job.filename).size(18.0).strong().color(c.text_main));
                            ui.add_space(8.0_f32);

                            // File & Transfer details grid
                            let details_card = Frame::none()
                                .fill(c.bg_raised)
                                .stroke(Stroke::new(1.0_f32, c.border))
                                .rounding(Rounding::same(8.0_f32))
                                .inner_margin(Margin::same(12.0_f32));

                            details_card.show(ui, |ui| {
                                ui.columns(2, |cols| {
                                    // Col 1: File info
                                    cols[0].label(RichText::new("FILE INFO").size(10.5).strong().color(c.text_dim));
                                    cols[0].add_space(4.0_f32);
                                    cols[0].label(RichText::new(format!("URL: {}", job.url)).size(11.5).color(c.text_muted));
                                    cols[0].label(RichText::new(format!("Path: {}", job.save_path)).size(11.5).color(c.text_muted));
                                    let (status_text, status_col) = match job.status {
                                        DownloadStatus::Downloading => ("Downloading", c.in_progress),
                                        DownloadStatus::Probing => ("Connecting…", c.probing_blue),
                                        DownloadStatus::Queued => ("Queued", c.warning),
                                        DownloadStatus::Paused => ("Paused", c.text_dim),
                                        DownloadStatus::Completed => ("Complete", c.success),
                                        DownloadStatus::Failed => ("Failed", c.danger),
                                        DownloadStatus::Cancelled => ("Stopped", c.text_dim),
                                    };
                                    cols[0].label(RichText::new(format!("Status: {}", status_text)).size(11.5).strong().color(status_col));

                                    // Col 2: Transfer metrics
                                    cols[1].label(RichText::new("TRANSFER METRICS").size(10.5).strong().color(c.text_dim));
                                    cols[1].add_space(4.0_f32);
                                    let total_str = if job.total_bytes > 0 { format_bytes(job.total_bytes) } else { "Unknown".to_string() };
                                    cols[1].label(RichText::new(format!("File Size: {}", total_str)).size(11.5).color(c.text_main));
                                    cols[1].label(RichText::new(format!("Downloaded: {}", format_bytes(job.downloaded_bytes))).size(11.5).color(c.text_main));
                                    let rate_str = if job.speed_bps > 0.0 { format_speed(job.speed_bps) } else { "—".to_string() };
                                    let rate_color = if job.speed_bps > 0.0 { c.in_progress } else { c.text_idle };
                                    cols[1].label(RichText::new(format!("Transfer Rate: {}", rate_str)).size(11.5).strong().color(rate_color));
                                    let eta_str = format_eta(job.eta_seconds);
                                    let eta_col = if eta_str != "—" { c.text_muted } else { c.text_idle };
                                    cols[1].label(RichText::new(format!("Time Left: {}", eta_str)).size(11.5).color(eta_col));
                                });
                            });

                            if let Some(ref err_msg) = job.error {
                                ui.add_space(8.0_f32);
                                let err_frame = Frame::none()
                                    .fill(Color32::from_rgba_unmultiplied(239, 68, 68, 24))
                                    .stroke(Stroke::new(1.0_f32, c.danger))
                                    .rounding(Rounding::same(6.0_f32))
                                    .inner_margin(Margin::symmetric(10.0_f32, 6.0_f32));
                                err_frame.show(ui, |ui| {
                                    ui.label(RichText::new(format!("⚠ Error: {}", err_msg)).size(11.5).color(c.danger));
                                });
                            }

                            ui.add_space(14.0_f32);

                            // Overall progress bar
                            let pct = if job.total_bytes > 0 {
                                ((job.downloaded_bytes as f32 / job.total_bytes as f32) * 100.0).clamp(0.0, 100.0)
                            } else if job.status == DownloadStatus::Completed {
                                100.0
                            } else {
                                0.0
                            };

                            let modal_bar_fill = if job.status == DownloadStatus::Completed {
                                c.success
                            } else {
                                c.in_progress
                            };

                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Overall Progress:").size(12.0).strong().color(c.text_muted));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(RichText::new(format!("{:.1}%", pct)).strong().color(modal_bar_fill));
                                });
                            });
                            ui.add_space(4.0_f32);
                            ui.add(egui::ProgressBar::new(pct / 100.0).fill(modal_bar_fill));

                            ui.add_space(12.0_f32);

                            // Dynamic Connections visualizer (IDM Style)
                            ui.label(RichText::new(format!("Thread Connections ({} segments):", job.segments.len().max(job.segment_count))).size(12.0).strong().color(c.text_muted));
                            ui.add_space(4.0_f32);

                            if !job.segments.is_empty() {
                                ui.horizontal(|ui| {
                                    for seg in &job.segments {
                                        let seg_pct = if seg.end >= seg.start {
                                            (seg.downloaded as f32 / (seg.end - seg.start + 1) as f32).clamp(0.0, 1.0)
                                        } else { 0.0 };
                                        ui.add(egui::ProgressBar::new(seg_pct).desired_width(12.0_f32).fill(modal_bar_fill));
                                    }
                                });
                            }

                            ui.add_space(18.0_f32);
                            ui.horizontal(|ui| {
                                if is_active {
                                    if ui.add(
                                        egui::Button::new(RichText::new("⏸ Pause").size(12.5).strong().color(Color32::WHITE))
                                            .fill(c.accent)
                                            .rounding(Rounding::same(8.0_f32))
                                            .min_size(Vec2::new(90.0_f32, 34.0_f32))
                                    ).clicked() {
                                        self.selected_job_id = Some(job.id);
                                        self.pause_selected();
                                    }
                                    ui.add_space(6.0_f32);
                                } else if is_paused {
                                    if ui.add(
                                        egui::Button::new(RichText::new("▶ Resume").size(12.5).strong().color(Color32::WHITE))
                                            .fill(c.success)
                                            .rounding(Rounding::same(8.0_f32))
                                            .min_size(Vec2::new(90.0_f32, 34.0_f32))
                                    ).clicked() {
                                        self.selected_job_id = Some(job.id);
                                        self.resume_selected();
                                    }
                                    ui.add_space(6.0_f32);
                                }

                                if ui.add(
                                    egui::Button::new(RichText::new("📁 Open Folder").size(12.5).color(c.text_main))
                                        .fill(c.bg_raised)
                                        .rounding(Rounding::same(8.0_f32))
                                        .min_size(Vec2::new(120.0_f32, 34.0_f32))
                                ).clicked() {
                                    self.open_folder(&job.save_path);
                                }

                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add(
                                        egui::Button::new(RichText::new("Close").size(12.5).color(c.text_muted))
                                            .fill(c.bg_raised)
                                            .rounding(Rounding::same(8.0_f32))
                                            .min_size(Vec2::new(80.0_f32, 34.0_f32))
                                    ).clicked() {
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
                    ui.add_space(20.0_f32);

                    if ui.add(
                        egui::Button::new(RichText::new("Save & Close").size(13.0).strong().color(Color32::WHITE))
                            .fill(c.accent)
                            .rounding(Rounding::same(8.0_f32))
                            .min_size(Vec2::new(120.0_f32, 36.0_f32))
                    ).clicked() {
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
                    ui.label(RichText::new("Version 0.1.14 (Native GPU Edition)").size(12.0).color(c.text_muted));
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
