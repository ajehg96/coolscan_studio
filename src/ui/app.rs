//! eframe / egui Desktop GUI Review Application.
//!
//! Provides the interactive review screen specified in Phase 8:
//! - Frame navigation header with roll profile metadata.
//! - Aspect-fit positive preview canvas with interactive click-drag selection rectangle.
//! - Instant re-rendering with LittleCMS 2 color management and Negadoctor inversion.
//! - Toolbar with rotation controls and scope options (this frame, remaining, whole strip).
//! - Collapsible advanced drawer for parameter inspection and manual slider adjustments.

use egui::{
    vec2, Align, Align2, Button, CentralPanel, Color32, ColorImage, CornerRadius, FontId, Layout,
    Panel, Pos2, Rect, RichText, ScrollArea, Slider, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui,
};

use super::review::{ReviewSession, SelectionStatus};
use crate::processing::analysis::SampleRect;
use crate::processing::orientation::OrientationScope;

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
}

impl ReviewApp {
    /// Creates a new review GUI app from a review session.
    pub fn new(session: ReviewSession) -> Self {
        Self {
            session,
            preview_texture: None,
            drag_start: None,
            drag_current: None,
            last_image_rect: None,
        }
    }

    /// Forces regeneration of the preview texture (e.g. after parameter or orientation change).
    pub fn invalidate_texture(&mut self) {
        if let Some(frame) = self.session.current_frame_mut() {
            frame.invalidate_preview();
        }
        self.preview_texture = None;
    }
}

impl eframe::App for ReviewApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Handle keyboard navigation shortcuts
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            if self.session.prev_frame() {
                self.preview_texture = None;
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            if self.session.next_frame() {
                self.preview_texture = None;
            }
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
                let total = self.session.frame_count();

                ui.heading(format!("Frame {current_num} of {total}"));

                if ui.add_enabled(has_next, Button::new("Next ▶")).clicked() {
                    self.session.next_frame();
                    self.preview_texture = None;
                }

                ui.separator();

                ui.label(format!("Roll: {}", self.session.roll.id));
                ui.colored_label(
                    Color32::from_rgb(180, 220, 255),
                    &self.session.roll.film_stock,
                );

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

                if let Some(frame) = self.session.current_frame_mut() {
                    ScrollArea::vertical().show(ui, |ui| {
                        ui.collapsing("Film Substrate (D-min)", |ui| {
                            ui.label(format!(
                                "R: {:.4}  G: {:.4}  B: {:.4}",
                                frame.params.dmin[0],
                                frame.params.dmin[1],
                                frame.params.dmin[2]
                            ));
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
                                .add(Slider::new(&mut frame.params.offset, -0.5..=0.5).step_by(0.01))
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
                ui.label("No frames loaded");
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
        let scale = (available_size.x / img_w).min(available_size.y / img_h).min(1.0);
        let target_w = img_w * scale;
        let target_h = img_h * scale;

        let canvas_rect = ui.available_rect_before_wrap();
        let center = canvas_rect.center();
        let image_rect = Rect::from_center_size(center, vec2(target_w, target_h));
        self.last_image_rect = Some(image_rect);

        let response = ui.allocate_rect(canvas_rect, egui::Sense::click_and_drag());

        let painter = ui.painter_at(canvas_rect);
        painter.rect_filled(canvas_rect, CornerRadius::ZERO, Color32::from_rgb(20, 20, 22));

        painter.image(
            texture_handle.id(),
            image_rect,
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );

        let mut committed_selection: Option<SampleRect> = None;

        if response.drag_started() {
            if let Some(pos) = response.interact_pointer_pos() {
                if image_rect.contains(pos) {
                    self.drag_start = Some(pos);
                    self.drag_current = Some(pos);
                }
            }
        } else if response.dragged() {
            if self.drag_start.is_some() {
                if let Some(pos) = response.interact_pointer_pos() {
                    self.drag_current = Some(pos);
                }
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

/// Launches the desktop GUI review window.
pub fn run_review_gui(session: ReviewSession) -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 600.0])
            .with_title("Coolscan Studio — Review"),
        ..Default::default()
    };
    eframe::run_native(
        "Coolscan Studio — Review",
        native_options,
        Box::new(|_cc| Ok(Box::new(ReviewApp::new(session)))),
    )
}

