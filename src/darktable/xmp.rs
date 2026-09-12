//! Darktable XMP sidecar parser, serializer, and history builder.
//!
//! Provides binary-level parity with Darktable 5.x history entries:
//! - `colorin` (modversion 7): ICC profile configuration
//! - `colorout` (modversion 5): standard output profile
//! - `gamma` (modversion 1): display gamma
//! - `flip` (modversion 2): non-destructive orientation
//! - `negadoctor` (modversion 2): 76-byte film inversion & tone curve parameters
//! - Standard `blendop_version="14"` and `blendop_params`
//!
//! Handles both uncompressed hexadecimal encoding and Darktable's compressed
//! `gz<factor><base64>` deflate format.

use base64::prelude::*;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::path::Path;

use crate::processing::negadoctor::NegadoctorParams;
use crate::processing::orientation::Orientation;

/// Constant Darktable blend operation parameters for standard normal blending.
pub const DEFAULT_BLENDOP_VERSION: u32 = 14;
pub const DEFAULT_BLENDOP_PARAMS: &str =
    "gz11eJxjYIAACQYYOOHEgAZY0QWAgBGLGANDgz0Ej1Q+dcF/IADRAGpyHQU=";

/// Initial auto-applied module parameter constants.
pub const COLORIN_DEFAULT_PARAMS: &str = "gz48eJxjZBgFowABWAbaAaNgwAEAEDgABg==";
pub const COLOROUT_DEFAULT_PARAMS: &str = "gz35eJxjZBgFo4CBAQAEEAAC";
pub const GAMMA_DEFAULT_PARAMS: &str = "0000000000000000";
pub const FLIP_AUTO_PARAMS: &str = "ffffffff";

/// Errors encountered during Darktable XMP processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DarktableError {
    HexDecodeError(String),
    CompressionError(String),
    InvalidStructSize { expected: usize, actual: usize },
    XmlParseError(String),
    MissingHistoryItem(String),
    InvalidParamValue(String),
    IoError(String),
}

impl std::fmt::Display for DarktableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DarktableError::HexDecodeError(msg) => write!(f, "Hex decode error: {msg}"),
            DarktableError::CompressionError(msg) => write!(f, "Compression error: {msg}"),
            DarktableError::InvalidStructSize { expected, actual } => {
                write!(f, "Invalid struct size: expected {expected}, got {actual}")
            }
            DarktableError::XmlParseError(msg) => write!(f, "XML parse error: {msg}"),
            DarktableError::MissingHistoryItem(msg) => write!(f, "Missing history item: {msg}"),
            DarktableError::InvalidParamValue(msg) => write!(f, "Invalid parameter value: {msg}"),
            DarktableError::IoError(msg) => write!(f, "I/O error: {msg}"),
        }
    }
}

impl std::error::Error for DarktableError {}

// ---------------------------------------------------------------------------
// Parameter Codec (Hex & Darktable gz<factor><base64>)
// ---------------------------------------------------------------------------

pub struct DarktableParamCodec;

impl DarktableParamCodec {
    /// Encodes binary bytes to lowercase hexadecimal string.
    pub fn encode_hex(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    /// Decodes a hexadecimal string to binary bytes.
    pub fn decode_hex(s: &str) -> Result<Vec<u8>, DarktableError> {
        let clean = s.trim();
        if clean.len() % 2 != 0 {
            return Err(DarktableError::HexDecodeError(
                "Hex string length must be even".into(),
            ));
        }
        let mut out = Vec::with_capacity(clean.len() / 2);
        for i in (0..clean.len()).step_by(2) {
            let byte = u8::from_str_radix(&clean[i..i + 2], 16).map_err(|e| {
                DarktableError::HexDecodeError(format!("Invalid byte at pos {i}: {e}"))
            })?;
            out.push(byte);
        }
        Ok(out)
    }

    /// Encodes binary bytes using Darktable's compressed `gz<factor><base64>` format.
    pub fn encode_gz(bytes: &[u8]) -> Result<String, DarktableError> {
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(bytes, 6);
        if compressed.is_empty() {
            return Err(DarktableError::CompressionError(
                "Deflate compression returned empty buffer".into(),
            ));
        }
        // Darktable factor calculation: MIN(len / destLen + 1, 99)
        let factor = std::cmp::min((bytes.len() / compressed.len() + 1) as u32, 99);
        let b64 = BASE64_STANDARD.encode(&compressed);
        Ok(format!("gz{:02}{b64}", factor))
    }

    /// Decodes Darktable's compressed `gz<factor><base64>` format to binary bytes.
    pub fn decode_gz(s: &str) -> Result<Vec<u8>, DarktableError> {
        let clean = s.trim();
        if !clean.starts_with("gz") || clean.len() < 4 {
            return Err(DarktableError::CompressionError(
                "String does not start with gz prefix".into(),
            ));
        }
        let _factor: u32 = clean[2..4].parse().map_err(|e| {
            DarktableError::CompressionError(format!("Invalid compression factor: {e}"))
        })?;
        let b64_part = &clean[4..];
        let compressed = BASE64_STANDARD
            .decode(b64_part)
            .map_err(|e| DarktableError::CompressionError(format!("Base64 decode error: {e}")))?;

        miniz_oxide::inflate::decompress_to_vec_zlib(&compressed)
            .map_err(|e| DarktableError::CompressionError(format!("Zlib decompression error: {e:?}")))
    }

    /// Automatically decodes either `gz...` or hexadecimal parameter strings.
    pub fn decode_param(s: &str) -> Result<Vec<u8>, DarktableError> {
        let clean = s.trim();
        if clean.starts_with("gz") {
            Self::decode_gz(clean)
        } else {
            Self::decode_hex(clean)
        }
    }
}

// ---------------------------------------------------------------------------
// Typed Module Structs
// ---------------------------------------------------------------------------

/// Parameters for Darktable's `colorin` module (`dt_iop_colorin_params_t`, 1044 bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorinParams {
    pub profile_type: i32,
    pub filename: String,
    pub intent: i32,
    pub normalize: i32,
    pub blue_mapping: i32,
    pub type_work: i32,
    pub filename_work: String,
}

impl ColorinParams {
    /// Creates a custom file profile (type 0) pointing to an ICC profile with Rec.2020 working space (type 4).
    pub fn for_icc_profile(profile_path_or_name: &str) -> Self {
        Self {
            profile_type: 0, // DT_COLORSPACE_FILE
            filename: profile_path_or_name.to_string(),
            intent: 0,       // DT_INTENT_PERCEPTUAL
            normalize: 0,    // DT_NORMALIZE_OFF
            blue_mapping: 0, // FALSE
            type_work: 4,    // DT_COLORSPACE_LIN_REC2020
            filename_work: String::new(),
        }
    }

    /// Serializes to the exact 1044-byte C struct layout.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(1044);
        buf.extend_from_slice(&self.profile_type.to_le_bytes());

        let mut fn_buf = [0u8; 512];
        let bytes = self.filename.as_bytes();
        let len = bytes.len().min(511);
        fn_buf[..len].copy_from_slice(&bytes[..len]);
        buf.extend_from_slice(&fn_buf);

        buf.extend_from_slice(&self.intent.to_le_bytes());
        buf.extend_from_slice(&self.normalize.to_le_bytes());
        buf.extend_from_slice(&self.blue_mapping.to_le_bytes());
        buf.extend_from_slice(&self.type_work.to_le_bytes());

        let mut fn_work_buf = [0u8; 512];
        let work_bytes = self.filename_work.as_bytes();
        let work_len = work_bytes.len().min(511);
        fn_work_buf[..work_len].copy_from_slice(&work_bytes[..work_len]);
        buf.extend_from_slice(&fn_work_buf);

        buf
    }

    /// Deserializes from the 1044-byte binary representation.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DarktableError> {
        if bytes.len() != 1044 {
            return Err(DarktableError::InvalidStructSize {
                expected: 1044,
                actual: bytes.len(),
            });
        }
        let profile_type = i32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let fn_raw = &bytes[4..516];
        let fn_len = fn_raw.iter().position(|&b| b == 0).unwrap_or(512);
        let filename = String::from_utf8_lossy(&fn_raw[..fn_len]).to_string();

        let intent = i32::from_le_bytes(bytes[516..520].try_into().unwrap());
        let normalize = i32::from_le_bytes(bytes[520..524].try_into().unwrap());
        let blue_mapping = i32::from_le_bytes(bytes[524..528].try_into().unwrap());
        let type_work = i32::from_le_bytes(bytes[528..532].try_into().unwrap());

        let fn_work_raw = &bytes[532..1044];
        let fn_work_len = fn_work_raw.iter().position(|&b| b == 0).unwrap_or(512);
        let filename_work = String::from_utf8_lossy(&fn_work_raw[..fn_work_len]).to_string();

        Ok(Self {
            profile_type,
            filename,
            intent,
            normalize,
            blue_mapping,
            type_work,
            filename_work,
        })
    }

    /// Encodes into a compressed Darktable `gz...` string.
    pub fn to_gz_param(&self) -> Result<String, DarktableError> {
        DarktableParamCodec::encode_gz(&self.to_bytes())
    }
}

/// Parameters for Darktable's `flip` module (`dt_iop_flip_params_t`, 4 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlipParams {
    pub raw: u32,
}

impl FlipParams {
    pub fn from_orientation(orientation: Orientation) -> Self {
        let raw = match orientation {
            Orientation::Normal => 0,
            Orientation::Rotate90 => 6,
            Orientation::Rotate180 => 3,
            Orientation::Rotate270 => 5,
        };
        Self { raw }
    }

    pub fn to_orientation(&self) -> Option<Orientation> {
        match self.raw {
            0 => Some(Orientation::Normal),
            6 => Some(Orientation::Rotate90),
            3 => Some(Orientation::Rotate180),
            5 => Some(Orientation::Rotate270),
            _ => None,
        }
    }

    pub fn to_hex(&self) -> String {
        let bytes = self.raw.to_le_bytes();
        DarktableParamCodec::encode_hex(&bytes)
    }

    pub fn from_hex(s: &str) -> Result<Self, DarktableError> {
        let bytes = DarktableParamCodec::decode_hex(s)?;
        if bytes.len() != 4 {
            return Err(DarktableError::InvalidStructSize {
                expected: 4,
                actual: bytes.len(),
            });
        }
        let raw = u32::from_le_bytes(bytes[..4].try_into().unwrap());
        Ok(Self { raw })
    }
}

/// Helper conversions between `NegadoctorParams` and Darktable's 76-byte C struct.
pub trait DarktableNegadoctorExt {
    fn to_darktable_bytes(&self) -> [u8; 76];
    fn to_darktable_hex(&self) -> String;
    fn from_darktable_bytes(bytes: &[u8]) -> Result<NegadoctorParams, DarktableError>;
    fn from_darktable_hex(hex: &str) -> Result<NegadoctorParams, DarktableError>;
}

impl DarktableNegadoctorExt for NegadoctorParams {
    fn to_darktable_bytes(&self) -> [u8; 76] {
        let mut buf = [0u8; 76];
        // 0..4: film_stock (1 = negative)
        buf[0..4].copy_from_slice(&1i32.to_le_bytes());

        // 4..20: dmin[4]
        buf[4..8].copy_from_slice(&self.dmin[0].to_le_bytes());
        buf[8..12].copy_from_slice(&self.dmin[1].to_le_bytes());
        buf[12..16].copy_from_slice(&self.dmin[2].to_le_bytes());
        buf[16..20].copy_from_slice(&1.0f32.to_le_bytes());

        // 20..36: wb_high[4]
        buf[20..24].copy_from_slice(&self.wb_high[0].to_le_bytes());
        buf[24..28].copy_from_slice(&self.wb_high[1].to_le_bytes());
        buf[28..32].copy_from_slice(&self.wb_high[2].to_le_bytes());
        buf[32..36].copy_from_slice(&1.0f32.to_le_bytes());

        // 36..52: wb_low[4]
        buf[36..40].copy_from_slice(&self.wb_low[0].to_le_bytes());
        buf[40..44].copy_from_slice(&self.wb_low[1].to_le_bytes());
        buf[44..48].copy_from_slice(&self.wb_low[2].to_le_bytes());
        buf[48..52].copy_from_slice(&1.0f32.to_le_bytes());

        // 52..56: dmax
        buf[52..56].copy_from_slice(&self.dmax.to_le_bytes());
        // 56..60: offset (scan_bias)
        buf[56..60].copy_from_slice(&self.offset.to_le_bytes());
        // 60..64: black (paper_black)
        buf[60..64].copy_from_slice(&self.paper_black.to_le_bytes());
        // 64..68: gamma (paper_grade)
        buf[64..68].copy_from_slice(&self.paper_grade.to_le_bytes());
        // 68..72: soft_clip (paper_gloss)
        buf[68..72].copy_from_slice(&self.paper_gloss.to_le_bytes());
        // 72..76: exposure (print_exposure)
        buf[72..76].copy_from_slice(&self.print_exposure.to_le_bytes());

        buf
    }

    fn to_darktable_hex(&self) -> String {
        DarktableParamCodec::encode_hex(&self.to_darktable_bytes())
    }

    fn from_darktable_bytes(bytes: &[u8]) -> Result<NegadoctorParams, DarktableError> {
        if bytes.len() != 76 {
            return Err(DarktableError::InvalidStructSize {
                expected: 76,
                actual: bytes.len(),
            });
        }
        let read_f32 = |offset: usize| -> f32 {
            f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
        };

        let dmin = [read_f32(4), read_f32(8), read_f32(12)];
        let wb_high = [read_f32(20), read_f32(24), read_f32(28)];
        let wb_low = [read_f32(36), read_f32(40), read_f32(44)];
        let dmax = read_f32(52);
        let offset = read_f32(56);
        let paper_black = read_f32(60);
        let paper_grade = read_f32(64);
        let paper_gloss = read_f32(68);
        let print_exposure = read_f32(72);

        Ok(NegadoctorParams {
            dmin,
            dmax,
            offset,
            wb_high,
            wb_low,
            paper_black,
            paper_grade,
            paper_gloss,
            print_exposure,
        })
    }

    fn from_darktable_hex(hex: &str) -> Result<NegadoctorParams, DarktableError> {
        let bytes = DarktableParamCodec::decode_param(hex)?;
        Self::from_darktable_bytes(&bytes)
    }
}

// ---------------------------------------------------------------------------
// History Entry & Complete Document Representation
// ---------------------------------------------------------------------------

/// A single operation item in the Darktable history stack (`<rdf:li>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DarktableHistoryItem {
    pub num: u32,
    pub operation: String,
    pub enabled: bool,
    pub modversion: u32,
    pub params: String,
    pub multi_name: String,
    pub multi_name_hand_edited: u32,
    pub multi_priority: u32,
    pub blendop_version: u32,
    pub blendop_params: String,
}

impl DarktableHistoryItem {
    /// Decodes the params string (either gz-deflated base64 or hex) into raw bytes.
    pub fn decode_params(&self) -> Result<Vec<u8>, DarktableError> {
        DarktableParamCodec::decode_param(&self.params)
    }
}

/// Darktable XMP sidecar document representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DarktableXmp {
    pub derived_from: String,
    pub xmp_version: u32,
    pub iop_order_version: u32,
    pub raw_params: u32,
    pub auto_presets_applied: u32,
    pub history_end: u32,
    pub history: Vec<DarktableHistoryItem>,
    pub history_basic_hash: Option<String>,
    pub history_current_hash: Option<String>,
}

impl DarktableXmp {
    /// Builds a canonical Darktable XMP sidecar for a scanned frame.
    ///
    /// Generates a clean 6- or 7-step history sequence:
    /// 0: colorin (default setup)
    /// 1: colorout (default sRGB)
    /// 2: gamma (default)
    /// 3: flip (auto ffffffff)
    /// 4: colorin (calibrated input ICC profile)
    /// 5: negadoctor (calibrated inversion parameters)
    /// 6: flip (if orientation is not Normal)
    pub fn for_frame(
        derived_from: &str,
        input_profile_name_or_path: &str,
        negadoctor: &NegadoctorParams,
        orientation: Orientation,
    ) -> Result<Self, DarktableError> {
        let mut history = Vec::new();

        // 0. Default colorin
        history.push(DarktableHistoryItem {
            num: 0,
            operation: "colorin".into(),
            enabled: true,
            modversion: 7,
            params: COLORIN_DEFAULT_PARAMS.into(),
            multi_name: String::new(),
            multi_name_hand_edited: 0,
            multi_priority: 0,
            blendop_version: DEFAULT_BLENDOP_VERSION,
            blendop_params: DEFAULT_BLENDOP_PARAMS.into(),
        });

        // 1. Default colorout
        history.push(DarktableHistoryItem {
            num: 1,
            operation: "colorout".into(),
            enabled: true,
            modversion: 5,
            params: COLOROUT_DEFAULT_PARAMS.into(),
            multi_name: String::new(),
            multi_name_hand_edited: 0,
            multi_priority: 0,
            blendop_version: DEFAULT_BLENDOP_VERSION,
            blendop_params: DEFAULT_BLENDOP_PARAMS.into(),
        });

        // 2. Default gamma
        history.push(DarktableHistoryItem {
            num: 2,
            operation: "gamma".into(),
            enabled: true,
            modversion: 1,
            params: GAMMA_DEFAULT_PARAMS.into(),
            multi_name: String::new(),
            multi_name_hand_edited: 0,
            multi_priority: 0,
            blendop_version: DEFAULT_BLENDOP_VERSION,
            blendop_params: DEFAULT_BLENDOP_PARAMS.into(),
        });

        // 3. Builtin auto flip
        history.push(DarktableHistoryItem {
            num: 3,
            operation: "flip".into(),
            enabled: true,
            modversion: 2,
            params: FLIP_AUTO_PARAMS.into(),
            multi_name: "_builtin_auto".into(),
            multi_name_hand_edited: 0,
            multi_priority: 0,
            blendop_version: DEFAULT_BLENDOP_VERSION,
            blendop_params: DEFAULT_BLENDOP_PARAMS.into(),
        });

        // 4. Calibrated scanner colorin
        let colorin_params = ColorinParams::for_icc_profile(input_profile_name_or_path);
        let colorin_gz = colorin_params.to_gz_param()?;
        history.push(DarktableHistoryItem {
            num: 4,
            operation: "colorin".into(),
            enabled: true,
            modversion: 7,
            params: colorin_gz,
            multi_name: String::new(),
            multi_name_hand_edited: 0,
            multi_priority: 0,
            blendop_version: DEFAULT_BLENDOP_VERSION,
            blendop_params: DEFAULT_BLENDOP_PARAMS.into(),
        });

        // 5. Calibrated negadoctor
        let negadoctor_hex = negadoctor.to_darktable_hex();
        history.push(DarktableHistoryItem {
            num: 5,
            operation: "negadoctor".into(),
            enabled: true,
            modversion: 2,
            params: negadoctor_hex,
            multi_name: String::new(),
            multi_name_hand_edited: 0,
            multi_priority: 0,
            blendop_version: DEFAULT_BLENDOP_VERSION,
            blendop_params: DEFAULT_BLENDOP_PARAMS.into(),
        });

        // 6. Non-normal orientation flip (if needed)
        if orientation != Orientation::Normal {
            let flip_params = FlipParams::from_orientation(orientation);
            let multi_name = match orientation {
                Orientation::Rotate90 => "_builtin_rotate by 90 degrees",
                Orientation::Rotate180 => "_builtin_rotate by 180 degrees",
                Orientation::Rotate270 => "_builtin_rotate by 270 degrees",
                Orientation::Normal => "",
            };
            history.push(DarktableHistoryItem {
                num: 6,
                operation: "flip".into(),
                enabled: true,
                modversion: 2,
                params: flip_params.to_hex(),
                multi_name: multi_name.into(),
                multi_name_hand_edited: 0,
                multi_priority: 0,
                blendop_version: DEFAULT_BLENDOP_VERSION,
                blendop_params: DEFAULT_BLENDOP_PARAMS.into(),
            });
        }

        let history_end = history.len() as u32;

        Ok(Self {
            derived_from: derived_from.to_string(),
            xmp_version: 5,
            iop_order_version: 5,
            raw_params: 0,
            auto_presets_applied: 1,
            history_end,
            history,
            history_basic_hash: None,
            history_current_hash: None,
        })
    }

    /// Returns true if any history entry matches the given operation name.
    pub fn has_operation(&self, operation: &str) -> bool {
        self.history.iter().any(|h| h.operation == operation)
    }

    /// Finds the first history entry with the specified operation name.
    pub fn find_operation(&self, operation: &str) -> Option<&DarktableHistoryItem> {
        self.history.iter().find(|h| h.operation == operation)
    }

    /// Finds the last history entry with the specified operation name.
    pub fn find_last_operation(&self, operation: &str) -> Option<&DarktableHistoryItem> {
        self.history.iter().rfind(|h| h.operation == operation)
    }

    /// Serializes this document to Darktable XMP XML.
    pub fn to_xmp_string(&self) -> String {
        let mut out = String::with_capacity(4096);
        out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        out.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"XMP Core 4.4.0-Exiv2\">\n");
        out.push_str(" <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
        out.push_str("  <rdf:Description rdf:about=\"\"\n");
        out.push_str("    xmlns:exif=\"http://ns.adobe.com/exif/1.0/\"\n");
        out.push_str("    xmlns:xmpMM=\"http://ns.adobe.com/xap/1.0/mm/\"\n");
        out.push_str("    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n");
        out.push_str("    xmlns:darktable=\"http://darktable.sf.net/\"\n");
        out.push_str("    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n");
        out.push_str("    xmlns:lr=\"http://ns.adobe.com/lightroom/1.0/\"\n");
        out.push_str("   exif:DateTimeOriginal=\"\"\n");
        let _ = writeln!(out, "   xmpMM:DerivedFrom=\"{}\"", self.derived_from);
        out.push_str("   xmp:Rating=\"1\"\n");
        let _ = writeln!(
            out,
            "   darktable:import_timestamp=\"{}\"",
            10000000000000000u64
        );
        let _ = writeln!(
            out,
            "   darktable:change_timestamp=\"{}\"",
            10000000000000000u64
        );
        out.push_str("   darktable:export_timestamp=\"-1\"\n");
        out.push_str("   darktable:print_timestamp=\"-1\"\n");
        let _ = writeln!(out, "   darktable:xmp_version=\"{}\"", self.xmp_version);
        let _ = writeln!(out, "   darktable:raw_params=\"{}\"", self.raw_params);
        let _ = writeln!(
            out,
            "   darktable:auto_presets_applied=\"{}\"",
            self.auto_presets_applied
        );
        let _ = writeln!(out, "   darktable:history_end=\"{}\"", self.history_end);
        let _ = writeln!(
            out,
            "   darktable:iop_order_version=\"{}\"",
            self.iop_order_version
        );

        if let Some(h) = &self.history_basic_hash {
            let _ = writeln!(out, "   darktable:history_basic_hash=\"{h}\"");
        }
        if let Some(h) = &self.history_current_hash {
            let _ = writeln!(out, "   darktable:history_current_hash=\"{h}\"");
        }

        out.push_str(">\n");
        out.push_str("   <darktable:masks_history>\n");
        out.push_str("    <rdf:Seq/>\n");
        out.push_str("   </darktable:masks_history>\n");
        out.push_str("   <darktable:history>\n");
        out.push_str("    <rdf:Seq>\n");

        for item in &self.history {
            let enabled_str = if item.enabled { "1" } else { "0" };
            let _ = writeln!(out, "     <rdf:li");
            let _ = writeln!(out, "      darktable:num=\"{}\"", item.num);
            let _ = writeln!(out, "      darktable:operation=\"{}\"", item.operation);
            let _ = writeln!(out, "      darktable:enabled=\"{}\"", enabled_str);
            let _ = writeln!(out, "      darktable:modversion=\"{}\"", item.modversion);
            let _ = writeln!(out, "      darktable:params=\"{}\"", item.params);
            let _ = writeln!(out, "      darktable:multi_name=\"{}\"", item.multi_name);
            let _ = writeln!(
                out,
                "      darktable:multi_name_hand_edited=\"{}\"",
                item.multi_name_hand_edited
            );
            let _ = writeln!(
                out,
                "      darktable:multi_priority=\"{}\"",
                item.multi_priority
            );
            let _ = writeln!(
                out,
                "      darktable:blendop_version=\"{}\"",
                item.blendop_version
            );
            let _ = writeln!(
                out,
                "      darktable:blendop_params=\"{}\"/>",
                item.blendop_params
            );
        }

        out.push_str("    </rdf:Seq>\n");
        out.push_str("   </darktable:history>\n");
        out.push_str("  </rdf:Description>\n");
        out.push_str(" </rdf:RDF>\n");
        out.push_str("</x:xmpmeta>\n");

        out
    }

    /// Writes this document to disk at the given path.
    pub fn write_to_file(&self, path: &Path) -> Result<(), DarktableError> {
        let content = self.to_xmp_string();
        fs::write(path, content)
            .map_err(|e| DarktableError::IoError(format!("Failed to write {path:?}: {e}")))
    }

    /// Parses a Darktable XMP XML string into a typed `DarktableXmp` structure.
    pub fn parse(xml_str: &str) -> Result<Self, DarktableError> {
        let mut reader = Reader::from_str(xml_str);
        reader.config_mut().trim_text(true);

        let mut derived_from = String::new();
        let mut xmp_version = 5u32;
        let mut iop_order_version = 5u32;
        let mut raw_params = 0u32;
        let mut auto_presets_applied = 1u32;
        let mut history_end = 0u32;
        let mut history_basic_hash = None;
        let mut history_current_hash = None;

        let mut history = Vec::new();
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                    let local_name = e.local_name();
                    if local_name.as_ref() == b"Description" {
                        for attr in e.attributes() {
                            let attr = attr.map_err(|err| {
                                DarktableError::XmlParseError(format!("Attribute error: {err}"))
                            })?;
                            let local = attr.key.local_name();
                            let val = String::from_utf8_lossy(&attr.value).to_string();

                            if local.as_ref() == b"DerivedFrom" {
                                derived_from = val;
                            } else if local.as_ref() == b"xmp_version" {
                                xmp_version = val.parse().unwrap_or(5);
                            } else if local.as_ref() == b"iop_order_version" {
                                iop_order_version = val.parse().unwrap_or(5);
                            } else if local.as_ref() == b"raw_params" {
                                raw_params = val.parse().unwrap_or(0);
                            } else if local.as_ref() == b"auto_presets_applied" {
                                auto_presets_applied = val.parse().unwrap_or(1);
                            } else if local.as_ref() == b"history_end" {
                                history_end = val.parse().unwrap_or(0);
                            } else if local.as_ref() == b"history_basic_hash" {
                                history_basic_hash = Some(val);
                            } else if local.as_ref() == b"history_current_hash" {
                                history_current_hash = Some(val);
                            }
                        }
                    } else if local_name.as_ref() == b"li" {
                        let mut num = 0u32;
                        let mut operation = String::new();
                        let mut enabled = true;
                        let mut modversion = 1u32;
                        let mut params = String::new();
                        let mut multi_name = String::new();
                        let mut multi_name_hand_edited = 0u32;
                        let mut multi_priority = 0u32;
                        let mut blendop_version = DEFAULT_BLENDOP_VERSION;
                        let mut blendop_params = DEFAULT_BLENDOP_PARAMS.to_string();
                        let mut is_history_li = false;

                        for attr in e.attributes() {
                            let attr = attr.map_err(|err| {
                                DarktableError::XmlParseError(format!("Attribute error: {err}"))
                            })?;
                            let local = attr.key.local_name();
                            let val = String::from_utf8_lossy(&attr.value).to_string();

                            if local.as_ref() == b"operation" {
                                operation = val;
                                is_history_li = true;
                            } else if local.as_ref() == b"num" {
                                num = val.parse().unwrap_or(0);
                            } else if local.as_ref() == b"enabled" {
                                enabled = val != "0";
                            } else if local.as_ref() == b"modversion" {
                                modversion = val.parse().unwrap_or(1);
                            } else if local.as_ref() == b"params" {
                                params = val;
                            } else if local.as_ref() == b"blendop_params" {
                                blendop_params = val;
                            } else if local.as_ref() == b"blendop_version" {
                                blendop_version = val.parse().unwrap_or(DEFAULT_BLENDOP_VERSION);
                            } else if local.as_ref() == b"multi_name" {
                                multi_name = val;
                            } else if local.as_ref() == b"multi_name_hand_edited" {
                                multi_name_hand_edited = val.parse().unwrap_or(0);
                            } else if local.as_ref() == b"multi_priority" {
                                multi_priority = val.parse().unwrap_or(0);
                            }
                        }

                        if is_history_li {
                            history.push(DarktableHistoryItem {
                                num,
                                operation,
                                enabled,
                                modversion,
                                params,
                                multi_name,
                                multi_name_hand_edited,
                                multi_priority,
                                blendop_version,
                                blendop_params,
                            });
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    return Err(DarktableError::XmlParseError(format!(
                        "Error at pos {}: {:?}",
                        reader.buffer_position(),
                        e
                    )));
                }
                _ => {}
            }
            buf.clear();
        }

        if history_end == 0 && !history.is_empty() {
            history_end = history.len() as u32;
        }

        Ok(Self {
            derived_from,
            xmp_version,
            iop_order_version,
            raw_params,
            auto_presets_applied,
            history_end,
            history,
            history_basic_hash,
            history_current_hash,
        })
    }

    /// Extracts the active / latest `negadoctor` parameters from history.
    pub fn extract_negadoctor_params(&self) -> Result<NegadoctorParams, DarktableError> {
        let entry = self
            .history
            .iter()
            .rev()
            .find(|item| item.operation == "negadoctor" && item.enabled)
            .ok_or_else(|| {
                DarktableError::MissingHistoryItem("No enabled negadoctor entry found".into())
            })?;

        NegadoctorParams::from_darktable_hex(&entry.params)
    }

    /// Extracts the active orientation from the latest `flip` history item.
    pub fn extract_orientation(&self) -> Orientation {
        for item in self.history.iter().rev() {
            if item.operation == "flip" && item.enabled && item.params != FLIP_AUTO_PARAMS {
                if let Ok(flip) = FlipParams::from_hex(&item.params) {
                    if let Some(orient) = flip.to_orientation() {
                        return orient;
                    }
                }
            }
        }
        Orientation::Normal
    }

    /// Extracts the configured input ICC profile name or path from the latest `colorin` item.
    pub fn extract_colorin_profile(&self) -> Option<String> {
        for item in self.history.iter().rev() {
            if item.operation == "colorin" && item.enabled {
                if let Ok(bytes) = DarktableParamCodec::decode_param(&item.params) {
                    if let Ok(colorin) = ColorinParams::from_bytes(&bytes) {
                        if !colorin.filename.is_empty() {
                            return Some(colorin.filename);
                        }
                    }
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_codec_round_trip() {
        let original = vec![0x00, 0x01, 0x02, 0x7f, 0x80, 0xff, 0xaa, 0x55];
        let hex = DarktableParamCodec::encode_hex(&original);
        assert_eq!(hex, "0001027f80ffaa55");
        let decoded = DarktableParamCodec::decode_hex(&hex).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn gz_codec_round_trip() {
        let original = b"Hello Darktable XMP Compression Test Buffer with repetitive data 1234567890 1234567890 1234567890";
        let gz = DarktableParamCodec::encode_gz(original).unwrap();
        assert!(gz.starts_with("gz"));
        let decoded = DarktableParamCodec::decode_gz(&gz).unwrap();
        assert_eq!(&decoded[..], &original[..]);
    }

    #[test]
    fn negadoctor_parameters_exact_golden_match() {
        // Exact 152-character hex from scans-strip-02/frame-1.tif.xmp
        let fixture_hex = "010000000581653fe1c7683f88b0613f0000803f4eadf93f20e2ce3f0000803f0000803f0000803f0000803f0000803f0000803f736551405219d93dd0cccc3dffff8f400000403f08ac6c3f";

        let params = NegadoctorParams::from_darktable_hex(fixture_hex).unwrap();

        assert!((params.dmin[0] - 0.8965).abs() < 1e-4);
        assert!((params.dmin[1] - 0.9093).abs() < 1e-4);
        assert!((params.dmin[2] - 0.8816).abs() < 1e-4);

        assert!((params.wb_high[0] - 1.9506).abs() < 1e-4);
        assert!((params.wb_high[1] - 1.6163).abs() < 1e-4);
        assert!((params.wb_high[2] - 1.0).abs() < 1e-4);

        assert!((params.dmax - 3.2718).abs() < 1e-4);
        assert!((params.offset - 0.1060).abs() < 1e-4);
        assert!((params.paper_black - 0.1000).abs() < 1e-4);
        assert!((params.paper_grade - 4.5).abs() < 1e-4);
        assert!((params.paper_gloss - 0.75).abs() < 1e-4);
        assert!((params.print_exposure - 0.9245).abs() < 1e-4);

        // Re-serialize and assert exact 100% byte & hex equality
        let reserialized_hex = params.to_darktable_hex();
        assert_eq!(reserialized_hex, fixture_hex);
    }

    #[test]
    fn flip_parameter_orientations() {
        let cases = [
            (Orientation::Normal, "00000000"),
            (Orientation::Rotate90, "06000000"),
            (Orientation::Rotate180, "03000000"),
            (Orientation::Rotate270, "05000000"),
        ];

        for (orient, expected_hex) in cases {
            let flip = FlipParams::from_orientation(orient);
            assert_eq!(flip.to_hex(), expected_hex);
            let parsed = FlipParams::from_hex(expected_hex).unwrap();
            assert_eq!(parsed.to_orientation(), Some(orient));
        }
    }

    #[test]
    fn colorin_profile_encoding_and_decompression() {
        // Known compressed colorin parameter from real scans
        let known_gz = "gz12eJxjYGBgcLaKCS1OLSqOcfRy9XCPcSwocEksSYzxyU9OzIlJSSzKLklMykmNSc7PyS+KycyL8fP2CTYxMDAAkfF+epnJyQyjYJgAloF2wCgYcAAAncoXAg==";

        let bytes = DarktableParamCodec::decode_gz(known_gz).unwrap();
        assert_eq!(bytes.len(), 1044);

        let colorin = ColorinParams::from_bytes(&bytes).unwrap();
        assert_eq!(colorin.profile_type, 0); // DT_COLORSPACE_FILE
        assert!(colorin.filename.ends_with("NKLS4000LS40_N.icc"));
        assert_eq!(colorin.type_work, 4); // DT_COLORSPACE_LIN_REC2020

        // Round trip
        let re_encoded_bytes = colorin.to_bytes();
        assert_eq!(re_encoded_bytes.len(), 1044);
        assert_eq!(re_encoded_bytes, bytes);
    }

    #[test]
    fn parse_real_darktable_fixture_xmp() {
        let fixture_path = "scans-strip-02/frame-1.tif.xmp";
        if !Path::new(fixture_path).exists() {
            return;
        }
        let content = fs::read_to_string(fixture_path).unwrap();
        let xmp = DarktableXmp::parse(&content).unwrap();

        assert_eq!(xmp.derived_from, "frame-1.tif");
        assert_eq!(xmp.xmp_version, 5);
        assert_eq!(xmp.iop_order_version, 5);
        assert_eq!(xmp.history.len(), 10);

        let negadoctor = xmp.extract_negadoctor_params().unwrap();
        assert!((negadoctor.dmin[0] - 0.8965).abs() < 1e-4);
        assert!((negadoctor.dmax - 3.2718).abs() < 1e-4);

        let orientation = xmp.extract_orientation();
        assert_eq!(orientation, Orientation::Rotate180);

        let profile = xmp.extract_colorin_profile().unwrap();
        assert!(profile.ends_with("NKLS4000LS40_N.icc"));
    }

    #[test]
    fn full_xmp_generation_and_round_trip_semantic_compare() {
        let negadoctor = NegadoctorParams {
            dmin: [0.8965, 0.9093, 0.8816],
            dmax: 3.2718,
            offset: 0.1060,
            wb_high: [1.9506, 1.6163, 1.0],
            wb_low: [1.0, 1.0, 1.0],
            paper_black: 0.1000,
            paper_grade: 4.5,
            paper_gloss: 0.75,
            print_exposure: 0.9245,
        };

        // 1. Generate XMP for rotated frame
        let xmp = DarktableXmp::for_frame(
            "frame-test.tif",
            "NKLS4000LS40_N.icc",
            &negadoctor,
            Orientation::Rotate90,
        )
        .unwrap();

        assert_eq!(xmp.history_end, 7);
        assert_eq!(xmp.history.len(), 7);

        // 2. Render to XML string
        let xml_str = xmp.to_xmp_string();
        assert!(xml_str.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml_str.contains("xmpMM:DerivedFrom=\"frame-test.tif\""));
        assert!(xml_str.contains("darktable:history_end=\"7\""));

        // 3. Re-parse XML string
        let reparsed = DarktableXmp::parse(&xml_str).unwrap();
        assert_eq!(reparsed.derived_from, "frame-test.tif");
        assert_eq!(reparsed.history.len(), 7);

        // 4. Semantic compare
        let extracted_negadoctor = reparsed.extract_negadoctor_params().unwrap();
        assert_eq!(extracted_negadoctor, negadoctor);

        let extracted_orientation = reparsed.extract_orientation();
        assert_eq!(extracted_orientation, Orientation::Rotate90);

        let extracted_profile = reparsed.extract_colorin_profile().unwrap();
        assert_eq!(extracted_profile, "NKLS4000LS40_N.icc");
    }

    #[test]
    fn darktable_cli_compatibility_check() {
        let cli_path = "C:/Program Files/darktable/bin/darktable-cli.exe";
        if !Path::new(cli_path).exists() {
            return;
        }

        let out_dir = Path::new("target/dt_verify");
        let _ = fs::create_dir_all(out_dir);

        let negadoctor = NegadoctorParams {
            dmin: [0.8965, 0.9093, 0.8816],
            dmax: 3.2718,
            offset: 0.1060,
            wb_high: [1.9506, 1.6163, 1.0],
            wb_low: [1.0, 1.0, 1.0],
            paper_black: 0.1000,
            paper_grade: 4.5,
            paper_gloss: 0.75,
            print_exposure: 0.9245,
        };

        let xmp = DarktableXmp::for_frame(
            "frame-1.tif",
            "NKLS4000LS40_N.icc",
            &negadoctor,
            Orientation::Rotate180,
        )
        .unwrap();

        let xmp_path = out_dir.join("frame-1.tif.xmp");
        xmp.write_to_file(&xmp_path).unwrap();

        let src_tif = Path::new("scans-strip-02/frame-1.tif");
        if !src_tif.exists() {
            return;
        }

        let dst_tif = out_dir.join("frame-1.tif");
        if !dst_tif.exists() {
            let _ = fs::copy(src_tif, &dst_tif);
        }

        let render_out = out_dir.join("test-rendered.jpg");
        if render_out.exists() {
            let _ = fs::remove_file(&render_out);
        }

        let output = std::process::Command::new(cli_path)
            .args([
                "C:/coolscan/coolscan-studio/target/dt_verify/frame-1.tif",
                "C:/coolscan/coolscan-studio/target/dt_verify/frame-1.tif.xmp",
                "C:/coolscan/coolscan-studio/target/dt_verify/test-rendered.jpg",
                "--width",
                "400",
                "--core",
                "--configdir",
                "C:/Users/AJEHG/AppData/Local/Temp/dt_verify",
                "--library",
                ":memory:",
            ])
            .output();

        if let Ok(out) = output {
            assert!(
                out.status.success(),
                "darktable-cli failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(
                render_out.exists(),
                "Rendered output JPG was not found at {:?}",
                render_out
            );
        }
    }
}
