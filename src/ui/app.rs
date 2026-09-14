//! eframe / egui Desktop GUI Review Application.
//!
//! Provides the interactive review screen specified in Phase 8:
//! - Frame navigation header with roll profile metadata.
//! - Aspect-fit positive preview canvas with interactive click-drag selection rectangle.
//! - Instant re-rendering with LittleCMS 2 color management and Negadoctor inversion.
//! - Toolbar with rotation controls and scope options (this frame, remaining, whole strip).
//! - Collapsible advanced drawer for parameter inspection and manual slider adjustments.

use egui::{
    Align, Align2, Button, CentralPanel, Color32, ColorImage, CornerRadius, FontId, Layout, Panel,
    Pos2, Rect, RichText, ScrollArea, Slider, Stroke, StrokeKind, TextureHandle, TextureOptions,
    Ui, vec2,
};

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::review::{ReviewSession, SelectionStatus};
use crate::processing::RollProfile;
use crate::processing::analysis::SampleRect;
use crate::processing::orientation::OrientationScope;
use crate::scanner::types::{FrameSelection, ScanEvent, ScanRequest, StripDiscovery};
use crate::scanner::worker::{ScanCommand, ScannerWorkerHandle, WorkerMessage};

/// Scanner quality preset for multi-sampling and fidelity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityPreset {
    Standard, // 1x pass
    Fine,     // 4x passes
    Ultimate, // 16x passes
    Custom,   // user-defined sample count
}

/// GUI Scan configuration state covering both Basic and Advanced scanning controls.
#[derive(Debug, Clone)]
pub struct ScanSetupState {
    // Basic Controls
    pub resolution_dpi: u16,
    pub dust_removal: bool,
    pub quality: QualityPreset,
    pub output_dir_str: String,
    pub film_stock_index: usize,
    pub roll_profile: RollProfile,

    // Advanced Controls
    pub show_setup_modal: bool,
    pub show_advanced: bool,
    pub samples: u8,
    pub all_frames: bool,
    pub frame_selected: [bool; 6],
    pub auto_crop: bool,
    pub scanner_offset_mm: f64,
    pub last_discovery: Option<StripDiscovery>,
}

impl Default for ScanSetupState {
    fn default() -> Self {
        Self {
            resolution_dpi: 2900,
            dust_removal: true,
            quality: QualityPreset::Standard,
            output_dir_str: "./scans".into(),
            film_stock_index: 0,
            roll_profile: RollProfile::pro_image_100(),
            show_setup_modal: false,
            show_advanced: false,
            samples: 1,
            all_frames: true,
            frame_selected: [true; 6],
            auto_crop: true,
            scanner_offset_mm: 0.0,
            last_discovery: None,
        }
    }
}

impl ScanSetupState {
    /// Creates a scan setup configured for a specific roll profile.
    pub fn for_roll(roll: RollProfile) -> Self {
        let film_stock_index = match roll.id.0.as_str() {
            "kodak-pro-image-100" => 0,
            "kodak-portra-400" => 1,
            "kodak-gold-200" => 2,
            _ => 3,
        };
        Self {
            film_stock_index,
            roll_profile: roll,
            ..Self::default()
        }
    }

    /// Updates the selected preset film stock index and updates the internal roll profile.
    pub fn set_film_stock_index(&mut self, index: usize) {
        self.film_stock_index = index;
        self.roll_profile = match index {
            0 => RollProfile::pro_image_100(),
            1 => RollProfile::portra_400(),
            2 => RollProfile::gold_200(),
            _ => self.roll_profile.clone(),
        };
    }

    /// Returns the active roll profile for scanning.
    pub fn selected_roll(&self) -> RollProfile {
        self.roll_profile.clone()
    }
}

/// The desktop review application state.
pub struct ReviewApp {
    /// Active multi-frame review session.
    pub session: ReviewSession,
    /// Cached egui texture handle for the active preview.
    preview_texture: Option<(usize, TextureHandle)>,
    /// Active drag rectangle start point in UI canvas coordinates.
    drag_start: Option<Pos2>,
    /// Current mouse drag position in UI canvas coordinates.
    drag_current: Option<Pos2>,
    /// Image display rectangle on canvas during the last frame paint.
    last_image_rect: Option<Rect>,
    /// Optional background scanner worker handle.
    pub worker: Option<ScannerWorkerHandle>,
    /// Whether a scan operation is currently running.
    pub is_scanning: bool,
    /// Current scan progress fraction in [0.0, 1.0].
    pub scan_progress: f32,
    /// Live scanner status message.
    pub status_message: String,
    /// Destination directory for saving TIFF and XMP sidecar files.
    pub output_dir: PathBuf,
    /// Timestamped save confirmation notification.
    pub save_notification: Option<(String, Instant)>,
    /// Live scanner setup and configuration options.
    pub scan_setup: ScanSetupState,
}

impl ReviewApp {
    /// Creates a new review GUI app from a review session.
    pub fn new(session: ReviewSession) -> Self {
        let scan_setup = ScanSetupState::for_roll(session.roll.clone());
        Self {
            scan_setup,
            session,
            preview_texture: None,
            drag_start: None,
            drag_current: None,
            last_image_rect: None,
            worker: None,
            is_scanning: false,
            scan_progress: 0.0,
            status_message: "Ready".into(),
            output_dir: PathBuf::from("./scans"),
            save_notification: None,
        }
    }

    /// Associates a background scanner worker with this review application.
    pub fn with_worker(mut self, worker: ScannerWorkerHandle) -> Self {
        self.worker = Some(worker);
        self
    }

    /// Configures the output directory for TIFF and XMP sidecar exports.
    pub fn with_output_dir(mut self, path: impl Into<PathBuf>) -> Self {
        let pb = path.into();
        self.scan_setup.output_dir_str = pb.to_string_lossy().to_string();
        self.output_dir = pb;
        self
    }

    /// Forces regeneration of the preview texture (e.g. after parameter or orientation change).
    pub fn invalidate_texture(&mut self) {
        if let Some(frame) = self.session.current_frame_mut() {
            frame.invalidate_preview();
        }
        self.preview_texture = None;
    }

    /// Polls messages from the background scanner worker and updates session state.
    pub fn poll_worker(&mut self) {
        let messages: Vec<WorkerMessage> = if let Some(worker) = &self.worker {
            let mut msgs = Vec::new();
            while let Some(msg) = worker.try_recv() {
                msgs.push(msg);
            }
            msgs
        } else {
            Vec::new()
        };

        for msg in messages {
            match msg {
                WorkerMessage::Event(evt) => match &evt {
                    ScanEvent::ScannerFound { description } => {
                        self.status_message = format!("Scanner: {description}");
                    }
                    ScanEvent::SessionReady => {
                        self.status_message = "Scanner ready".into();
                    }
                    ScanEvent::MediaChecked { loaded } => {
                        self.status_message = if *loaded {
                            "Film loaded".into()
                        } else {
                            "No film loaded".into()
                        };
                    }
                    ScanEvent::DiscoveryStarted { .. } => {
                        self.status_message = "Discovering strip boundaries...".into();
                    }
                    ScanEvent::DiscoveryCompleted { detected_count } => {
                        self.status_message =
                            format!("Discovered {detected_count} frames on strip");
                    }
                    ScanEvent::FrameStarted {
                        frame_number,
                        total_frames,
                        dpi,
                        ..
                    } => {
                        self.status_message = format!(
                            "Scanning Frame {frame_number} of {total_frames} ({dpi} DPI)..."
                        );
                        self.scan_progress = 0.0;
                        self.is_scanning = true;
                    }
                    ScanEvent::Progress { percent, .. } => {
                        self.scan_progress = (*percent as f32 / 100.0).clamp(0.0, 1.0);
                    }
                    ScanEvent::FrameCompleted { frame_number } => {
                        self.status_message = format!("Frame {frame_number} acquired");
                        self.scan_progress = 1.0;
                    }
                    _ => {}
                },
                WorkerMessage::FrameReady(prepared) => {
                    let was_empty = self.session.frames.is_empty();
                    if let Err(e) = self.session.add_prepared_frame(*prepared) {
                        self.status_message = format!("Frame processing error: {e}");
                    } else if was_empty {
                        self.session.current_index = 0;
                        self.invalidate_texture();
                    }
                }
                WorkerMessage::DiscoveryReady(discovery) => {
                    self.status_message = format!("Discovered {} frames", discovery.frames().len());
                    self.scan_setup.last_discovery = Some(discovery);
                }
                WorkerMessage::StripScanComplete => {
                    self.is_scanning = false;
                    if !self.session.frames.is_empty() {
                        self.status_message = "Scan complete — ready for review".into();
                    }
                }
                WorkerMessage::ScanCancelled { reason } => {
                    self.is_scanning = false;
                    self.status_message = format!("Scan stopped: {reason}");
                }
                WorkerMessage::FilmEjected => {
                    self.status_message = "Film ejected".into();
                    self.is_scanning = false;
                }
                WorkerMessage::Error(err) => {
                    self.status_message = format!("Scanner error: {err}");
                    self.is_scanning = false;
                }
            }
        }
    }

    /// Constructs a validated ScanRequest from the current GUI setup state.
    pub fn build_scan_request(&self) -> ScanRequest {
        let frames = if self.scan_setup.all_frames {
            FrameSelection::All
        } else {
            let list = self
                .scan_setup
                .frame_selected
                .iter()
                .enumerate()
                .filter_map(|(i, &selected)| selected.then_some(i + 1))
                .collect();

            FrameSelection::List(list)
        };

        let samples = match self.scan_setup.quality {
            QualityPreset::Standard => 1,
            QualityPreset::Fine => 4,
            QualityPreset::Ultimate => 16,
            QualityPreset::Custom => self.scan_setup.samples,
        };

        let roll = self.scan_setup.selected_roll();

        ScanRequest::new(
            frames,
            self.scan_setup.resolution_dpi,
            samples,
            self.scan_setup.dust_removal,
            self.scan_setup.auto_crop,
        )
        .with_roll(roll)
        .with_offset_mm(self.scan_setup.scanner_offset_mm)
    }

    /// Triggers a scan using the current GUI configuration settings.
    pub fn start_scan(&mut self) {
        let req = self.build_scan_request();
        if let Err(err) = req.validate() {
            self.status_message = format!("Invalid scan settings: {err}");
            return;
        }
        if let Some(roll) = &req.roll {
            if !roll.is_calibrated() {
                self.status_message = format!(
                    "Cannot scan: film stock '{}' is uncalibrated (D-min calibration required)",
                    roll.name
                );
                return;
            }
            self.session.roll = roll.clone();
        }
        if let Some(worker) = &self.worker {
            worker.send(ScanCommand::StartScan(Box::new(req)));
            self.is_scanning = true;
            self.status_message = "Starting scan...".into();
            self.scan_setup.show_setup_modal = false;
        } else {
            self.status_message = "Cannot start scan: no scanner connected".into();
        }
    }

    /// Renders the scanner setup modal dialog.
    pub fn render_scan_setup_window(&mut self, ctx: &egui::Context) {
        if !self.scan_setup.show_setup_modal {
            return;
        }

        let mut is_open = self.scan_setup.show_setup_modal;
        egui::Window::new("⚙ Scanner Setup & Controls")
            .open(&mut is_open)
            .resizable(true)
            .default_width(450.0)
            .show(ctx, |ui| {
                ui.heading("Basic Controls");
                ui.separator();

                // Resolution
                ui.horizontal(|ui| {
                    ui.label("Resolution:");
                    ui.radio_value(
                        &mut self.scan_setup.resolution_dpi,
                        725,
                        "725 DPI (Preview)",
                    );
                    ui.radio_value(&mut self.scan_setup.resolution_dpi, 1450, "1450 DPI");
                    ui.radio_value(
                        &mut self.scan_setup.resolution_dpi,
                        2900,
                        "2900 DPI (Native)",
                    );
                });

                // Dust removal
                ui.checkbox(
                    &mut self.scan_setup.dust_removal,
                    "Dust & Scratch Removal (OpenICE)",
                );

                // Quality preset
                ui.horizontal(|ui| {
                    ui.label("Quality Preset:");
                    if ui
                        .radio_value(
                            &mut self.scan_setup.quality,
                            QualityPreset::Standard,
                            "Standard (1x)",
                        )
                        .clicked()
                    {
                        self.scan_setup.samples = 1;
                    }
                    if ui
                        .radio_value(
                            &mut self.scan_setup.quality,
                            QualityPreset::Fine,
                            "Fine (4x)",
                        )
                        .clicked()
                    {
                        self.scan_setup.samples = 4;
                    }
                    if ui
                        .radio_value(
                            &mut self.scan_setup.quality,
                            QualityPreset::Ultimate,
                            "Ultimate (16x)",
                        )
                        .clicked()
                    {
                        self.scan_setup.samples = 16;
                    }
                });

                // Output folder
                ui.horizontal(|ui| {
                    ui.label("Output folder:");
                    if ui
                        .text_edit_singleline(&mut self.scan_setup.output_dir_str)
                        .changed()
                    {
                        self.output_dir = PathBuf::from(&self.scan_setup.output_dir_str);
                    }
                });

                // Film profile
                ui.horizontal(|ui| {
                    ui.label("Film Stock:");
                    let current_text = match self.scan_setup.film_stock_index {
                        0 => "Kodak Pro Image 100".to_string(),
                        1 => "Kodak Portra 400".to_string(),
                        2 => "Kodak Gold 200".to_string(),
                        _ => format!("Custom: {}", self.scan_setup.roll_profile.name),
                    };
                    egui::ComboBox::from_id_salt("film_stock_combo")
                        .selected_text(current_text)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(
                                    self.scan_setup.film_stock_index == 0,
                                    "Kodak Pro Image 100",
                                )
                                .clicked()
                            {
                                self.scan_setup.set_film_stock_index(0);
                                self.session.roll = self.scan_setup.selected_roll();
                            }
                            if ui
                                .selectable_label(
                                    self.scan_setup.film_stock_index == 1,
                                    "Kodak Portra 400",
                                )
                                .clicked()
                            {
                                self.scan_setup.set_film_stock_index(1);
                                self.session.roll = self.scan_setup.selected_roll();
                            }
                            if ui
                                .selectable_label(
                                    self.scan_setup.film_stock_index == 2,
                                    "Kodak Gold 200",
                                )
                                .clicked()
                            {
                                self.scan_setup.set_film_stock_index(2);
                                self.session.roll = self.scan_setup.selected_roll();
                            }
                            if self.scan_setup.film_stock_index > 2 {
                                let label =
                                    format!("Custom: {}", self.scan_setup.roll_profile.name);
                                let _ = ui.selectable_label(true, label);
                            }
                        });
                });

                let (status_text, status_color) = if self.scan_setup.selected_roll().is_calibrated()
                {
                    (
                        "Measured roll profile loaded",
                        Color32::from_rgb(120, 220, 120),
                    )
                } else {
                    (
                        "D-min calibration required",
                        Color32::from_rgb(255, 180, 100),
                    )
                };
                ui.horizontal(|ui| {
                    ui.label("Calibration:");
                    ui.colored_label(status_color, status_text);
                });

                ui.add_space(8.0);
                ui.collapsing("▶ Advanced Scanner Controls", |ui| {
                    // Samples slider
                    ui.horizontal(|ui| {
                        ui.label("Multi-sample count:");
                        if ui
                            .add(Slider::new(&mut self.scan_setup.samples, 1..=16).text("passes"))
                            .changed()
                        {
                            self.scan_setup.quality = QualityPreset::Custom;
                        }
                    });

                    // Auto-crop
                    ui.checkbox(
                        &mut self.scan_setup.auto_crop,
                        "Automatic edge detection & border crop",
                    );

                    // Travel offset (disabled until Phase 4 hardware offset integration)
                    ui.add_enabled_ui(false, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Scanner travel offset (unsupported):");
                            ui.add(
                                Slider::new(&mut self.scan_setup.scanner_offset_mm, -5.0..=5.0)
                                    .step_by(0.1)
                                    .suffix(" mm"),
                            );
                        });
                    });

                    // Manual frame selection
                    ui.add_space(4.0);
                    ui.label(RichText::new("Frame Selection:").strong());
                    ui.radio_value(
                        &mut self.scan_setup.all_frames,
                        true,
                        "Scan all discovered frames on strip",
                    );
                    ui.radio_value(
                        &mut self.scan_setup.all_frames,
                        false,
                        "Manual frame selection",
                    );
                    if !self.scan_setup.all_frames {
                        ui.horizontal(|ui| {
                            for f in 0..6 {
                                ui.checkbox(
                                    &mut self.scan_setup.frame_selected[f],
                                    format!("#{}", f + 1),
                                );
                            }
                        });
                    }

                    // System & Environment Diagnostics
                    ui.add_space(4.0);
                    ui.separator();
                    ui.label(RichText::new("Environment & Darktable:").strong());
                    if let Some(dt) = crate::diagnostics::detect_darktable_cli() {
                        ui.colored_label(
                            Color32::from_rgb(100, 240, 100),
                            format!("✓ darktable-cli detected ({})", dt.display()),
                        );
                    } else {
                        ui.colored_label(
                            Color32::from_rgb(240, 200, 100),
                            "Notice: darktable-cli not found in standard paths",
                        );
                    }
                    ui.small("Color engine: LittleCMS 2 (Linear Rec.2020 / sRGB)");

                    // Overscan Diagnostics
                    if let Some(d) = &self.scan_setup.last_discovery {
                        ui.add_space(4.0);
                        ui.separator();
                        ui.label(RichText::new("Strip Diagnostics:").strong());
                        ui.small(format!(
                            "Optical DPI: {} x {}",
                            d.optical_dpi.0, d.optical_dpi.1
                        ));
                        ui.small(format!(
                            "Discovered frames count: {}",
                            d.detected_frames.len()
                        ));
                        ui.small(format!("Framing method: {:?}", d.framing));
                    }
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let is_calibrated = self.scan_setup.selected_roll().is_calibrated();
                    let can_scan = self.worker.is_some() && is_calibrated;
                    let start_btn =
                        Button::new(RichText::new("▶ Start Scan").color(Color32::WHITE).strong())
                            .fill(Color32::from_rgb(40, 160, 60));

                    if ui.add_enabled(can_scan, start_btn).clicked() {
                        self.start_scan();
                    }
                    if self.worker.is_none() {
                        ui.colored_label(
                            Color32::from_rgb(200, 160, 100),
                            "(No scanner connected)",
                        );
                    } else if !is_calibrated {
                        ui.colored_label(
                            Color32::from_rgb(255, 180, 100),
                            "(D-min calibration required)",
                        );
                    }

                    if ui.button("Close").clicked() {
                        self.scan_setup.show_setup_modal = false;
                    }
                });
            });

        self.scan_setup.show_setup_modal = is_open;
    }
}

impl eframe::App for ReviewApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Process incoming background scanner messages
        self.poll_worker();

        // Render modal windows
        self.render_scan_setup_window(&ctx);

        // Handle keyboard navigation shortcuts
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) && self.session.prev_frame() {
            self.preview_texture = None;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) && self.session.next_frame() {
            self.preview_texture = None;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::R)) {
            self.session.rotate(self.session.orientation_scope, true);
            self.invalidate_texture();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::L)) {
            self.session.rotate(self.session.orientation_scope, false);
            self.invalidate_texture();
        }

        // Top Header Panel
        Panel::top("header_panel").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let has_prev = self.session.current_index > 0;
                let has_next = self.session.current_index + 1 < self.session.frame_count();

                if ui.add_enabled(has_prev, Button::new("◀ Prev")).clicked() {
                    self.session.prev_frame();
                    self.preview_texture = None;
                }

                let current_num = self
                    .session
                    .current_frame()
                    .map(|f| f.frame_number)
                    .unwrap_or(0);
                let current_idx = self.session.current_index + 1;
                let total = self.session.frame_count();

                if total > 1 {
                    ui.heading(format!("Frame #{current_num} ({current_idx}/{total})"));
                } else if total == 1 {
                    ui.heading(format!("Frame #{current_num}"));
                } else {
                    ui.heading("No Frames");
                }

                if ui.add_enabled(has_next, Button::new("Next ▶")).clicked() {
                    self.session.next_frame();
                    self.preview_texture = None;
                }

                ui.separator();

                ui.label(format!("Roll: {}", self.session.roll.id));
                ui.colored_label(
                    Color32::from_rgb(180, 220, 255),
                    &self.session.roll.film_stock.name,
                );
                if self.session.roll.is_calibrated() {
                    ui.colored_label(
                        Color32::from_rgb(120, 220, 120),
                        "Measured roll profile loaded",
                    );
                } else {
                    ui.colored_label(
                        Color32::from_rgb(255, 180, 100),
                        "D-min calibration required",
                    );
                }

                // Scanner Worker Live Controls
                if self.worker.is_some() {
                    ui.separator();
                    if self.is_scanning {
                        if ui
                            .button(
                                RichText::new("⏹ Cancel Frame")
                                    .color(Color32::from_rgb(255, 120, 120)),
                            )
                            .clicked()
                            && let Some(w) = &self.worker
                        {
                            w.cancel_current();
                        }
                        if ui.button("⏸ Stop After Frame").clicked()
                            && let Some(w) = &self.worker
                        {
                            w.stop_after_current();
                        }
                    } else {
                        let is_calibrated = self.scan_setup.selected_roll().is_calibrated();
                        let scan_btn =
                            Button::new(RichText::new("▶ Scan Strip").color(if is_calibrated {
                                Color32::from_rgb(120, 240, 120)
                            } else {
                                Color32::from_rgb(160, 160, 160)
                            }));
                        if ui.add_enabled(is_calibrated, scan_btn).clicked() {
                            self.start_scan();
                        }
                        if ui.button("⚙ Setup").clicked() {
                            self.scan_setup.show_setup_modal = true;
                        }
                        if ui.button("🔍 Discover").clicked() {
                            if let Some(w) = &self.worker {
                                w.send(ScanCommand::DiscoverStrip);
                            }
                            self.status_message = "Discovering strip...".into();
                        }
                        if ui.button("⏏ Eject").clicked()
                            && let Some(w) = &self.worker
                        {
                            w.eject();
                        }
                    }
                } else {
                    ui.separator();
                    ui.colored_label(Color32::from_rgb(200, 160, 100), "No scanner connected");
                    if ui.button("⚙ Setup").clicked() {
                        self.scan_setup.show_setup_modal = true;
                    }
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(frame) = self.session.current_frame() {
                        if frame.accepted {
                            ui.colored_label(Color32::from_rgb(100, 240, 100), "✓ ACCEPTED");
                        } else {
                            ui.colored_label(Color32::from_rgb(240, 200, 100), "Reviewing");
                        }
                    }
                });
            });

            // Live status text and scan progress bar
            if self.is_scanning || !self.status_message.is_empty() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if self.is_scanning {
                        ui.add(egui::ProgressBar::new(self.scan_progress).desired_width(140.0));
                    }
                    ui.label(
                        RichText::new(&self.status_message)
                            .size(12.0)
                            .color(Color32::from_rgb(200, 200, 200)),
                    );

                    if let Some((notification, inst)) = &self.save_notification
                        && inst.elapsed() < Duration::from_secs(4)
                    {
                        ui.separator();
                        ui.colored_label(
                            Color32::from_rgb(100, 255, 100),
                            format!("✓ {notification}"),
                        );
                    }
                });
            }

            ui.add_space(6.0);
        });

        // Bottom Controls Toolbar
        Panel::bottom("bottom_toolbar").show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("↶ Rotate Left").clicked() {
                    self.session.rotate(self.session.orientation_scope, false);
                    self.invalidate_texture();
                }
                if ui.button("180°").clicked() {
                    self.session.flip_180(self.session.orientation_scope);
                    self.invalidate_texture();
                }
                if ui.button("Rotate Right ↷").clicked() {
                    self.session.rotate(self.session.orientation_scope, true);
                    self.invalidate_texture();
                }

                ui.separator();

                ui.label("Apply to:");
                ui.radio_value(
                    &mut self.session.orientation_scope,
                    OrientationScope::CurrentFrame,
                    "this frame",
                );
                ui.radio_value(
                    &mut self.session.orientation_scope,
                    OrientationScope::RemainingFrames,
                    "remaining",
                );
                ui.radio_value(
                    &mut self.session.orientation_scope,
                    OrientationScope::WholeStrip,
                    "whole strip",
                );

                ui.separator();

                if ui.button("Reset WB").clicked() {
                    if let Some(frame) = self.session.current_frame_mut() {
                        frame.reset_highlight_wb();
                    }
                    self.invalidate_texture();
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let accept_btn = Button::new(RichText::new("Accept & Next ➡").strong())
                        .fill(Color32::from_rgb(40, 140, 60));

                    if ui.add(accept_btn).clicked() {
                        let frame_num = self
                            .session
                            .current_frame()
                            .map(|f| f.frame_number)
                            .unwrap_or(0);
                        match self.session.save_current_frame_and_xmp(&self.output_dir) {
                            Ok((tif, _xmp)) => {
                                let filename = tif
                                    .file_name()
                                    .map(|s| s.to_string_lossy().to_string())
                                    .unwrap_or_else(|| format!("frame-{frame_num}.tif"));
                                self.save_notification = Some((
                                    format!("Saved {filename} + Darktable XMP"),
                                    Instant::now(),
                                ));
                            }
                            Err(err) => {
                                self.save_notification =
                                    Some((format!("Save failed: {err}"), Instant::now()));
                            }
                        }
                        self.session.accept_and_next();
                        self.preview_texture = None;
                    }
                });
            });
            ui.add_space(8.0);
        });

        // Right Advanced Parameters Drawer
        let mut params_changed = false;

        Panel::right("advanced_drawer")
            .resizable(true)
            .default_size(280.0)
            .show(ui, |ui| {
                ui.add_space(8.0);
                ui.heading("Parameters");
                ui.separator();

                let roll_is_calibrated = self.session.roll.is_calibrated();
                if let Some(frame) = self.session.current_frame_mut() {
                    ScrollArea::vertical().show(ui, |ui| {
                        ui.collapsing("Film Substrate (D-min)", |ui| {
                            if roll_is_calibrated {
                                ui.label(format!(
                                    "R: {:.4}  G: {:.4}  B: {:.4}",
                                    frame.params.dmin[0],
                                    frame.params.dmin[1],
                                    frame.params.dmin[2]
                                ));
                                ui.colored_label(
                                    Color32::from_rgb(120, 220, 120),
                                    "Measured roll profile loaded",
                                );
                            } else {
                                ui.colored_label(
                                    Color32::from_rgb(255, 180, 100),
                                    "D-min calibration required",
                                );
                            }
                        });

                        ui.add_space(4.0);
                        ui.label(RichText::new("Negative Properties").strong());

                        ui.horizontal(|ui| {
                            ui.label("D max:");
                            if ui
                                .add(Slider::new(&mut frame.params.dmax, 0.5..=5.0).step_by(0.01))
                                .changed()
                            {
                                params_changed = true;
                            }
                        });

                        ui.horizontal(|ui| {
                            ui.label("Scan bias:");
                            if ui
                                .add(
                                    Slider::new(&mut frame.params.offset, -0.5..=0.5).step_by(0.01),
                                )
                                .changed()
                            {
                                params_changed = true;
                            }
                        });

                        ui.add_space(4.0);
                        ui.label(RichText::new("White Balance (Illuminant)").strong());
                        ui.horizontal(|ui| {
                            ui.label("R:");
                            if ui
                                .add(
                                    Slider::new(&mut frame.params.wb_high[0], 0.5..=3.5)
                                        .step_by(0.01),
                                )
                                .changed()
                            {
                                params_changed = true;
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("G:");
                            if ui
                                .add(
                                    Slider::new(&mut frame.params.wb_high[1], 0.5..=3.5)
                                        .step_by(0.01),
                                )
                                .changed()
                            {
                                params_changed = true;
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("B:");
                            if ui
                                .add(
                                    Slider::new(&mut frame.params.wb_high[2], 0.5..=3.5)
                                        .step_by(0.01),
                                )
                                .changed()
                            {
                                params_changed = true;
                            }
                        });

                        ui.add_space(4.0);
                        ui.label(RichText::new("Print Properties").strong());
                        ui.horizontal(|ui| {
                            ui.label("Paper black:");
                            if ui
                                .add(
                                    Slider::new(&mut frame.params.paper_black, -0.2..=0.5)
                                        .step_by(0.005),
                                )
                                .changed()
                            {
                                params_changed = true;
                            }
                        });

                        ui.horizontal(|ui| {
                            ui.label("Paper grade:");
                            if ui
                                .add(
                                    Slider::new(&mut frame.params.paper_grade, 1.0..=8.0)
                                        .step_by(0.1),
                                )
                                .changed()
                            {
                                params_changed = true;
                            }
                        });

                        ui.horizontal(|ui| {
                            ui.label("Paper gloss:");
                            if ui
                                .add(
                                    Slider::new(&mut frame.params.paper_gloss, 0.1..=0.99)
                                        .step_by(0.01),
                                )
                                .changed()
                            {
                                params_changed = true;
                            }
                        });

                        ui.horizontal(|ui| {
                            ui.label("Exposure:");
                            if ui
                                .add(
                                    Slider::new(&mut frame.params.print_exposure, 0.2..=3.0)
                                        .step_by(0.01),
                                )
                                .changed()
                            {
                                params_changed = true;
                            }
                        });

                        let ev = frame.params.print_exposure.log2();
                        ui.small(format!("Relative EV: {ev:+.2} EV"));
                    });
                }
            });

        if params_changed {
            self.invalidate_texture();
        }

        // Central Canvas with Positive Preview & Drag Selection
        CentralPanel::default().show(ui, |ui| {
            self.render_preview_canvas(ui);
        });
    }
}

impl ReviewApp {
    fn render_preview_canvas(&mut self, ui: &mut Ui) {
        let available_size = ui.available_size();

        let current_index = self.session.current_index;
        let pipeline = &self.session.color_pipeline;

        let Some(frame) = self.session.frames.get_mut(current_index) else {
            ui.centered_and_justified(|ui| {
                if self.is_scanning {
                    ui.vertical_centered(|ui| {
                        ui.spinner();
                        ui.add_space(8.0);
                        ui.label(RichText::new(&self.status_message).size(16.0));
                        ui.add_space(8.0);
                        ui.add(egui::ProgressBar::new(self.scan_progress).desired_width(220.0));
                    });
                } else {
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("No frames loaded").size(18.0).strong());
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("Load or scan a strip to begin post-scan review.")
                                .size(13.0)
                                .color(Color32::from_rgb(160, 160, 160)),
                        );
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            let is_calibrated = self.scan_setup.selected_roll().is_calibrated();
                            let can_scan = self.worker.is_some() && is_calibrated;
                            let scan_btn = Button::new(
                                RichText::new("▶ Scan Strip").color(Color32::WHITE).strong(),
                            )
                            .fill(Color32::from_rgb(40, 160, 60));
                            if ui.add_enabled(can_scan, scan_btn).clicked() {
                                self.start_scan();
                            }
                            if self.worker.is_none() {
                                ui.colored_label(
                                    Color32::from_rgb(200, 160, 100),
                                    "(No scanner connected)",
                                );
                            } else if !is_calibrated {
                                ui.colored_label(
                                    Color32::from_rgb(255, 180, 100),
                                    "(D-min calibration required)",
                                );
                            }
                            if ui.button("⚙ Scan Settings").clicked() {
                                self.scan_setup.show_setup_modal = true;
                            }
                        });
                    });
                }
            });
            return;
        };

        let (pw, ph, raw_pixels) = frame.get_or_render_preview(pipeline);

        // Fetch or allocate texture
        let texture_handle = match &self.preview_texture {
            Some((idx, handle)) if *idx == current_index => handle.clone(),
            _ => {
                let color_image = ColorImage::from_rgba_unmultiplied([pw, ph], raw_pixels);
                let handle =
                    ui.ctx()
                        .load_texture("review_preview", color_image, TextureOptions::LINEAR);
                self.preview_texture = Some((current_index, handle.clone()));
                handle
            }
        };

        // Aspect-fit target rectangle
        let img_w = pw as f32;
        let img_h = ph as f32;
        let scale = (available_size.x / img_w)
            .min(available_size.y / img_h)
            .min(1.0);
        let target_w = img_w * scale;
        let target_h = img_h * scale;

        let canvas_rect = ui.available_rect_before_wrap();
        let center = canvas_rect.center();
        let image_rect = Rect::from_center_size(center, vec2(target_w, target_h));
        self.last_image_rect = Some(image_rect);

        let response = ui.allocate_rect(canvas_rect, egui::Sense::click_and_drag());

        let painter = ui.painter_at(canvas_rect);
        painter.rect_filled(
            canvas_rect,
            CornerRadius::ZERO,
            Color32::from_rgb(20, 20, 22),
        );

        painter.image(
            texture_handle.id(),
            image_rect,
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );

        let mut committed_selection: Option<SampleRect> = None;

        if response.drag_started() {
            if let Some(pos) = response.interact_pointer_pos()
                && image_rect.contains(pos)
            {
                self.drag_start = Some(pos);
                self.drag_current = Some(pos);
            }
        } else if response.dragged() {
            if self.drag_start.is_some()
                && let Some(pos) = response.interact_pointer_pos()
            {
                self.drag_current = Some(pos);
            }
        } else if response.drag_stopped() {
            if let (Some(start), Some(curr)) = (self.drag_start, self.drag_current) {
                let min_pos = Pos2::new(start.x.min(curr.x), start.y.min(curr.y));
                let max_pos = Pos2::new(start.x.max(curr.x), start.y.max(curr.y));

                let u0 = ((min_pos.x - image_rect.left()) / image_rect.width()).clamp(0.0, 1.0);
                let v0 = ((min_pos.y - image_rect.top()) / image_rect.height()).clamp(0.0, 1.0);
                let u1 = ((max_pos.x - image_rect.left()) / image_rect.width()).clamp(0.0, 1.0);
                let v1 = ((max_pos.y - image_rect.top()) / image_rect.height()).clamp(0.0, 1.0);

                let px_x0 = (u0 * img_w).round() as usize;
                let px_y0 = (v0 * img_h).round() as usize;
                let px_x1 = (u1 * img_w).round() as usize;
                let px_y1 = (v1 * img_h).round() as usize;

                let w = px_x1.saturating_sub(px_x0);
                let h = px_y1.saturating_sub(px_y0);

                if w > 0 && h > 0 {
                    committed_selection = Some(SampleRect::new(px_x0, px_y0, w, h));
                }
            }
            self.drag_start = None;
            self.drag_current = None;
        }

        if let Some(sel) = committed_selection {
            if let Some(frame) = self.session.current_frame_mut() {
                frame.apply_highlight_wb_selection(sel);
            }
            self.invalidate_texture();
        }

        // Draw active drag overlay
        if let (Some(start), Some(curr)) = (self.drag_start, self.drag_current) {
            let drag_rect = Rect::from_two_pos(start, curr);
            painter.rect_stroke(
                drag_rect,
                CornerRadius::ZERO,
                Stroke::new(2.0, Color32::from_rgb(255, 230, 80)),
                StrokeKind::Outside,
            );
            painter.rect_filled(
                drag_rect,
                CornerRadius::ZERO,
                Color32::from_rgba_unmultiplied(255, 230, 80, 40),
            );
        } else if let Some(frame) = self.session.current_frame() {
            // Draw existing committed highlight WB selection
            if let Some(sel) = frame.highlight_wb_rect {
                let u0 = sel.x as f32 / img_w;
                let v0 = sel.y as f32 / img_h;
                let u1 = (sel.x + sel.width) as f32 / img_w;
                let v1 = (sel.y + sel.height) as f32 / img_h;

                let rect = Rect::from_min_max(
                    Pos2::new(
                        image_rect.left() + u0 * image_rect.width(),
                        image_rect.top() + v0 * image_rect.height(),
                    ),
                    Pos2::new(
                        image_rect.left() + u1 * image_rect.width(),
                        image_rect.top() + v1 * image_rect.height(),
                    ),
                );

                painter.rect_stroke(
                    rect,
                    CornerRadius::ZERO,
                    Stroke::new(2.0, Color32::from_rgb(80, 220, 255)),
                    StrokeKind::Outside,
                );
                painter.rect_filled(
                    rect,
                    CornerRadius::ZERO,
                    Color32::from_rgba_unmultiplied(80, 220, 255, 40),
                );
            }
        }

        // Status banner
        if let Some(frame) = self.session.current_frame() {
            let status_text = match frame.selection_status {
                SelectionStatus::None => "Drag rectangle on a neutral highlight".to_string(),
                SelectionStatus::Valid => format!(
                    "✓ Highlight WB: {:.2} / {:.2} / {:.2}",
                    frame.params.wb_high[0], frame.params.wb_high[1], frame.params.wb_high[2]
                ),
                SelectionStatus::TooSmall { width, height } => {
                    format!("⚠ Selection too small ({width}x{height}, minimum 4x4)")
                }
                SelectionStatus::TooDark => {
                    "⚠ Selected area is in shadows; choose a bright neutral highlight".to_string()
                }
                SelectionStatus::Clipped => {
                    "⚠ Selected area is sensor-clipped; pick an unclipped highlight".to_string()
                }
            };

            let banner_color = match frame.selection_status {
                SelectionStatus::None => Color32::from_rgb(180, 180, 180),
                SelectionStatus::Valid => Color32::from_rgb(100, 240, 120),
                SelectionStatus::TooSmall { .. }
                | SelectionStatus::TooDark
                | SelectionStatus::Clipped => Color32::from_rgb(255, 140, 100),
            };

            let banner_rect = Rect::from_min_size(
                Pos2::new(canvas_rect.left() + 16.0, canvas_rect.top() + 16.0),
                vec2(400.0, 32.0),
            );
            painter.rect_filled(
                banner_rect,
                CornerRadius::same(6),
                Color32::from_black_alpha(180),
            );
            painter.text(
                banner_rect.center(),
                Align2::CENTER_CENTER,
                status_text,
                FontId::proportional(14.0),
                banner_color,
            );
        }
    }
}

/// Launches the desktop GUI review and scanning studio with the given app state.
pub fn run_gui(app: ReviewApp) -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 600.0])
            .with_title("Coolscan Studio"),
        ..Default::default()
    };
    eframe::run_native(
        "Coolscan Studio",
        native_options,
        Box::new(|_cc| Ok(Box::new(app))),
    )
}

/// Launches the desktop GUI review window for a pre-loaded session.
pub fn run_review_gui(session: ReviewSession) -> eframe::Result<()> {
    run_gui(ReviewApp::new(session))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::color::ScannerColorPipeline;
    use crate::scanner::types::ScanRequestError;

    fn test_app() -> ReviewApp {
        let pipeline = ScannerColorPipeline::default_ls40().unwrap();
        let session = ReviewSession::empty(RollProfile::pro_image_100(), pipeline);
        ReviewApp::new(session)
    }

    #[test]
    fn test_empty_manual_frame_selection_does_not_dispatch_scan() {
        let mut app = test_app();
        app.scan_setup.all_frames = false;
        app.scan_setup.frame_selected = [false; 6];

        let req = app.build_scan_request();
        assert_eq!(req.frames, FrameSelection::List(vec![]));
        assert_eq!(req.validate(), Err(ScanRequestError::EmptyFrameList));

        app.start_scan();
        assert!(!app.is_scanning);
        assert!(app.status_message.contains("Invalid scan settings"));
    }

    #[test]
    fn test_manual_frame_selection_sparse_preserves_frames() {
        let mut app = test_app();
        app.scan_setup.all_frames = false;
        app.scan_setup.frame_selected = [false, true, false, true, false, true];

        let req = app.build_scan_request();
        assert_eq!(req.frames, FrameSelection::List(vec![2, 4, 6]));
        assert_eq!(req.validate(), Ok(()));
    }

    #[test]
    fn test_uncalibrated_roll_scan_is_blocked() {
        let mut app = test_app();
        app.scan_setup.set_film_stock_index(1); // Portra 400 (uncalibrated)
        assert!(!app.scan_setup.selected_roll().is_calibrated());

        app.start_scan();
        assert!(!app.is_scanning);
        assert!(app.status_message.contains("uncalibrated"));
    }

    #[test]
    fn test_custom_roll_profile_preserved() {
        let pipeline = ScannerColorPipeline::default_ls40().unwrap();
        let custom_roll =
            RollProfile::new("my-custom-roll", "My Custom Film", "Custom Stock", None).unwrap();
        let session = ReviewSession::empty(custom_roll.clone(), pipeline);
        let app = ReviewApp::new(session);

        assert_eq!(app.scan_setup.selected_roll().id.0, "my-custom-roll");
        assert_eq!(app.scan_setup.film_stock_index, 3);
        let req = app.build_scan_request();
        assert_eq!(req.roll.unwrap().id.0, "my-custom-roll");
    }

    #[test]
    fn test_custom_roll_with_preset_stock_name_is_classified_as_custom() {
        use crate::processing::{FilmStock, ScannerProfile};
        let custom = RollProfile::calibrated(
            "roll-001",
            "My Portra Roll",
            FilmStock::portra_400(),
            [0.8, 0.8, 0.8],
            ScannerProfile::ls40_negative(),
        ).unwrap();

        let setup = ScanSetupState::for_roll(custom.clone());

        assert_eq!(setup.film_stock_index, 3);
        assert_eq!(setup.selected_roll(), custom);
        
        let pipeline = ScannerColorPipeline::default_ls40().unwrap();
        let session = ReviewSession::empty(custom.clone(), pipeline);
        let app = ReviewApp::new(session);
        let req = app.build_scan_request();
        assert_eq!(req.roll.unwrap(), custom);
    }
}
