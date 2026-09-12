//! Image orientation model, coordinate mapping, and Darktable flip representation.
//!
//! Orientation is maintained as a view and export transformation without destructive
//! modification of the underlying raw scan pixels. Selection rectangles created on
//! rotated previews are accurately mapped to the underlying sensor coordinates.

use serde::{Deserialize, Serialize};

use super::analysis::{SampleRect, WorkingImage};

/// Frame orientation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Orientation {
    /// Native scanner orientation (0 degrees).
    #[default]
    Normal,
    /// Rotated 90 degrees clockwise.
    Rotate90,
    /// Rotated 180 degrees.
    Rotate180,
    /// Rotated 270 degrees clockwise (90 degrees counter-clockwise).
    Rotate270,
}

/// Scope of orientation changes applied during review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OrientationScope {
    /// Apply orientation only to the active frame.
    #[default]
    CurrentFrame,
    /// Apply orientation to the active frame and all subsequent frames in the strip.
    RemainingFrames,
    /// Apply orientation to all frames in the entire strip.
    WholeStrip,
}

impl Orientation {
    /// Rotates 90 degrees clockwise.
    pub fn rotate_clockwise(self) -> Self {
        match self {
            Orientation::Normal => Orientation::Rotate90,
            Orientation::Rotate90 => Orientation::Rotate180,
            Orientation::Rotate180 => Orientation::Rotate270,
            Orientation::Rotate270 => Orientation::Normal,
        }
    }

    /// Rotates 90 degrees counter-clockwise.
    pub fn rotate_counter_clockwise(self) -> Self {
        match self {
            Orientation::Normal => Orientation::Rotate270,
            Orientation::Rotate90 => Orientation::Normal,
            Orientation::Rotate180 => Orientation::Rotate90,
            Orientation::Rotate270 => Orientation::Rotate180,
        }
    }

    /// Returns the dimensions `(width, height)` of the preview after applying orientation.
    pub fn preview_dimensions(self, width: usize, height: usize) -> (usize, usize) {
        match self {
            Orientation::Normal | Orientation::Rotate180 => (width, height),
            Orientation::Rotate90 | Orientation::Rotate270 => (height, width),
        }
    }

    /// Maps a source pixel `(x, y)` in `[0..width, 0..height]` to preview coordinate `(u, v)`.
    pub fn source_to_preview(
        self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
    ) -> (usize, usize) {
        if width == 0 || height == 0 {
            return (0, 0);
        }
        let x = x.min(width - 1);
        let y = y.min(height - 1);

        match self {
            Orientation::Normal => (x, y),
            Orientation::Rotate90 => (height - 1 - y, x),
            Orientation::Rotate180 => (width - 1 - x, height - 1 - y),
            Orientation::Rotate270 => (y, width - 1 - x),
        }
    }

    /// Maps a preview coordinate `(u, v)` back to source coordinate `(x, y)`.
    pub fn preview_to_source(
        self,
        u: usize,
        v: usize,
        width: usize,
        height: usize,
    ) -> (usize, usize) {
        if width == 0 || height == 0 {
            return (0, 0);
        }
        let (pw, ph) = self.preview_dimensions(width, height);
        let u = u.min(pw - 1);
        let v = v.min(ph - 1);

        match self {
            Orientation::Normal => (u, v),
            Orientation::Rotate90 => (v, height - 1 - u),
            Orientation::Rotate180 => (width - 1 - u, height - 1 - v),
            Orientation::Rotate270 => (width - 1 - v, u),
        }
    }

    /// Maps a preview selection rectangle back to underlying source coordinates.
    pub fn preview_rect_to_source_rect(
        self,
        rect: SampleRect,
        width: usize,
        height: usize,
    ) -> SampleRect {
        if rect.width == 0 || rect.height == 0 || width == 0 || height == 0 {
            return SampleRect::new(0, 0, 0, 0);
        }

        let (pw, ph) = self.preview_dimensions(width, height);
        let u0 = rect.x.min(pw - 1);
        let v0 = rect.y.min(ph - 1);
        let u1 = (rect.x + rect.width - 1).min(pw - 1);
        let v1 = (rect.y + rect.height - 1).min(ph - 1);

        let corners = [
            self.preview_to_source(u0, v0, width, height),
            self.preview_to_source(u1, v0, width, height),
            self.preview_to_source(u0, v1, width, height),
            self.preview_to_source(u1, v1, width, height),
        ];

        let min_x = corners.iter().map(|(x, _)| *x).min().unwrap();
        let max_x = corners.iter().map(|(x, _)| *x).max().unwrap();
        let min_y = corners.iter().map(|(_, y)| *y).min().unwrap();
        let max_y = corners.iter().map(|(_, y)| *y).max().unwrap();

        SampleRect::new(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1)
    }

    /// Maps a source rectangle into preview coordinates.
    pub fn source_rect_to_preview_rect(
        self,
        rect: SampleRect,
        width: usize,
        height: usize,
    ) -> SampleRect {
        if rect.width == 0 || rect.height == 0 || width == 0 || height == 0 {
            return SampleRect::new(0, 0, 0, 0);
        }

        let x0 = rect.x.min(width - 1);
        let y0 = rect.y.min(height - 1);
        let x1 = (rect.x + rect.width - 1).min(width - 1);
        let y1 = (rect.y + rect.height - 1).min(height - 1);

        let corners = [
            self.source_to_preview(x0, y0, width, height),
            self.source_to_preview(x1, y0, width, height),
            self.source_to_preview(x0, y1, width, height),
            self.source_to_preview(x1, y1, width, height),
        ];

        let min_u = corners.iter().map(|(u, _)| *u).min().unwrap();
        let max_u = corners.iter().map(|(u, _)| *u).max().unwrap();
        let min_v = corners.iter().map(|(_, v)| *v).min().unwrap();
        let max_v = corners.iter().map(|(_, v)| *v).max().unwrap();

        SampleRect::new(min_u, min_v, max_u - min_u + 1, max_v - min_v + 1)
    }

    /// Returns Darktable flip module parameters for this orientation.
    ///
    /// Formatted as little-endian 32-bit integer hex string (matching Darktable XMP sidecars):
    /// - `Normal`: `None` (flip module disabled/unneeded)
    /// - `Rotate180`: `03000000` (ORIENTATION_ROTATE_180_DEG = 3)
    /// - `Rotate90`: `06000000` (ORIENTATION_ROTATE_CCW_90_DEG = 6)
    /// - `Rotate270`: `05000000` (ORIENTATION_ROTATE_CW_90_DEG = 5)
    pub fn darktable_flip_params(self) -> Option<(&'static str, &'static str)> {
        match self {
            Orientation::Normal => None,
            Orientation::Rotate90 => Some(("06000000", "_builtin_rotate by 90 degrees")),
            Orientation::Rotate180 => Some(("03000000", "_builtin_rotate by 180 degrees")),
            Orientation::Rotate270 => Some(("05000000", "_builtin_rotate by -90 degrees")),
        }
    }

    /// Reconstructs an Orientation from Darktable flip module binary parameter bytes.
    pub fn from_darktable_flip_params(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 {
            return None;
        }
        let code = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        match code {
            0 => Some(Orientation::Normal),
            6 => Some(Orientation::Rotate90),
            3 => Some(Orientation::Rotate180),
            5 => Some(Orientation::Rotate270),
            _ => None,
        }
    }

    /// Renders an oriented copy of a `WorkingImage` for preview display.
    pub fn render_oriented_image(self, image: &WorkingImage) -> WorkingImage {
        let (pw, ph) = self.preview_dimensions(image.width, image.height);
        let mut pixels = vec![[0.0f32; 3]; pw * ph];

        for v in 0..ph {
            let row_offset = v * pw;
            for u in 0..pw {
                let (sx, sy) = self.preview_to_source(u, v, image.width, image.height);
                pixels[row_offset + u] = image.pixel(sx, sy).unwrap_or([0.0; 3]);
            }
        }

        WorkingImage::new(pw, ph, pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_transitions_cycle_correctly() {
        let mut o = Orientation::Normal;
        o = o.rotate_clockwise();
        assert_eq!(o, Orientation::Rotate90);
        o = o.rotate_clockwise();
        assert_eq!(o, Orientation::Rotate180);
        o = o.rotate_clockwise();
        assert_eq!(o, Orientation::Rotate270);
        o = o.rotate_clockwise();
        assert_eq!(o, Orientation::Normal);

        o = o.rotate_counter_clockwise();
        assert_eq!(o, Orientation::Rotate270);
        o = o.rotate_counter_clockwise();
        assert_eq!(o, Orientation::Rotate180);
    }

    #[test]
    fn preview_dimensions_swap_for_90_and_270() {
        let (w, h) = (100, 200);
        assert_eq!(Orientation::Normal.preview_dimensions(w, h), (100, 200));
        assert_eq!(Orientation::Rotate90.preview_dimensions(w, h), (200, 100));
        assert_eq!(Orientation::Rotate180.preview_dimensions(w, h), (100, 200));
        assert_eq!(Orientation::Rotate270.preview_dimensions(w, h), (200, 100));
    }

    #[test]
    fn coordinate_round_trip_for_all_orientations() {
        let (w, h) = (100, 200);
        let test_points = [(0, 0), (99, 0), (0, 199), (99, 199), (50, 80)];
        let orientations = [
            Orientation::Normal,
            Orientation::Rotate90,
            Orientation::Rotate180,
            Orientation::Rotate270,
        ];

        for &o in &orientations {
            for &(x, y) in &test_points {
                let (u, v) = o.source_to_preview(x, y, w, h);
                let (rx, ry) = o.preview_to_source(u, v, w, h);
                assert_eq!(
                    (x, y),
                    (rx, ry),
                    "Orientation {o:?} failed round-trip for ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn preview_selection_rectangle_maps_to_underlying_pixels() {
        let (w, h) = (100, 200);
        // Source feature located at x: 20..30 (width 11), y: 40..60 (height 21)
        let source_rect = SampleRect::new(20, 40, 11, 21);

        for &o in &[
            Orientation::Normal,
            Orientation::Rotate90,
            Orientation::Rotate180,
            Orientation::Rotate270,
        ] {
            // Forward map source rect to preview rect
            let preview_rect = o.source_rect_to_preview_rect(source_rect, w, h);

            // Backward map preview rect to source rect
            let mapped_back = o.preview_rect_to_source_rect(preview_rect, w, h);

            assert_eq!(
                source_rect, mapped_back,
                "Failed rect mapping consistency for {o:?}"
            );
        }
    }

    #[test]
    fn darktable_flip_params_match_expected_constants() {
        assert_eq!(Orientation::Normal.darktable_flip_params(), None);
        assert_eq!(
            Orientation::Rotate180.darktable_flip_params(),
            Some(("03000000", "_builtin_rotate by 180 degrees"))
        );
        assert_eq!(
            Orientation::Rotate90.darktable_flip_params(),
            Some(("06000000", "_builtin_rotate by 90 degrees"))
        );
        assert_eq!(
            Orientation::Rotate270.darktable_flip_params(),
            Some(("05000000", "_builtin_rotate by -90 degrees"))
        );
    }

    #[test]
    fn render_oriented_image_transposes_and_reorients_correctly() {
        // 2x3 image
        // [1, 2]
        // [3, 4]
        // [5, 6]
        let pixels = vec![
            [1.0, 1.0, 1.0],
            [2.0, 2.0, 2.0],
            [3.0, 3.0, 3.0],
            [4.0, 4.0, 4.0],
            [5.0, 5.0, 5.0],
            [6.0, 6.0, 6.0],
        ];
        let img = WorkingImage::new(2, 3, pixels);

        let rot90 = Orientation::Rotate90.render_oriented_image(&img);
        assert_eq!(rot90.width, 3);
        assert_eq!(rot90.height, 2);

        // Under 90 deg CW, top row is [5, 3, 1], bottom row is [6, 4, 2]
        assert_eq!(rot90.pixel(0, 0), Some([5.0, 5.0, 5.0]));
        assert_eq!(rot90.pixel(1, 0), Some([3.0, 3.0, 3.0]));
        assert_eq!(rot90.pixel(2, 0), Some([1.0, 1.0, 1.0]));
        assert_eq!(rot90.pixel(0, 1), Some([6.0, 6.0, 6.0]));
        assert_eq!(rot90.pixel(1, 1), Some([4.0, 4.0, 4.0]));
        assert_eq!(rot90.pixel(2, 1), Some([2.0, 2.0, 2.0]));
    }
}
