//! Headless review session state machine, selection validation, and frame management.
//!
//! Provides the core review logic decoupled from the GUI framework:
//! - Multi-frame review session management.
//! - Highlight white-balance selection and validation (too small, too dark, clipped).
//! - Automatic recalculation of paper black and print exposure upon calibration.
//! - Non-destructive orientation changes across frame scopes (current, remaining, whole strip).
//! - Positive preview generation in 8-bit display sRGB.

use serde::{Deserialize, Serialize};

use crate::darktable::xmp::{DarktableError, DarktableXmp};
use crate::processing::analysis::{
    SampleRect, TechnicalAnalysis, WorkingImage, finish_after_white_balance, sample_highlight_wb,
};
use crate::processing::color::{ColorError, ScannerColorPipeline};
use crate::processing::negadoctor::{NegadoctorParams, THRESHOLD};
use crate::processing::orientation::{Orientation, OrientationScope};
use crate::processing::roll::{PreparedFrame, RollProfile};

/// Status and feedback of a highlight white-balance selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectionStatus {
    /// No selection has been made yet.
    None,
    /// Selection is valid and white-balance has been calibrated.
    Valid,
    /// Selection rectangle is too small (minimum 4x4 pixels).
    TooSmall { width: usize, height: usize },
    /// Selected region is in image shadows rather than a highlight.
    TooDark,
    /// Selected region contains saturated or clipped sensor samples.
    Clipped,
}

impl std::fmt::Display for SelectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SelectionStatus::None => write!(f, "Drag rectangle on a neutral highlight"),
            SelectionStatus::Valid => write!(f, "Highlight white-balance calibrated"),
            SelectionStatus::TooSmall { width, height } => {
                write!(f, "Selection too small ({width}x{height}, minimum 4x4)")
            }
            SelectionStatus::TooDark => {
                write!(
                    f,
                    "Selected area is too dark; pick a bright neutral highlight"
                )
            }
            SelectionStatus::Clipped => {
                write!(
                    f,
                    "Selected area is sensor-clipped; pick an unclipped highlight"
                )
            }
        }
    }
}

/// Review state for an individual film frame.
#[derive(Debug, Clone)]
pub struct ReviewFrameState {
    /// 1-based frame number on the strip.
    pub frame_number: usize,
    /// Full-resolution master working image in linear Rec.2020 space.
    pub working_image: WorkingImage,
    /// Downscaled working preview image for fast interactive rendering.
    pub preview_image: WorkingImage,
    /// Current view orientation.
    pub orientation: Orientation,
    /// Active Negadoctor parameters.
    pub params: NegadoctorParams,
    /// Technical analysis calculated before white balance.
    pub technical: TechnicalAnalysis,
    /// Highlight white-balance selection rectangle in preview coordinates.
    pub highlight_wb_rect: Option<SampleRect>,
    /// Status of the active highlight selection.
    pub selection_status: SelectionStatus,
    /// Whether the user has accepted this frame.
    pub accepted: bool,
    /// Original scanner frame artifact if retained.
    pub source_artifact: Option<crate::scanner::types::FrameArtifact>,
    /// Cached unoriented display sRGB pixels of the preview image.
    cached_srgb: Option<Vec<[u8; 3]>>,
    /// Cached oriented display sRGB RGBA8 pixels and preview dimensions.
    cached_preview: Option<(usize, usize, Vec<u8>)>,
}

impl ReviewFrameState {
    /// Creates a review frame from a `PreparedFrame` and working image.
    pub fn new(
        frame_number: usize,
        working_image: WorkingImage,
        roll: &RollProfile,
        technical: TechnicalAnalysis,
    ) -> Self {
        let mut params = NegadoctorParams::from_dmin(roll.dmin);
        params.dmax = technical.dmax;
        params.offset = technical.scan_bias;
        finish_after_white_balance(&working_image, &mut params);

        let preview_image = working_image.downscale_to_preview(1440);

        Self {
            frame_number,
            working_image,
            preview_image,
            orientation: Orientation::Normal,
            params,
            technical,
            highlight_wb_rect: None,
            selection_status: SelectionStatus::None,
            accepted: false,
            source_artifact: None,
            cached_srgb: None,
            cached_preview: None,
        }
    }

    pub fn with_artifact(mut self, artifact: crate::scanner::types::FrameArtifact) -> Self {
        self.source_artifact = Some(artifact);
        self
    }

    /// Sets the orientation and invalidates the cached oriented preview (preserving unoriented sRGB).
    pub fn set_orientation(&mut self, orientation: Orientation) {
        if self.orientation != orientation {
            self.orientation = orientation;
            self.highlight_wb_rect = None;
            self.cached_preview = None;
        }
    }

    /// Applies a highlight white-balance selection rectangle drawn in preview coordinates.
    pub fn apply_highlight_wb_selection(&mut self, preview_rect: SampleRect) {
        if preview_rect.width < 4 || preview_rect.height < 4 {
            self.selection_status = SelectionStatus::TooSmall {
                width: preview_rect.width,
                height: preview_rect.height,
            };
            return;
        }

        // Map preview rectangle to preview sensor coordinates
        let preview_source = self.orientation.preview_rect_to_source_rect(
            preview_rect,
            self.preview_image.width,
            self.preview_image.height,
        );

        // Map to master sensor coordinates for full precision sampling
        let source_rect = if self.working_image.width == self.preview_image.width
            && self.working_image.height == self.preview_image.height
        {
            preview_source
        } else {
            let scale_x = self.working_image.width as f64 / self.preview_image.width as f64;
            let scale_y = self.working_image.height as f64 / self.preview_image.height as f64;
            let mx = (preview_source.x as f64 * scale_x).round() as usize;
            let my = (preview_source.y as f64 * scale_y).round() as usize;
            let mw = ((preview_source.width as f64 * scale_x).round() as usize)
                .max(1)
                .min(self.working_image.width.saturating_sub(mx));
            let mh = ((preview_source.height as f64 * scale_y).round() as usize)
                .max(1)
                .min(self.working_image.height.saturating_sub(my));
            SampleRect::new(mx, my, mw, mh)
        };

        let stats = self.working_image.sample_region(Some(source_rect));

        // 1. Check for sensor clipping (near zero in transmission)
        if stats.min[0] <= THRESHOLD * 1.5
            || stats.min[1] <= THRESHOLD * 1.5
            || stats.min[2] <= THRESHOLD * 1.5
        {
            self.selection_status = SelectionStatus::Clipped;
            return;
        }

        // 2. Check if selected area is in positive shadows (near D-min in negative transmission)
        let is_too_dark = stats.mean[0] > self.params.dmin[0] * 0.90
            && stats.mean[1] > self.params.dmin[1] * 0.90
            && stats.mean[2] > self.params.dmin[2] * 0.90;

        if is_too_dark {
            self.selection_status = SelectionStatus::TooDark;
            return;
        }

        // 3. Valid selection: calculate highlight WB and update paper black & print exposure on full-res master
        let wb_high = sample_highlight_wb(&self.working_image, &self.params, Some(source_rect));
        self.params.wb_high = wb_high;

        finish_after_white_balance(&self.working_image, &mut self.params);

        self.highlight_wb_rect = Some(preview_rect);
        self.selection_status = SelectionStatus::Valid;
        self.invalidate_preview();
    }

    /// Resets highlight white balance back to neutral 1.0.
    pub fn reset_highlight_wb(&mut self) {
        self.params.wb_high = [1.0, 1.0, 1.0];
        finish_after_white_balance(&self.working_image, &mut self.params);
        self.highlight_wb_rect = None;
        self.selection_status = SelectionStatus::None;
        self.invalidate_preview();
    }

    /// Invalidates cached preview pixels so the texture will be regenerated.
    pub fn invalidate_preview(&mut self) {
        self.cached_preview = None;
        self.cached_srgb = None;
    }

    /// Retrieves or computes display sRGB RGBA8 preview pixels.
    pub fn get_or_render_preview(
        &mut self,
        color_pipeline: &ScannerColorPipeline,
    ) -> (usize, usize, &[u8]) {
        if self.cached_preview.is_none() {
            // Step 1 & 2: Ensure unoriented sRGB pixels of the preview image are available
            if self.cached_srgb.is_none() {
                let prepared = self.params.prepare();
                let mut positive_linear = vec![[0.0f32; 3]; self.preview_image.pixels.len()];
                prepared.render_positive(&self.preview_image.pixels, &mut positive_linear);

                let mut srgb = vec![[0u8; 3]; self.preview_image.pixels.len()];
                color_pipeline.bulk_working_to_display_srgb(&positive_linear, &mut srgb);
                self.cached_srgb = Some(srgb);
            }

            let srgb = self.cached_srgb.as_ref().unwrap();
            let (pw, ph) = self
                .orientation
                .preview_dimensions(self.preview_image.width, self.preview_image.height);

            // Step 3: Transpose into oriented RGBA8 output
            let mut rgba = vec![255u8; pw * ph * 4];
            for v in 0..ph {
                let row_offset = v * pw;
                for u in 0..pw {
                    let (sx, sy) = self.orientation.preview_to_source(
                        u,
                        v,
                        self.preview_image.width,
                        self.preview_image.height,
                    );
                    let px = srgb[sy * self.preview_image.width + sx];
                    let idx = (row_offset + u) * 4;
                    rgba[idx] = px[0];
                    rgba[idx + 1] = px[1];
                    rgba[idx + 2] = px[2];
                    rgba[idx + 3] = 255;
                }
            }

            self.cached_preview = Some((pw, ph, rgba));
        }
        let (w, h, pixels) = self.cached_preview.as_ref().unwrap();
        (*w, *h, pixels.as_slice())
    }
}

/// Generates an oriented RGBA8 display preview from linear working image and Negadoctor parameters.
pub fn generate_preview_rgba(
    working_image: &WorkingImage,
    params: &NegadoctorParams,
    orientation: Orientation,
    color_pipeline: &ScannerColorPipeline,
) -> (usize, usize, Vec<u8>) {
    let (pw, ph) = orientation.preview_dimensions(working_image.width, working_image.height);
    let prepared = params.prepare();

    // 1. Invert negative pixels to positive linear Rec.2020
    let mut positive_linear = vec![[0.0f32; 3]; working_image.pixels.len()];
    prepared.render_positive(&working_image.pixels, &mut positive_linear);

    // 2. Convert to 8-bit display sRGB
    let mut srgb_pixels = vec![[0u8; 3]; working_image.pixels.len()];
    color_pipeline.bulk_working_to_display_srgb(&positive_linear, &mut srgb_pixels);

    // 3. Map into oriented RGBA8 output
    let mut rgba = vec![255u8; pw * ph * 4];
    for v in 0..ph {
        let row_offset = v * pw;
        for u in 0..pw {
            let (sx, sy) =
                orientation.preview_to_source(u, v, working_image.width, working_image.height);
            let px = srgb_pixels[sy * working_image.width + sx];
            let idx = (row_offset + u) * 4;
            rgba[idx] = px[0];
            rgba[idx + 1] = px[1];
            rgba[idx + 2] = px[2];
            rgba[idx + 3] = 255;
        }
    }

    (pw, ph, rgba)
}

/// Multi-frame review session.
pub struct ReviewSession {
    /// Frames loaded into this session.
    pub frames: Vec<ReviewFrameState>,
    /// Index of the active frame (0-based).
    pub current_index: usize,
    /// Associated roll profile.
    pub roll: RollProfile,
    /// Default orientation scope for rotation controls.
    pub orientation_scope: OrientationScope,
    /// LittleCMS 2 color pipeline.
    pub color_pipeline: ScannerColorPipeline,
}

impl ReviewSession {
    /// Creates a review session from existing frame states.
    pub fn new(
        frames: Vec<ReviewFrameState>,
        roll: RollProfile,
        color_pipeline: ScannerColorPipeline,
    ) -> Self {
        Self {
            frames,
            current_index: 0,
            roll,
            orientation_scope: OrientationScope::CurrentFrame,
            color_pipeline,
        }
    }

    /// Initializes a review session from prepared scan frames.
    pub fn from_prepared_frames(
        prepared: Vec<PreparedFrame>,
        color_pipeline: ScannerColorPipeline,
    ) -> Result<Self, ColorError> {
        let roll = prepared
            .first()
            .map(|f| f.roll.clone())
            .unwrap_or_else(RollProfile::pro_image_100);

        let mut frames = Vec::with_capacity(prepared.len());

        for p in prepared {
            let effective = p
                .source
                .get_effective_image()
                .map_err(|e| ColorError::Lcms(e.to_string()))?;
            let working_image = WorkingImage::from_scanner_samples(
                effective.samples(),
                effective.pass(),
                &color_pipeline,
            )?;
            let mut frame_state =
                ReviewFrameState::new(p.source.frame_number, working_image, &p.roll, p.technical);
            frame_state.orientation = p.orientation;
            frame_state.params = p.params;
            frame_state.source_artifact = Some(p.source);
            frames.push(frame_state);
        }

        Ok(Self::new(frames, roll, color_pipeline))
    }

    /// Creates an empty review session ready to receive scanned frames dynamically.
    pub fn empty(roll: RollProfile, color_pipeline: ScannerColorPipeline) -> Self {
        Self::new(Vec::new(), roll, color_pipeline)
    }

    /// Appends a newly scanned prepared frame to this review session.
    pub fn add_prepared_frame(&mut self, p: PreparedFrame) -> Result<usize, ColorError> {
        let effective = p
            .source
            .get_effective_image()
            .map_err(|e| ColorError::Lcms(e.to_string()))?;
        let working_image = WorkingImage::from_scanner_samples(
            effective.samples(),
            effective.pass(),
            &self.color_pipeline,
        )?;
        let frame_num = p.source.frame_number;
        let mut frame_state = ReviewFrameState::new(frame_num, working_image, &p.roll, p.technical);
        frame_state.orientation = p.orientation;
        frame_state.params = p.params;
        frame_state.source_artifact = Some(p.source);
        self.frames.push(frame_state);
        Ok(self.frames.len() - 1)
    }

    /// Number of frames in the session.
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Retrieves a reference to the active frame.
    pub fn current_frame(&self) -> Option<&ReviewFrameState> {
        self.frames.get(self.current_index)
    }

    /// Retrieves a mutable reference to the active frame.
    pub fn current_frame_mut(&mut self) -> Option<&mut ReviewFrameState> {
        self.frames.get_mut(self.current_index)
    }

    /// Advances to the next frame. Returns true if moved, false if already at the last frame.
    pub fn next_frame(&mut self) -> bool {
        if self.current_index + 1 < self.frames.len() {
            self.current_index += 1;
            true
        } else {
            false
        }
    }

    /// Moves to the previous frame. Returns true if moved, false if already at the first frame.
    pub fn prev_frame(&mut self) -> bool {
        if self.current_index > 0 {
            self.current_index -= 1;
            true
        } else {
            false
        }
    }

    /// Navigates to a specific frame index.
    pub fn go_to_frame(&mut self, index: usize) -> bool {
        if index < self.frames.len() {
            self.current_index = index;
            true
        } else {
            false
        }
    }

    /// Rotates frames according to the given scope and direction.
    pub fn rotate(&mut self, scope: OrientationScope, clockwise: bool) {
        if self.frames.is_empty() {
            return;
        }

        let range: Box<dyn Iterator<Item = usize>> = match scope {
            OrientationScope::CurrentFrame => Box::new(std::iter::once(self.current_index)),
            OrientationScope::RemainingFrames => Box::new(self.current_index..self.frames.len()),
            OrientationScope::WholeStrip => Box::new(0..self.frames.len()),
        };

        for idx in range {
            if let Some(frame) = self.frames.get_mut(idx) {
                let new_orient = if clockwise {
                    frame.orientation.rotate_clockwise()
                } else {
                    frame.orientation.rotate_counter_clockwise()
                };
                frame.set_orientation(new_orient);
            }
        }
    }

    /// Flips the current frame 180 degrees.
    pub fn flip_180(&mut self, scope: OrientationScope) {
        if self.frames.is_empty() {
            return;
        }

        let range: Box<dyn Iterator<Item = usize>> = match scope {
            OrientationScope::CurrentFrame => Box::new(std::iter::once(self.current_index)),
            OrientationScope::RemainingFrames => Box::new(self.current_index..self.frames.len()),
            OrientationScope::WholeStrip => Box::new(0..self.frames.len()),
        };

        for idx in range {
            if let Some(frame) = self.frames.get_mut(idx) {
                let new_orient = frame.orientation.rotate_clockwise().rotate_clockwise();
                frame.set_orientation(new_orient);
            }
        }
    }

    /// Accepts the current frame and advances to the next unaccepted frame.
    ///
    /// Returns true if advanced, false if all frames are accepted or at end.
    pub fn accept_and_next(&mut self) -> bool {
        if let Some(frame) = self.current_frame_mut() {
            frame.accepted = true;
        }

        // Find next unaccepted frame
        for i in (self.current_index + 1)..self.frames.len() {
            if !self.frames[i].accepted {
                self.current_index = i;
                return true;
            }
        }

        // If all remaining are accepted, attempt simple next
        self.next_frame()
    }

    /// Returns true if all frames in the session are marked accepted.
    pub fn is_all_accepted(&self) -> bool {
        !self.frames.is_empty() && self.frames.iter().all(|f| f.accepted)
    }

    /// Generates the Darktable XMP sidecar for the current frame.
    pub fn current_darktable_xmp(&self) -> Option<DarktableXmp> {
        let frame = self.current_frame()?;
        let derived_from = format!("frame-{}.tif", frame.frame_number);
        let profile_name = self.roll.scanner_profile.icc_profile_name();
        DarktableXmp::for_frame(
            &derived_from,
            profile_name,
            &frame.params,
            frame.orientation,
        )
        .ok()
    }

    /// Saves the Darktable XMP sidecar for the current frame to the specified directory.
    pub fn save_current_xmp(
        &self,
        output_dir: &std::path::Path,
    ) -> Result<std::path::PathBuf, DarktableError> {
        let frame = self
            .current_frame()
            .ok_or_else(|| DarktableError::MissingHistoryItem("No active frame".into()))?;
        let xmp = self.current_darktable_xmp().ok_or_else(|| {
            DarktableError::MissingHistoryItem("Could not generate XMP for frame".into())
        })?;
        let path = output_dir.join(format!("frame-{}.tif.xmp", frame.frame_number));
        xmp.write_to_file(&path)?;
        Ok(path)
    }

    /// Saves both the TIFF image (cropped if applicable) and the Darktable XMP sidecar.
    pub fn save_current_frame_and_xmp(
        &self,
        output_dir: &std::path::Path,
    ) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
        let frame = self
            .current_frame()
            .ok_or_else(|| "No active frame".to_string())?;
        if !output_dir.exists() {
            std::fs::create_dir_all(output_dir).map_err(|e| e.to_string())?;
        }
        let tiff_path = output_dir.join(format!("frame-{}.tif", frame.frame_number));

        // Save TIFF if source artifact is present
        if let Some(artifact) = &frame.source_artifact {
            let effective = artifact.get_effective_image().map_err(|e| e.to_string())?;
            let path_str = tiff_path.to_str().ok_or("Invalid TIFF path string")?;
            crate::tiff::write_tiff(
                path_str,
                effective.samples(),
                effective.pass(),
                artifact.dpi,
            )
            .map_err(|e| e.to_string())?;
        }

        // Save Darktable XMP sidecar
        let xmp_path = self
            .save_current_xmp(output_dir)
            .map_err(|e| e.to_string())?;

        Ok((tiff_path, xmp_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::FrameArtifact;

    fn mock_working_image(width: usize, height: usize) -> WorkingImage {
        // Creates an image with highlights in top-left, shadows in bottom-right
        let mut pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                // Top-left: dense negative (positive highlight) ~ 0.005
                // Bottom-right: thin negative (positive shadow) ~ 0.40
                let fx = x as f32 / width as f32;
                let fy = y as f32 / height as f32;
                let factor = (fx + fy) * 0.5;
                let val = 0.005 + factor * (0.40 - 0.005);
                pixels.push([val, val * 1.01, val * 0.99]);
            }
        }
        WorkingImage::new(width, height, pixels)
    }

    #[test]
    fn review_session_frame_switching_and_state_persistence() {
        let pipeline = ScannerColorPipeline::default_ls40().unwrap();
        let roll = RollProfile::pro_image_100();

        let img1 = mock_working_image(20, 20);
        let tech1 = TechnicalAnalysis {
            dmax: 3.27,
            scan_bias: 0.10,
        };
        let frame1 = ReviewFrameState::new(1, img1, &roll, tech1);

        let img2 = mock_working_image(20, 20);
        let tech2 = TechnicalAnalysis {
            dmax: 2.50,
            scan_bias: 0.02,
        };
        let frame2 = ReviewFrameState::new(2, img2, &roll, tech2);

        let mut session = ReviewSession::new(vec![frame1, frame2], roll, pipeline);

        assert_eq!(session.current_index, 0);
        assert_eq!(session.current_frame().unwrap().frame_number, 1);

        // Modify frame 1 orientation
        session.rotate(OrientationScope::CurrentFrame, true);
        assert_eq!(
            session.current_frame().unwrap().orientation,
            Orientation::Rotate90
        );

        // Advance to frame 2
        assert!(session.next_frame());
        assert_eq!(session.current_index, 1);
        assert_eq!(session.current_frame().unwrap().frame_number, 2);
        assert_eq!(
            session.current_frame().unwrap().orientation,
            Orientation::Normal
        );

        // Return to frame 1 and verify persistence
        assert!(session.prev_frame());
        assert_eq!(session.current_index, 0);
        assert_eq!(
            session.current_frame().unwrap().orientation,
            Orientation::Rotate90
        );
    }

    #[test]
    fn batch_orientation_scopes() {
        let pipeline = ScannerColorPipeline::default_ls40().unwrap();
        let roll = RollProfile::pro_image_100();

        let frames = (1..=4)
            .map(|i| {
                let img = mock_working_image(10, 10);
                let tech = TechnicalAnalysis {
                    dmax: 3.0,
                    scan_bias: 0.05,
                };
                ReviewFrameState::new(i, img, &roll, tech)
            })
            .collect();

        let mut session = ReviewSession::new(frames, roll, pipeline);

        // Current index at 1 (Frame 2)
        session.go_to_frame(1);

        // Rotate remaining frames (Frames 2, 3, 4)
        session.rotate(OrientationScope::RemainingFrames, true);

        assert_eq!(session.frames[0].orientation, Orientation::Normal);
        assert_eq!(session.frames[1].orientation, Orientation::Rotate90);
        assert_eq!(session.frames[2].orientation, Orientation::Rotate90);
        assert_eq!(session.frames[3].orientation, Orientation::Rotate90);

        // Whole strip flip 180
        session.flip_180(OrientationScope::WholeStrip);
        assert_eq!(session.frames[0].orientation, Orientation::Rotate180);
        assert_eq!(session.frames[1].orientation, Orientation::Rotate270);
    }

    #[test]
    fn selection_validation_rejection_of_invalid_boxes() {
        let roll = RollProfile::pro_image_100();
        let img = mock_working_image(50, 50);
        let tech = TechnicalAnalysis {
            dmax: 3.27,
            scan_bias: 0.10,
        };
        let mut frame = ReviewFrameState::new(1, img, &roll, tech);

        // 1. Too small (3x3)
        frame.apply_highlight_wb_selection(SampleRect::new(5, 5, 3, 3));
        assert!(matches!(
            frame.selection_status,
            SelectionStatus::TooSmall {
                width: 3,
                height: 3
            }
        ));

        // 2. Too dark (shadow region in bottom-right where factor ~ 1.0, val ~ 0.40)
        // Note: mock_working_image bottom-right has values near 0.40, while dmin is ~0.89.
        // Wait, for Pro Image dmin is [0.8965, 0.9093, 0.8816].
        // If values are ~0.40, density is log10(0.89/0.40) = 0.35, which is shadow.
        // But our threshold for TooDark is val > dmin * 0.90 (~0.80).
        // Let's create an area with substrate base val = 0.88 to trigger TooDark:
        let base_pixels = vec![[0.89f32; 3]; 100];
        let base_img = WorkingImage::new(10, 10, base_pixels);
        let mut base_frame = ReviewFrameState::new(1, base_img, &roll, tech);

        base_frame.apply_highlight_wb_selection(SampleRect::new(2, 2, 5, 5));
        assert_eq!(base_frame.selection_status, SelectionStatus::TooDark);

        // 3. Valid highlight selection on bright highlights (top-left)
        frame.apply_highlight_wb_selection(SampleRect::new(1, 1, 6, 6));
        assert_eq!(frame.selection_status, SelectionStatus::Valid);
        assert!(frame.highlight_wb_rect.is_some());
    }

    #[test]
    fn parameter_recalculation_upon_selection() {
        let roll = RollProfile::pro_image_100();
        let img = mock_working_image(50, 50);
        let tech = TechnicalAnalysis {
            dmax: 3.27,
            scan_bias: 0.10,
        };
        let mut frame = ReviewFrameState::new(1, img, &roll, tech);

        let initial_wb = frame.params.wb_high;
        let _initial_black = frame.params.paper_black;

        // Apply highlight WB selection
        frame.apply_highlight_wb_selection(SampleRect::new(2, 2, 8, 8));
        assert_eq!(frame.selection_status, SelectionStatus::Valid);

        // Parameters should be recalculated
        assert_ne!(frame.params.wb_high, initial_wb);
        // Minimum WB channel must be normalized to 1.0
        let min_wb = frame.params.wb_high[0]
            .min(frame.params.wb_high[1])
            .min(frame.params.wb_high[2]);
        assert!((min_wb - 1.0).abs() < 1e-4);

        // Paper black and exposure are updated
        assert!(frame.params.paper_black.is_finite());
        assert!(frame.params.print_exposure > 0.0);
    }

    #[test]
    fn accept_and_next_workflow() {
        let pipeline = ScannerColorPipeline::default_ls40().unwrap();
        let roll = RollProfile::pro_image_100();
        let frames = (1..=3)
            .map(|i| {
                let img = mock_working_image(10, 10);
                let tech = TechnicalAnalysis {
                    dmax: 3.0,
                    scan_bias: 0.05,
                };
                ReviewFrameState::new(i, img, &roll, tech)
            })
            .collect();

        let mut session = ReviewSession::new(frames, roll, pipeline);
        assert!(!session.is_all_accepted());

        // Accept frame 1 -> advances to frame 2
        assert!(session.accept_and_next());
        assert_eq!(session.current_index, 1);
        assert!(session.frames[0].accepted);

        // Accept frame 2 -> advances to frame 3
        assert!(session.accept_and_next());
        assert_eq!(session.current_index, 2);
        assert!(session.frames[1].accepted);

        // Accept frame 3 -> cannot advance further
        assert!(!session.accept_and_next());
        assert_eq!(session.current_index, 2);
        assert!(session.is_all_accepted());
    }

    #[test]
    fn preview_rgba_generation() {
        let pipeline = ScannerColorPipeline::default_ls40().unwrap();
        let roll = RollProfile::pro_image_100();
        let img = mock_working_image(10, 20);
        let tech = TechnicalAnalysis {
            dmax: 3.0,
            scan_bias: 0.05,
        };
        let mut frame = ReviewFrameState::new(1, img, &roll, tech);

        let (w, h, pixels) = frame.get_or_render_preview(&pipeline);
        assert_eq!(w, 10);
        assert_eq!(h, 20);
        assert_eq!(pixels.len(), 10 * 20 * 4);

        // Alpha channel is fully opaque
        for i in 0..(10 * 20) {
            assert_eq!(pixels[i * 4 + 3], 255);
        }

        // Rotating swaps dimensions in rendered preview
        frame.set_orientation(Orientation::Rotate90);
        let (rw, rh, rpixels) = frame.get_or_render_preview(&pipeline);
        assert_eq!(rw, 20);
        assert_eq!(rh, 10);
        assert_eq!(rpixels.len(), 20 * 10 * 4);
    }

    #[test]
    fn review_session_darktable_xmp_save() {
        let pipeline = ScannerColorPipeline::default_ls40().unwrap();
        let roll = RollProfile::pro_image_100();
        let img = mock_working_image(10, 10);
        let tech = TechnicalAnalysis {
            dmax: 3.10,
            scan_bias: 0.08,
        };
        let mut frame = ReviewFrameState::new(3, img, &roll, tech);
        frame.orientation = Orientation::Rotate270;

        let session = ReviewSession::new(vec![frame], roll, pipeline);

        let temp_dir = std::env::temp_dir().join(format!("dt_xmp_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);

        let saved_path = session.save_current_xmp(&temp_dir).unwrap();
        assert!(saved_path.exists());
        assert_eq!(
            saved_path.file_name().unwrap().to_str().unwrap(),
            "frame-3.tif.xmp"
        );

        let parsed = DarktableXmp::parse(&std::fs::read_to_string(&saved_path).unwrap()).unwrap();
        assert_eq!(parsed.derived_from, "frame-3.tif");
        assert_eq!(parsed.extract_orientation(), Orientation::Rotate270);
        let extracted_params = parsed.extract_negadoctor_params().unwrap();
        assert!((extracted_params.dmax - 3.10).abs() < 1e-4);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    fn mock_prepared_frame(frame_number: usize) -> PreparedFrame {
        let profile = RollProfile::pro_image_100();
        let artifact = FrameArtifact {
            frame_number,
            total_frames: 6,
            dpi: 725,
            raw_rect: nkscan::protocol::data::Rect {
                left: 0,
                right: 2,
                top: 0,
                bottom: 2,
            },
            samples: nkscan::protocol::decode::Samples {
                colors: vec![
                    vec![1000, 1000, 1000, 1000],
                    vec![1000, 1000, 1000, 1000],
                    vec![1000, 1000, 1000, 1000],
                ],
                ir: None,
            },
            pass: nkscan::scan::pass::Pass {
                layout: nkscan::protocol::image::Layout::single_line(2, 2, vec![1]),
                cooperation: Vec::new(),
                complete: true,
                blocks: 1,
                rows: 2,
                cols: 2,
            },
            crop: None,
            scan_metadata: crate::scanner::types::ScanMetadata {
                scanner_model: None,
                focus_position: None,
                exposures: None,
                hardware_samples: 1,
                software_passes: 1,
                infrared_cleaned_pixels: None,
            },
            roll: Some(profile.clone()),
        };
        PreparedFrame::new(artifact, profile)
    }

    #[test]
    fn sparse_frame_numbering_preserved_end_to_end() {
        let pipeline = ScannerColorPipeline::default_ls40().unwrap();
        let prepared = vec![
            mock_prepared_frame(2),
            mock_prepared_frame(4),
            mock_prepared_frame(6),
        ];

        let mut session = ReviewSession::from_prepared_frames(prepared, pipeline).unwrap();
        assert_eq!(session.frame_count(), 3);
        assert_eq!(session.frames[0].frame_number, 2);
        assert_eq!(session.frames[1].frame_number, 4);
        assert_eq!(session.frames[2].frame_number, 6);

        let temp_dir =
            std::env::temp_dir().join(format!("sparse_frames_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);

        // Frame 2
        let (tif2, xmp2) = session.save_current_frame_and_xmp(&temp_dir).unwrap();
        assert_eq!(tif2.file_name().unwrap(), "frame-2.tif");
        assert_eq!(xmp2.file_name().unwrap(), "frame-2.tif.xmp");

        // Frame 4
        assert!(session.next_frame());
        let (tif4, xmp4) = session.save_current_frame_and_xmp(&temp_dir).unwrap();
        assert_eq!(tif4.file_name().unwrap(), "frame-4.tif");
        assert_eq!(xmp4.file_name().unwrap(), "frame-4.tif.xmp");

        // Frame 6
        assert!(session.next_frame());
        let (tif6, xmp6) = session.save_current_frame_and_xmp(&temp_dir).unwrap();
        assert_eq!(tif6.file_name().unwrap(), "frame-6.tif");
        assert_eq!(xmp6.file_name().unwrap(), "frame-6.tif.xmp");

        assert!(!temp_dir.join("frame-1.tif").exists());
        assert!(!temp_dir.join("frame-3.tif").exists());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn single_frame_and_dynamic_add_preserves_numbering() {
        let pipeline = ScannerColorPipeline::default_ls40().unwrap();
        let mut session =
            ReviewSession::from_prepared_frames(vec![mock_prepared_frame(4)], pipeline).unwrap();
        assert_eq!(session.frame_count(), 1);
        assert_eq!(session.current_frame().unwrap().frame_number, 4);

        // Dynamically append frame 7
        session.add_prepared_frame(mock_prepared_frame(7)).unwrap();
        assert_eq!(session.frame_count(), 2);
        assert_eq!(session.frames[1].frame_number, 7);
    }
}
