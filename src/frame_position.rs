use nkscan::protocol::data::Rect;

/// A fixed-origin axis reports its image width separately from its origin.
pub fn axis_end(start: u32, last: u32, boundary: u32) -> Option<u32> {
    if start == last {
        start.checked_add(boundary.checked_sub(1)?)
    } else {
        Some(last)
    }
}

/// Computes the scanner address bounds rectangle from axis start, last, and boundary limits.
pub fn scanner_bounds(
    x_start: u32,
    x_last: u32,
    x_boundary: u32,
    y_start: u32,
    y_last: u32,
    y_boundary: u32,
) -> Option<Rect> {
    Some(Rect {
        left: x_start,
        right: axis_end(x_start, x_last, x_boundary)?,
        top: y_start,
        bottom: axis_end(y_start, y_last, y_boundary)?,
    })
}

/// Shifts a detected frame rectangle by `offset_mm` along the film travel axis (Y)
/// and validates that the resulting rectangle remains within scanner bounds and adapter limits.
///
/// Returns `(adjusted_rect, offset_dots)` on success.
pub fn apply_offset_within(
    detected: Rect,
    offset_mm: f64,
    y_dpi: u16,
    bounds: Rect,
    max_width: u32,
    max_height: u32,
) -> Result<(Rect, i32), &'static str> {
    let mut position = FramePosition::new(detected);
    position.set_offset_mm(offset_mm, y_dpi)?;
    let adjusted = position.adjusted_within(bounds, max_width, max_height)?;
    Ok((adjusted, position.offset_dots))
}

/// An independent shift along film travel, preserving the detected rectangle.
pub struct FramePosition {
    pub detected: Rect,
    pub offset_dots: i32,
}

impl FramePosition {
    pub fn set_offset_mm(&mut self, mm: f64, dpi: u16) -> Result<(), &'static str> {
        let dots = (mm * f64::from(dpi) / 25.4).round();
        if dpi == 0 || !dots.is_finite() || dots < f64::from(i32::MIN) || dots > f64::from(i32::MAX)
        {
            return Err("Offset cannot be represented at the scanner's optical resolution.");
        }
        self.offset_dots = dots as i32;
        Ok(())
    }

    /// Rejects out-of-range positions instead of silently cropping them.
    pub fn adjusted_within(
        &self,
        bounds: Rect,
        max_width: u32,
        max_height: u32,
    ) -> Result<Rect, &'static str> {
        let rect = self
            .adjusted()
            .ok_or("Adjusted coordinates are invalid or overflow.")?;
        if rect.left < bounds.left
            || rect.right > bounds.right
            || rect.top < bounds.top
            || rect.bottom > bounds.bottom
        {
            return Err("Adjusted frame exceeds the scanner address range.");
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width == 0 || height == 0 || width > max_width || height > max_height {
            return Err("Frame dimensions exceed the adapter limits or are empty.");
        }
        Ok(rect)
    }

    pub fn new(detected: Rect) -> Self {
        Self {
            detected,
            offset_dots: 0,
        }
    }

    /// Checks coordinate arithmetic only. Scanner travel limits must also be
    /// validated before an adjusted rectangle is used for a hardware scan.
    pub fn adjusted(&self) -> Option<Rect> {
        if self.detected.left > self.detected.right || self.detected.top > self.detected.bottom {
            return None;
        }
        Some(Rect {
            top: self.detected.top.checked_add_signed(self.offset_dots)?,
            bottom: self.detected.bottom.checked_add_signed(self.offset_dots)?,
            ..self.detected
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_origin_axis_includes_its_reported_width() {
        assert_eq!(axis_end(0, 0, 2870), Some(2869));
        assert_eq!(axis_end(0, 34656, 4332), Some(34656));
        assert_eq!(axis_end(0, 0, 0), None);
        assert_eq!(axis_end(u32::MAX, u32::MAX, 2), None);
        let frame = FramePosition::new(Rect {
            left: 0,
            right: 2869,
            top: 4664,
            bottom: 8713,
        });
        let bounds = Rect {
            left: 0,
            right: axis_end(0, 0, 2870).unwrap(),
            top: 0,
            bottom: 34656,
        };
        assert!(frame.adjusted_within(bounds, 2870, 4332).is_ok());
    }

    fn detected() -> Rect {
        Rect {
            left: 10,
            right: 2879,
            top: 200,
            bottom: 4300,
        }
    }

    #[test]
    fn converts_millimetres_at_optical_resolution() {
        let mut frame = FramePosition::new(detected());
        frame.set_offset_mm(1.7, 2900).unwrap();
        assert_eq!(frame.offset_dots, 194);
        frame.set_offset_mm(-1.7, 2900).unwrap();
        assert_eq!(frame.offset_dots, -194);
        for mm in [f64::NAN, f64::INFINITY, f64::MAX] {
            assert!(frame.set_offset_mm(mm, 2900).is_err());
            assert_eq!(frame.offset_dots, -194);
        }
        assert!(frame.set_offset_mm(1.0, 0).is_err());
    }

    #[test]
    fn checks_address_edges_and_adapter_dimensions() {
        let bounds = Rect {
            left: 10,
            right: 2879,
            top: 100,
            bottom: 4400,
        };
        let mut frame = FramePosition::new(detected());
        for offset in [-100, 100] {
            frame.offset_dots = offset;
            assert!(frame.adjusted_within(bounds, 2869, 4100).is_ok());
        }
        for offset in [-101, 101] {
            frame.offset_dots = offset;
            assert!(frame.adjusted_within(bounds, 2869, 4100).is_err());
        }
        frame.offset_dots = 0;
        assert!(frame.adjusted_within(bounds, 2868, 4100).is_err());
        assert!(frame.adjusted_within(bounds, 2869, 4099).is_err());
        assert!(
            frame
                .adjusted_within(Rect { left: 11, ..bounds }, 2869, 4100)
                .is_err()
        );
        assert!(
            frame
                .adjusted_within(
                    Rect {
                        right: 2878,
                        ..bounds
                    },
                    2869,
                    4100
                )
                .is_err()
        );
        frame.detected.bottom = frame.detected.top;
        assert!(frame.adjusted_within(bounds, 2869, 4100).is_err());
    }

    #[test]
    fn new_frame_preserves_detection_with_zero_offset() {
        let frame = FramePosition::new(detected());
        assert_eq!(frame.offset_dots, 0);
        assert_eq!(frame.adjusted(), Some(detected()));
    }

    #[test]
    fn shifts_both_travel_edges_without_changing_width_or_detection() {
        for (offset_dots, top, bottom) in [(100, 300, 4400), (-100, 100, 4200), (-200, 0, 4100)] {
            let frame = FramePosition {
                detected: detected(),
                offset_dots,
            };
            assert_eq!(
                frame.adjusted(),
                Some(Rect {
                    left: 10,
                    right: 2879,
                    top,
                    bottom
                })
            );
            assert_eq!(frame.detected, detected());
        }
    }

    #[test]
    fn rejects_underflow_including_minimum_signed_offset() {
        for offset_dots in [-201, i32::MIN] {
            assert_eq!(
                FramePosition {
                    detected: detected(),
                    offset_dots
                }
                .adjusted(),
                None
            );
        }
    }

    #[test]
    fn accepts_maximum_coordinate_but_rejects_overflow() {
        let mut frame = FramePosition::new(Rect {
            top: u32::MAX - 100,
            bottom: u32::MAX - 1,
            ..detected()
        });
        frame.offset_dots = 1;
        assert_eq!(frame.adjusted().unwrap().bottom, u32::MAX);
        frame.offset_dots = 2;
        assert_eq!(frame.adjusted(), None);
        frame.offset_dots = i32::MAX;
        assert_eq!(frame.adjusted(), None);
    }

    #[test]
    fn rejects_reversed_rectangles() {
        for rect in [
            Rect {
                left: 2880,
                ..detected()
            },
            Rect {
                top: 4301,
                ..detected()
            },
        ] {
            assert_eq!(FramePosition::new(rect).adjusted(), None);
        }
    }

    #[test]
    fn ls40_frame_1_and_frame_6_boundary_cases() {
        let bounds = scanner_bounds(0, 0, 2870, 0, 34656, 4332).unwrap();
        assert_eq!(
            bounds,
            Rect {
                left: 0,
                right: 2869,
                top: 0,
                bottom: 34656,
            }
        );

        // Frame 1 starts near the leading travel origin (top = 90 dots, ~0.79 mm at 2900 DPI)
        let frame_1 = Rect {
            left: 0,
            right: 2869,
            top: 90,
            bottom: 4259,
        };
        // +0.5 mm (+57 dots) is valid
        let (adj_1_pos, dots_1_pos) =
            apply_offset_within(frame_1, 0.5, 2900, bounds, 2870, 4332).unwrap();
        assert_eq!(dots_1_pos, 57);
        assert_eq!(adj_1_pos.top, 147);
        assert_eq!(adj_1_pos.bottom, 4316);

        // -0.78 mm (-89 dots -> top = 1) is right at the boundary and valid
        let (adj_1_edge, _) =
            apply_offset_within(frame_1, -0.78, 2900, bounds, 2870, 4332).unwrap();
        assert_eq!(adj_1_edge.top, 1);

        // -1.0 mm (-114 dots) underflows top = 0 and must be rejected
        assert!(apply_offset_within(frame_1, -1.0, 2900, bounds, 2870, 4332).is_err());

        // Frame 6 near the trailing travel limit (bottom = 34556, 100 dots from 34656)
        let frame_6 = Rect {
            left: 0,
            right: 2869,
            top: 30387,
            bottom: 34556,
        };
        // +0.85 mm (+97 dots -> bottom = 34653 <= 34656) is valid
        let (adj_6_ok, _) = apply_offset_within(frame_6, 0.85, 2900, bounds, 2870, 4332).unwrap();
        assert_eq!(adj_6_ok.bottom, 34653);

        // +1.0 mm (+114 dots -> bottom = 34670 > 34656) exceeds travel range and must be rejected
        assert!(apply_offset_within(frame_6, 1.0, 2900, bounds, 2870, 4332).is_err());
    }
}
