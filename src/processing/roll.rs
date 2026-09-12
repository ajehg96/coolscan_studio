use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

use super::analysis::{
    analyse_pre_white_balance, finish_after_white_balance, SampleRect, TechnicalAnalysis,
    WorkingImage,
};
use super::color::{ColorError, ColorTransform};
use super::negadoctor::NegadoctorParams;
use super::orientation::Orientation;
use crate::scanner::types::FrameArtifact;


/// Current schema version for roll profiles.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

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
}

/// Roll-level film characteristics and scanner calibration.
///
/// D-min (unexposed film base color) is a property of the whole roll,
/// not an individual frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RollProfile {
    pub version: u32,
    pub id: RollId,
    pub name: String,
    pub film_stock: String,
    pub dmin: [f32; 3],
    pub scanner_profile: ScannerProfile,
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
                write!(
                    f,
                    "Invalid D-min for channel {channel} ({value}): {reason}"
                )
            }
            RollProfileError::InvalidId(msg) => write!(f, "Invalid Roll ID: {msg}"),
            RollProfileError::UnsupportedVersion { found, current } => {
                write!(
                    f,
                    "Unsupported roll profile version {found}; current version is {current}"
                )
            }
            RollProfileError::Io(e) => write!(f, "I/O error: {e}"),
            RollProfileError::Json(e) => write!(f, "JSON error: {e}"),
        }
    }
}

impl std::error::Error for RollProfileError {}

impl RollProfile {
    /// Creates and validates a new `RollProfile`.
    pub fn new(
        id: impl Into<RollId>,
        name: impl Into<String>,
        film_stock: impl Into<String>,
        dmin: [f32; 3],
        scanner_profile: ScannerProfile,
    ) -> Result<Self, RollProfileError> {
        let profile = Self {
            version: CURRENT_SCHEMA_VERSION,
            id: id.into(),
            name: name.into(),
            film_stock: film_stock.into(),
            dmin,
            scanner_profile,
        };
        profile.validate()?;
        Ok(profile)
    }

    /// Validates roll profile constraints:
    /// - Non-empty identifier
    /// - Positive, finite, plausible D-min RGB values
    pub fn validate(&self) -> Result<(), RollProfileError> {
        let id_str = self.id.0.trim();
        if id_str.is_empty() {
            return Err(RollProfileError::InvalidId(
                "Roll identifier cannot be empty".into(),
            ));
        }

        let channel_names = ["Red", "Green", "Blue"];
        for (i, &val) in self.dmin.iter().enumerate() {
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

    /// Kodak Pro Image 100 baseline profile.
    ///
    /// Values measured empirically from LS-40 scans:
    /// R = 0.8965, G = 0.9093, B = 0.8816
    pub fn pro_image_100() -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            id: RollId("kodak-pro-image-100".into()),
            name: "Kodak Pro Image 100 Baseline".into(),
            film_stock: "Kodak Pro Image 100".into(),
            dmin: [0.8965, 0.9093, 0.8816],
            scanner_profile: ScannerProfile::ls40_negative(),
        }
    }

    /// Serializes to formatted JSON.
    pub fn to_json(&self) -> Result<String, RollProfileError> {
        serde_json::to_string_pretty(self).map_err(RollProfileError::Json)
    }

    /// Deserializes a roll profile from JSON, automatically migrating legacy schema versions.
    pub fn from_json(json: &str) -> Result<Self, RollProfileError> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum VersionedJson {
            V1(RollProfile),
            V0(LegacyV0),
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
            VersionedJson::V1(p) => {
                if p.version > CURRENT_SCHEMA_VERSION {
                    return Err(RollProfileError::UnsupportedVersion {
                        found: p.version,
                        current: CURRENT_SCHEMA_VERSION,
                    });
                }
                p
            }
            VersionedJson::V0(v0) => {
                let scanner_profile = ScannerProfile {
                    id: v0
                        .scanner_profile_id
                        .unwrap_or_else(|| "ls-40-negative".into()),
                    name: "Nikon LS-40 ED Negative".into(),
                    icc_path: None,
                };
                RollProfile {
                    version: CURRENT_SCHEMA_VERSION,
                    id: RollId(v0.id),
                    name: v0.film_name.clone(),
                    film_stock: v0.film_name,
                    dmin: v0.dmin,
                    scanner_profile,
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
    /// Creates a prepared frame from an artifact and roll profile with default parameters.
    pub fn new(source: FrameArtifact, roll: RollProfile) -> Self {
        let params = NegadoctorParams::from_dmin(roll.dmin);
        Self {
            source,
            roll,
            orientation: Orientation::Normal,
            params,
            technical: TechnicalAnalysis {
                dmax: 2.046,
                scan_bias: -0.05,
            },
            highlight_wb_rect: None,
        }
    }

    /// Prepares a frame by performing technical analysis on the effective cropped image area.
    pub fn from_artifact(
        source: FrameArtifact,
        roll: RollProfile,
        pipeline: &impl ColorTransform,
    ) -> Result<Self, ColorError> {
        let effective = source
            .get_effective_image()
            .map_err(|e| ColorError::Lcms(e.to_string()))?;
        let working_image =
            WorkingImage::from_scanner_samples(effective.samples(), effective.pass(), pipeline)?;
        let technical = analyse_pre_white_balance(&working_image, &roll);

        let mut params = NegadoctorParams::from_dmin(roll.dmin);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization_round_trip() {
        let profile = RollProfile::pro_image_100();
        let json = profile.to_json().unwrap();
        let loaded = RollProfile::from_json(&json).unwrap();
        assert_eq!(profile, loaded);
        assert_eq!(loaded.version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn invalid_dmin_rejection() {
        // Negative channel
        let err = RollProfile::new(
            "test",
            "Test",
            "Film",
            [-0.1, 0.9, 0.9],
            ScannerProfile::ls40_negative(),
        )
        .unwrap_err();
        assert!(matches!(err, RollProfileError::InvalidDmin { channel: "Red", .. }));

        // Zero channel
        let err = RollProfile::new(
            "test",
            "Test",
            "Film",
            [0.8, 0.0, 0.9],
            ScannerProfile::ls40_negative(),
        )
        .unwrap_err();
        assert!(matches!(err, RollProfileError::InvalidDmin { channel: "Green", .. }));

        // NaN channel
        let err = RollProfile::new(
            "test",
            "Test",
            "Film",
            [0.8, 0.9, f32::NAN],
            ScannerProfile::ls40_negative(),
        )
        .unwrap_err();
        assert!(matches!(err, RollProfileError::InvalidDmin { channel: "Blue", .. }));

        // Value too high
        let err = RollProfile::new(
            "test",
            "Test",
            "Film",
            [6.0, 0.9, 0.9],
            ScannerProfile::ls40_negative(),
        )
        .unwrap_err();
        assert!(matches!(err, RollProfileError::InvalidDmin { channel: "Red", .. }));
    }

    #[test]
    fn invalid_id_rejection() {
        let err = RollProfile::new(
            "   ",
            "Test",
            "Film",
            [0.8, 0.9, 0.9],
            ScannerProfile::ls40_negative(),
        )
        .unwrap_err();
        assert!(matches!(err, RollProfileError::InvalidId(_)));
    }

    #[test]
    fn legacy_v0_schema_migration() {
        let legacy_json = r#"{
            "id": "legacy-gold-200",
            "film_name": "Kodak Gold 200",
            "dmin": [0.8540, 0.8820, 0.8110],
            "scanner_profile_id": "ls-40-negative"
        }"#;

        let migrated = RollProfile::from_json(legacy_json).unwrap();
        assert_eq!(migrated.version, CURRENT_SCHEMA_VERSION);
        assert_eq!(migrated.id.0, "legacy-gold-200");
        assert_eq!(migrated.film_stock, "Kodak Gold 200");
        assert_eq!(migrated.dmin, [0.8540, 0.8820, 0.8110]);
        assert_eq!(migrated.scanner_profile.id, "ls-40-negative");
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
}
