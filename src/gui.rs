#![cfg(windows)]

use crate::{config::Config, discord::DiscordClient, fs::OublietteFs, store::Store};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tokio::runtime::Runtime;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

const WINFSP_SEARCH_PATHS: &[&str] = &[
    r"C:\Program Files (x86)\WinFsp\bin\winfsp-x64.dll",
    r"C:\Program Files\WinFsp\bin\winfsp-x64.dll",
];

pub fn run(runtime: Arc<Runtime>, cfg_path: PathBuf) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([720.0, 580.0])
            .with_min_inner_size([560.0, 440.0])
            .with_title("Oubliette"),
        ..Default::default()
    };
    eframe::run_native(
        "Oubliette",
        options,
        Box::new(move |_cc| Ok(Box::new(App::new(runtime, cfg_path)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))
}

#[derive(PartialEq, Clone, Copy)]
enum Screen {
    Welcome,
    WinFspCheck,
    BotInstructions,
    TokenEntry,
    ServerEntry,
    ConfirmInit,
    Done,
    Controls,
    Stats,
}

struct App {
    runtime: Arc<Runtime>,
    cfg_path: PathBuf,
    screen: Screen,
    token: String,
    guild_id_text: String,
    show_token: bool,
    error: Option<String>,
    busy: Option<String>,
    winfsp_path: Option<PathBuf>,
    config: Option<Config>,
    bot_name: Option<String>,
    auth_result: Arc<Mutex<Option<Result<(String, u64), String>>>>,
    init_result: Arc<Mutex<Option<Result<Config, String>>>>,
    mount: Option<MountState>,
    uploads: Vec<UploadJob>,
    stats: Arc<Mutex<Option<StatsSnapshot>>>,
    stats_refreshing: bool,
    #[allow(dead_code)]
    tray: Option<TrayIcon>,
    tray_ids: TrayMenuIds,
    quit_requested: bool,
}

#[derive(Default, Clone)]
struct TrayMenuIds {
    show: String,
    mount: String,
    unmount: String,
    open_z: String,
    quit: String,
}

struct MountState {
    stop_tx: std::sync::mpsc::Sender<()>,
    join: Option<JoinHandle<()>>,
    status: Arc<Mutex<MountStatus>>,
    mountpoint: String,
}

#[derive(Clone, Default)]
struct MountStatus {
    state: String,
    error: Option<String>,
    ready: bool,
    ended: bool,
}

struct UploadJob {
    name: String,
    total: u64,
    progress: Arc<AtomicU64>,
    state: Arc<Mutex<UploadState>>,
}

#[derive(Clone, Default)]
enum UploadState {
    #[default]
    Pending,
    Running,
    Done,
    Failed(String),
}

#[derive(Clone, Default)]
struct StatsSnapshot {
    total_files: u64,
    total_bytes: u64,
    cache_inodes: u64,
    cache_chunks: u64,
    cache_bytes: u64,
}

impl App {
    fn new(runtime: Arc<Runtime>, cfg_path: PathBuf) -> Self {
        let existing = Config::load(&cfg_path).ok();
        let screen = if existing.is_some() {
            Screen::Controls
        } else {
            Screen::Welcome
        };
        let (tray, tray_ids) = build_tray();
        App {
            runtime,
            cfg_path,
            screen,
            token: String::new(),
            guild_id_text: String::new(),
            show_token: false,
            error: None,
            busy: None,
            winfsp_path: detect_winfsp(),
            config: existing,
            bot_name: None,
            auth_result: Arc::new(Mutex::new(None)),
            init_result: Arc::new(Mutex::new(None)),
            mount: None,
            uploads: Vec::new(),
            stats: Arc::new(Mutex::new(None)),
            stats_refreshing: false,
            tray,
            tray_ids,
            quit_requested: false,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_tray_events(ctx);
        self.handle_close_button(ctx);
        self.poll_background(ctx);
        self.handle_dropped_files(ctx);
        self.poll_uploads(ctx);

        egui::TopBottomPanel::top("hdr").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading("Oubliette");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.config.is_some() {
                        ui.selectable_value(&mut self.screen, Screen::Stats, "Stats");
                        ui.selectable_value(&mut self.screen, Screen::Controls, "Controls");
                    } else {
                        ui.label(egui::RichText::new("First-time setup").weak());
                    }
                });
            });
            ui.add_space(6.0);
            ui.separator();
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(16.0);
            match self.screen {
                Screen::Welcome => self.draw_welcome(ui),
                Screen::WinFspCheck => self.draw_winfsp(ui),
                Screen::BotInstructions => self.draw_bot_instructions(ui),
                Screen::TokenEntry => self.draw_token_entry(ui),
                Screen::ServerEntry => self.draw_server_entry(ui),
                Screen::ConfirmInit => self.draw_confirm_init(ui),
                Screen::Done => self.draw_done(ui),
                Screen::Controls => self.draw_controls(ui, ctx),
                Screen::Stats => self.draw_stats(ui, ctx),
            }

            if let Some(err) = self.error.clone() {
                ui.add_space(12.0);
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), format!("⚠  {err}"));
            }
            if let Some(msg) = self.busy.clone() {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(msg);
                });
            }
        });
    }
}

impl App {
    fn handle_tray_events(&mut self, ctx: &egui::Context) {
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id.0.clone();
            if id == self.tray_ids.show {
                self.show_window(ctx);
            } else if id == self.tray_ids.mount {
                if self.mount.is_none() && self.config.is_some() {
                    self.start_mount(ctx);
                }
            } else if id == self.tray_ids.unmount {
                self.stop_mount();
            } else if id == self.tray_ids.open_z {
                let _ = std::process::Command::new("explorer").arg("Z:\\").spawn();
            } else if id == self.tray_ids.quit {
                if self.mount.is_some() {
                    self.stop_mount();
                }
                self.quit_requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn handle_close_button(&mut self, ctx: &egui::Context) {
        if self.quit_requested {
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        if self.busy.is_some() {
            let mut auth = self.auth_result.lock().unwrap();
            if let Some(r) = auth.take() {
                drop(auth);
                self.busy = None;
                match r {
                    Ok((name, _gid)) => {
                        self.bot_name = Some(name);
                        self.error = None;
                        self.screen = Screen::ConfirmInit;
                    }
                    Err(e) => self.error = Some(e),
                }
            } else {
                drop(auth);
                let mut init = self.init_result.lock().unwrap();
                if let Some(r) = init.take() {
                    drop(init);
                    self.busy = None;
                    match r {
                        Ok(cfg) => {
                            if let Err(e) = cfg.save(&self.cfg_path) {
                                self.error = Some(format!("save config: {e}"));
                            } else {
                                self.config = Some(cfg);
                                self.error = None;
                                let _ = copy_winfsp_dll(self.winfsp_path.as_deref());
                                self.screen = Screen::Done;
                            }
                        }
                        Err(e) => self.error = Some(e),
                    }
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_millis(150));
                }
            }
        }

        if let Some(m) = self.mount.as_ref() {
            let st = m.status.lock().unwrap().clone();
            if st.ended {
                drop(st);
                let mut taken = self.mount.take().unwrap();
                if let Some(j) = taken.join.take() {
                    let _ = j.join();
                }
            }
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|d| d.path.clone())
                .collect()
        });
        for path in dropped {
            if !path.is_file() {
                continue;
            }
            let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
            let total = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let progress = Arc::new(AtomicU64::new(0));
            let state = Arc::new(Mutex::new(UploadState::Running));

            let job = UploadJob {
                name: name.clone(),
                total,
                progress: progress.clone(),
                state: state.clone(),
            };
            self.uploads.push(job);

            let cfg = self.config.clone();
            let runtime = self.runtime.clone();
            let progress_clone = progress.clone();
            let state_clone = state.clone();
            let ctx_clone = ctx.clone();
            std::thread::spawn(move || {
                let Some(cfg) = cfg else {
                    *state_clone.lock().unwrap() =
                        UploadState::Failed("no config loaded".into());
                    ctx_clone.request_repaint();
                    return;
                };
                let result = runtime.block_on(async move {
                    let store = Store::open(cfg)?;
                    store
                        .put_file_with_progress(&path, &format!("/{name}"), progress_clone)
                        .await
                });
                match result {
                    Ok(_) => *state_clone.lock().unwrap() = UploadState::Done,
                    Err(e) => *state_clone.lock().unwrap() = UploadState::Failed(e.to_string()),
                }
                ctx_clone.request_repaint();
            });
        }
    }

    fn poll_uploads(&mut self, ctx: &egui::Context) {
        if !self.uploads.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        // Remove jobs that have been Done for a while? Keep them visible for now.
    }

    // ── screen draws ─────────────────────────────────────────────────────────

    fn draw_welcome(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Welcome").size(24.0).strong());
        ui.add_space(8.0);
        ui.label("This wizard turns a Discord server you own into an encrypted Windows drive.");
        ui.label("Files copied in get chunked, encrypted with AES-256, and uploaded as Discord");
        ui.label("messages. Files copied out get downloaded and decrypted on the fly.");
        ui.add_space(8.0);
        ui.label("Setup takes about 2 minutes.");
        ui.add_space(20.0);
        if big_button(ui, "Get Started ▶").clicked() {
            self.screen = Screen::WinFspCheck;
            self.error = None;
        }
    }

    fn draw_winfsp(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Step 1 of 4 — WinFSP").size(20.0).strong());
        ui.add_space(8.0);
        match &self.winfsp_path {
            Some(p) => {
                ui.colored_label(egui::Color32::from_rgb(80, 200, 120), "✓ WinFSP is installed.");
                ui.label(format!("Found at: {}", p.display()));
                ui.add_space(20.0);
                if big_button(ui, "Continue ▶").clicked() {
                    self.screen = Screen::BotInstructions;
                    self.error = None;
                }
            }
            None => {
                ui.colored_label(egui::Color32::from_rgb(220, 180, 60), "WinFSP isn't installed yet.");
                ui.add_space(6.0);
                ui.label("WinFSP is a tiny kernel driver (~5 MB, MIT-licensed) that lets the");
                ui.label("oubliette mount as a real Windows drive letter.");
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if ui.button("Open winfsp.dev in browser").clicked() {
                        open_url("https://winfsp.dev");
                    }
                    if ui.button("Re-check after installing").clicked() {
                        self.winfsp_path = detect_winfsp();
                        if self.winfsp_path.is_none() {
                            self.error = Some("Still not found. Did the installer finish?".into());
                        } else {
                            self.error = None;
                        }
                    }
                });
            }
        }
    }

    fn draw_bot_instructions(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Step 2 of 4 — Create a Discord Bot").size(20.0).strong());
        ui.add_space(8.0);
        ui.label("Open the Discord Developer Portal and create a bot:");
        ui.add_space(6.0);
        for (i, line) in [
            "Click \"New Application\". Give it any name.",
            "Accept the developer terms.",
            "In the left sidebar, click \"Bot\".",
            "Click \"Reset Token\" → \"Yes, do it\".",
            "Click \"Copy\" to copy the token.",
        ]
        .iter()
        .enumerate()
        {
            ui.label(format!("    {}. {}", i + 1, line));
        }
        ui.add_space(8.0);
        ui.colored_label(
            egui::Color32::from_rgb(220, 180, 60),
            "⚠  The token is a password. Don't share it. Don't post it online.",
        );
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            if ui.button("Open Discord Dev Portal").clicked() {
                open_url("https://discord.com/developers/applications");
            }
            if big_button(ui, "I have my token ▶").clicked() {
                self.screen = Screen::TokenEntry;
                self.error = None;
            }
        });
    }

    fn draw_token_entry(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Step 2 — Paste your bot token").size(20.0).strong());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let mut tf = egui::TextEdit::singleline(&mut self.token)
                .hint_text("MTUw...XYZ.abc.123…")
                .desired_width(440.0);
            if !self.show_token {
                tf = tf.password(true);
            }
            ui.add(tf);
            ui.checkbox(&mut self.show_token, "show");
        });
        ui.add_space(12.0);
        ui.label("Then invite your bot to a private Discord server you own:");
        for (i, line) in [
            "In the dev portal, click \"OAuth2\" → \"URL Generator\".",
            "Under \"Scopes\", tick \"bot\".",
            "Under \"Bot Permissions\", tick: Manage Channels, Send Messages,",
            "    Manage Messages, Read Message History, Attach Files, View Channels.",
            "Copy the URL at the bottom, open it in your browser.",
            "Pick your server (or create a fresh one) and Authorize.",
        ]
        .iter()
        .enumerate()
        {
            ui.label(format!("    {}. {}", i + 7, line));
        }
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            if ui.button("◀ Back").clicked() {
                self.screen = Screen::BotInstructions;
                self.error = None;
            }
            let ready = self.token.trim().len() >= 50 && self.token.contains('.');
            if ui
                .add_enabled(ready, egui::Button::new("Continue ▶").min_size([120.0, 28.0].into()))
                .clicked()
            {
                self.screen = Screen::ServerEntry;
                self.error = None;
            }
        });
    }

    fn draw_server_entry(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Step 3 of 4 — Server ID").size(20.0).strong());
        ui.add_space(8.0);
        ui.label("In Discord:");
        ui.label("    1. User Settings → Advanced → Developer Mode = ON");
        ui.label("    2. Right-click your server icon → Copy Server ID");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Server ID:");
            ui.add(
                egui::TextEdit::singleline(&mut self.guild_id_text)
                    .hint_text("1234567890123456789")
                    .desired_width(280.0),
            );
        });
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            if ui.button("◀ Back").clicked() {
                self.screen = Screen::TokenEntry;
                self.error = None;
            }
            let valid = self.guild_id_text.trim().parse::<u64>().is_ok();
            if ui
                .add_enabled(
                    valid && self.busy.is_none(),
                    egui::Button::new("Verify with Discord ▶").min_size([180.0, 28.0].into()),
                )
                .clicked()
            {
                self.start_auth();
            }
        });
    }

    fn draw_confirm_init(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Step 4 of 4 — Create channels").size(20.0).strong());
        ui.add_space(8.0);
        ui.colored_label(
            egui::Color32::from_rgb(80, 200, 120),
            format!(
                "✓ Authenticated as bot: {}",
                self.bot_name.as_deref().unwrap_or("?")
            ),
        );
        ui.add_space(10.0);
        ui.label("Ready to create these channels in your Discord server:");
        ui.add_space(4.0);
        ui.label("    • category \"oubliette\"");
        ui.label("    • #fs-metadata");
        ui.label("    • #fs-data-0 through #fs-data-3");
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            if ui.button("◀ Back").clicked() {
                self.screen = Screen::ServerEntry;
                self.error = None;
            }
            if ui
                .add_enabled(
                    self.busy.is_none(),
                    egui::Button::new("Create channels ▶").min_size([180.0, 28.0].into()),
                )
                .clicked()
            {
                self.start_init();
            }
        });
    }

    fn draw_done(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("All set!")
                .size(24.0)
                .strong()
                .color(egui::Color32::from_rgb(80, 200, 120)),
        );
        ui.add_space(10.0);
        ui.label("Your oubliette is ready. The config was saved to:");
        ui.label(format!("    {}", self.cfg_path.display()));
        ui.add_space(8.0);
        ui.label("Click below to open the mount controls.");
        ui.add_space(20.0);
        if big_button(ui, "Open controls ▶").clicked() {
            self.screen = Screen::Controls;
            self.error = None;
        }
    }

    fn draw_controls(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(egui::RichText::new("Mount Controls").size(22.0).strong());
        ui.add_space(12.0);

        match &self.mount {
            None => {
                ui.colored_label(egui::Color32::from_rgb(160, 160, 160), "● not mounted");
            }
            Some(m) => {
                let st = m.status.lock().unwrap().clone();
                let color = if st.ready {
                    egui::Color32::from_rgb(80, 200, 120)
                } else if st.error.is_some() {
                    egui::Color32::from_rgb(220, 80, 80)
                } else {
                    egui::Color32::from_rgb(220, 180, 60)
                };
                ui.colored_label(color, format!("● {}", st.state));
                if let Some(e) = st.error {
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), format!("error: {e}"));
                }
                if st.ready {
                    ui.label(format!("Drive: {}", m.mountpoint));
                }
            }
        }

        ui.add_space(16.0);
        ui.horizontal(|ui| {
            let mount_enabled = self.mount.is_none() && self.config.is_some();
            if ui
                .add_enabled(
                    mount_enabled,
                    egui::Button::new("Mount Z:").min_size([130.0, 32.0].into()),
                )
                .clicked()
            {
                self.start_mount(ctx);
            }
            let unmount_enabled = self
                .mount
                .as_ref()
                .map(|m| m.status.lock().unwrap().ready)
                .unwrap_or(false);
            if ui
                .add_enabled(
                    unmount_enabled,
                    egui::Button::new("Unmount").min_size([100.0, 32.0].into()),
                )
                .clicked()
            {
                self.stop_mount();
            }
            if ui
                .add_enabled(
                    unmount_enabled,
                    egui::Button::new("Open Z:\\").min_size([100.0, 32.0].into()),
                )
                .clicked()
            {
                let _ = std::process::Command::new("explorer").arg("Z:\\").spawn();
            }
        });

        ui.add_space(24.0);
        ui.separator();
        ui.add_space(12.0);

        ui.label(egui::RichText::new("Drop zone").size(18.0).strong());
        ui.add_space(6.0);
        ui.label("Drag files onto this window to upload them straight to the root of your oubliette.");
        ui.label("Files will be encrypted and chunked into Discord regardless of whether the drive is mounted.");
        ui.add_space(8.0);

        let drop_rect = ui.allocate_space([ui.available_width(), 90.0].into()).1;
        let painter = ui.painter_at(drop_rect);
        painter.rect_stroke(
            drop_rect,
            8.0,
            egui::Stroke::new(2.0, egui::Color32::from_gray(100)),
        );
        painter.text(
            drop_rect.center(),
            egui::Align2::CENTER_CENTER,
            "⤓  drop files here  ⤓",
            egui::FontId::proportional(16.0),
            egui::Color32::from_gray(160),
        );

        if !self.uploads.is_empty() {
            ui.add_space(16.0);
            ui.label(egui::RichText::new("Uploads").size(16.0).strong());
            ui.add_space(4.0);
            for job in &self.uploads {
                let state = job.state.lock().unwrap().clone();
                let bytes = job.progress.load(Ordering::Relaxed);
                let frac = if job.total > 0 {
                    (bytes as f32 / job.total as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                ui.horizontal(|ui| {
                    ui.label(format!("{}  ({})", job.name, fmt_bytes(job.total)));
                    match state {
                        UploadState::Pending => {
                            ui.label("queued…");
                        }
                        UploadState::Running => {
                            ui.add(egui::ProgressBar::new(frac).desired_width(200.0).show_percentage());
                        }
                        UploadState::Done => {
                            ui.colored_label(egui::Color32::from_rgb(80, 200, 120), "✓ done");
                        }
                        UploadState::Failed(e) => {
                            ui.colored_label(egui::Color32::from_rgb(220, 80, 80), format!("✗ {e}"));
                        }
                    }
                });
            }
            if ui.small_button("Clear list").clicked() {
                self.uploads
                    .retain(|j| matches!(*j.state.lock().unwrap(), UploadState::Running | UploadState::Pending));
            }
        }
    }

    fn draw_stats(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(egui::RichText::new("Stats").size(22.0).strong());
        ui.add_space(8.0);

        let snap = self.stats.lock().unwrap().clone();
        match snap {
            None => {
                ui.label("No stats loaded yet.");
                ui.add_space(8.0);
                if !self.stats_refreshing && ui.button("Refresh from Discord").clicked() {
                    self.refresh_stats(ctx);
                } else if self.stats_refreshing {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Counting…");
                    });
                }
            }
            Some(s) => {
                ui.label(format!("Files in root  : {}", s.total_files));
                ui.label(format!("Total bytes    : {} ({})", s.total_bytes, fmt_bytes(s.total_bytes)));
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Local cache").strong());
                ui.label(format!("Inodes cached  : {}", s.cache_inodes));
                ui.label(format!(
                    "Chunks cached  : {} ({})",
                    s.cache_chunks,
                    fmt_bytes(s.cache_bytes)
                ));
                ui.add_space(12.0);
                if let Some(cfg) = &self.config {
                    ui.label(egui::RichText::new("Configuration").strong());
                    ui.label(format!("Guild ID       : {}", cfg.guild_id));
                    ui.label(format!("Data channels  : {}", cfg.data_channel_ids.len()));
                    ui.label(format!("Chunk target   : {} ({})", cfg.chunk_target, fmt_bytes(cfg.chunk_target as u64)));
                    let key_fp = hex::encode(&cfg.master_key[..4]);
                    ui.label(format!("Master key fp  : {key_fp}… (first 4 bytes)"));
                }
                ui.add_space(12.0);
                if !self.stats_refreshing && ui.button("Refresh").clicked() {
                    self.refresh_stats(ctx);
                } else if self.stats_refreshing {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Refreshing…");
                    });
                }
            }
        }
    }

    // ── action triggers ─────────────────────────────────────────────────────

    fn start_auth(&mut self) {
        self.error = None;
        let token = self.token.trim().to_string();
        let Ok(guild_id) = self.guild_id_text.trim().parse::<u64>() else {
            self.error = Some("Server ID isn't a number".into());
            return;
        };
        let runtime = self.runtime.clone();
        let result = self.auth_result.clone();
        *result.lock().unwrap() = None;
        std::thread::spawn(move || {
            let disc = DiscordClient::new(&token, guild_id);
            let r = runtime.block_on(disc.verify_token());
            let out = r
                .map(|n| (n, guild_id))
                .map_err(|e| format!(
                    "Could not authenticate. Check the token was copied fully and that the bot was invited to your server.\n\n({e})"
                ));
            *result.lock().unwrap() = Some(out);
        });
        self.busy = Some("Connecting to Discord…".into());
    }

    fn start_init(&mut self) {
        self.error = None;
        let token = self.token.trim().to_string();
        let Ok(guild_id) = self.guild_id_text.trim().parse::<u64>() else {
            self.error = Some("Server ID isn't a number".into());
            return;
        };
        let runtime = self.runtime.clone();
        let result = self.init_result.clone();
        *result.lock().unwrap() = None;
        std::thread::spawn(move || {
            let r = runtime.block_on(Store::init(token, guild_id, 4));
            let out = r.map_err(|e| format!("Channel creation failed: {e}"));
            *result.lock().unwrap() = Some(out);
        });
        self.busy = Some("Creating channels in your Discord server…".into());
    }

    fn start_mount(&mut self, ctx: &egui::Context) {
        let Some(cfg) = self.config.clone() else {
            self.error = Some("No config loaded".into());
            return;
        };
        let runtime = self.runtime.clone();
        let status = Arc::new(Mutex::new(MountStatus {
            state: "Initializing…".into(),
            ..Default::default()
        }));
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let st_for_thread = status.clone();
        let ctx_for_thread = ctx.clone();
        let mountpoint = "Z:".to_string();
        let mp_for_thread = mountpoint.clone();

        let join = std::thread::spawn(move || {
            let result = run_mount(runtime, cfg, &mp_for_thread, st_for_thread.clone(), stop_rx);
            if let Err(e) = result {
                let mut s = st_for_thread.lock().unwrap();
                s.error = Some(e.to_string());
                s.state = "Mount failed".into();
            }
            st_for_thread.lock().unwrap().ended = true;
            ctx_for_thread.request_repaint();
        });

        self.mount = Some(MountState {
            stop_tx,
            join: Some(join),
            status,
            mountpoint,
        });
    }

    fn stop_mount(&mut self) {
        if let Some(m) = self.mount.as_ref() {
            let _ = m.stop_tx.send(());
        }
    }

    fn refresh_stats(&mut self, ctx: &egui::Context) {
        let Some(cfg) = self.config.clone() else {
            return;
        };
        let runtime = self.runtime.clone();
        let stats = self.stats.clone();
        let ctx = ctx.clone();
        self.stats_refreshing = true;
        std::thread::spawn(move || {
            let snap = runtime.block_on(async {
                let store = Store::open(cfg).ok()?;
                let entries = store.list("/").await.ok()?;
                let mut total_files = 0;
                let mut total_bytes = 0u64;
                for e in entries {
                    if let crate::inode::Inode::File { size, .. } = e {
                        total_files += 1;
                        total_bytes += size;
                    }
                }
                let (ic, _ib, cc, cb) = store.cache.stats().ok()?;
                Some(StatsSnapshot {
                    total_files,
                    total_bytes,
                    cache_inodes: ic,
                    cache_chunks: cc,
                    cache_bytes: cb,
                })
            });
            *stats.lock().unwrap() = snap;
            ctx.request_repaint();
        });
    }
}

fn run_mount(
    runtime: Arc<Runtime>,
    cfg: Config,
    mountpoint: &str,
    status: Arc<Mutex<MountStatus>>,
    stop_rx: std::sync::mpsc::Receiver<()>,
) -> anyhow::Result<()> {
    use winfsp::host::{FileSystemHost, FineGuard, VolumeParams};

    let store = Arc::new(Store::open(cfg)?);
    let _init = winfsp::winfsp_init_or_die();

    let mut params = VolumeParams::new();
    params
        .sector_size(4096)
        .sectors_per_allocation_unit(1)
        .max_component_length(255)
        .file_info_timeout(60_000)
        .case_sensitive_search(true)
        .case_preserved_names(true)
        .unicode_on_disk(true)
        .filesystem_name("oubliette");

    let fs = OublietteFs {
        store,
        runtime: runtime.clone(),
    };

    status.lock().unwrap().state = "Constructing host…".into();
    let mut host: FileSystemHost<OublietteFs, FineGuard> =
        FileSystemHost::new(params, fs).map_err(|e| anyhow::anyhow!("FileSystemHost::new: {e:?}"))?;

    status.lock().unwrap().state = "Mounting…".into();
    host.mount(mountpoint).map_err(|e| anyhow::anyhow!("mount: {e:?}"))?;

    status.lock().unwrap().state = "Starting dispatcher…".into();
    host.start().map_err(|e| anyhow::anyhow!("start: {e:?}"))?;

    {
        let mut s = status.lock().unwrap();
        s.state = format!("Mounted at {mountpoint}");
        s.ready = true;
    }

    let _ = stop_rx.recv();

    {
        let mut s = status.lock().unwrap();
        s.state = "Unmounting…".into();
        s.ready = false;
    }
    host.stop();
    host.unmount();
    Ok(())
}

fn build_tray() -> (Option<TrayIcon>, TrayMenuIds) {
    let menu = Menu::new();
    let show = MenuItem::new("Show window", true, None);
    let mount = MenuItem::new("Mount Z:", true, None);
    let unmount = MenuItem::new("Unmount", true, None);
    let open_z = MenuItem::new("Open Z:\\", true, None);
    let quit = MenuItem::new("Quit", true, None);

    let ids = TrayMenuIds {
        show: show.id().0.clone(),
        mount: mount.id().0.clone(),
        unmount: unmount.id().0.clone(),
        open_z: open_z.id().0.clone(),
        quit: quit.id().0.clone(),
    };

    let _ = menu.append(&show);
    let _ = menu.append(&mount);
    let _ = menu.append(&unmount);
    let _ = menu.append(&open_z);
    let _ = menu.append(&tray_icon::menu::PredefinedMenuItem::separator());
    let _ = menu.append(&quit);

    let icon = make_icon();
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Oubliette")
        .with_icon(icon)
        .build()
        .ok();

    (tray, ids)
}

fn make_icon() -> tray_icon::Icon {
    let size: i32 = 32;
    let mut data = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            let dx = x - size / 2;
            let dy = y - size / 2;
            let d2 = dx * dx + dy * dy;
            let outer = (size / 2 - 2).pow(2);
            let inner = (size / 4).pow(2);
            if d2 <= outer && d2 >= inner {
                data[i] = 70;
                data[i + 1] = 140;
                data[i + 2] = 220;
                data[i + 3] = 255;
            } else if d2 < outer {
                data[i] = 20;
                data[i + 1] = 24;
                data[i + 2] = 36;
                data[i + 3] = 255;
            } else {
                data[i] = 0;
                data[i + 1] = 0;
                data[i + 2] = 0;
                data[i + 3] = 0;
            }
        }
    }
    tray_icon::Icon::from_rgba(data, size as u32, size as u32)
        .expect("valid icon bytes")
}

fn detect_winfsp() -> Option<PathBuf> {
    WINFSP_SEARCH_PATHS
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

fn copy_winfsp_dll(src: Option<&Path>) -> std::io::Result<()> {
    let Some(src) = src else {
        return Ok(());
    };
    let exe_dir = std::env::current_exe()?
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent"))?
        .to_path_buf();
    let dest = exe_dir.join("winfsp-x64.dll");
    if !dest.exists() {
        std::fs::copy(src, &dest)?;
    }
    Ok(())
}

fn open_url(url: &str) {
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", "", url])
        .spawn();
}

fn big_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(egui::Button::new(label).min_size([160.0, 32.0].into()))
}

fn fmt_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}
