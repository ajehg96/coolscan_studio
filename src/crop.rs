//! Automatic cropping decision with confidence and conservative fallback.
//!
//! Enforces the safety criteria from investigation/PLAN.md:
//! 1. Fewer than 5% film-like pixels means confidently inside the image.
//! 2. A high film-like fraction (> 50%) means confidently in the gap.
//! 3. Intermediate or inconsistent values are treated as ambiguous.
//! 4. Require the result across consecutive columns (`run_length`, default 8)
//!    to suppress dust and scratches.
//! 5. Crop only when BOTH edges have adequate confidence.
//! 6. Retain conservative overscan when either edge is ambiguous.
//! 7. The system must NEVER remove image content.

/// Edge detection confidence along one boundary (leading or trailing travel edge).
#[derive(Debug, Clone, PartialEq)]
pub enum EdgeConfidence {
    /// Confidently detected inside-image boundary at the given column and scanner dot.
    Confident { column: usize, dots: u32 },
    /// The edge is ambiguous; fallback retains the safe overscan boundary.
    Ambiguous {
        fallback_column: usize,
        fallback_dots: u32,
        reason: &'static str,
    },
}

impl EdgeConfidence {
    pub fn column(&self) -> usize {
        match *self {
            EdgeConfidence::Confident { column, .. } => column,
            EdgeConfidence::Ambiguous {
                fallback_column, ..
            } => fallback_column,
        }
    }

    pub fn dots(&self) -> u32 {
        match *self {
            EdgeConfidence::Confident { dots, .. } => dots,
            EdgeConfidence::Ambiguous { fallback_dots, .. } => fallback_dots,
        }
    }

    pub fn is_confident(&self) -> bool {
        matches!(self, EdgeConfidence::Confident { .. })
    }
}

/// Chromaticity model learned from confirmed unexposed film gaps.
#[derive(Debug, Clone, PartialEq)]
pub struct FilmChromaticityModel {
    /// Normalized chromaticity center: [R/mean, G/mean, B/mean].
    pub center: [f64; 3],
    /// Tolerance radius in normalized chromaticity space.
    pub radius: f64,
}

impl FilmChromaticityModel {
    /// Creates a new model from a slice of RGB pixels sampled from confirmed gap regions.
    /// Each pixel is an [R, G, B] triplet.
    pub fn from_gap_pixels(pixels: &[[f64; 3]]) -> Result<Self, &'static str> {
        if pixels.len() < 8 {
            return Err("Need at least 8 gap pixels to fit chromaticity model");
        }

        let mut r_chromas = Vec::with_capacity(pixels.len());
        let mut g_chromas = Vec::with_capacity(pixels.len());
        let mut b_chromas = Vec::with_capacity(pixels.len());

        for &[r, g, b] in pixels {
            let mean = ((r + g + b) / 3.0).max(1.0);
            r_chromas.push(r / mean);
            g_chromas.push(g / mean);
            b_chromas.push(b / mean);
        }

        let median_r = median(&mut r_chromas);
        let median_g = median(&mut g_chromas);
        let median_b = median(&mut b_chromas);
        let center = [median_r, median_g, median_b];

        let mut distances: Vec<f64> = pixels
            .iter()
            .map(|&[r, g, b]| {
                let mean = ((r + g + b) / 3.0).max(1.0);
                let dr = (r / mean - center[0]).abs();
                let dg = (g / mean - center[1]).abs();
                let db = (b / mean - center[2]).abs();
                dr.max(dg).max(db)
            })
            .collect();

        // Use 95th percentile distance * 2.0, with a floor of 0.025
        let p95 = percentile(&mut distances, 95.0);
        let radius = (p95 * 2.0).max(0.025);

        Ok(Self { center, radius })
    }

    /// Classifies an [R, G, B] pixel as film-base-like.
    #[inline]
    pub fn is_film_like(&self, r: f64, g: f64, b: f64) -> bool {
        let mean = ((r + g + b) / 3.0).max(1.0);
        let dr = (r / mean - self.center[0]).abs();
        let dg = (g / mean - self.center[1]).abs();
        let db = (b / mean - self.center[2]).abs();
        dr.max(dg).max(db) <= self.radius
    }

    /// Computes the fraction of pixels in a column classified as film-like.
    pub fn column_fraction(&self, column_pixels: &[[f64; 3]]) -> f64 {
        if column_pixels.is_empty() {
            return 0.0;
        }
        let film_count = column_pixels
            .iter()
            .filter(|&&[r, g, b]| self.is_film_like(r, g, b))
            .count();
        film_count as f64 / column_pixels.len() as f64
    }
}

/// Options controlling the automatic crop decision.
#[derive(Debug, Clone, PartialEq)]
pub struct CropOptions {
    /// Maximum film-like fraction to consider a column confidently inside the image (default: 0.05).
    pub inside_threshold: f64,
    /// Minimum film-like fraction to consider a column confidently in an unexposed gap (default: 0.50).
    pub gap_threshold: f64,
    /// Number of consecutive columns required to confirm inside-image status (default: 8).
    pub run_length: usize,
    /// Maximum search window fraction of width near each edge (default: 0.20).
    pub edge_search_fraction: f64,
    /// Inset applied to confident horizontal edges (columns) to remove aperture penumbra (default: 5).
    pub edge_inset_columns: usize,
    /// Optional target aspect ratio as (height_ratio, width_ratio), e.g. Some((2, 3)) for standard 35mm film.
    pub aspect_ratio: Option<(u32, u32)>,
}

impl Default for CropOptions {
    fn default() -> Self {
        Self {
            inside_threshold: 0.05,
            gap_threshold: 0.50,
            run_length: 8,
            edge_search_fraction: 0.20,
            edge_inset_columns: 5,
            aspect_ratio: Some((2, 3)),
        }
    }
}

/// The result of a crop decision.
#[derive(Debug, Clone, PartialEq)]
pub struct CropDecision {
    /// Leading edge decision.
    pub leading: EdgeConfidence,
    /// Trailing edge decision.
    pub trailing: EdgeConfidence,
    /// Chosen column range [start, end) in preview coordinates.
    pub columns: (usize, usize),
    /// Chosen row range [start, end) in preview coordinates.
    pub rows: (usize, usize),
    /// Chosen travel range in scanner dots [top, bottom).
    pub travel_dots: (u32, u32),
    /// True only if both leading and trailing edges are confident.
    pub accepted: bool,
    /// True if the frame was detected as blank / unexposed throughout.
    pub is_blank: bool,
}

impl CropDecision {
    /// Evaluates edge transitions directly from high-resolution preview samples
    /// using robust log-transmission cross-row gradients within expected overscan margins.
    pub fn from_preview_samples(
        samples: &nkscan::protocol::decode::Samples,
        pass: &nkscan::scan::pass::Pass,
        origin_dots: u32,
        dots_per_col: f64,
        options: &CropOptions,
    ) -> Result<Self, &'static str> {
        let cols = pass.cols;
        let rows = pass.rows;
        if cols < 32 || rows < 16 {
            return Err("Preview is too small for edge detection");
        }
        if samples.colors.is_empty() {
            return Err("No color planes in samples");
        }

        let row_start = rows / 8;
        let row_end = rows - rows / 8;
        let band_rows = row_end - row_start;
        if band_rows == 0 {
            return Err("Central band is empty");
        }

        let num_channels = samples.colors.len();

        // Compute central transmission profile: mean over central rows and channels
        let mut profile = Vec::with_capacity(cols);
        for col in 0..cols {
            let mut sum = 0.0;
            for plane in &samples.colors {
                for row in row_start..row_end {
                    sum += plane[row * cols + col] as f64;
                }
            }
            let mean = sum / (band_rows * num_channels) as f64;
            profile.push((mean.max(1.0)).ln_1p());
        }

        // Gradient of log-transmission along travel columns
        let mut gradient = Vec::with_capacity(cols - 1);
        for i in 0..(cols - 1) {
            gradient.push(profile[i + 1] - profile[i]);
        }

        // Noise floor via Median Absolute Deviation (MAD)
        let mut g_copy = gradient.clone();
        let med_g = median(&mut g_copy);
        let mut abs_dev: Vec<f64> = gradient.iter().map(|&g| (g - med_g).abs()).collect();
        let noise = median(&mut abs_dev) * 1.4826;
        let threshold = (noise * 6.0).max(0.03);

        // Search window within overscan
        let search = ((cols as f64 * options.edge_search_fraction).round() as usize)
            .max(options.run_length * 2)
            .min(cols / 4);

        // Leading edge: minimum gradient (drop entering negative emulsion)
        let left_window = &gradient[..search];
        let mut min_idx = 0;
        let mut min_val = left_window[0];
        for (i, &val) in left_window.iter().enumerate().skip(1) {
            if val < min_val {
                min_val = val;
                min_idx = i;
            }
        }
        let left = min_idx + 1;
        let left_strength = min_val.abs();
        let leading_confident = left_strength >= threshold;

        let leading = if leading_confident {
            let dots = origin_dots.saturating_add((left as f64 * dots_per_col).round() as u32);
            EdgeConfidence::Confident { column: left, dots }
        } else {
            EdgeConfidence::Ambiguous {
                fallback_column: 0,
                fallback_dots: origin_dots,
                reason: "Leading edge transition below confidence threshold",
            }
        };

        // Trailing edge: maximum gradient (jump exiting negative emulsion)
        let right_start = cols - 1 - search;
        let right_window = &gradient[right_start..];
        let mut max_idx = 0;
        let mut max_val = right_window[0];
        for (i, &val) in right_window.iter().enumerate().skip(1) {
            if val > max_val {
                max_val = val;
                max_idx = i;
            }
        }
        let right = cols - search + max_idx;
        let right_strength = max_val.abs();
        // Trailing edge requires sufficient remaining overscan columns to confirm it didn't clip
        let trailing_confident = right_strength >= threshold && right > left && (cols - right) >= 4;

        let fallback_bottom =
            origin_dots.saturating_add((cols as f64 * dots_per_col).round() as u32);
        let trailing = if trailing_confident {
            let dots = origin_dots.saturating_add((right as f64 * dots_per_col).round() as u32);
            EdgeConfidence::Confident {
                column: right,
                dots,
            }
        } else {
            EdgeConfidence::Ambiguous {
                fallback_column: cols,
                fallback_dots: fallback_bottom,
                reason: "Trailing edge transition below confidence threshold or insufficient overscan",
            }
        };

        let frame_cols = right.saturating_sub(left);
        let accepted = leading_confident && trailing_confident && frame_cols >= cols / 2;

        let (rows_range, columns) = if accepted {
            let inset = options.edge_inset_columns;
            let left_inset = (left + inset).min(right);
            let right_inset = right.saturating_sub(inset).max(left_inset);

            let (top_aperture, bottom_aperture) = detect_row_aperture(samples, rows, cols);

            if let Some((h_ratio, w_ratio)) = options.aspect_ratio {
                fit_centered_aspect_ratio(
                    (top_aperture, bottom_aperture),
                    (left_inset, right_inset),
                    h_ratio,
                    w_ratio,
                )
            } else {
                ((top_aperture, bottom_aperture), (left_inset, right_inset))
            }
        } else {
            ((0, rows), (0, cols))
        };

        let travel_dots = (
            origin_dots.saturating_add((columns.0 as f64 * dots_per_col).round() as u32),
            origin_dots.saturating_add((columns.1 as f64 * dots_per_col).round() as u32),
        );

        Ok(CropDecision {
            leading,
            trailing,
            columns,
            rows: rows_range,
            travel_dots,
            accepted,
            is_blank: false,
        })
    }
}

/// Decides travel crops given column film-like fractions.
///
/// # Arguments
/// - `fractions`: per-column fraction of film-like pixels (length = cols)
/// - `origin_dots`: starting scanner dot address of the scanned preview (top)
/// - `dots_per_col`: scanner dots per preview column (e.g. 2900 / 725 = 4)
/// - `expected_leading`: discovery-predicted leading column index (if known)
/// - `expected_trailing`: discovery-predicted trailing column index (if known)
/// - `options`: configuration thresholds
pub fn decide_crop(
    fractions: &[f64],
    origin_dots: u32,
    dots_per_col: f64,
    expected_leading: Option<usize>,
    expected_trailing: Option<usize>,
    options: &CropOptions,
) -> CropDecision {
    let cols = fractions.len();
    if cols == 0 {
        return CropDecision {
            leading: EdgeConfidence::Ambiguous {
                fallback_column: 0,
                fallback_dots: origin_dots,
                reason: "Empty column data",
            },
            trailing: EdgeConfidence::Ambiguous {
                fallback_column: 0,
                fallback_dots: origin_dots,
                reason: "Empty column data",
            },
            columns: (0, 0),
            rows: (0, 0),
            travel_dots: (origin_dots, origin_dots),
            accepted: false,
            is_blank: true,
        };
    }

    // Apply a 3-tap median filter to suppress single-column dust specks and scratches
    let smoothed = median_filter_3(fractions);

    // Check for blank frame: if there are no inside-image columns (< inside_threshold)
    // or if > 90% of columns exceed the gap threshold
    let inside_count = smoothed
        .iter()
        .filter(|&&f| f < options.inside_threshold)
        .count();
    let high_gap_count = smoothed
        .iter()
        .filter(|&&f| f > options.gap_threshold)
        .count();
    if inside_count == 0 || (high_gap_count as f64 / cols as f64) > 0.90 {
        let fallback_bottom =
            origin_dots.saturating_add((cols as f64 * dots_per_col).round() as u32);
        return CropDecision {
            leading: EdgeConfidence::Ambiguous {
                fallback_column: 0,
                fallback_dots: origin_dots,
                reason: "Blank frame: no confirmed image content",
            },
            trailing: EdgeConfidence::Ambiguous {
                fallback_column: cols,
                fallback_dots: fallback_bottom,
                reason: "Blank frame: no confirmed image content",
            },
            columns: (0, cols),
            rows: (0, 0),
            travel_dots: (origin_dots, fallback_bottom),
            accepted: false,
            is_blank: true,
        };
    }

    // Search window for leading edge
    let search_len = ((cols as f64 * options.edge_search_fraction).round() as usize)
        .max(options.run_length * 2)
        .min(cols);

    let leading_search_limit = expected_leading
        .map(|el| (el + search_len / 2).min(cols))
        .unwrap_or(search_len);

    // Leading edge: find first run of `run_length` consecutive columns < inside_threshold
    let leading = match find_first_run(
        &smoothed[..leading_search_limit],
        options.inside_threshold,
        options.run_length,
    ) {
        Some(col) => {
            let dots = origin_dots.saturating_add((col as f64 * dots_per_col).round() as u32);
            EdgeConfidence::Confident { column: col, dots }
        }
        None => EdgeConfidence::Ambiguous {
            fallback_column: 0,
            fallback_dots: origin_dots,
            reason: "Leading edge ambiguous or not found in search window",
        },
    };

    // Search window for trailing edge: look from the right boundary
    let trailing_search_start = expected_trailing
        .map(|et| et.saturating_sub(search_len / 2))
        .unwrap_or(cols.saturating_sub(search_len));

    // Trailing edge: find where image content ends before entering gap.
    // We search backwards from the end of the image.
    let trailing = match find_trailing_edge(
        &smoothed[trailing_search_start..],
        options.inside_threshold,
        options.run_length,
    ) {
        Some(offset) => {
            let col = trailing_search_start + offset;
            let dots = origin_dots.saturating_add((col as f64 * dots_per_col).round() as u32);
            EdgeConfidence::Confident { column: col, dots }
        }
        None => {
            let fallback_bottom =
                origin_dots.saturating_add((cols as f64 * dots_per_col).round() as u32);
            EdgeConfidence::Ambiguous {
                fallback_column: cols,
                fallback_dots: fallback_bottom,
                reason: "Trailing edge ambiguous: no trailing gap confirmed in scan window",
            }
        }
    };

    let accepted =
        leading.is_confident() && trailing.is_confident() && trailing.column() > leading.column();

    let columns = if accepted {
        (leading.column(), trailing.column())
    } else {
        (0, cols)
    };

    let travel_dots = (
        origin_dots.saturating_add((columns.0 as f64 * dots_per_col).round() as u32),
        origin_dots.saturating_add((columns.1 as f64 * dots_per_col).round() as u32),
    );

    CropDecision {
        leading,
        trailing,
        columns,
        rows: (0, 0),
        travel_dots,
        accepted,
        is_blank: false,
    }
}

/// Detects the vertical optical aperture between the top and bottom adapter mask rails.
pub fn detect_row_aperture(
    samples: &nkscan::protocol::decode::Samples,
    rows: usize,
    cols: usize,
) -> (usize, usize) {
    if rows < 16 || cols < 16 || samples.colors.is_empty() {
        return (0, rows);
    }
    // Nominal physical aperture bounds for the SA-21 film adapter
    // (At 725 DPI with 717 sensor rows, top rail clears at row 25, bottom rail clears at row 689).
    let nominal_top = ((rows as f64 * 25.0 / 717.0).round() as usize).min(rows / 2);
    let nominal_bottom = ((rows as f64 * 689.0 / 717.0).round() as usize).max(nominal_top + 1);

    let col_start = cols / 4;
    let col_end = cols - cols / 4;
    let col_count = col_end.saturating_sub(col_start);
    if col_count == 0 {
        return (nominal_top, nominal_bottom);
    }
    let num_channels = samples.colors.len();

    let mut row_means = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut sum = 0.0;
        for plane in &samples.colors {
            for col in col_start..col_end {
                sum += plane[row * cols + col] as f64;
            }
        }
        row_means.push(sum / (col_count * num_channels) as f64);
    }

    let max_mean = row_means.iter().copied().fold(0.0f64, f64::max);
    if max_mean < 100.0 {
        return (nominal_top, nominal_bottom);
    }
    // Threshold: 15% of peak brightness, or at least 1500 counts (in 16-bit)
    let threshold = (max_mean * 0.15).max(1500.0);

    // Dynamic row penumbra inset (2 rows at 725 DPI, 8 rows at 2900 DPI)
    let row_inset = ((rows as f64 * 2.0 / 717.0).round() as usize).max(2);

    // Search around the expected top rail (first 20% of rows)
    let search_top = (rows / 5).min(rows / 2);
    let mut top = nominal_top;
    if row_means[0] < threshold {
        for (r, &mean) in row_means.iter().enumerate().take(search_top) {
            if mean >= threshold {
                top = (r + row_inset).max(nominal_top);
                break;
            }
        }
    }

    // Search around the expected bottom rail (last 20% of rows)
    let search_bottom = rows.saturating_sub(rows / 5);
    let mut bottom = nominal_bottom;
    if row_means[rows - 1] < threshold {
        for r in (search_bottom..rows).rev() {
            if row_means[r] >= threshold {
                bottom = r.saturating_sub(row_inset).min(nominal_bottom);
                break;
            }
        }
    }

    (top.min(bottom), bottom.max(top))
}

/// Fits the largest centered sub-rectangle matching `h_ratio : w_ratio`
/// inside `row_range` and `col_range`.
pub fn fit_centered_aspect_ratio(
    row_range: (usize, usize),
    col_range: (usize, usize),
    h_ratio: u32,
    w_ratio: u32,
) -> ((usize, usize), (usize, usize)) {
    let (top, bottom) = row_range;
    let (left, right) = col_range;
    let available_h = bottom.saturating_sub(top);
    let available_w = right.saturating_sub(left);
    if available_h == 0 || available_w == 0 || h_ratio == 0 || w_ratio == 0 {
        return (row_range, col_range);
    }

    let h_r = h_ratio as f64;
    let w_r = w_ratio as f64;

    // Cross-multiply to determine which dimension is constrained:
    // available_w * h_ratio vs available_h * w_ratio
    let lhs = available_w as u64 * h_ratio as u64;
    let rhs = available_h as u64 * w_ratio as u64;

    if lhs >= rhs {
        // Height is the limiting dimension; width has excess
        let target_w = ((available_h as f64 * w_r / h_r).round() as usize).min(available_w);
        let excess_w = available_w.saturating_sub(target_w);
        let start_col = left + excess_w / 2;
        let end_col = start_col + target_w;
        ((top, bottom), (start_col, end_col))
    } else {
        // Width is the limiting dimension; height has excess
        let target_h = ((available_w as f64 * h_r / w_r).round() as usize).min(available_h);
        let excess_h = available_h.saturating_sub(target_h);
        let start_row = top + excess_h / 2;
        let end_row = start_row + target_h;
        ((start_row, end_row), (left, right))
    }
}

/// Finds the starting index of the first run of `n` consecutive elements < `threshold`.
fn find_first_run(values: &[f64], threshold: f64, n: usize) -> Option<usize> {
    if values.len() < n {
        return None;
    }
    for i in 0..=(values.len() - n) {
        if values[i..i + n].iter().all(|&v| v < threshold) {
            return Some(i);
        }
    }
    None
}

/// Finds the trailing edge column (exclusive bound of image) in a trailing window.
/// Searches backwards from the end: finds where the trailing gap transitions into the image.
fn find_trailing_edge(window: &[f64], threshold: f64, n: usize) -> Option<usize> {
    if window.len() < n {
        return None;
    }
    // Search backwards: we want the last column that was inside image before a run of gap columns,
    // or from the right: the first index where looking leftwards has `n` columns < threshold.
    // Specifically, if window ends in gap, reversed window starts in gap.
    // Finding where reversed window transitions into image gives the trailing image edge!
    for i in (n..=window.len()).rev() {
        if window[i - n..i].iter().all(|&v| v < threshold) {
            // Found image content up to column index `i`
            // If the window continues past `i`, check that there is actually gap after `i`
            // to avoid claiming confidence when scan ended inside the photo!
            let remaining = &window[i..];
            if !remaining.is_empty() {
                // Must have gap-like evidence in the remaining columns
                let gap_like = remaining.iter().filter(|&&v| v > threshold).count();
                if gap_like > 0 {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// 3-tap running median filter to eliminate 1-column impulse noise (dust / scratches).
fn median_filter_3(data: &[f64]) -> Vec<f64> {
    let len = data.len();
    if len <= 2 {
        return data.to_vec();
    }
    let mut out = Vec::with_capacity(len);
    out.push(data[0]); // First element unchanged
    for i in 1..(len - 1) {
        let mut triad = [data[i - 1], data[i], data[i + 1]];
        triad.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        out.push(triad[1]);
    }
    out.push(data[len - 1]); // Last element unchanged
    out
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = v.len() / 2;
    if v.len().is_multiple_of(2) {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    }
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if v.is_empty() {
        return 0.0;
    }
    let idx = ((p / 100.0) * (v.len() - 1) as f64).round() as usize;
    v[idx.min(v.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_edges_are_accepted() {
        let mut fractions = vec![1.0; 30]; // 30 leading gap columns
        fractions.extend(vec![0.0; 200]); // 200 inside image columns
        fractions.extend(vec![1.0; 40]); // 40 trailing gap columns

        let decision = decide_crop(
            &fractions,
            4000,
            4.0,
            Some(30),
            Some(230),
            &CropOptions::default(),
        );

        assert!(decision.accepted);
        assert!(!decision.is_blank);
        assert_eq!(decision.columns, (30, 230));
        assert_eq!(decision.travel_dots, (4000 + 30 * 4, 4000 + 230 * 4));
        assert_eq!(
            decision.leading,
            EdgeConfidence::Confident {
                column: 30,
                dots: 4120
            }
        );
        assert_eq!(
            decision.trailing,
            EdgeConfidence::Confident {
                column: 230,
                dots: 4920
            }
        );
    }

    #[test]
    fn missing_trailing_gap_falls_back_conservatively() {
        // Simulates strip-b-02 where trailing edge ended inside the image (no overscan)
        let mut fractions = vec![1.0; 35]; // 35 leading gap columns
        fractions.extend(vec![0.0; 250]); // 250 image columns, scan ends inside photo!

        let decision = decide_crop(
            &fractions,
            4470,
            4.0,
            Some(35),
            None,
            &CropOptions::default(),
        );

        // Leading edge was found, but trailing is ambiguous because there was no trailing gap
        assert!(!decision.accepted);
        assert!(decision.leading.is_confident());
        assert!(!decision.trailing.is_confident());
        // Safety criterion: when one edge is ambiguous, retain conservative overscan (0..cols)
        assert_eq!(decision.columns, (0, 285));
        assert_eq!(decision.travel_dots, (4470, 4470 + 285 * 4));
    }

    #[test]
    fn blank_frame_is_detected_and_not_trimmed() {
        // Entire scan is unexposed film base
        let fractions = vec![0.95; 300];

        let decision = decide_crop(&fractions, 0, 4.0, None, None, &CropOptions::default());

        assert!(!decision.accepted);
        assert!(decision.is_blank);
        assert_eq!(decision.columns, (0, 300));
    }

    #[test]
    fn dust_and_scratches_do_not_fool_detector() {
        let mut fractions = vec![1.0; 40];
        // Dust in gap at column 10 (fraction temporarily drops)
        fractions[10] = 0.0;

        // Image from 40 to 240
        let mut img = vec![0.0; 200];
        // Scratch in image at column 70 (relative 30)
        img[30] = 0.35;
        fractions.extend(img);

        // Trailing gap from 240 to 280
        let mut trailing = vec![1.0; 40];
        // Dust in trailing gap
        trailing[15] = 0.0;
        fractions.extend(trailing);

        let decision = decide_crop(
            &fractions,
            0,
            4.0,
            Some(40),
            Some(240),
            &CropOptions::default(),
        );

        assert!(decision.accepted);
        assert_eq!(decision.columns, (40, 240));
    }

    #[test]
    fn film_chromaticity_model_classification() {
        let base_rgb = [560.0, 270.0, 155.0];
        let mut gap_pixels = Vec::new();
        for i in 0..100 {
            let factor = 0.8 + (i as f64) * 0.004;
            gap_pixels.push([
                base_rgb[0] * factor,
                base_rgb[1] * factor,
                base_rgb[2] * factor,
            ]);
        }

        let model = FilmChromaticityModel::from_gap_pixels(&gap_pixels).unwrap();
        // Film base pixel should classify as film-like
        assert!(model.is_film_like(560.0, 270.0, 155.0));
        assert!(model.is_film_like(280.0, 135.0, 77.5)); // Different exposure, same ratios

        // Scene pixel (e.g. blue sky or neutral grey) should NOT classify as film-like
        assert!(!model.is_film_like(200.0, 200.0, 200.0));
        assert!(!model.is_film_like(100.0, 200.0, 300.0));
    }

    fn mock_pass(rows: usize, cols: usize) -> nkscan::scan::pass::Pass {
        nkscan::scan::pass::Pass {
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
        }
    }

    #[test]
    fn from_preview_samples_accepts_clean_transitions() {
        // Synthesize a 32-row, 200-col preview frame
        // Gap: cols 0..20 and cols 180..200 (bright: 55000)
        // Image: cols 20..180 (dark: 15000)
        let rows = 32;
        let cols = 200;
        let mut plane = Vec::with_capacity(rows * cols);
        for _ in 0..rows {
            for col in 0..cols {
                let val = if (20..180).contains(&col) {
                    15000u16
                } else {
                    55000u16
                };
                plane.push(val);
            }
        }
        let samples = nkscan::protocol::decode::Samples {
            colors: vec![plane.clone(), plane.clone(), plane],
            ir: None,
        };
        let pass = mock_pass(rows, cols);

        let raw_options = CropOptions {
            edge_inset_columns: 0,
            aspect_ratio: None,
            ..Default::default()
        };
        let decision =
            CropDecision::from_preview_samples(&samples, &pass, 4400, 4.0, &raw_options)
                .unwrap();

        assert!(decision.accepted);
        assert_eq!(decision.columns, (20, 180));
        assert_eq!(decision.travel_dots, (4400 + 20 * 4, 4400 + 180 * 4));
    }

    #[test]
    fn from_preview_samples_falls_back_when_no_trailing_overscan() {
        // Image extends all the way to the end (cols 20..200)
        let rows = 32;
        let cols = 200;
        let mut plane = Vec::with_capacity(rows * cols);
        for _ in 0..rows {
            for col in 0..cols {
                let val = if col >= 20 { 15000u16 } else { 55000u16 };
                plane.push(val);
            }
        }
        let samples = nkscan::protocol::decode::Samples {
            colors: vec![plane],
            ir: None,
        };
        let pass = mock_pass(rows, cols);

        let decision =
            CropDecision::from_preview_samples(&samples, &pass, 4400, 4.0, &CropOptions::default())
                .unwrap();

        assert!(!decision.accepted);
        assert_eq!(decision.columns, (0, 200)); // Conservative fallback
        assert_eq!(decision.rows, (0, 32));
    }

    #[test]
    fn fit_centered_aspect_ratio_centers_correctly() {
        // Available: 667 rows high, 1020 cols wide
        // 2:3 aspect ratio (H:W = 2:3) -> 1.5 ratio
        // Target width = 667 * 1.5 = 1000.5 -> 1000
        // Excess width = 20 cols -> 10 cols trimmed from each side
        let (rows, cols) = fit_centered_aspect_ratio((24, 691), (34, 1054), 2, 3);
        assert_eq!(rows, (24, 691));
        assert_eq!(cols, (43, 1044));
        assert_eq!(cols.1 - cols.0, 1001);
        assert_eq!(rows.1 - rows.0, 667);
    }

    #[test]
    fn detect_row_aperture_identifies_mask() {
        // 100 rows, 100 cols:
        // rows 0..10: black mask (val 50)
        // rows 10..90: image (val 25000)
        // rows 90..100: black mask (val 50)
        let rows = 100;
        let cols = 100;
        let mut plane = Vec::with_capacity(rows * cols);
        for row in 0..rows {
            let val = if (10..90).contains(&row) { 25000u16 } else { 50u16 };
            for _ in 0..cols {
                plane.push(val);
            }
        }
        let samples = nkscan::protocol::decode::Samples {
            colors: vec![plane],
            ir: None,
        };

        let (top, bottom) = detect_row_aperture(&samples, rows, cols);
        // top should be 10 + 2 = 12 (inset into image)
        // bottom should be 89 - 2 = 87 (inset into image)
        assert_eq!(top, 12);
        assert_eq!(bottom, 87);
    }

    #[test]
    fn detect_row_aperture_handles_dark_scene_without_including_mask() {
        // 717 rows, 200 cols (typical 725 DPI preview):
        // rows 0..22: black adapter mask (val 50)
        // rows 22..200: dark scene (val 800)
        // rows 200..692: bright scene (val 40000)
        // rows 692..717: black adapter mask (val 50)
        let rows = 717;
        let cols = 200;
        let mut plane = Vec::with_capacity(rows * cols);
        for row in 0..rows {
            let val = if (0..22).contains(&row) || (692..717).contains(&row) {
                50u16
            } else if (22..200).contains(&row) {
                800u16
            } else {
                40000u16
            };
            for _ in 0..cols {
                plane.push(val);
            }
        }
        let samples = nkscan::protocol::decode::Samples {
            colors: vec![plane],
            ir: None,
        };

        let (top, bottom) = detect_row_aperture(&samples, rows, cols);
        // Even though top scene is dark (< 15% threshold), top must NOT fall back to 0 (the mask)!
        // It must safely clamp to nominal_top (25).
        assert_eq!(top, 25);
        // Bottom was bright, so it detected row 691 and inset by 2 -> 689.
        assert_eq!(bottom, 689);
    }
}
