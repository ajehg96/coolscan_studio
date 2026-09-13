//! Negadoctor negative inversion and print tone reproduction mathematics.
//!
//! Ported directly from Darktable's `src/iop/negadoctor.c`.
//! This module contains no scanner code, no GUI code, and no disk I/O.
//! It operates exclusively in floating-point linear working colour space
//! (e.g. Linear Rec.2020).

use serde::{Deserialize, Serialize};

/// Small positive floor constant (-32 EV) used by Darktable to avoid log(0) or division by zero.
pub const THRESHOLD: f32 = 2.3283064e-10;

/// Errors occurring during Negadoctor parameter validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NegadoctorError {
    InvalidDmin,
    InvalidDmax,
    InvalidWb,
    InvalidPaperGrade,
    InvalidPaperGloss,
    InvalidExposure,
    NonFiniteParameter(String),
}

impl std::fmt::Display for NegadoctorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NegadoctorError::InvalidDmin => {
                write!(f, "Film substrate D-min must be positive and finite")
            }
            NegadoctorError::InvalidDmax => write!(f, "D-max must be positive and finite"),
            NegadoctorError::InvalidWb => {
                write!(f, "White balance coefficients must be positive and finite")
            }
            NegadoctorError::InvalidPaperGrade => {
                write!(f, "Paper grade (gamma) must be positive and finite")
            }
            NegadoctorError::InvalidPaperGloss => {
                write!(f, "Paper gloss (soft clip) must be between 0.0 and 1.0")
            }
            NegadoctorError::InvalidExposure => {
                write!(f, "Print exposure must be positive and finite")
            }
            NegadoctorError::NonFiniteParameter(p) => write!(f, "Parameter '{p}' is not finite"),
        }
    }
}

impl std::error::Error for NegadoctorError {}

/// Full parameter set for Negadoctor negative inversion and paper printing emulation.
///
/// Matches Darktable's `dt_iop_negadoctor_params_t`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NegadoctorParams {
    /// Color of the unexposed film substrate in linear working space.
    /// Darktable: `Dmin[4]`
    pub dmin: [f32; 3],

    /// Maximum density (dynamic range) of the film negative.
    /// Darktable: `D_max`
    pub dmax: f32,

    /// Scan exposure bias (inversion offset).
    /// Darktable: `offset`
    pub offset: f32,

    /// Illuminant / highlight white balance multipliers.
    /// Darktable: `wb_high[4]`
    pub wb_high: [f32; 3],

    /// Base light / shadow white balance multipliers.
    /// Darktable: `wb_low[4]`
    pub wb_low: [f32; 3],

    /// Display paper black level (density correction).
    /// Darktable: `black`
    pub paper_black: f32,

    /// Paper grade / contrast gamma.
    /// Darktable: `gamma`
    pub paper_grade: f32,

    /// Paper gloss / highlights roll-off soft clip threshold.
    /// Darktable: `soft_clip`
    pub paper_gloss: f32,

    /// Print exposure adjustment.
    /// Darktable: `exposure`
    pub print_exposure: f32,
}

impl Default for NegadoctorParams {
    fn default() -> Self {
        Self {
            dmin: [1.0, 1.0, 1.0],
            dmax: 2.046,
            offset: -0.05,
            wb_high: [1.0, 1.0, 1.0],
            wb_low: [1.0, 1.0, 1.0],
            paper_black: 0.0755,
            paper_grade: 4.0,
            paper_gloss: 0.75,
            print_exposure: 0.9245,
        }
    }
}

impl NegadoctorParams {
    /// Creates parameters initialized with film substrate D-min and standard defaults.
    pub fn from_dmin(dmin: [f32; 3]) -> Self {
        Self {
            dmin,
            ..Default::default()
        }
    }

    /// Precomputes constants into a `PreparedNegadoctor` for fast per-pixel inversion.
    pub fn prepare(&self) -> PreparedNegadoctor {
        PreparedNegadoctor::new(self)
    }

    /// Validates parameter ranges and finiteness.
    pub fn validate(&self) -> Result<(), NegadoctorError> {
        for c in 0..3 {
            if !self.dmin[c].is_finite() || self.dmin[c] <= 0.0 {
                return Err(NegadoctorError::InvalidDmin);
            }
            if !self.wb_high[c].is_finite() || self.wb_high[c] <= 0.0 {
                return Err(NegadoctorError::InvalidWb);
            }
            if !self.wb_low[c].is_finite() || self.wb_low[c] <= 0.0 {
                return Err(NegadoctorError::InvalidWb);
            }
        }
        if !self.dmax.is_finite() || self.dmax <= 0.0 {
            return Err(NegadoctorError::InvalidDmax);
        }
        if !self.offset.is_finite() {
            return Err(NegadoctorError::NonFiniteParameter("offset".into()));
        }
        if !self.paper_black.is_finite() {
            return Err(NegadoctorError::NonFiniteParameter("paper_black".into()));
        }
        if !self.paper_grade.is_finite() || self.paper_grade <= 0.0 {
            return Err(NegadoctorError::InvalidPaperGrade);
        }
        if !self.paper_gloss.is_finite() || self.paper_gloss <= 0.0 || self.paper_gloss >= 1.0 {
            return Err(NegadoctorError::InvalidPaperGloss);
        }
        if !self.print_exposure.is_finite() || self.print_exposure <= 0.0 {
            return Err(NegadoctorError::InvalidExposure);
        }
        Ok(())
    }
}

/// Precomputed execution state for high-performance pixel processing.
///
/// Corresponds to Darktable's `dt_iop_negadoctor_data_t` generated in `commit_params`.
#[derive(Debug, Clone)]
pub struct PreparedNegadoctor {
    dmin: [f32; 3],
    wb_high: [f32; 3],
    offset: [f32; 3],
    black: f32,
    exposure: f32,
    gamma: f32,
    soft_clip: f32,
    soft_clip_comp: f32,
}

impl PreparedNegadoctor {
    /// Prepares parameters according to Darktable's `commit_params`.
    pub fn new(p: &NegadoctorParams) -> Self {
        let mut wb_high = [0.0f32; 3];
        let mut offset = [0.0f32; 3];
        for c in 0..3 {
            // Premultiply wb_high with 1.0 / D_max
            wb_high[c] = p.wb_high[c] / p.dmax;
            offset[c] = p.wb_high[c] * p.offset * p.wb_low[c];
        }

        // Arithmetic trick allowing to rewrite pixel inversion as FMA
        let black = -p.print_exposure * (1.0f32 + p.paper_black);
        let soft_clip_comp = 1.0f32 - p.paper_gloss;

        Self {
            dmin: p.dmin,
            wb_high,
            offset,
            black,
            exposure: p.print_exposure,
            gamma: p.paper_grade,
            soft_clip: p.paper_gloss,
            soft_clip_comp,
        }
    }

    /// Transforms a single linear negative pixel into positive print space.
    ///
    /// Corresponds to Darktable's `_process_pixel`.
    #[inline]
    pub fn invert_pixel(&self, pix_in: [f32; 3]) -> [f32; 3] {
        let mut pix_out = [0.0f32; 3];

        for c in 0..3 {
            let clamped = pix_in[c].max(THRESHOLD);
            // Convert transmission to density using Dmin as a fulcrum
            // In Darktable: density = Dmin / clamped; log_density = -log10(Dmin / clamped) = log10(clamped / Dmin)
            let log_density = -(self.dmin[c] / clamped).log10();

            // Correct density in log space
            let corrected_de = self.wb_high[c] * log_density + self.offset[c];
            let ten_to_x = 10.0f32.powf(corrected_de);

            // Print density on paper: ((1 - 10^corrected_de + black) * exposure)^gamma rewritten for FMA
            let val = -(self.exposure * ten_to_x + self.black);
            let print_linear = val.max(0.0);
            let print_gamma = print_linear.powf(self.gamma);

            // Compress highlights (OpenEXR soft-clip roll-off)
            if print_gamma > self.soft_clip {
                let clipped_gamma = -(print_gamma - self.soft_clip) / self.soft_clip_comp;
                let e_to_gamma = clipped_gamma.exp();
                pix_out[c] = self.soft_clip + (1.0f32 - e_to_gamma) * self.soft_clip_comp;
            } else {
                pix_out[c] = print_gamma;
            }
        }

        pix_out
    }

    /// Bulk transforms a slice of negative pixels into positive print space.
    pub fn render_positive(&self, input: &[[f32; 3]], output: &mut [[f32; 3]]) {
        assert_eq!(
            input.len(),
            output.len(),
            "Input and output slice lengths must match"
        );
        for (src, dst) in input.iter().zip(output.iter_mut()) {
            *dst = self.invert_pixel(*src);
        }
    }

    /// Bulk transforms a slice of negative pixels into a newly allocated vector.
    pub fn render_positive_vec(&self, input: &[[f32; 3]]) -> Vec<[f32; 3]> {
        let mut out = vec![[0.0f32; 3]; input.len()];
        self.render_positive(input, &mut out);
        out
    }
}

// ---------------------------------------------------------------------------
// Darktable Auto-Tuners / Pickers
// ---------------------------------------------------------------------------

/// Computes film dynamic range (D-max) from substrate D-min and the minimum
/// sampled channel values (negative highlights).
///
/// Ported from Darktable `apply_auto_Dmax`:
/// ```c
/// RGB[c] = log10f(p->Dmin[c] / fmaxf(self->picked_color_min[c], THRESHOLD));
/// p->D_max = v_maxf(RGB);
/// ```
pub fn auto_dmax(dmin: [f32; 3], sample_min: [f32; 3]) -> f32 {
    let mut max_density = 0.0f32;
    for c in 0..3 {
        let clamped = sample_min[c].max(THRESHOLD);
        let density = (dmin[c] / clamped).log10();
        max_density = max_density.max(density);
    }
    // Slider min in Darktable is 0.1
    max_density.max(0.1)
}

/// Computes scan exposure bias (offset) from D-min, D-max, and the maximum
/// sampled channel values (negative shadows / unexposed areas).
///
/// Ported from Darktable `apply_auto_offset`:
/// ```c
/// RGB[c] = log10f(p->Dmin[c] / fmaxf(self->picked_color_max[c], THRESHOLD)) / p->D_max;
/// p->offset = v_minf(RGB);
/// ```
pub fn auto_scan_bias(dmin: [f32; 3], dmax: f32, sample_max: [f32; 3]) -> f32 {
    let dmax = dmax.max(0.01);
    let mut min_norm_density = f32::INFINITY;
    for c in 0..3 {
        let clamped = sample_max[c].max(THRESHOLD);
        let norm_density = (dmin[c] / clamped).log10() / dmax;
        min_norm_density = min_norm_density.min(norm_density);
    }
    if min_norm_density.is_finite() {
        min_norm_density
    } else {
        0.0
    }
}

/// Computes shadow white balance (base light) from D-min, D-max, and sample mean.
///
/// Ported from Darktable `apply_auto_WB_low`:
/// ```c
/// RGB_min[c] = log10f(p->Dmin[c] / fmaxf(self->picked_color[c], THRESHOLD)) / p->D_max;
/// const float RGB_v_min = v_minf(RGB_min);
/// for(int c = 0; c < 3; c++) p->wb_low[c] = RGB_v_min / RGB_min[c];
/// ```
pub fn auto_shadow_wb(dmin: [f32; 3], dmax: f32, sample_mean: [f32; 3]) -> [f32; 3] {
    let dmax = dmax.max(0.01);
    let mut rgb_min = [0.0f32; 3];
    for c in 0..3 {
        let clamped = sample_mean[c].max(THRESHOLD);
        rgb_min[c] = (dmin[c] / clamped).log10() / dmax;
    }
    let v_min = rgb_min[0].min(rgb_min[1]).min(rgb_min[2]);
    let mut wb_low = [1.0f32; 3];
    for c in 0..3 {
        if rgb_min[c].abs() > 1e-6 {
            wb_low[c] = v_min / rgb_min[c];
        }
    }
    wb_low
}

/// Computes highlight white balance (illuminant) from D-min, D-max, scan bias,
/// shadow WB, and sample mean.
///
/// Ported from Darktable `apply_auto_WB_high`:
/// ```c
/// RGB_min[c] = fabsf(-1.0f / (p->offset * p->wb_low[c] - log10f(p->Dmin[c] / fmaxf(self->picked_color[c], THRESHOLD)) / p->D_max));
/// const float RGB_v_min = v_minf(RGB_min);
/// for(int c = 0; c < 3; c++) p->wb_high[c] = RGB_min[c] / RGB_v_min;
/// ```
///
/// Always normalises the minimum multiplier to `1.0`.
pub fn auto_highlight_wb(
    dmin: [f32; 3],
    dmax: f32,
    offset: f32,
    wb_low: [f32; 3],
    sample_mean: [f32; 3],
) -> [f32; 3] {
    let dmax = dmax.max(0.01);
    let mut rgb_min = [0.0f32; 3];
    for c in 0..3 {
        let clamped = sample_mean[c].max(THRESHOLD);
        let denom = offset * wb_low[c] - (dmin[c] / clamped).log10() / dmax;
        rgb_min[c] = (-1.0f32 / denom).abs();
    }
    let v_min = rgb_min[0].min(rgb_min[1]).min(rgb_min[2]);
    let mut wb_high = [1.0f32; 3];
    for c in 0..3 {
        if v_min > 1e-6 {
            wb_high[c] = rgb_min[c] / v_min;
        }
    }
    wb_high
}

/// Computes paper black level (density correction) from D-min, D-max, scan bias,
/// highlight WB, shadow WB, and maximum sample channel values (negative shadows).
///
/// Ported from Darktable `apply_auto_black`:
/// ```c
/// RGB[c] = -log10f(p->Dmin[c] / fmaxf(self->picked_color_max[c], THRESHOLD));
/// RGB[c] *= p->wb_high[c] / p->D_max;
/// RGB[c] += p->wb_low[c] * p->offset * p->wb_high[c];
/// RGB[c] = 0.1f - (1.0f - fast_exp10f(RGB[c]));
/// p->black = v_maxf(RGB);
/// ```
pub fn auto_paper_black(
    dmin: [f32; 3],
    dmax: f32,
    offset: f32,
    wb_high: [f32; 3],
    wb_low: [f32; 3],
    sample_max: [f32; 3],
) -> f32 {
    let dmax = dmax.max(0.01);
    let mut max_black = -f32::INFINITY;
    for c in 0..3 {
        let clamped = sample_max[c].max(THRESHOLD);
        let mut val = -(dmin[c] / clamped).log10();
        val *= wb_high[c] / dmax;
        val += wb_low[c] * offset * wb_high[c];
        let black_c = 0.1f32 - (1.0f32 - 10.0f32.powf(val));
        max_black = max_black.max(black_c);
    }
    if max_black.is_finite() {
        max_black
    } else {
        0.0755
    }
}

/// Computes print exposure adjustment from D-min, D-max, scan bias,
/// highlight WB, shadow WB, paper black, and minimum sample channel values (negative highlights).
///
/// Ported from Darktable `apply_auto_exposure`:
/// ```c
/// RGB[c] = -log10f(p->Dmin[c] / fmaxf(self->picked_color_min[c], THRESHOLD));
/// RGB[c] *= p->wb_high[c] / p->D_max;
/// RGB[c] += p->wb_low[c] * p->offset;
/// RGB[c] = 0.96f / (1.0f - fast_exp10f(RGB[c]) + p->black);
/// p->exposure = v_minf(RGB);
/// ```
pub fn auto_print_exposure(
    dmin: [f32; 3],
    dmax: f32,
    offset: f32,
    wb_high: [f32; 3],
    wb_low: [f32; 3],
    paper_black: f32,
    sample_min: [f32; 3],
) -> f32 {
    let dmax = dmax.max(0.01);
    let mut min_exp = f32::INFINITY;
    for c in 0..3 {
        let clamped = sample_min[c].max(THRESHOLD);
        let mut val = -(dmin[c] / clamped).log10();
        val *= wb_high[c] / dmax;
        val += wb_low[c] * offset;
        let denom = 1.0f32 - 10.0f32.powf(val) + paper_black;
        if denom.abs() > 1e-6 {
            let exp_c = 0.96f32 / denom;
            min_exp = min_exp.min(exp_c);
        }
    }
    if min_exp.is_finite() && min_exp > 0.0 {
        min_exp
    } else {
        0.9245
    }
}

/// Convenience function rendering positive image from Negadoctor parameters.
pub fn render_positive(params: &NegadoctorParams, input: &[[f32; 3]], output: &mut [[f32; 3]]) {
    let prepared = params.prepare();
    prepared.render_positive(input, output);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_dmax_picks_channel_maximum_density() {
        let dmin = [0.90, 0.90, 0.90];
        // Channel 0 has density log10(0.90 / 0.09) = 1.0
        // Channel 1 has density log10(0.90 / 0.009) = 2.0
        // Channel 2 has density log10(0.90 / 0.0009) = 3.0
        let sample_min = [0.09, 0.009, 0.0009];
        let dmax = auto_dmax(dmin, sample_min);
        assert!((dmax - 3.0).abs() < 1e-4, "Expected ~3.0, got {dmax}");
    }

    #[test]
    fn auto_scan_bias_picks_minimum_normalized_ratio() {
        let dmin = [0.90, 0.90, 0.90];
        let dmax = 2.0;
        // Channel 0: log10(0.90 / 0.90) / 2.0 = 0.0
        // Channel 1: log10(0.90 / 0.45) / 2.0 = log10(2) / 2.0 = 0.1505
        // Channel 2: log10(0.90 / 1.80) / 2.0 = log10(0.5) / 2.0 = -0.1505
        let sample_max = [0.90, 0.45, 1.80];
        let bias = auto_scan_bias(dmin, dmax, sample_max);
        assert!(
            (bias - (-0.1505)).abs() < 1e-4,
            "Expected -0.1505, got {bias}"
        );
    }

    #[test]
    fn auto_highlight_wb_normalises_minimum_channel_to_one() {
        let dmin = [0.8965, 0.9093, 0.8816];
        let dmax = 3.2718;
        let offset = 0.1060;
        let wb_low = [1.0, 1.0, 1.0];
        // Picked neutral grey / white patch
        let sample_mean = [0.015, 0.020, 0.035];
        let wb_high = auto_highlight_wb(dmin, dmax, offset, wb_low, sample_mean);

        // One channel must be exactly 1.0, and none below 1.0
        let min_coeff = wb_high[0].min(wb_high[1]).min(wb_high[2]);
        assert!(
            (min_coeff - 1.0).abs() < 1e-5,
            "Expected minimum WB channel to be 1.0, got {min_coeff}"
        );
        for &val in &wb_high {
            assert!(val >= 1.0 - 1e-5);
        }
    }

    #[test]
    fn paper_black_changes_with_highlight_wb() {
        let dmin = [0.8965, 0.9093, 0.8816];
        let dmax = 3.2718;
        let offset = 0.1060;
        let wb_low = [1.0, 1.0, 1.0];
        let sample_max = [0.88, 0.89, 0.86];

        let wb_high_1 = [1.0, 1.0, 1.0];
        let black_1 = auto_paper_black(dmin, dmax, offset, wb_high_1, wb_low, sample_max);

        let wb_high_2 = [1.95, 1.62, 1.0];
        let black_2 = auto_paper_black(dmin, dmax, offset, wb_high_2, wb_low, sample_max);

        assert!(
            (black_1 - black_2).abs() > 1e-4,
            "Paper black should change when highlight WB changes: {black_1} vs {black_2}"
        );
    }

    #[test]
    fn print_exposure_changes_with_paper_black() {
        let dmin = [0.8965, 0.9093, 0.8816];
        let dmax = 3.2718;
        let offset = 0.1060;
        let wb_high = [1.95, 1.62, 1.0];
        let wb_low = [1.0, 1.0, 1.0];
        let sample_min = [0.0005, 0.0006, 0.0007];

        let exp_1 = auto_print_exposure(dmin, dmax, offset, wb_high, wb_low, 0.05, sample_min);
        let exp_2 = auto_print_exposure(dmin, dmax, offset, wb_high, wb_low, 0.15, sample_min);

        assert!(
            (exp_1 - exp_2).abs() > 1e-4,
            "Print exposure should change when paper black changes: {exp_1} vs {exp_2}"
        );
    }

    #[test]
    fn golden_darktable_values_reproduction() {
        // Values from real Darktable XMP sidecar: scans-strip-02/frame-1.tif.xmp
        let dmin = [0.8965, 0.9093, 0.8816];
        let expected_dmax = 3.2718;
        let expected_offset = 0.1060;
        let expected_black = 0.1000;

        // An unexposed film base area in the cropped frame
        let sample_max = [0.4034, 0.4092, 0.3967];
        let offset = auto_scan_bias(dmin, expected_dmax, sample_max);
        assert!(
            (offset - expected_offset).abs() < 1e-3,
            "Scan bias expected {expected_offset}, got {offset}"
        );

        // Frame shadows (sample_max) yield paper black of ~0.10
        let black = auto_paper_black(
            dmin,
            expected_dmax,
            expected_offset,
            [1.9437, 1.6161, 1.0],
            [1.0, 1.0, 1.0],
            sample_max,
        );
        assert!(
            (black - expected_black).abs() < 1e-3,
            "Paper black expected {expected_black}, got {black}"
        );
    }

    #[test]
    fn render_positive_inversion_sanity() {
        let params = NegadoctorParams {
            dmin: [0.8965, 0.9093, 0.8816],
            dmax: 3.2718,
            offset: 0.1060,
            wb_high: [1.9437, 1.6161, 1.0],
            wb_low: [1.0, 1.0, 1.0],
            paper_black: 0.1000,
            paper_grade: 4.5,
            paper_gloss: 0.75,
            print_exposure: 1.1173,
        };
        let prepared = params.prepare();

        // 1. Unexposed film base (dmin) should render to pure print black [0, 0, 0]
        let base_out = prepared.invert_pixel(params.dmin);
        assert!(base_out[0] < 1e-4, "Base Red: {}", base_out[0]);
        assert!(base_out[1] < 1e-4, "Base Green: {}", base_out[1]);
        assert!(base_out[2] < 1e-4, "Base Blue: {}", base_out[2]);

        // 2. High density negative area (positive highlight) should be close to 1.0 with soft roll-off
        let highlight_negative = [
            params.dmin[0] * 10.0f32.powf(-2.8),
            params.dmin[1] * 10.0f32.powf(-2.8),
            params.dmin[2] * 10.0f32.powf(-2.8),
        ];
        let highlight_out = prepared.invert_pixel(highlight_negative);
        assert!(
            highlight_out[0] > 0.9,
            "Highlight Red: {}",
            highlight_out[0]
        );
        assert!(
            highlight_out[1] > 0.9,
            "Highlight Green: {}",
            highlight_out[1]
        );
        assert!(
            highlight_out[2] > 0.85,
            "Highlight Blue: {}",
            highlight_out[2]
        );
        assert!(highlight_out[0] <= 1.0, "Highlights must not exceed 1.0");
    }

    #[test]
    fn parameter_validation_catches_invalid_inputs() {
        let mut p = NegadoctorParams::default();
        assert!(p.validate().is_ok());

        p.dmin[0] = -1.0;
        assert_eq!(p.validate(), Err(NegadoctorError::InvalidDmin));

        p.dmin[0] = 1.0;
        p.dmax = 0.0;
        assert_eq!(p.validate(), Err(NegadoctorError::InvalidDmax));

        p.dmax = 2.0;
        p.paper_gloss = 1.5;
        assert_eq!(p.validate(), Err(NegadoctorError::InvalidPaperGloss));

        p.paper_gloss = 0.75;
        p.paper_grade = f32::NAN;
        assert_eq!(p.validate(), Err(NegadoctorError::InvalidPaperGrade));
    }
}
