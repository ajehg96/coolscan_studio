use nkscan::{
    protocol::{
        data::{BoundaryType2, Rect},
        decode::Samples,
    },
    scan::{framing::Framing, pass::Pass},
};

use crate::processing::roll::RollProfile;

/// Selection of frames to scan from a strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameSelection {
    /// Scan all discovered frames on the strip (1-indexed).
    All,
    /// Scan a single specific frame (1-indexed).
    Specific(usize),
    /// Scan an explicit list of frames (1-indexed).
    List(Vec<usize>),
}

impl FrameSelection {
    pub fn resolve(&self, total_frames: usize) -> Vec<usize> {
        match self {
            FrameSelection::All => (1..=total_frames).collect(),
            FrameSelection::Specific(f) => vec![*f],
            FrameSelection::List(list) => list.clone(),
        }
    }
}

/// Request parameters for a strip scan.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanRequest {
    pub frames: FrameSelection,
    pub dpi: u16,
    pub samples: u8,
    pub clean: bool,
    pub auto_crop: bool,
    pub roll: Option<RollProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanRequestError {
    InvalidDpi(u16),
    InvalidSamples(u8),
    InvalidFrameNumber(usize),
    EmptyFrameList,
}

impl std::fmt::Display for ScanRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanRequestError::InvalidDpi(dpi) => {
                write!(f, "Invalid DPI: {dpi}. Supported range is 90 to 2900 DPI.")
            }
            ScanRequestError::InvalidSamples(s) => {
                write!(f, "Invalid sample count: {s}. Must be between 1 and 64.")
            }
            ScanRequestError::InvalidFrameNumber(n) => {
                write!(f, "Invalid frame number: {n}. Frames are numbered from 1.")
            }
            ScanRequestError::EmptyFrameList => {
                write!(f, "Frame selection list cannot be empty.")
            }
        }
    }
}

impl std::error::Error for ScanRequestError {}

impl ScanRequest {
    pub fn new(
        frames: FrameSelection,
        dpi: u16,
        samples: u8,
        clean: bool,
        auto_crop: bool,
    ) -> Self {
        Self {
            frames,
            dpi,
            samples,
            clean,
            auto_crop,
            roll: None,
        }
    }

    /// Associates a roll profile with this scan request.
    pub fn with_roll(mut self, roll: RollProfile) -> Self {
        self.roll = Some(roll);
        self
    }

    pub fn validate(&self) -> Result<(), ScanRequestError> {
        if self.dpi < 90 || self.dpi > 2900 {
            return Err(ScanRequestError::InvalidDpi(self.dpi));
        }
        if self.samples == 0 || self.samples > 64 {
            return Err(ScanRequestError::InvalidSamples(self.samples));
        }
        match &self.frames {
            FrameSelection::Specific(0) => return Err(ScanRequestError::InvalidFrameNumber(0)),
            FrameSelection::List(list) => {
                if list.is_empty() {
                    return Err(ScanRequestError::EmptyFrameList);
                }
                for &f in list {
                    if f == 0 {
                        return Err(ScanRequestError::InvalidFrameNumber(0));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Metadata captured during scanner acquisition.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanMetadata {
    pub scanner_model: Option<String>,
    pub focus_position: Option<u16>,
    pub exposures: Option<nkscan::scan::autoexpose::Exposures>,
    pub hardware_samples: u8,
    pub software_passes: u8,
    pub infrared_cleaned_pixels: Option<usize>,
}

impl ScanMetadata {
    /// Returns the red, green, and blue exposure durations in scanner ticks if available.
    pub fn rgb_exposures(&self) -> Option<[u32; 3]> {
        let exp = self.exposures.as_ref()?;
        Some([
            exp.get(nkscan::protocol::window::Channel::Red)?,
            exp.get(nkscan::protocol::window::Channel::Green)?,
            exp.get(nkscan::protocol::window::Channel::Blue)?,
        ])
    }
}

/// Discovered strip metadata and frame rectangles.
#[derive(Debug, Clone)]
pub struct StripDiscovery {
    pub framing: Framing,
    pub detected_frames: Vec<Rect>,
    pub overscan_frames: Vec<Rect>,
    pub optical_dpi: (u16, u16),
    pub boundary_table: Option<BoundaryType2>,
    pub thumbnail: Option<Pass>,
    pub thumbnail_samples: Option<Samples>,
}

impl StripDiscovery {
    /// Returns the active frame boundaries (overscan if configured, else detected).
    pub fn frames(&self) -> &[Rect] {
        if !self.overscan_frames.is_empty() {
            &self.overscan_frames
        } else {
            &self.detected_frames
        }
    }

    pub fn frame_count(&self) -> usize {
        self.frames().len()
    }
}

/// Result of a strip scan containing all acquired frame artifacts.
#[derive(Debug, Clone)]
pub struct StripScanResult {
    pub frames: Vec<FrameArtifact>,
    pub discovery: StripDiscovery,
    pub roll: Option<RollProfile>,
}

/// A scanned frame artifact preserving high-bit-depth master samples,
/// pass geometry, crop decision, and acquisition metadata.
#[derive(Debug, Clone)]
pub struct FrameArtifact {
    pub frame_number: usize,
    pub total_frames: usize,
    pub dpi: u16,
    pub raw_rect: Rect,
    /// High-bit-depth untouched scanner acquisition
    pub samples: Samples,
    /// Pass parameters (dimensions, layout) of master acquisition
    pub pass: Pass,
    /// Crop decision if auto-crop was evaluated
    pub crop: Option<crate::crop::CropDecision>,
    /// Metadata captured during scan
    pub scan_metadata: ScanMetadata,
    /// Roll profile associated with this frame, if specified
    pub roll: Option<RollProfile>,
}

/// Effective view of frame image data (either cropped or master).
pub enum EffectiveImage<'a> {
    Cropped(Samples, Pass),
    Master(&'a Samples, &'a Pass),
}

impl<'a> EffectiveImage<'a> {
    pub fn samples(&self) -> &Samples {
        match self {
            EffectiveImage::Cropped(s, _) => s,
            EffectiveImage::Master(s, _) => s,
        }
    }

    pub fn pass(&self) -> &Pass {
        match self {
            EffectiveImage::Cropped(_, p) => p,
            EffectiveImage::Master(_, p) => p,
        }
    }
}

impl FrameArtifact {
    /// Returns cropped samples and pass if auto-crop was accepted, or None.
    pub fn cropped_samples(&self) -> Option<Result<(Samples, Pass), &'static str>> {
        let crop = self.crop.as_ref()?;
        if !crop.accepted {
            return None;
        }
        Some(crate::bmp::crop_samples(
            &self.samples,
            &self.pass,
            crop.rows,
            crop.columns,
        ))
    }

    /// Returns the effective image: cropped if crop was accepted, or master reference.
    pub fn get_effective_image(&self) -> Result<EffectiveImage<'_>, &'static str> {
        if let Some(crop) = &self.crop
            && crop.accepted
        {
            let (s, p) = crate::bmp::crop_samples(
                &self.samples,
                &self.pass,
                crop.rows,
                crop.columns,
            )?;
            return Ok(EffectiveImage::Cropped(s, p));
        }
        Ok(EffectiveImage::Master(&self.samples, &self.pass))
    }
}

/// Scanner scan phase for progress tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanPhase {
    Metering,
    Acquiring,
}

/// Reason for falling back to uncropped overscan bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CropFallbackReason {
    Ambiguous,
    Error(String),
}

/// Strongly typed progress and status events emitted during scanning.
#[derive(Debug, Clone)]
pub enum ScanEvent {
    ScannerFound {
        description: String,
    },
    SessionReady,
    MediaChecked {
        loaded: bool,
    },
    DiscoveryStarted {
        method: Framing,
    },
    DiscoveryCompleted {
        detected_count: usize,
    },
    OverscanConfigured {
        frame_count: usize,
    },
    MultiSamplingConfigured {
        hardware_samples: u8,
        software_passes: u8,
        noise_reduction_db: f64,
    },
    FrameStarted {
        frame_number: usize,
        total_frames: usize,
        dpi: u16,
        is_super_fine: bool,
        features: Vec<String>,
    },
    FrameAttemptStarted {
        frame_number: usize,
        attempt: usize,
        max_attempts: usize,
    },
    FramePassStarted {
        frame_number: usize,
        pass: usize,
        total_passes: usize,
    },
    Progress {
        frame_number: usize,
        phase: ScanPhase,
        percent: u8,
        pass: usize,
        total_passes: usize,
    },
    ProgressPhaseEnd,
    UsbResetting {
        frame_number: usize,
    },
    UsbReconnectFailed {
        frame_number: usize,
        error: String,
    },
    CleaningStarted {
        frame_number: usize,
    },
    CleaningFinished {
        frame_number: usize,
        cleaned_pixels: usize,
    },
    CleaningNotice {
        frame_number: usize,
        message: String,
    },
    CropAccepted {
        frame_number: usize,
        crop: crate::crop::CropDecision,
        width_cols: usize,
        height_rows: usize,
        width_mm: f64,
        height_mm: f64,
        aspect_ratio: f64,
    },
    CropFallback {
        frame_number: usize,
        reason: CropFallbackReason,
    },
    UsbRefreshStarting {
        next_frame_number: usize,
    },
    UsbRefreshReady {
        next_frame_number: usize,
    },
    UsbRefreshRetrying {
        next_frame_number: usize,
        error: String,
    },
    UsbRefreshReconnected {
        next_frame_number: usize,
    },
    UsbRefreshFailed {
        next_frame_number: usize,
        error: String,
    },
    FrameCompleted {
        frame_number: usize,
    },
    FrameSkipped {
        frame_number: usize,
        reason: String,
    },
}

/// Errors that can occur during scanner pipeline execution.
#[derive(Debug)]
pub enum ScanError {
    NoScannersFound,
    DeviceOpen(nkscan::error::Error),
    SessionStart(nkscan::error::Error),
    MediaState(nkscan::error::Error),
    NoMediaLoaded,
    FramingDiscovery(nkscan::error::Error),
    UnsupportedRecipe(String),
    FrameUnavailable(usize),
    ReconnectFailed(String),
    AllFramesFailed,
    Request(ScanRequestError),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanError::NoScannersFound => write!(f, "No Nikon Coolscan scanners found."),
            ScanError::DeviceOpen(e) => write!(f, "Failed to open scanner: {e}"),
            ScanError::SessionStart(e) => write!(f, "Failed to start scanner session: {e}"),
            ScanError::MediaState(e) => write!(f, "Could not determine film state: {e}"),
            ScanError::NoMediaLoaded => write!(f, "No film loaded."),
            ScanError::FramingDiscovery(e) => write!(f, "Frame detection failed: {e}"),
            ScanError::UnsupportedRecipe(msg) => write!(f, "Scan recipe unsupported: {msg}"),
            ScanError::FrameUnavailable(n) => write!(f, "Frame {n} is unavailable."),
            ScanError::ReconnectFailed(msg) => write!(f, "Scanner reconnect failed: {msg}"),
            ScanError::AllFramesFailed => write!(f, "All requested frames failed to scan."),
            ScanError::Request(e) => write!(f, "Invalid scan request: {e}"),
        }
    }
}

impl std::error::Error for ScanError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_request_validation() {
        let valid = ScanRequest::new(FrameSelection::All, 2900, 1, false, true);
        assert!(valid.validate().is_ok());

        let invalid_dpi = ScanRequest::new(FrameSelection::All, 50, 1, false, true);
        assert_eq!(invalid_dpi.validate(), Err(ScanRequestError::InvalidDpi(50)));

        let invalid_samples = ScanRequest::new(FrameSelection::All, 725, 0, false, true);
        assert_eq!(
            invalid_samples.validate(),
            Err(ScanRequestError::InvalidSamples(0))
        );

        let invalid_frame = ScanRequest::new(FrameSelection::Specific(0), 725, 1, false, true);
        assert_eq!(
            invalid_frame.validate(),
            Err(ScanRequestError::InvalidFrameNumber(0))
        );

        let empty_list = ScanRequest::new(FrameSelection::List(vec![]), 725, 1, false, true);
        assert_eq!(
            empty_list.validate(),
            Err(ScanRequestError::EmptyFrameList)
        );
    }

    #[test]
    fn frame_selection_resolution() {
        assert_eq!(FrameSelection::All.resolve(6), vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(FrameSelection::Specific(3).resolve(6), vec![3]);
        assert_eq!(FrameSelection::List(vec![2, 4]).resolve(6), vec![2, 4]);
    }

    fn mock_pass(rows: usize, cols: usize) -> Pass {
        Pass {
            layout: nkscan::protocol::image::Layout::single_line(rows as u32, cols as u32, vec![1]),
            cooperation: Vec::new(),
            complete: true,
            blocks: 1,
            rows,
            cols,
        }
    }

    #[test]
    fn frame_artifact_effective_image_unaccepted_crop() {
        let pass = mock_pass(4, 4);
        let samples = Samples {
            colors: vec![vec![100; 16]],
            ir: None,
        };
        let artifact = FrameArtifact {
            frame_number: 1,
            total_frames: 1,
            dpi: 725,
            raw_rect: Rect { left: 0, right: 4, top: 0, bottom: 4 },
            samples,
            pass,
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

        assert!(artifact.cropped_samples().is_none());
        let effective = artifact.get_effective_image().unwrap();
        assert_eq!(effective.pass().rows, 4);
        assert_eq!(effective.pass().cols, 4);
    }

    #[test]
    fn frame_artifact_effective_image_accepted_crop() {
        let pass = mock_pass(4, 4);
        let samples = Samples {
            colors: vec![vec![100; 16]],
            ir: None,
        };
        let crop = crate::crop::CropDecision {
            leading: crate::crop::EdgeConfidence::Confident { column: 1, dots: 100 },
            trailing: crate::crop::EdgeConfidence::Confident { column: 3, dots: 300 },
            columns: (1, 3),
            rows: (1, 3),
            travel_dots: (100, 300),
            accepted: true,
            is_blank: false,
        };
        let artifact = FrameArtifact {
            frame_number: 1,
            total_frames: 1,
            dpi: 725,
            raw_rect: Rect { left: 0, right: 4, top: 0, bottom: 4 },
            samples,
            pass,
            crop: Some(crop),
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

        let cropped = artifact.cropped_samples().unwrap().unwrap();
        assert_eq!(cropped.1.rows, 2);
        assert_eq!(cropped.1.cols, 2);

        let effective = artifact.get_effective_image().unwrap();
        assert_eq!(effective.pass().rows, 2);
        assert_eq!(effective.pass().cols, 2);
    }

    #[test]
    fn scan_request_with_roll_profile() {
        let roll = RollProfile::pro_image_100();
        let request = ScanRequest::new(FrameSelection::All, 2900, 1, true, true)
            .with_roll(roll.clone());
        assert_eq!(request.roll, Some(roll));
    }
}
