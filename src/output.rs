use nkscan::{protocol::decode::Samples, scan::pass::Pass};
use std::path::{Path, PathBuf};

use crate::{bmp, scanner::types::FrameArtifact, tiff};

/// Configuration specifying which image formats and destinations to write.
#[derive(Debug, Clone)]
pub struct OutputPolicy {
    pub output_dir: PathBuf,
    pub save_bmp: bool,
    pub save_tiff: bool,
}

impl OutputPolicy {
    pub fn new(output_dir: impl Into<PathBuf>, save_bmp: bool, save_tiff: bool) -> Self {
        Self {
            output_dir: output_dir.into(),
            save_bmp,
            save_tiff,
        }
    }
}

/// Output file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    Bmp,
    Tiff,
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
    pub crop_status: SavedCropStatus,
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

    let mut saved = Vec::new();

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
            crop_status,
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
            crop_status,
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
}
