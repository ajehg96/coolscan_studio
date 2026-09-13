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
use super::roll::RollProfile;
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
#[derive(Debug, Clone, Copy, PartialEq)]
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
pub fn analyse_pre_white_balance(image: &WorkingImage, roll: &RollProfile) -> TechnicalAnalysis {
    let stats = image.sample_region(None);
    let dmax = auto_dmax(roll.dmin, stats.min);
    let scan_bias = auto_scan_bias(roll.dmin, dmax, stats.max);
    TechnicalAnalysis { dmax, scan_bias }
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
    let stats = image.sample_region(None);
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
        let tech = analyse_pre_white_balance(&img, &roll);
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

        // Initialize NegadoctorParams with stage 1 results
        let mut params = NegadoctorParams::from_dmin(roll.dmin);
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
}
