use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui::{self, Color32, ColorImage, Frame, TextureHandle, Vec2};
use rfd::FileDialog;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const WINDOW_WIDTH: f32 = 390.0;
const WINDOW_HEIGHT_BASE: f32 = 390.0;
const WINDOW_HEIGHT_STEP: f32 = 70.0;
const WINDOW_HEIGHT_MAX: f32 = WINDOW_HEIGHT_BASE * 2.5;
const LIST_GROW_START: usize = 3;
const LIST_MAX_FILES: usize = 8;

#[derive(Clone)]
struct FileEntry {
    path: PathBuf,
    icon: String,
    status: String,
    unlock_result: Option<bool>,
    output_path: Option<PathBuf>,
}

enum UnlockMessage {
    FileResult {
        index: usize,
        success: bool,
        output_path: Option<PathBuf>,
    },
    Info(String),
    Done,
}

#[derive(PartialEq, Eq)]
enum AnimationMode {
    Logo,
    HappyLoop,
    Peck,
    Success,
}

struct AnimationState {
    mode: AnimationMode,
    frame_index: usize,
    loops_left: u32,
}

struct CrackLeafApp {
    frames: HashMap<&'static str, Vec<TextureHandle>>,
    file_entries: Vec<FileEntry>,
    animation: AnimationState,
    last_frame_time: Instant,
    frame_interval: Duration,
    unlock_in_progress: bool,
    unlock_ready_for_success: bool,
    unlock_work_done: bool,
    result_text: String,
    unlock_rx: Option<Receiver<UnlockMessage>>,
    last_window_height: f32,
    success_reverse: bool,
    qpdf_ok: bool,
    qpdf_error: Option<String>,
    qpdf_version: Option<String>,
    qpdf_warning: Option<String>,
    had_unlock: bool,
    qpdf_prompted: bool,
}

impl CrackLeafApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let assets_dir = resolve_assets_dir();
        apply_custom_font(&cc.egui_ctx, &assets_dir);
        apply_theme(&cc.egui_ctx);
        let frames = load_frames(&cc.egui_ctx, &assets_dir);
        let qpdf_status = check_qpdf_ready();
        Self {
            frames,
            file_entries: Vec::new(),
            animation: AnimationState {
                mode: AnimationMode::Logo,
                frame_index: 0,
                loops_left: 0,
            },
            last_frame_time: Instant::now(),
            frame_interval: Duration::from_millis(150),
            unlock_in_progress: false,
            unlock_ready_for_success: false,
            unlock_work_done: false,
            result_text: String::new(),
            unlock_rx: None,
            last_window_height: WINDOW_HEIGHT_BASE,
            success_reverse: false,
            qpdf_ok: qpdf_status.ok,
            qpdf_error: qpdf_status.error,
            qpdf_version: qpdf_status.version,
            qpdf_warning: qpdf_status.warning,
            had_unlock: false,
            qpdf_prompted: false,
        }
    }

    fn current_texture(&self) -> &TextureHandle {
        let key = match self.animation.mode {
            AnimationMode::Logo => "logo",
            AnimationMode::HappyLoop => "happy_loop",
            AnimationMode::Peck => "peck",
            AnimationMode::Success => {
                if self.success_reverse {
                    "success_reverse"
                } else {
                    "success"
                }
            }
        };
        let frames = self
            .frames
            .get(key)
            .or_else(|| self.frames.get("logo"))
            .expect("missing frame set");
        let idx = self.animation.frame_index.min(frames.len().saturating_sub(1));
        &frames[idx]
    }

    fn set_mode(&mut self, mode: AnimationMode) {
        if self.animation.mode != mode {
            self.animation.mode = mode;
            self.animation.frame_index = 0;
            self.animation.loops_left = 0;
        }
    }

    fn start_happy_loop(&mut self) {
        self.set_mode(AnimationMode::HappyLoop);
    }

    fn start_peck(&mut self) {
        self.animation.mode = AnimationMode::Peck;
        self.animation.frame_index = 0;
        self.animation.loops_left = 2;
    }

    fn start_success(&mut self, reverse: bool) {
        self.success_reverse = reverse;
        self.animation.mode = AnimationMode::Success;
        self.animation.frame_index = 0;
        self.animation.loops_left = 1;
    }

    fn tick_animation(&mut self, ctx: &egui::Context) {
        if self.animation.mode == AnimationMode::Logo {
            return;
        }

        if self.last_frame_time.elapsed() < self.frame_interval {
            ctx.request_repaint();
            return;
        }
        self.last_frame_time = Instant::now();

        let frame_count = match self.animation.mode {
            AnimationMode::Logo => 1,
            AnimationMode::HappyLoop => self.frames.get("happy_loop").map(|v| v.len()).unwrap_or(1),
            AnimationMode::Peck => self.frames.get("peck").map(|v| v.len()).unwrap_or(1),
            AnimationMode::Success => {
                if self.success_reverse {
                    self.frames.get("success_reverse").map(|v| v.len()).unwrap_or(1)
                } else {
                    self.frames.get("success").map(|v| v.len()).unwrap_or(1)
                }
            }
        };

        if frame_count == 0 {
            return;
        }

        self.animation.frame_index = (self.animation.frame_index + 1) % frame_count;

        match self.animation.mode {
            AnimationMode::HappyLoop => {}
            AnimationMode::Peck => {
                if self.animation.frame_index == 0 {
                    if self.animation.loops_left > 0 {
                        self.animation.loops_left -= 1;
                    }
                    if self.animation.loops_left == 0 {
                        self.unlock_ready_for_success = true;
                        self.set_mode(AnimationMode::Logo);
                        self.maybe_start_success_animation();
                    }
                }
            }
            AnimationMode::Success => {
                if self.animation.frame_index == 0 {
                    self.animation.loops_left = self.animation.loops_left.saturating_sub(1);
                    if self.animation.loops_left == 0 {
                        self.unlock_in_progress = false;
                        if !self.file_entries.is_empty() {
                            self.start_happy_loop();
                        } else {
                            self.set_mode(AnimationMode::Logo);
                        }
                    }
                }
            }
            AnimationMode::Logo => {}
        }

        ctx.request_repaint();
    }

    fn maybe_start_success_animation(&mut self) {
        if !(self.unlock_ready_for_success && self.unlock_work_done) {
            return;
        }

        let success_count = self
            .file_entries
            .iter()
            .filter(|f| f.unlock_result == Some(true))
            .count();
        let total_count = self.file_entries.len();
        let is_failure = total_count > 0 && success_count == 0;

        if success_count == total_count && total_count > 0 {
            self.result_text = "解锁成功".to_string();
        } else if success_count > 0 {
            self.result_text = format!("部分成功: {success_count}/{total_count}");
        } else {
            self.result_text = "解锁失败".to_string();
        }

        self.start_success(is_failure);
    }

    fn update_window_size(&mut self, ctx: &egui::Context) {
        let count = self.file_entries.len();
        let height = if count <= 2 {
            WINDOW_HEIGHT_BASE
        } else if count <= LIST_MAX_FILES {
            WINDOW_HEIGHT_BASE + (count.saturating_sub(2) as f32) * WINDOW_HEIGHT_STEP
        } else {
            WINDOW_HEIGHT_MAX
        };

        if (height - self.last_window_height).abs() > f32::EPSILON {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(
                WINDOW_WIDTH,
                height,
            )));
            self.last_window_height = height;
        }
    }

    fn add_files(&mut self, paths: Vec<PathBuf>) {
        let mut added = false;
        if self.had_unlock {
            self.file_entries.clear();
            self.result_text.clear();
            self.had_unlock = false;
        }
        for path in paths {
            if !is_pdf(&path) {
                continue;
            }
            if self.file_entries.iter().any(|f| f.path == path) {
                continue;
            }
            let (icon, status) = match detect_encrypted(&path) {
                Some(true) => ("🔒".to_string(), "加密受限".to_string()),
                Some(false) => ("🔓".to_string(), "未受限".to_string()),
                None => ("🔒".to_string(), "未知".to_string()),
            };
            self.file_entries.push(FileEntry {
                path,
                icon,
                status,
                unlock_result: None,
                output_path: None,
            });
            added = true;
        }
        if added {
            self.result_text.clear();
        }
    }

    fn start_unlock(&mut self) {
        if self.unlock_in_progress || self.file_entries.is_empty() {
            return;
        }

        self.unlock_in_progress = true;
        self.unlock_ready_for_success = false;
        self.unlock_work_done = false;
        self.result_text = "处理中...".to_string();
        self.start_peck();

        let files = self.file_entries.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || run_unlock(files, tx));
        self.unlock_rx = Some(rx);
    }

    fn handle_unlock_messages(&mut self) {
        let Some(rx) = self.unlock_rx.take() else {
            return;
        };

        let mut completed = false;

        while let Ok(msg) = rx.try_recv() {
            match msg {
                UnlockMessage::FileResult {
                    index,
                    success,
                    output_path,
                } => {
                    if let Some(entry) = self.file_entries.get_mut(index) {
                        entry.unlock_result = Some(success);
                        if success {
                            entry.output_path = output_path;
                        }
                        if success {
                            entry.status = "解锁成功".to_string();
                            if let Some(path) = entry.output_path.as_ref() {
                                if let Some(is_encrypted) = detect_encrypted(path) {
                                    entry.icon = if is_encrypted { "🔒" } else { "🔓" }.to_string();
                                } else {
                                    entry.icon = "🔓".to_string();
                                }
                            } else {
                                entry.icon = "🔓".to_string();
                            }
                        } else {
                            entry.status = "解锁失败".to_string();
                        }
                    }
                }
                UnlockMessage::Info(msg) => {
                    if self.result_text.is_empty() || self.result_text == "处理中..." {
                        self.result_text = msg;
                    }
                }
                UnlockMessage::Done => {
                    self.unlock_work_done = true;
                    self.had_unlock = true;
                    self.maybe_start_success_animation();
                    completed = true;
                }
            }
        }

        if !completed {
            self.unlock_rx = Some(rx);
        }
    }
}

fn apply_custom_font(ctx: &egui::Context, assets_dir: &Path) {
    let font_path = assets_dir.join("Huiwenfangsong.ttf");
    let font_data = std::fs::read(font_path).ok();
    if let Some(bytes) = font_data {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "huiwenfangsong".to_string(),
            egui::FontData::from_owned(bytes),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "huiwenfangsong".to_string());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("huiwenfangsong".to_string());
        ctx.set_fonts(fonts);
    } else {
        eprintln!("Failed to load font: Huiwenfangsong.ttf");
    }
}

fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = Color32::from_rgb(0xFC, 0xF5, 0xEA);
    ctx.set_visuals(visuals);
    ctx.set_pixels_per_point(1.1);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (egui::TextStyle::Heading, egui::FontId::new(24.0, egui::FontFamily::Proportional)),
        (egui::TextStyle::Body, egui::FontId::new(22.0, egui::FontFamily::Proportional)),
        (egui::TextStyle::Button, egui::FontId::new(22.0, egui::FontFamily::Proportional)),
        (egui::TextStyle::Small, egui::FontId::new(20.0, egui::FontFamily::Proportional)),
    ]
    .into();
    ctx.set_style(style);
}

impl eframe::App for CrackLeafApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tick_animation(ctx);
        self.handle_unlock_messages();

        let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());
        if !dropped_files.is_empty() {
            let paths: Vec<PathBuf> = dropped_files
                .into_iter()
                .filter_map(|f| f.path)
                .collect();
            self.add_files(paths);
            if !self.file_entries.is_empty() {
                self.start_happy_loop();
            }
            self.update_window_size(ctx);
        }

        egui::CentralPanel::default()
            .frame(Frame::none().fill(Color32::from_rgb(0xFC, 0xF5, 0xEA)))
            .show(ctx, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.vertical_centered(|ui| {
                let logo_size = (WINDOW_WIDTH * 0.5).clamp(60.0, 240.0);
                let image = egui::Image::new(self.current_texture())
                    .fit_to_exact_size(Vec2::splat(logo_size));
                let response = ui.add(egui::ImageButton::new(image).frame(false));

                if !self.unlock_in_progress && !self.file_entries.is_empty() {
                    if response.hovered() {
                        self.set_mode(AnimationMode::Logo);
                    } else if self.animation.mode != AnimationMode::HappyLoop {
                        self.start_happy_loop();
                    }
                }

                if response.clicked() {
                    if self.file_entries.is_empty() {
                        if let Some(paths) = FileDialog::new().add_filter("PDF", &["pdf"]).pick_files() {
                            self.add_files(paths);
                            if !self.file_entries.is_empty() {
                                self.start_happy_loop();
                                self.update_window_size(ctx);
                            }
                        }
                    } else {
                        if !self.qpdf_ok {
                            if let Some(msg) = &self.qpdf_error {
                                self.result_text = msg.clone();
                            }
                            return;
                        }
                        self.start_unlock();
                    }
                }

                let hint = if self.file_entries.is_empty() {
                    "点击或者拖入文件".to_string()
                } else if self.file_entries.len() == 1 {
                    let entry = &self.file_entries[0];
                    format!("{} {}", entry.icon, entry.path.file_name().unwrap_or_default().to_string_lossy())
                } else {
                    format!("已导入 {} 个文件", self.file_entries.len())
                };
                ui.label(hint);

                if self.file_entries.len() > 1 {
                    let max_list_height = if self.file_entries.len() >= LIST_GROW_START {
                        WINDOW_HEIGHT_MAX - WINDOW_HEIGHT_BASE
                    } else {
                        (self.file_entries.len().saturating_sub(1) as f32) * 40.0
                    };

                    let list_width = WINDOW_WIDTH - 40.0;
                    let available_height = (ui.available_height() - 40.0).max(60.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(list_width, available_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(0.0, 8.0);
                            egui::ScrollArea::vertical()
                                .max_height(available_height)
                                .show(ui, |ui| {
                                    for entry in &self.file_entries {
                                        let filename = entry
                                            .path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy();
                                        let icon = entry.icon.clone();
                                        ui.horizontal(|ui| {
                                            ui.label(icon);
                                            let text_width = (ui.available_width() - 50.0).max(80.0);
                                            let label = egui::Label::new(filename).wrap();
                                            let label_response = ui
                                                .add_sized(Vec2::new(text_width, 0.0), label)
                                                .interact(egui::Sense::click());

                                            let can_open = entry.output_path.is_some();
                                            if can_open {
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(egui::Align::Center),
                                                    |ui| {
                                                        if ui
                                                            .add_sized(
                                                                Vec2::new(24.0, 24.0),
                                                                egui::Button::new("开"),
                                                            )
                                                            .clicked()
                                                        {
                                                            open_entry(entry);
                                                        }
                                                    },
                                                );
                                            }

                                            if label_response.double_clicked() {
                                                open_entry(entry);
                                            }
                                        });
                                    }
                                });
                        },
                    );
                }

                if !self.qpdf_ok {
                    if let Some(msg) = &self.qpdf_error {
                        ui.label(msg);
                    }
                } else {
                    if let Some(msg) = &self.qpdf_warning {
                        ui.label(msg);
                    }
                }
                });

                if !self.result_text.is_empty() {
                    let offset = (ui.available_height() - 36.0).max(0.0);
                    ui.add_space(offset);
                    ui.vertical_centered(|ui| {
                        ui.label(&self.result_text);
                    });
                }
            });
        });

        if !self.qpdf_ok && !self.qpdf_prompted {
            self.qpdf_prompted = true;
            show_qpdf_setup_dialog();
        }
    }
}

fn resolve_assets_dir() -> PathBuf {
    if let Ok(cwd) = std::env::current_dir() {
        let assets = cwd.join("assets");
        if assets.exists() {
            return assets;
        }
    }
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let assets = exe_dir.join("assets");
            if assets.exists() {
                return assets;
            }
            let macos_bundle_assets = exe_dir.join("..").join("Resources").join("assets");
            if macos_bundle_assets.exists() {
                return macos_bundle_assets;
            }
        }
    }
    PathBuf::from("assets")
}

fn load_frames(ctx: &egui::Context, assets_dir: &Path) -> HashMap<&'static str, Vec<TextureHandle>> {
    let mut frames = HashMap::new();

    let sets: &[(&str, &[&str])] = &[
        ("logo", &["crackleaf"]),
        ("happy_loop", &["高兴1", "高兴2", "高兴3", "高兴4", "高兴3", "高兴2", "高兴1"]),
        ("peck", &["啄1", "啄2"]),
        ("success", &["成功1", "成功2", "成功3", "成功4", "成功5"]),
        ("success_reverse", &["成功5", "成功4", "成功3", "成功2", "成功1"]),
    ];

    for (key, names) in sets {
        let mut textures = Vec::new();
        for (idx, name) in names.iter().enumerate() {
            let path = assets_dir.join(format!("{name}.png"));
            match load_texture(ctx, &path, &format!("{key}_{idx}")) {
                Ok(texture) => textures.push(texture),
                Err(err) => {
                    eprintln!("Failed to load {:?}: {err}", path);
                    textures.push(load_placeholder(ctx, &format!("{key}_placeholder_{idx}")));
                }
            }
        }
        frames.insert(*key, textures);
    }

    frames
}

fn load_texture(ctx: &egui::Context, path: &Path, name: &str) -> Result<TextureHandle> {
    let image = image::open(path)?;
    let size = [image.width() as usize, image.height() as usize];
    let rgba = image.to_rgba8();
    let color_image = ColorImage::from_rgba_unmultiplied(size, &rgba);
    Ok(ctx.load_texture(name.to_string(), color_image, egui::TextureOptions::LINEAR))
}

fn load_placeholder(ctx: &egui::Context, name: &str) -> TextureHandle {
    let image = ColorImage::new([64, 64], egui::Color32::from_rgb(200, 50, 50));
    ctx.load_texture(name.to_string(), image, egui::TextureOptions::LINEAR)
}

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

fn detect_encrypted(path: &Path) -> Option<bool> {
    let mut cmd = Command::new(resolve_qpdf_command());
    cmd.arg("--show-encryption").arg(path);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x08000000);

    let output = cmd.output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();
    if stdout.contains("file is encrypted")
        || stdout.contains("encryption: yes")
        || stdout.contains("user password")
        || stdout.contains("owner password")
    {
        Some(true)
    } else if stdout.contains("file is not encrypted") || stdout.contains("not encrypted") {
        Some(false)
    } else {
        None
    }
}

fn run_unlock(files: Vec<FileEntry>, tx: Sender<UnlockMessage>) {
    for (index, entry) in files.into_iter().enumerate() {
        match unlock_pdf(&entry.path) {
            Ok(output_path) => {
                let success = output_path.is_some();
                let _ = tx.send(UnlockMessage::FileResult {
                    index,
                    success,
                    output_path,
                });
            }
            Err(err) => {
                let _ = tx.send(UnlockMessage::FileResult {
                    index,
                    success: false,
                    output_path: None,
                });
                let _ = tx.send(UnlockMessage::Info(format!(
                    "解锁失败: {}",
                    err
                )));
                continue;
            }
        }
    }

    let _ = tx.send(UnlockMessage::Done);
}

fn unlock_pdf(path: &Path) -> Result<Option<PathBuf>> {
    let output_dir = resolve_download_dir().unwrap_or_else(|| {
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    });
    let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let output_path = unique_output_path(&output_dir, file_stem);

    let mut cmd = Command::new(resolve_qpdf_command());
    cmd.arg("--password=").arg("--decrypt").arg(path).arg(&output_path);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x08000000);

    let status = cmd.status().map_err(|err| {
        anyhow::anyhow!("qpdf 执行失败（请把 qpdf 放在程序同目录或加入 PATH）: {err}")
    })?;

    if !status.success() {
        return Ok(None);
    }
    if output_path.exists() {
        Ok(Some(output_path))
    } else {
        Ok(None)
    }
}

fn unique_output_path(output_dir: &Path, file_stem: &str) -> PathBuf {
    let base = format!("{file_stem}_unlocked");
    let mut candidate = output_dir.join(format!("{base}.pdf"));
    if !candidate.exists() {
        return candidate;
    }
    for idx in 1..=9999 {
        candidate = output_dir.join(format!("{base}_{idx}.pdf"));
        if !candidate.exists() {
            return candidate;
        }
    }
    output_dir.join(format!("{base}_overflow.pdf"))
}

fn resolve_download_dir() -> Option<PathBuf> {
    if let Some(dir) = dirs::download_dir() {
        let _ = std::fs::create_dir_all(&dir);
        return Some(dir);
    }
    if let Some(home) = dirs::home_dir() {
        let dir = home.join("Downloads");
        let _ = std::fs::create_dir_all(&dir);
        return Some(dir);
    }
    None
}

fn open_file(path: &Path) {
    let path_str = path.to_string_lossy();

    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "windows")]
    let cmd = "cmd";
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";

    #[cfg(target_os = "windows")]
    let args = ["/C", "start", "", path_str.as_ref()];
    #[cfg(not(target_os = "windows"))]
    let args = [path_str.as_ref()];

    let _ = Command::new(cmd).args(args).status();
}

fn open_entry(entry: &FileEntry) {
    if let Some(path) = entry.output_path.as_ref() {
        if path.exists() {
            open_file(path);
            return;
        }
    }
    open_file(&entry.path);
}

struct QpdfStatus {
    ok: bool,
    error: Option<String>,
    version: Option<String>,
    warning: Option<String>,
}

fn check_qpdf_ready() -> QpdfStatus {
    let qpdf = resolve_qpdf_command();
    let mut cmd = Command::new(&qpdf);
    cmd.arg("--version");
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x08000000);

    match cmd.output() {
        Ok(output) => {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let version = parse_qpdf_version(&stdout);
                let warning = if version.is_none() {
                    Some("已检测到 qpdf，但版本无法识别".to_string())
                } else {
                    None
                };
                QpdfStatus {
                    ok: true,
                    error: None,
                    version,
                    warning,
                }
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let msg = if stderr.is_empty() {
                    "qpdf 运行失败（依赖缺失或版本不匹配）".to_string()
                } else {
                    format!("qpdf 运行失败：{stderr}")
                };
                QpdfStatus {
                    ok: false,
                    error: Some(msg),
                    version: None,
                    warning: None,
                }
            }
        }
        Err(err) => QpdfStatus {
            ok: false,
            error: Some(format!(
                "qpdf 不可用（请把 qpdf 放在程序同目录）：{err}"
            )),
            version: None,
            warning: None,
        },
    }
}

fn parse_qpdf_version(output: &str) -> Option<String> {
    for token in output.split_whitespace() {
        if token.chars().next()?.is_ascii_digit() {
            return Some(token.trim().to_string());
        }
    }
    None
}

fn show_qpdf_setup_dialog() {
    let msg = if cfg!(target_os = "macos") {
        "未检测到 qpdf。\n\n请在终端执行：\nbrew install qpdf\n\n安装完成后重启程序。".to_string()
    } else if cfg!(target_os = "windows") {
        let arch = if cfg!(target_pointer_width = "64") {
            "msvc64"
        } else {
            "msvc32"
        };
        format!(
            "未检测到 qpdf。\n\n请前往：\nhttps://github.com/qpdf/qpdf/releases\n\n下载 {arch} 版本（例如 qpdf-<version>-{arch}.zip），\n解压后将 qpdf.exe 放到程序同目录。"
        )
    } else {
        "未检测到 qpdf，请安装后重启程序。".to_string()
    };

    let _ = rfd::MessageDialog::new()
        .set_title("需要安装 qpdf")
        .set_description(&msg)
        .set_buttons(rfd::MessageButtons::Ok)
        .set_level(rfd::MessageLevel::Error)
        .show();
}

fn resolve_qpdf_command() -> PathBuf {
    let filename = qpdf_filename();
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let candidate = exe_dir.join(filename);
            if candidate.exists() {
                return candidate;
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join(filename);
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from(filename)
}

fn qpdf_filename() -> &'static str {
    if cfg!(target_os = "windows") {
        "qpdf.exe"
    } else {
        "qpdf"
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(Vec2::new(WINDOW_WIDTH, WINDOW_HEIGHT_BASE))
            .with_resizable(false),
        ..Default::default()
    };

    eframe::run_native(
        "CrackLeaf",
        options,
        Box::new(|cc| Ok(Box::new(CrackLeafApp::new(cc)))),
    )
}
