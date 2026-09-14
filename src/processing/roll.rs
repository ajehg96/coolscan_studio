use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::analysis::{
    SampleRect, TechnicalAnalysis, WorkingImage, analyse_pre_white_balance,
    finish_after_white_balance,
};
use super::color::{ColorError, ColorTransform};
use super::negadoctor::NegadoctorParams;
use super::orientation::Orientation;
use crate::scanner::types::FrameArtifact;

/// Current schema version for roll profiles.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

/// Unique identifier for a film roll profile.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RollId(pub String);

impl std::fmt::Display for RollId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<S: Into<String>> From<S> for RollId {
    fn from(s: S) -> Self {
        RollId(s.into())
    }
}

/// Scanner color input profile descriptor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScannerProfile {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icc_path: Option<PathBuf>,
}

impl ScannerProfile {
    /// Standard Nikon LS-40 ED / LS-4000 negative profile.
    pub fn ls40_negative() -> Self {
        Self {
            id: "ls-40-negative".to_string(),
            name: "Nikon LS-40 ED Negative".to_string(),
            icc_path: None,
        }
    }

    /// Returns the ICC profile filename for Darktable sidecar generation.
    pub fn icc_profile_name(&self) -> &str {
        if let Some(path) = &self.icc_path
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
        {
            return name;
        }
        "NKLS4000LS40_N.icc"
    }
}

/// Static catalog metadata identifying a film stock.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FilmStock {
    pub id: String,
    pub manufacturer: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iso: Option<u32>,
}

impl FilmStock {
    pub fn new(
        id: impl Into<String>,
        manufacturer: impl Into<String>,
        name: impl Into<String>,
        iso: Option<u32>,
    ) -> Self {
        Self {
            id: id.into(),
            manufacturer: manufacturer.into(),
            name: name.into(),
            iso,
        }
    }

    /// Kodak Pro Image 100 stock metadata.
    pub fn pro_image_100() -> Self {
        Self {
            id: "kodak-pro-image-100".to_string(),
            manufacturer: "Kodak".to_string(),
            name: "Kodak Pro Image 100".to_string(),
            iso: Some(100),
        }
    }

    /// Kodak Portra 400 stock metadata.
    pub fn portra_400() -> Self {
        Self {
            id: "kodak-portra-400".to_string(),
            manufacturer: "Kodak".to_string(),
            name: "Kodak Portra 400".to_string(),
            iso: Some(400),
        }
    }

    /// Kodak Gold 200 stock metadata.
    pub fn gold_200() -> Self {
        Self {
            id: "kodak-gold-200".to_string(),
            manufacturer: "Kodak".to_string(),
            name: "Kodak Gold 200".to_string(),
            iso: Some(200),
        }
    }
}

impl std::fmt::Display for FilmStock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl PartialEq<&str> for FilmStock {
    fn eq(&self, other: &&str) -> bool {
        self.name == *other || self.id == *other
    }
}

impl PartialEq<FilmStock> for &str {
    fn eq(&self, other: &FilmStock) -> bool {
        *self == other.name || *self == other.id
    }
}

impl From<&str> for FilmStock {
    fn from(name: &str) -> Self {
        let id = name.to_lowercase().replace(' ', "-");
        Self {
            id,
            manufacturer: String::new(),
            name: name.to_string(),
            iso: None,
        }
    }
}

impl From<String> for FilmStock {
    fn from(name: String) -> Self {
        Self::from(name.as_str())
    }
}

/// Physical, empirical roll calibration data.
///
/// Contains measured D-min values and associated scanner profile from a calibrated roll.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RollCalibration {
    pub dmin: [f32; 3],
    pub scanner_profile: ScannerProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl RollCalibration {
    /// Creates and validates roll calibration data.
    pub fn new(
        dmin: [f32; 3],
        scanner_profile: ScannerProfile,
        measured_at: Option<String>,
        notes: Option<String>,
    ) -> Result<Self, RollProfileError> {
        validate_dmin(dmin)?;
        Ok(Self {
            dmin,
            scanner_profile,
            measured_at,
            notes,
        })
    }
}

/// Validates that RGB D-min values are finite, strictly positive, and plausible (<= 5.0).
pub fn validate_dmin(dmin: [f32; 3]) -> Result<(), RollProfileError> {
    let channel_names = ["Red", "Green", "Blue"];
    for (i, &val) in dmin.iter().enumerate() {
        if val.is_nan() || val.is_infinite() {
            return Err(RollProfileError::InvalidDmin {
                channel: channel_names[i],
                value: val,
                reason: "D-min value must be a finite number",
            });
        }
        if val <= 0.0 {
            return Err(RollProfileError::InvalidDmin {
                channel: channel_names[i],
                value: val,
                reason: "D-min value must be strictly positive (> 0.0)",
            });
        }
        if val > 5.0 {
            return Err(RollProfileError::InvalidDmin {
                channel: channel_names[i],
                value: val,
                reason: "D-min value exceeds maximum plausible range (5.0)",
            });
        }
    }
    Ok(())
}

/// Roll-level film characteristics and optional empirical calibration.
///
/// Disentangles static catalog metadata (`film_stock`) from physical roll calibration (`calibration`).
/// D-min (unexposed film base color) is an empirical property of a specific roll.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RollProfile {
    pub version: u32,
    pub id: RollId,
    pub name: String,
    pub film_stock: FilmStock,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration: Option<RollCalibration>,
}

/// Errors occurring during roll profile validation, serialization, or loading.
#[derive(Debug)]
pub enum RollProfileError {
    InvalidDmin {
        channel: &'static str,
        value: f32,
        reason: &'static str,
    },
    InvalidId(String),
    UnsupportedVersion {
        found: u32,
        current: u32,
    },
    UncalibratedRoll,
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for RollProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RollProfileError::InvalidDmin {
                channel,
                value,
                reason,
            } => {
                write!(f, "Invalid D-min for channel {channel} ({value}): {reason}")
            }
            RollProfileError::InvalidId(msg) => write!(f, "Invalid Roll ID: {msg}"),
            RollProfileError::UnsupportedVersion { found, current } => {
                write!(
                    f,
                    "Unsupported roll profile version {found}; current version is {current}"
                )
            }
            RollProfileError::UncalibratedRoll => {
                write!(
                    f,
                    "Roll profile is uncalibrated: D-min calibration required"
                )
            }
            RollProfileError::Io(e) => write!(f, "I/O error: {e}"),
            RollProfileError::Json(e) => write!(f, "JSON error: {e}"),
        }
    }
}

impl std::error::Error for RollProfileError {}

impl From<RollProfileError> for ColorError {
    fn from(err: RollProfileError) -> Self {
        match err {
            RollProfileError::UncalibratedRoll => ColorError::UncalibratedRoll(
                "Roll profile is uncalibrated; D-min calibration required before auto-processing"
                    .to_string(),
            ),
            other => ColorError::Lcms(other.to_string()),
        }
    }
}

impl RollProfile {
    /// Creates and validates a new `RollProfile`.
    pub fn new(
        id: impl Into<RollId>,
        name: impl Into<String>,
        film_stock: impl Into<FilmStock>,
        calibration: Option<RollCalibration>,
    ) -> Result<Self, RollProfileError> {
        let profile = Self {
            version: CURRENT_SCHEMA_VERSION,
            id: id.into(),
            name: name.into(),
            film_stock: film_stock.into(),
            calibration,
        };
        profile.validate()?;
        Ok(profile)
    }

    /// Creates a calibrated roll profile with verified D-min values.
    pub fn calibrated(
        id: impl Into<RollId>,
        name: impl Into<String>,
        film_stock: impl Into<FilmStock>,
        dmin: [f32; 3],
        scanner_profile: ScannerProfile,
    ) -> Result<Self, RollProfileError> {
        let calibration = RollCalibration::new(dmin, scanner_profile, None, None)?;
        Self::new(id, name, film_stock, Some(calibration))
    }

    /// Creates an uncalibrated roll profile.
    pub fn uncalibrated(
        id: impl Into<RollId>,
        name: impl Into<String>,
        film_stock: impl Into<FilmStock>,
    ) -> Result<Self, RollProfileError> {
        Self::new(id, name, film_stock, None)
    }

    /// Validates roll profile constraints:
    /// - Non-empty identifier
    /// - Valid D-min values if calibration is present
    pub fn validate(&self) -> Result<(), RollProfileError> {
        let id_str = self.id.0.trim();
        if id_str.is_empty() {
            return Err(RollProfileError::InvalidId(
                "Roll identifier cannot be empty".into(),
            ));
        }

        if let Some(ref cal) = self.calibration {
            validate_dmin(cal.dmin)?;
        }
        Ok(())
    }

    /// Returns true if this roll profile has empirical calibration data.
    pub fn is_calibrated(&self) -> bool {
        self.calibration.is_some()
    }

    /// Returns the measured D-min RGB values if calibrated.
    pub fn dmin(&self) -> Option<[f32; 3]> {
        self.calibration.as_ref().map(|c| c.dmin)
    }

    /// Returns the associated scanner profile, or default LS-40 profile if uncalibrated.
    pub fn scanner_profile(&self) -> ScannerProfile {
        self.calibration
            .as_ref()
            .map(|c| c.scanner_profile.clone())
            .unwrap_or_else(ScannerProfile::ls40_negative)
    }

    /// Kodak Pro Image 100 baseline profile.
    ///
    /// Calibrated profile with values measured empirically from LS-40 scans:
    /// R = 0.8965, G = 0.9093, B = 0.8816
    pub fn pro_image_100() -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            id: RollId("kodak-pro-image-100".into()),
            name: "Kodak Pro Image 100 Baseline (Calibrated)".into(),
            film_stock: FilmStock::pro_image_100(),
            calibration: Some(RollCalibration {
                dmin: [0.8965, 0.9093, 0.8816],
                scanner_profile: ScannerProfile::ls40_negative(),
                measured_at: None,
                notes: Some("Measured empirically from LS-40 scans".into()),
            }),
        }
    }

    /// Kodak Portra 400 preset profile (uncalibrated).
    ///
    /// Uncalibrated film stock catalog entry; requires empirical calibration before inversion.
    pub fn portra_400() -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            id: RollId("kodak-portra-400".into()),
            name: "Kodak Portra 400 (Uncalibrated)".into(),
            film_stock: FilmStock::portra_400(),
            calibration: None,
        }
    }

    /// Kodak Gold 200 preset profile (uncalibrated).
    ///
    /// Uncalibrated film stock catalog entry; requires empirical calibration before inversion.
    pub fn gold_200() -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            id: RollId("kodak-gold-200".into()),
            name: "Kodak Gold 200 (Uncalibrated)".into(),
            film_stock: FilmStock::gold_200(),
            calibration: None,
        }
    }

    /// Serializes to formatted JSON.
    pub fn to_json(&self) -> Result<String, RollProfileError> {
        serde_json::to_string_pretty(self).map_err(RollProfileError::Json)
    }

    /// Deserializes a roll profile from JSON, automatically migrating legacy schema versions (v0 and v1).
    pub fn from_json(json: &str) -> Result<Self, RollProfileError> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum VersionedJson {
            V2(RollProfile),
            V1(LegacyV1),
            V0(LegacyV0),
        }

        #[derive(Deserialize)]
        struct LegacyV1 {
            version: Option<u32>,
            id: RollId,
            name: String,
            film_stock: String,
            dmin: [f32; 3],
            scanner_profile: ScannerProfile,
        }

        #[derive(Deserialize)]
        struct LegacyV0 {
            id: String,
            film_name: String,
            dmin: [f32; 3],
            scanner_profile_id: Option<String>,
        }

        let parsed: VersionedJson = serde_json::from_str(json).map_err(RollProfileError::Json)?;

        let profile = match parsed {
            VersionedJson::V2(p) => {
                if p.version > CURRENT_SCHEMA_VERSION {
                    return Err(RollProfileError::UnsupportedVersion {
                        found: p.version,
                        current: CURRENT_SCHEMA_VERSION,
                    });
                }
                p
            }
            VersionedJson::V1(v1) => {
                if let Some(v) = v1.version
                    && v > CURRENT_SCHEMA_VERSION
                {
                    return Err(RollProfileError::UnsupportedVersion {
                        found: v,
                        current: CURRENT_SCHEMA_VERSION,
                    });
                }
                let film_stock = FilmStock::from(v1.film_stock.as_str());
                let calibration = migrate_legacy_calibration(
                    &v1.id.0,
                    &v1.name,
                    &v1.film_stock,
                    v1.dmin,
                    v1.scanner_profile,
                );
                RollProfile {
                    version: CURRENT_SCHEMA_VERSION,
                    id: v1.id,
                    name: v1.name,
                    film_stock,
                    calibration,
                }
            }
            VersionedJson::V0(v0) => {
                let scanner_profile = ScannerProfile {
                    id: v0
                        .scanner_profile_id
                        .unwrap_or_else(|| "ls-40-negative".into()),
                    name: "Nikon LS-40 ED Negative".into(),
                    icc_path: None,
                };
                let film_stock = FilmStock::from(v0.film_name.as_str());
                let calibration = migrate_legacy_calibration(
                    &v0.id,
                    &v0.film_name,
                    &v0.film_name,
                    v0.dmin,
                    scanner_profile,
                );
                RollProfile {
                    version: CURRENT_SCHEMA_VERSION,
                    id: RollId(v0.id),
                    name: v0.film_name,
                    film_stock,
                    calibration,
                }
            }
        };

        profile.validate()?;
        Ok(profile)
    }

    /// Saves the roll profile to a JSON file.
    pub fn save_to_file(&self, path: &Path) -> Result<(), RollProfileError> {
        let json = self.to_json()?;
        std::fs::write(path, json).map_err(RollProfileError::Io)
    }

    /// Loads and validates a roll profile from a JSON file.
    pub fn load_from_file(path: &Path) -> Result<Self, RollProfileError> {
        let content = std::fs::read_to_string(path).map_err(RollProfileError::Io)?;
        Self::from_json(&content)
    }
}

/// Conservatively determines whether legacy D-min values represent verified empirical calibration.
///
/// Recognises the empirically measured Kodak Pro Image 100 baseline.
/// Old generic presets (Portra 400, Gold 200) and unknown legacy profiles lacking verified
/// provenance are migrated as uncalibrated (`calibration: None`), ensuring no fabricated
/// D-min values are silently promoted to verified calibration.
fn migrate_legacy_calibration(
    id: &str,
    name: &str,
    film_stock: &str,
    dmin: [f32; 3],
    scanner_profile: ScannerProfile,
) -> Option<RollCalibration> {
    let lower_id = id.to_lowercase();
    let lower_name = name.to_lowercase();
    let lower_stock = film_stock.to_lowercase();

    // Check if this matches the verified empirical Kodak Pro Image 100 baseline:
    let is_pro_image = lower_id.contains("pro-image")
        || lower_name.contains("pro image")
        || lower_stock.contains("pro image");
    let matches_pro_image_dmin = (dmin[0] - 0.8965).abs() < 1e-3
        && (dmin[1] - 0.9093).abs() < 1e-3
        && (dmin[2] - 0.8816).abs() < 1e-3;

    if is_pro_image && matches_pro_image_dmin {
        return Some(RollCalibration {
            dmin: [0.8965, 0.9093, 0.8816],
            scanner_profile,
            measured_at: None,
            notes: Some("Measured empirically from LS-40 scans".to_string()),
        });
    }

    // All other legacy profiles (generic presets or unknown stocks without provenance)
    // are migrated as uncalibrated.
    None
}

/// A scanned frame coupled with its roll profile, orientation, and Negadoctor processing state.
#[derive(Debug, Clone)]
pub struct PreparedFrame {
    pub source: FrameArtifact,
    pub roll: RollProfile,
    pub orientation: Orientation,
    pub params: NegadoctorParams,
    pub technical: TechnicalAnalysis,
    pub highlight_wb_rect: Option<SampleRect>,
}

impl PreparedFrame {
    /// Creates a prepared frame from an artifact and calibrated roll profile with default parameters.
    ///
    /// Requires that the roll profile contains valid calibration data.
    pub fn new(source: FrameArtifact, roll: RollProfile) -> Result<Self, RollProfileError> {
        let dmin = roll.dmin().ok_or(RollProfileError::UncalibratedRoll)?;
        let params = NegadoctorParams::from_dmin(dmin);
        Ok(Self {
            source,
            roll,
            orientation: Orientation::Normal,
            params,
            technical: TechnicalAnalysis {
                dmax: 2.046,
                scan_bias: -0.05,
            },
            highlight_wb_rect: None,
        })
    }

    /// Prepares a frame by performing technical analysis on the effective cropped image area.
    ///
    /// Requires that the roll profile contains valid calibration data (D-min).
    pub fn from_artifact(
        source: FrameArtifact,
        roll: RollProfile,
        pipeline: &impl ColorTransform,
    ) -> Result<Self, ColorError> {
        let dmin = roll.dmin().ok_or_else(|| {
            ColorError::UncalibratedRoll(format!(
                "Roll '{}' has no calibration data; D-min calibration required before auto-processing",
                roll.id
            ))
        })?;

        let effective = source
            .get_effective_image()
            .map_err(|e| ColorError::Lcms(e.to_string()))?;
        let working_image =
            WorkingImage::from_scanner_samples(effective.samples(), effective.pass(), pipeline)?;
        let technical = analyse_pre_white_balance(&working_image, &roll)
            .map_err(|e| ColorError::UncalibratedRoll(e.to_string()))?;

        let mut params = NegadoctorParams::from_dmin(dmin);
        params.dmax = technical.dmax;
        params.offset = technical.scan_bias;
        finish_after_white_balance(&working_image, &mut params);

        Ok(Self {
            source,
            roll,
            orientation: Orientation::Normal,
            params,
            technical,
            highlight_wb_rect: None,
        })
    }

    /// Generates the Darktable XMP sidecar for this prepared frame.
    ///
    /// Requires calibration data to be present on the roll profile.
    pub fn to_darktable_xmp(
        &self,
        image_filename: &str,
    ) -> Result<crate::darktable::xmp::DarktableXmp, crate::darktable::xmp::DarktableError> {
        let cal = self.roll.calibration.as_ref().ok_or_else(|| {
            crate::darktable::xmp::DarktableError::MissingHistoryItem(format!(
                "Roll '{}' is uncalibrated: D-min calibration required before XMP generation",
                self.roll.id
            ))
        })?;
        let profile_name = cal.scanner_profile.icc_profile_name();
        crate::darktable::xmp::DarktableXmp::for_frame(
            image_filename,
            profile_name,
            &self.params,
            self.orientation,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncalibrated_roll_cannot_generate_calibrated_negadoctor_output() {
        let roll = RollProfile::portra_400();
        assert!(!roll.is_calibrated());
        assert_eq!(roll.dmin(), None);

        // Technical analysis on working image must reject uncalibrated roll
        let pixels = vec![
            [1000.0, 1200.0, 800.0],
            [1500.0, 1600.0, 1400.0],
            [2000.0, 2200.0, 1800.0],
            [3000.0, 3200.0, 2800.0],
        ];
        let img = WorkingImage::new(2, 2, pixels);
        assert!(matches!(
            analyse_pre_white_balance(&img, &roll),
            Err(RollProfileError::UncalibratedRoll)
        ));

        // PreparedFrame::from_artifact must reject uncalibrated roll
        let artifact = FrameArtifact {
            frame_number: 1,
            total_frames: 1,
            dpi: 2900,
            raw_rect: nkscan::protocol::data::Rect {
                left: 0,
                right: 2,
                top: 0,
                bottom: 2,
            },
            samples: nkscan::protocol::decode::Samples {
                colors: vec![vec![1000; 4], vec![1200; 4], vec![800; 4]],
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
            roll: Some(roll.clone()),
        };

        let pipeline = crate::processing::color::IdentityColorTransform;
        let from_art_res = PreparedFrame::from_artifact(artifact.clone(), roll.clone(), &pipeline);
        assert!(matches!(from_art_res, Err(ColorError::UncalibratedRoll(_))));

        // PreparedFrame::new must reject uncalibrated roll
        let frame_res = PreparedFrame::new(artifact.clone(), roll.clone());
        assert!(matches!(frame_res, Err(RollProfileError::UncalibratedRoll)));

        // PreparedFrame::to_darktable_xmp must reject uncalibrated roll
        let mut frame = PreparedFrame::new(artifact, RollProfile::pro_image_100()).unwrap();
        frame.roll = roll;
        let xmp_res = frame.to_darktable_xmp("frame-1.tif");
        assert!(matches!(
            xmp_res,
            Err(crate::darktable::xmp::DarktableError::MissingHistoryItem(_))
        ));
    }

    #[test]
    fn calibrated_roll_round_trip() {
        let profile = RollProfile::pro_image_100();
        assert!(profile.is_calibrated());
        assert_eq!(profile.dmin(), Some([0.8965, 0.9093, 0.8816]));

        let json = profile.to_json().unwrap();
        let loaded = RollProfile::from_json(&json).unwrap();
        assert_eq!(profile, loaded);
        assert_eq!(loaded.version, CURRENT_SCHEMA_VERSION);
        assert_eq!(loaded.film_stock.name, "Kodak Pro Image 100");
        assert_eq!(loaded.dmin(), Some([0.8965, 0.9093, 0.8816]));
    }

    #[test]
    fn schema_migration_v0_and_v1() {
        // v0 migration: Kodak Gold 200 preset without provenance migrates as uncalibrated
        let legacy_v0 = r#"{
            "id": "legacy-gold-200",
            "film_name": "Kodak Gold 200",
            "dmin": [0.8540, 0.8820, 0.8110],
            "scanner_profile_id": "ls-40-negative"
        }"#;

        let migrated_v0 = RollProfile::from_json(legacy_v0).unwrap();
        assert_eq!(migrated_v0.version, CURRENT_SCHEMA_VERSION);
        assert_eq!(migrated_v0.id.0, "legacy-gold-200");
        assert_eq!(migrated_v0.film_stock.name, "Kodak Gold 200");
        assert_eq!(migrated_v0.dmin(), None);
        assert!(!migrated_v0.is_calibrated());
        assert_eq!(migrated_v0.scanner_profile().id, "ls-40-negative");

        // v1 migration: Kodak Pro Image 100 with known empirical baseline migrates as calibrated
        let legacy_v1 = r#"{
            "version": 1,
            "id": "v1-pro-image-100",
            "name": "Kodak Pro Image 100 Baseline",
            "film_stock": "Kodak Pro Image 100",
            "dmin": [0.8965, 0.9093, 0.8816],
            "scanner_profile": {
                "id": "ls-40-negative",
                "name": "Nikon LS-40 ED Negative",
                "icc_path": null
            }
        }"#;

        let migrated_v1 = RollProfile::from_json(legacy_v1).unwrap();
        assert_eq!(migrated_v1.version, CURRENT_SCHEMA_VERSION);
        assert_eq!(migrated_v1.id.0, "v1-pro-image-100");
        assert_eq!(migrated_v1.film_stock.name, "Kodak Pro Image 100");
        assert_eq!(migrated_v1.dmin(), Some([0.8965, 0.9093, 0.8816]));
        assert!(migrated_v1.is_calibrated());
        assert_eq!(migrated_v1.scanner_profile().id, "ls-40-negative");

        // Legacy Portra 400 with old invented preset values migrates as uncalibrated
        let legacy_portra = r#"{
            "version": 1,
            "id": "v1-portra-400",
            "name": "Kodak Portra 400",
            "film_stock": "Kodak Portra 400",
            "dmin": [0.8850, 0.9020, 0.8750],
            "scanner_profile": {
                "id": "ls-40-negative",
                "name": "Nikon LS-40 ED Negative",
                "icc_path": null
            }
        }"#;

        let migrated_portra = RollProfile::from_json(legacy_portra).unwrap();
        assert_eq!(migrated_portra.calibration, None);
        assert_eq!(migrated_portra.dmin(), None);
        assert!(!migrated_portra.is_calibrated());

        // Unknown legacy profile without provenance migrates as uncalibrated
        let legacy_unknown = r#"{
            "id": "custom-film-stock",
            "film_name": "Custom Unprovenanced Stock",
            "dmin": [0.8200, 0.8500, 0.8100],
            "scanner_profile_id": "ls-40-negative"
        }"#;

        let migrated_unknown = RollProfile::from_json(legacy_unknown).unwrap();
        assert_eq!(migrated_unknown.calibration, None);
        assert_eq!(migrated_unknown.dmin(), None);
        assert!(!migrated_unknown.is_calibrated());

        // Unsupported future version
        let future_json = r#"{
            "version": 99,
            "id": "future-roll",
            "name": "Future Roll",
            "film_stock": {
                "id": "future-film",
                "manufacturer": "Future",
                "name": "Future Film",
                "iso": 400
            },
            "calibration": null
        }"#;
        assert!(matches!(
            RollProfile::from_json(future_json),
            Err(RollProfileError::UnsupportedVersion {
                found: 99,
                current: CURRENT_SCHEMA_VERSION
            })
        ));
    }

    #[test]
    fn invalid_dmin_rejection() {
        // Negative channel
        let err = RollCalibration::new(
            [-0.1, 0.9, 0.9],
            ScannerProfile::ls40_negative(),
            None,
            None,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            RollProfileError::InvalidDmin { channel: "Red", .. }
        ));

        // Zero channel
        let err =
            RollCalibration::new([0.8, 0.0, 0.9], ScannerProfile::ls40_negative(), None, None)
                .unwrap_err();
        assert!(matches!(
            err,
            RollProfileError::InvalidDmin {
                channel: "Green",
                ..
            }
        ));

        // NaN channel
        let err = RollCalibration::new(
            [0.8, 0.9, f32::NAN],
            ScannerProfile::ls40_negative(),
            None,
            None,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            RollProfileError::InvalidDmin {
                channel: "Blue",
                ..
            }
        ));

        // Value too high
        let err =
            RollCalibration::new([6.0, 0.9, 0.9], ScannerProfile::ls40_negative(), None, None)
                .unwrap_err();
        assert!(matches!(
            err,
            RollProfileError::InvalidDmin { channel: "Red", .. }
        ));
    }

    #[test]
    fn stock_metadata_independent_of_calibration() {
        let stock = FilmStock::new("kodak-portra-400", "Kodak", "Kodak Portra 400", Some(400));
        assert_eq!(stock.id, "kodak-portra-400");
        assert_eq!(stock.manufacturer, "Kodak");
        assert_eq!(stock.name, "Kodak Portra 400");
        assert_eq!(stock.iso, Some(400));

        let uncalibrated =
            RollProfile::uncalibrated("roll-2026-001", "Vacation Roll 1", stock.clone()).unwrap();
        assert!(!uncalibrated.is_calibrated());
        assert_eq!(uncalibrated.dmin(), None);
        assert_eq!(uncalibrated.film_stock, "Kodak Portra 400");

        let calibration = RollCalibration::new(
            [0.85, 0.88, 0.82],
            ScannerProfile::ls40_negative(),
            Some("2026-09-13T20:00:00Z".to_string()),
            Some("Leader measured on LS-40".to_string()),
        )
        .unwrap();

        let calibrated =
            RollProfile::new("roll-2026-001", "Vacation Roll 1", stock, Some(calibration)).unwrap();
        assert!(calibrated.is_calibrated());
        assert_eq!(calibrated.dmin(), Some([0.85, 0.88, 0.82]));
        assert_eq!(calibrated.film_stock, "Kodak Portra 400");
    }

    #[test]
    fn invalid_id_rejection() {
        let err = RollProfile::uncalibrated("   ", "Test", "Film").unwrap_err();
        assert!(matches!(err, RollProfileError::InvalidId(_)));
    }

    #[test]
    fn file_save_and_load_round_trip() {
        let temp_file = std::env::temp_dir().join(format!("test_roll_{}.json", std::process::id()));
        let profile = RollProfile::pro_image_100();
        profile.save_to_file(&temp_file).unwrap();

        let loaded = RollProfile::load_from_file(&temp_file).unwrap();
        assert_eq!(profile, loaded);

        let _ = std::fs::remove_file(&temp_file);
    }

    #[test]
    fn prepared_frame_darktable_xmp_generation() {
        let profile = RollProfile::pro_image_100();
        let artifact = FrameArtifact {
            frame_number: 1,
            total_frames: 1,
            dpi: 2900,
            raw_rect: nkscan::protocol::data::Rect {
                left: 0,
                right: 4,
                top: 0,
                bottom: 4,
            },
            samples: nkscan::protocol::decode::Samples {
                colors: vec![vec![1000; 16]],
                ir: None,
            },
            pass: nkscan::scan::pass::Pass {
                layout: nkscan::protocol::image::Layout::single_line(4, 4, vec![1]),
                cooperation: Vec::new(),
                complete: true,
                blocks: 1,
                rows: 4,
                cols: 4,
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

        let mut frame = PreparedFrame::new(artifact, profile).unwrap();
        frame.orientation = Orientation::Rotate180;
        frame.params.dmax = 3.25;
        frame.params.offset = 0.12;

        let xmp = frame.to_darktable_xmp("frame-1.tif").unwrap();
        assert_eq!(xmp.derived_from, "frame-1.tif");
        assert_eq!(xmp.extract_orientation(), Orientation::Rotate180);
        let extracted_params = xmp.extract_negadoctor_params().unwrap();
        assert!((extracted_params.dmax - 3.25).abs() < 1e-4);
        assert!((extracted_params.offset - 0.12).abs() < 1e-4);
        assert_eq!(xmp.extract_colorin_profile().unwrap(), "NKLS4000LS40_N.icc");
    }
}
