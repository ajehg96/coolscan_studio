//! Automatic frame analysis and Negadoctor parameter derivation.
//!
//! Provides the two-stage technical analysis pipeline:
//! 1. `analyse_pre_white_balance`: Whole-frame extrema analysis computing D-max and scan exposure bias.
//! 2. `finish_after_white_balance`: Post-white-balance analysis computing paper black and print exposure.

use serde::{Deserialize, Serialize};

use super::color::{ColorError, ColorTransform};
use super::negadoctor::{
    NegadoctorParams, auto_dmax, auto_highlight_wb, auto_paper_black, auto_print_exposure,
    auto_scan_bias, auto_shadow_wb,
};
use super::roll::{RollProfile, RollProfileError};
use nkscan::{protocol::decode::Samples, scan::pass::Pass};

/// Axis-aligned pixel rectangle for regional sampling and analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleRect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl SampleRect {
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Statistics extracted from an image or region.
///
/// Matches Darktable's color picker statistics (`picked_color_min`, `picked_color_max`, `picked_color`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct ImageSampleStats {
    /// Minimum value per channel across the sample.
    pub min: [f32; 3],
    /// Maximum value per channel across the sample.
    pub max: [f32; 3],
    /// Arithmetic mean per channel across the sample.
    pub mean: [f32; 3],
    /// Number of pixels sampled.
    pub count: usize,
}

/// In-memory image buffer in floating-point linear working colour space (e.g. Linear Rec.2020).
#[derive(Debug, Clone, PartialEq)]
pub struct WorkingImage {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[f32; 3]>,
}

impl WorkingImage {
    /// Creates a working image from dimensions and interleaved RGB float pixels.
    pub fn new(width: usize, height: usize, pixels: Vec<[f32; 3]>) -> Self {
        assert_eq!(
            pixels.len(),
            width * height,
            "Pixel buffer length must equal width * height"
        );
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Converts raw scanner planar 16-bit samples into a linear working image via a color pipeline.
    pub fn from_scanner_samples(
        samples: &Samples,
        pass: &Pass,
        color_transform: &impl ColorTransform,
    ) -> Result<Self, ColorError> {
        let pixels = color_transform.transform_planar_samples(samples, pass)?;
        Ok(Self {
            width: pass.cols,
            height: pass.rows,
            pixels,
        })
    }

    /// Retrieves a single pixel at `(x, y)`.
    pub fn pixel(&self, x: usize, y: usize) -> Option<[f32; 3]> {
        if x < self.width && y < self.height {
            Some(self.pixels[y * self.width + x])
        } else {
            None
        }
    }

    /// Computes channel statistics over a specified rectangle (or the entire image if None).
    ///
    /// Follows Darktable's exact picker logic:
    /// - min per channel
    /// - max per channel
    /// - arithmetic mean per channel
    pub fn sample_region(&self, rect: Option<SampleRect>) -> ImageSampleStats {
        let (x_start, y_start, x_end, y_end) = match rect {
            Some(r) => {
                let x0 = r.x.min(self.width);
                let y0 = r.y.min(self.height);
                let x1 = (r.x + r.width).min(self.width);
                let y1 = (r.y + r.height).min(self.height);
                (x0, y0, x1, y1)
            }
            None => (0, 0, self.width, self.height),
        };

        if x_start >= x_end || y_start >= y_end {
            return ImageSampleStats {
                min: [0.0; 3],
                max: [1.0; 3],
                mean: [0.5; 3],
                count: 0,
            };
        }

        let mut low = [f32::INFINITY; 3];
        let mut high = [-f32::INFINITY; 3];
        let mut acc = [0.0f64; 3];
        let mut count = 0usize;

        for y in y_start..y_end {
            let row_offset = y * self.width;
            for x in x_start..x_end {
                let px = self.pixels[row_offset + x];
                for c in 0..3 {
                    low[c] = low[c].min(px[c]);
                    high[c] = high[c].max(px[c]);
                    acc[c] += px[c] as f64;
                }
                count += 1;
            }
        }

        if count == 0 {
            return ImageSampleStats {
                min: [0.0; 3],
                max: [1.0; 3],
                mean: [0.5; 3],
                count: 0,
            };
        }

        let mean = [
            (acc[0] / count as f64) as f32,
            (acc[1] / count as f64) as f32,
            (acc[2] / count as f64) as f32,
        ];

        ImageSampleStats {
            min: low,
            max: high,
            mean,
            count,
        }
    }

    /// Creates a downscaled working preview buffer using box-filter area averaging.
    ///
    /// If both dimensions are already within `max_dimension`, returns a clone.
    pub fn downscale_to_preview(&self, max_dimension: usize) -> Self {
        let max_dim = self.width.max(self.height);
        if max_dim <= max_dimension || max_dimension == 0 {
            return self.clone();
        }

        let scale = ((max_dim as f64 / max_dimension as f64).ceil() as usize).max(1);
        let new_w = self.width.div_ceil(scale);
        let new_h = self.height.div_ceil(scale);
        let mut downscaled_pixels = Vec::with_capacity(new_w * new_h);

        for by in 0..new_h {
            let y_start = by * scale;
            let y_end = (y_start + scale).min(self.height);
            for bx in 0..new_w {
                let x_start = bx * scale;
                let x_end = (x_start + scale).min(self.width);

                let mut sum = [0.0f32; 3];
                let mut count = 0usize;

                for y in y_start..y_end {
                    let row_offset = y * self.width;
                    for x in x_start..x_end {
                        let px = self.pixels[row_offset + x];
                        sum[0] += px[0];
                        sum[1] += px[1];
                        sum[2] += px[2];
                        count += 1;
                    }
                }

                if count > 0 {
                    let inv = 1.0 / count as f32;
                    downscaled_pixels.push([sum[0] * inv, sum[1] * inv, sum[2] * inv]);
                } else {
                    downscaled_pixels.push([0.0, 0.0, 0.0]);
                }
            }
        }

        Self {
            width: new_w,
            height: new_h,
            pixels: downscaled_pixels,
        }
    }

    /// Computes channel statistics over a safe interior region (excluding a 5% border inset
    /// on images at least 64×64 pixels) so residual SA-21 adapter mask edges or uncropped film
    /// rebate slivers at the border do not skew whole-frame `min`/`max` extrema.
    ///
    /// On small synthetic or fixture images (< 64 pixels on either axis), samples the full image.
    pub fn sample_interior_extrema(&self) -> ImageSampleStats {
        if self.width >= 64 && self.height >= 64 {
            let inset_x = (self.width / 20).max(2);
            let inset_y = (self.height / 20).max(2);
            let rect = SampleRect::new(
                inset_x,
                inset_y,
                self.width.saturating_sub(inset_x * 2),
                self.height.saturating_sub(inset_y * 2),
            );
            self.sample_region(Some(rect))
        } else {
            self.sample_region(None)
        }
    }
}

/// Technical analysis results computed prior to white-balance calibration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TechnicalAnalysis {
    /// Film dynamic range (D-max).
    pub dmax: f32,
    /// Scan exposure bias (offset).
    pub scan_bias: f32,
}

/// Stage 1 analysis: Computes whole-frame technical parameters (D-max and scan exposure bias)
/// from the cropped image area and roll substrate profile.
pub fn analyse_pre_white_balance(
    image: &WorkingImage,
    roll: &RollProfile,
) -> Result<TechnicalAnalysis, RollProfileError> {
    let dmin = roll.dmin().ok_or(RollProfileError::UncalibratedRoll)?;
    let stats = image.sample_interior_extrema();
    let dmax = auto_dmax(dmin, stats.min);
    let scan_bias = auto_scan_bias(dmin, dmax, stats.max);
    Ok(TechnicalAnalysis { dmax, scan_bias })
}

/// Samples a region of unexposed film base (rebate / inter-frame gap) to calibrate `D-min`.
pub fn sample_dmin_region(
    image: &WorkingImage,
    rect: SampleRect,
) -> Result<[f32; 3], RollProfileError> {
    let stats = image.sample_region(Some(rect));
    if stats.count == 0 {
        return Err(RollProfileError::InvalidDmin {
            channel: "RGB",
            value: 0.0,
            reason: "Empty sample region",
        });
    }
    super::roll::validate_dmin(stats.mean)?;
    Ok(stats.mean)
}

/// Automatically estimates linear working-space `D-min` from the unexposed inter-frame gap
/// (overscan columns) of a raw `FrameArtifact`.
///
/// Inspects the overscan columns outside the cropped frame (or the outer 12% travel columns
/// when uncropped) across the central 50% vertical rows (safely inside the SA-21 aperture),
/// selects the brightest unclipped film-base columns, and transforms their mean 16-bit sensor
/// RGB into linear Rec.2020 working space.
pub fn estimate_dmin_from_overscan(
    artifact: &crate::scanner::types::FrameArtifact,
    color_transform: &impl ColorTransform,
) -> Option<[f32; 3]> {
    let rows = artifact.pass.rows;
    let cols = artifact.pass.cols;
    if rows == 0 || cols < 4 || artifact.samples.colors.len() < 3 {
        return None;
    }

    let r_plane = &artifact.samples.colors[0];
    let g_plane = &artifact.samples.colors[1];
    let b_plane = &artifact.samples.colors[2];
    let total_pixels = rows * cols;
    if r_plane.len() < total_pixels || g_plane.len() < total_pixels || b_plane.len() < total_pixels
    {
        return None;
    }

    let row_start = rows / 4;
    let row_end = (rows * 3 / 4).max(row_start + 1).min(rows);
    let row_count = (row_end - row_start) as u64;

    // Candidate travel columns: prefer overscan outside `artifact.crop`, falling back to outer 12% edges
    let mut candidate_cols: Vec<usize> = Vec::new();
    if let Some(ref crop) = artifact.crop
        && crop.accepted
    {
        let left_end = crop.columns.0.saturating_sub(1).min(cols);
        let right_start = (crop.columns.1 + 1).min(cols);
        candidate_cols.extend(0..left_end);
        candidate_cols.extend(right_start..cols);
    }
    if candidate_cols.is_empty() {
        let edge_span = (cols / 8).clamp(1, cols / 2);
        candidate_cols.extend(0..edge_span);
        candidate_cols.extend(cols.saturating_sub(edge_span)..cols);
    }

    // Compute per-column average [R, G, B] in 16-bit sensor space over central rows
    let mut col_averages: Vec<([u16; 3], u64)> = Vec::with_capacity(candidate_cols.len());
    for col in candidate_cols {
        let mut r_sum = 0u64;
        let mut g_sum = 0u64;
        let mut b_sum = 0u64;
        for row in row_start..row_end {
            let idx = row * cols + col;
            r_sum += r_plane[idx] as u64;
            g_sum += g_plane[idx] as u64;
            b_sum += b_plane[idx] as u64;
        }
        let r_avg = (r_sum / row_count) as u16;
        let g_avg = (g_sum / row_count) as u16;
        let b_avg = (b_sum / row_count) as u16;
        let luma = r_avg as u64 + g_avg as u64 + b_avg as u64;
        // Exclude black adapter mask (< 1500 per channel) and hard sensor saturation
        if luma >= 4500 && r_avg < 65500 && g_avg < 65500 && b_avg < 65500 {
            col_averages.push(([r_avg, g_avg, b_avg], luma));
        }
    }

    if col_averages.is_empty() {
        return None;
    }

    // Sort by transmission luminance descending: unexposed film base has the highest transmission
    col_averages.sort_by_key(|a| std::cmp::Reverse(a.1));
    let top_n = (col_averages.len() / 3).clamp(1, 16);
    let mut r_acc = 0u64;
    let mut g_acc = 0u64;
    let mut b_acc = 0u64;
    for &(rgb, _) in &col_averages[..top_n] {
        r_acc += rgb[0] as u64;
        g_acc += rgb[1] as u64;
        b_acc += rgb[2] as u64;
    }
    let mean_u16 = [
        (r_acc / top_n as u64) as u16,
        (g_acc / top_n as u64) as u16,
        (b_acc / top_n as u64) as u16,
    ];

    let linear_dmin = color_transform.transform_u16_rgb(mean_u16);
    if super::roll::validate_dmin(linear_dmin).is_ok()
        && linear_dmin[0] >= 0.05
        && linear_dmin[1] >= 0.05
        && linear_dmin[2] >= 0.05
    {
        Some(linear_dmin)
    } else {
        None
    }
}

/// Samples a neutral highlight patch to derive illuminant white balance.
pub fn sample_highlight_wb(
    image: &WorkingImage,
    params: &NegadoctorParams,
    rect: Option<SampleRect>,
) -> [f32; 3] {
    let stats = image.sample_region(rect);
    auto_highlight_wb(
        params.dmin,
        params.dmax,
        params.offset,
        params.wb_low,
        stats.mean,
    )
}

/// Samples a neutral shadow patch to derive base light white balance.
pub fn sample_shadow_wb(
    image: &WorkingImage,
    params: &NegadoctorParams,
    rect: Option<SampleRect>,
) -> [f32; 3] {
    let stats = image.sample_region(rect);
    auto_shadow_wb(params.dmin, params.dmax, stats.mean)
}

/// Stage 2 analysis: Completes parameter calculation after white balance has been established,
/// computing paper black and print exposure adjustment over the full cropped image.
pub fn finish_after_white_balance(image: &WorkingImage, params: &mut NegadoctorParams) {
    let stats = image.sample_interior_extrema();
    params.paper_black = auto_paper_black(
        params.dmin,
        params.dmax,
        params.offset,
        params.wb_high,
        params.wb_low,
        stats.max,
    );
    params.print_exposure = auto_print_exposure(
        params.dmin,
        params.dmax,
        params.offset,
        params.wb_high,
        params.wb_low,
        params.paper_black,
        stats.min,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_image_pixel_access_and_bounds() {
        let pixels = vec![
            [0.1, 0.2, 0.3],
            [0.4, 0.5, 0.6],
            [0.7, 0.8, 0.9],
            [1.0, 1.1, 1.2],
        ];
        let img = WorkingImage::new(2, 2, pixels);
        assert_eq!(img.pixel(0, 0), Some([0.1, 0.2, 0.3]));
        assert_eq!(img.pixel(1, 1), Some([1.0, 1.1, 1.2]));
        assert_eq!(img.pixel(2, 0), None);
        assert_eq!(img.pixel(0, 2), None);
    }

    #[test]
    fn regional_sampling_calculates_channel_min_max_mean() {
        // 3x3 image
        let mut pixels = vec![[0.5f32; 3]; 9];
        // Set specific pixels in region
        pixels[0] = [0.1, 0.2, 0.3];
        pixels[1] = [0.9, 0.8, 0.7];
        pixels[3] = [0.3, 0.4, 0.5];
        pixels[4] = [0.7, 0.6, 0.5];

        let img = WorkingImage::new(3, 3, pixels);
        let rect = SampleRect::new(0, 0, 2, 2);
        let stats = img.sample_region(Some(rect));

        assert_eq!(stats.count, 4);
        assert!((stats.min[0] - 0.1).abs() < 1e-5);
        assert!((stats.max[0] - 0.9).abs() < 1e-5);
        assert!((stats.mean[0] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn out_of_bounds_sample_rect_is_clipped_safely() {
        let img = WorkingImage::new(2, 2, vec![[0.5f32; 3]; 4]);
        let rect = SampleRect::new(1, 1, 10, 10);
        let stats = img.sample_region(Some(rect));
        assert_eq!(stats.count, 1);

        let disjoint = SampleRect::new(10, 10, 5, 5);
        let empty_stats = img.sample_region(Some(disjoint));
        assert_eq!(empty_stats.count, 0);
    }

    #[test]
    fn two_stage_analysis_pipeline_golden_match() {
        // Synthetic image matching known extrema from Pro Image frame-1:
        // sample_min = [0.0004794, 0.0005, 0.0006] -> yields dmax = 3.2718
        // sample_max = [0.4034, 0.4092, 0.3967] -> yields scan bias = 0.1060
        let roll = RollProfile::pro_image_100();
        // Roll dmin: [0.8965, 0.9093, 0.8816]

        let pixels = vec![
            [0.0004794, 0.0005, 0.0006], // pixel 0: negative highlights / densest
            [0.4034, 0.4092, 0.3967],    // pixel 1: negative shadows / thinnest
            [0.05, 0.06, 0.07],          // pixel 2: midtone
            [0.05, 0.06, 0.07],          // pixel 3: midtone
        ];
        let img = WorkingImage::new(2, 2, pixels);

        // Stage 1: Pre-WB Technical Analysis
        let tech = analyse_pre_white_balance(&img, &roll).unwrap();
        assert!(
            (tech.dmax - 3.2718).abs() < 1e-3,
            "Expected D-max ~3.2718, got {}",
            tech.dmax
        );
        assert!(
            (tech.scan_bias - 0.1060).abs() < 1e-3,
            "Expected scan bias ~0.1060, got {}",
            tech.scan_bias
        );

        // Verify uncalibrated roll is rejected
        let uncalibrated = RollProfile::portra_400();
        assert!(matches!(
            analyse_pre_white_balance(&img, &uncalibrated),
            Err(RollProfileError::UncalibratedRoll)
        ));

        // Initialize NegadoctorParams with stage 1 results
        let mut params = NegadoctorParams::from_dmin(roll.dmin().unwrap());
        params.dmax = tech.dmax;
        params.offset = tech.scan_bias;

        // Simulate WB selection (from frame-1)
        params.wb_high = [1.9437, 1.6161, 1.0];

        // Stage 2: Finish after white balance
        finish_after_white_balance(&img, &mut params);
        assert!(
            (params.paper_black - 0.1000).abs() < 1e-3,
            "Expected paper black ~0.1000, got {}",
            params.paper_black
        );
        assert!(
            (params.print_exposure - 0.8844).abs() < 1e-3,
            "Expected print exposure ~0.8844, got {}",
            params.print_exposure
        );
    }

    #[test]
    fn downscale_to_preview_preserves_area_averages() {
        // 4x4 image with known block values
        let mut pixels = Vec::with_capacity(16);
        for y in 0..4 {
            for x in 0..4 {
                let v = if x < 2 && y < 2 {
                    0.2f32
                } else if x >= 2 && y < 2 {
                    0.4f32
                } else if x < 2 && y >= 2 {
                    0.6f32
                } else {
                    0.8f32
                };
                pixels.push([v, v, v]);
            }
        }
        let img = WorkingImage::new(4, 4, pixels);

        // Downscale with max_dimension 2 (scale factor 2 -> 2x2 output)
        let preview = img.downscale_to_preview(2);
        assert_eq!(preview.width, 2);
        assert_eq!(preview.height, 2);

        assert!((preview.pixel(0, 0).unwrap()[0] - 0.2).abs() < 1e-5);
        assert!((preview.pixel(1, 0).unwrap()[0] - 0.4).abs() < 1e-5);
        assert!((preview.pixel(0, 1).unwrap()[0] - 0.6).abs() < 1e-5);
        assert!((preview.pixel(1, 1).unwrap()[0] - 0.8).abs() < 1e-5);

        // Downscale with max_dimension >= 4 returns unchanged clone
        let no_change = img.downscale_to_preview(4);
        assert_eq!(no_change.width, 4);
        assert_eq!(no_change.height, 4);
        assert_eq!(no_change.pixels, img.pixels);
    }

    #[test]
    fn interior_extrema_ignores_border_mask_and_rebate_on_real_frames() {
        let roll = RollProfile::pro_image_100();
        let width = 100;
        let height = 100;
        let mut pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let is_border = !(3..97).contains(&x) || !(3..97).contains(&y);
                if is_border {
                    // Simulate black SA-21 mask rail (0.00005) on top/bottom and bright rebate (0.89) on left/right
                    if !(3..97).contains(&y) {
                        pixels.push([0.00005, 0.00005, 0.00005]);
                    } else {
                        pixels.push([0.89, 0.90, 0.88]);
                    }
                } else if x == 50 && y == 50 {
                    pixels.push([0.005, 0.006, 0.007]); // interior highlight
                } else if x == 60 && y == 60 {
                    pixels.push([0.35, 0.36, 0.34]); // interior shadow
                } else {
                    pixels.push([0.08, 0.09, 0.10]);
                }
            }
        }
        let img = WorkingImage::new(width, height, pixels);
        let stats = img.sample_interior_extrema();
        assert!((stats.min[0] - 0.005).abs() < 1e-5);
        assert!((stats.max[0] - 0.35).abs() < 1e-5);

        let tech = analyse_pre_white_balance(&img, &roll).unwrap();
        // Should match interior min=0.005, max=0.35 rather than the 0.00005 border mask!
        assert!(
            tech.dmax < 2.5,
            "Border mask must not spike D-max: {}",
            tech.dmax
        );
    }

    #[test]
    fn estimate_dmin_from_overscan_recovers_film_base_transmission() {
        use crate::processing::color::ScannerColorPipeline;
        use crate::scanner::types::{FrameArtifact, ScanMetadata};

        let pipeline = ScannerColorPipeline::default_ls40().unwrap();
        let rows = 40;
        let cols = 100;
        let mut r_plane = Vec::with_capacity(rows * cols);
        let mut g_plane = Vec::with_capacity(rows * cols);
        let mut b_plane = Vec::with_capacity(rows * cols);

        for _ in 0..rows {
            for col in 0..cols {
                // Columns 0..10 and 90..100: unexposed film base gap (bright orange mask)
                if !(10..90).contains(&col) {
                    r_plane.push(52000u16);
                    g_plane.push(44000u16);
                    b_plane.push(36000u16);
                } else {
                    // Exposed negative image area (denser, lower transmission)
                    r_plane.push(12000u16);
                    g_plane.push(10000u16);
                    b_plane.push(8000u16);
                }
            }
        }

        let artifact = FrameArtifact {
            frame_number: 1,
            total_frames: 6,
            dpi: 725,
            raw_rect: nkscan::protocol::data::Rect {
                left: 0,
                right: cols as u32,
                top: 0,
                bottom: rows as u32,
            },
            samples: nkscan::protocol::decode::Samples {
                colors: vec![r_plane, g_plane, b_plane],
                ir: None,
            },
            pass: nkscan::scan::pass::Pass {
                layout: nkscan::protocol::image::Layout::single_line(
                    rows as u32,
                    cols as u32,
                    vec![1, 2, 3],
                ),
                cooperation: Vec::new(),
                complete: true,
                blocks: 1,
                rows,
                cols,
            },
            crop: None,
            scan_metadata: ScanMetadata {
                scanner_model: None,
                focus_position: None,
                exposures: None,
                hardware_samples: 1,
                software_passes: 1,
                infrared_cleaned_pixels: None,
            },
            roll: None,
        };

        let expected = pipeline.transform_u16_rgb([52000, 44000, 36000]);
        let estimated = estimate_dmin_from_overscan(&artifact, &pipeline).unwrap();
        for c in 0..3 {
            assert!(
                (estimated[c] - expected[c]).abs() < 1e-4,
                "Channel {c} mismatch: got {}, expected {}",
                estimated[c],
                expected[c]
            );
        }
    }
}
