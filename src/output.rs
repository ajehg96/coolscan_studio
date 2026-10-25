use nkscan::{protocol::decode::Samples, scan::pass::Pass};
use std::path::{Path, PathBuf};

use crate::{bmp, processing::color::DEFAULT_LS40_ICC_BYTES, scanner::types::FrameArtifact, tiff};

/// Configuration specifying which image formats and destinations to write.
///
/// Distinguishes between:
/// - **Scanner Master TIFF (`frame-<N>-master.tif`)**: Untouched, uncropped 16-bit scanner samples
///   with optional embedded Nikon LS-40 ICC profile (`NKLS4000LS40_N.icc` in TIFF Tag 34675).
/// - **Working TIFF (`frame-<N>.tif`)**: Cropped 16-bit scanner samples paired with a companion
///   Darktable XMP sidecar (`frame-<N>.tif.xmp`).
/// - **Preview BMP (`frame-<N>.bmp`)**: 8-bit preview bitmap.
#[derive(Debug, Clone)]
pub struct OutputPolicy {
    pub output_dir: PathBuf,
    pub save_bmp: bool,
    pub save_tiff: bool,
    pub save_master_tiff: bool,
    pub embed_icc_in_master: bool,
}

impl OutputPolicy {
    pub fn new(output_dir: impl Into<PathBuf>, save_bmp: bool, save_tiff: bool) -> Self {
        Self {
            output_dir: output_dir.into(),
            save_bmp,
            save_tiff,
            save_master_tiff: false,
            embed_icc_in_master: true,
        }
    }

    /// Enables or disables saving the uncropped scanner master TIFF (`frame-<N>-master.tif`).
    pub fn with_master_tiff(mut self, save_master_tiff: bool) -> Self {
        self.save_master_tiff = save_master_tiff;
        self
    }

    /// Configures whether the Nikon LS-40 ICC profile is embedded in scanner master TIFFs.
    pub fn with_embedded_icc(mut self, embed_icc_in_master: bool) -> Self {
        self.embed_icc_in_master = embed_icc_in_master;
        self
    }
}

/// Output file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    Bmp,
    Tiff,
}

/// Conceptual role of a saved output file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavedFileRole {
    /// Untouched, uncropped 16-bit scanner master (`frame-<N>-master.tif`).
    ScannerMaster,
    /// Cropped 16-bit working TIFF intended for Darktable sidecar pairing (`frame-<N>.tif`).
    WorkingTiff,
    /// 8-bit preview bitmap (`frame-<N>.bmp`).
    PreviewBmp,
}

/// Border cropping status of the saved file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavedCropStatus {
    Cropped,
    AmbiguousFallback,
    Uncropped,
}

/// Record of an individual file saved to disk.
#[derive(Debug, Clone)]
pub struct SavedFile {
    pub path: PathBuf,
    pub format: FileFormat,
    pub role: SavedFileRole,
    pub crop_status: SavedCropStatus,
    pub icc_embedded: bool,
}

#[derive(Debug)]
pub enum OutputError {
    Io(std::io::Error),
    CropSlice(&'static str),
    CreateDirectory(std::io::Error),
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputError::Io(e) => write!(f, "I/O error: {e}"),
            OutputError::CropSlice(e) => write!(f, "Crop slicing error: {e}"),
            OutputError::CreateDirectory(e) => write!(f, "Failed to create directory: {e}"),
        }
    }
}

impl std::error::Error for OutputError {}

/// Saves an untouched, uncropped 16-bit scanner master TIFF (`frame-<N>-master.tif`),
/// optionally embedding the Nikon LS-40 ICC profile in Tag 34675.
pub fn save_master_tiff(
    artifact: &FrameArtifact,
    output_dir: &Path,
    embed_icc: bool,
) -> Result<SavedFile, OutputError> {
    if !output_dir.exists() {
        std::fs::create_dir_all(output_dir).map_err(OutputError::CreateDirectory)?;
    }
    let master_path = output_dir.join(format!("frame-{}-master.tif", artifact.frame_number));
    let path_str = master_path.to_str().ok_or_else(|| {
        OutputError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid master TIFF path string",
        ))
    })?;
    let icc_bytes = if embed_icc {
        Some(DEFAULT_LS40_ICC_BYTES)
    } else {
        None
    };
    tiff::write_tiff_with_icc(
        path_str,
        &artifact.samples,
        &artifact.pass,
        artifact.dpi,
        icc_bytes,
    )
    .map_err(OutputError::Io)?;

    Ok(SavedFile {
        path: master_path,
        format: FileFormat::Tiff,
        role: SavedFileRole::ScannerMaster,
        crop_status: SavedCropStatus::Uncropped,
        icc_embedded: embed_icc,
    })
}

/// Saves image files for a scanned frame artifact according to the output policy.
pub fn save_frame_outputs(
    artifact: &FrameArtifact,
    policy: &OutputPolicy,
) -> Result<Vec<SavedFile>, OutputError> {
    if !policy.output_dir.exists() {
        std::fs::create_dir_all(&policy.output_dir).map_err(OutputError::CreateDirectory)?;
    }

    let base_name = format!("frame-{}", artifact.frame_number);
    let bmp_path = policy.output_dir.join(format!("{base_name}.bmp"));
    let tiff_path = policy.output_dir.join(format!("{base_name}.tif"));

    let mut saved = Vec::new();

    if policy.save_master_tiff {
        let master = save_master_tiff(artifact, &policy.output_dir, policy.embed_icc_in_master)?;
        saved.push(master);
    }

    let (samples, pass, crop_status) = if let Some(crop) = &artifact.crop {
        if crop.accepted {
            match bmp::crop_samples(&artifact.samples, &artifact.pass, crop.rows, crop.columns) {
                Ok((s, p)) => (s, p, SavedCropStatus::Cropped),
                Err(e) => return Err(OutputError::CropSlice(e)),
            }
        } else {
            (
                artifact.samples.clone(),
                artifact.pass.clone(),
                SavedCropStatus::AmbiguousFallback,
            )
        }
    } else {
        (
            artifact.samples.clone(),
            artifact.pass.clone(),
            SavedCropStatus::Uncropped,
        )
    };

    if policy.save_bmp {
        let path_str = bmp_path.to_str().ok_or_else(|| {
            OutputError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid BMP path string",
            ))
        })?;
        bmp::write_bmp(path_str, &samples, &pass).map_err(OutputError::Io)?;
        saved.push(SavedFile {
            path: bmp_path,
            format: FileFormat::Bmp,
            role: SavedFileRole::PreviewBmp,
            crop_status,
            icc_embedded: false,
        });
    }

    if policy.save_tiff {
        let path_str = tiff_path.to_str().ok_or_else(|| {
            OutputError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid TIFF path string",
            ))
        })?;
        tiff::write_tiff(path_str, &samples, &pass, artifact.dpi).map_err(OutputError::Io)?;
        saved.push(SavedFile {
            path: tiff_path,
            format: FileFormat::Tiff,
            role: SavedFileRole::WorkingTiff,
            crop_status,
            icc_embedded: false,
        });
    }

    Ok(saved)
}

/// Saves the discovery strip overview thumbnail as a BMP.
pub fn save_discovery_thumbnail(
    thumbnail: &Pass,
    samples: &Samples,
    path: &Path,
) -> Result<(), OutputError> {
    let mut thumbnail_samples = samples.clone();
    thumbnail_samples.to_full_scale(thumbnail.layout.bits_per_sample);
    let path_str = path.to_str().ok_or_else(|| {
        OutputError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid discovery thumbnail path",
        ))
    })?;
    bmp::write_bmp(path_str, &thumbnail_samples, thumbnail).map_err(OutputError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nkscan::protocol::data::Rect;

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
    fn output_policy_saves_requested_formats() {
        let temp_dir = std::env::temp_dir().join(format!("coolscan_test_{}", std::process::id()));
        let policy = OutputPolicy::new(&temp_dir, true, true);

        let pass = mock_pass(2, 2);
        let samples = Samples {
            colors: vec![vec![1000; 4], vec![2000; 4], vec![3000; 4]],
            ir: None,
        };
        let artifact = FrameArtifact {
            frame_number: 1,
            total_frames: 1,
            dpi: 725,
            raw_rect: Rect {
                left: 0,
                right: 2,
                top: 0,
                bottom: 2,
            },
            samples,
            pass,
            crop: None,
            scan_metadata: crate::scanner::types::ScanMetadata {
                scanner_model: None,
                focus_position: None,
                exposures: None,
                hardware_samples: 1,
                software_passes: 1,
                infrared_cleaned_pixels: None,
            },
            roll: None,
        };

        let saved = save_frame_outputs(&artifact, &policy).unwrap();
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0].format, FileFormat::Bmp);
        assert_eq!(saved[0].crop_status, SavedCropStatus::Uncropped);
        assert_eq!(saved[1].format, FileFormat::Tiff);
        assert_eq!(saved[1].crop_status, SavedCropStatus::Uncropped);

        assert!(saved[0].path.exists());
        assert!(saved[1].path.exists());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn master_tiff_preserves_uncropped_samples_and_embeds_icc_while_working_tiff_is_cropped() {
        let temp_dir =
            std::env::temp_dir().join(format!("coolscan_master_test_{}", std::process::id()));
        let policy = OutputPolicy::new(&temp_dir, false, true).with_master_tiff(true);

        // 4x4 full scanner frame with accepted 2x2 crop (rows 1..3, cols 1..3)
        let pass = mock_pass(4, 4);
        let samples = Samples {
            colors: vec![vec![1100; 16], vec![2200; 16], vec![3300; 16]],
            ir: None,
        };
        let artifact = FrameArtifact {
            frame_number: 3,
            total_frames: 6,
            dpi: 2900,
            raw_rect: Rect {
                left: 0,
                right: 4,
                top: 0,
                bottom: 4,
            },
            samples,
            pass,
            crop: Some(crate::crop::CropDecision {
                leading: crate::crop::EdgeConfidence::Confident { column: 1, dots: 1 },
                trailing: crate::crop::EdgeConfidence::Confident { column: 3, dots: 3 },
                columns: (1, 3),
                rows: (1, 3),
                travel_dots: (1, 3),
                accepted: true,
                is_blank: false,
            }),
            scan_metadata: crate::scanner::types::ScanMetadata {
                scanner_model: Some("LS-40 ED".into()),
                focus_position: None,
                exposures: None,
                hardware_samples: 1,
                software_passes: 1,
                infrared_cleaned_pixels: None,
            },
            roll: None,
        };

        let saved = save_frame_outputs(&artifact, &policy).unwrap();
        assert_eq!(saved.len(), 2);

        // 1. Scanner Master TIFF (frame-3-master.tif): uncropped 4x4 + embedded ICC
        assert_eq!(saved[0].role, SavedFileRole::ScannerMaster);
        assert_eq!(saved[0].crop_status, SavedCropStatus::Uncropped);
        assert!(saved[0].icc_embedded);
        assert_eq!(saved[0].path.file_name().unwrap(), "frame-3-master.tif");
        let parsed_master = tiff::read_tiff(&saved[0].path).unwrap();
        assert_eq!(parsed_master.pass.rows, 4);
        assert_eq!(parsed_master.pass.cols, 4);
        assert_eq!(parsed_master.dpi, 2900);
        assert_eq!(
            parsed_master.icc_profile.as_deref(),
            Some(DEFAULT_LS40_ICC_BYTES)
        );

        // 2. Working TIFF (frame-3.tif): cropped 2x2, untagged for Darktable sidecar pairing
        assert_eq!(saved[1].role, SavedFileRole::WorkingTiff);
        assert_eq!(saved[1].crop_status, SavedCropStatus::Cropped);
        assert!(!saved[1].icc_embedded);
        assert_eq!(saved[1].path.file_name().unwrap(), "frame-3.tif");
        let parsed_working = tiff::read_tiff(&saved[1].path).unwrap();
        assert_eq!(parsed_working.pass.rows, 2);
        assert_eq!(parsed_working.pass.cols, 2);
        assert!(parsed_working.icc_profile.is_none());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
