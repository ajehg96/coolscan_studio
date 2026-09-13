use lcms2::{
    CIExyY, CIExyYTRIPLE, DisallowCache, Flags, GlobalContext, Intent, PixelFormat, Profile,
    ToneCurve, Transform,
};
use nkscan::{protocol::decode::Samples, scan::pass::Pass};
use std::path::Path;

/// Default embedded Nikon LS-4000 / LS-40 Negative input ICC profile.
pub const DEFAULT_LS40_ICC_BYTES: &[u8] = include_bytes!("../../profiles/NKLS4000LS40_N.icc");

/// Working colour space for image processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkingColorSpace {
    /// Linear Rec.2020 RGB (Darktable default working space for scene-referred pipeline).
    LinearRec2020,
    /// Linear sRGB (ITU-R BT.709 primaries with linear 1.0 gamma).
    LinearSrgb,
    /// Standard display sRGB (IEC 61966-2-1).
    StandardSrgb,
}

/// Errors occurring during color profile loading or transformation.
#[derive(Debug)]
pub enum ColorError {
    Lcms(String),
    Io(std::io::Error),
    InvalidChannelCount(usize),
    IncompletePlane { expected: usize, found: usize },
    UncalibratedRoll(String),
}

impl std::fmt::Display for ColorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ColorError::Lcms(msg) => write!(f, "LittleCMS color management error: {msg}"),
            ColorError::Io(e) => write!(f, "I/O error reading color profile: {e}"),
            ColorError::InvalidChannelCount(c) => {
                write!(
                    f,
                    "Invalid color channel count: expected 3 (RGB), found {c}"
                )
            }
            ColorError::IncompletePlane { expected, found } => {
                write!(
                    f,
                    "Incomplete color plane: expected at least {expected} samples, found {found}"
                )
            }
            ColorError::UncalibratedRoll(msg) => write!(f, "Uncalibrated roll: {msg}"),
        }
    }
}

impl std::error::Error for ColorError {}

/// Abstract color transformation trait.
pub trait ColorTransform: Send + Sync {
    /// Transforms an RGB triplet in [0.0, 1.0] normalized space into working space.
    fn transform_rgb(&self, rgb: [f32; 3]) -> [f32; 3];

    /// Transforms a 16-bit raw scanner RGB sample triplet in [0, 65535] into working space.
    fn transform_u16_rgb(&self, rgb: [u16; 3]) -> [f32; 3];

    /// Bulk transforms an interleaved slice of 16-bit RGB pixels into floating-point working space.
    fn transform_u16_slice(&self, src: &[[u16; 3]], dst: &mut [[f32; 3]]);

    /// Bulk transforms planar 16-bit scanner samples into interleaved working-space floating point pixels.
    fn transform_planar_samples(
        &self,
        samples: &Samples,
        pass: &Pass,
    ) -> Result<Vec<[f32; 3]>, ColorError>;
}

/// Creates a linear Rec.2020 RGB profile (D65 white point, ITU-R BT.2020 primaries, linear gamma 1.0).
pub fn make_linear_rec2020_profile() -> Result<Profile, ColorError> {
    let d65 = CIExyY {
        x: 0.3127,
        y: 0.3290,
        Y: 1.0,
    };
    let primaries = CIExyYTRIPLE {
        Red: CIExyY {
            x: 0.708,
            y: 0.292,
            Y: 1.0,
        },
        Green: CIExyY {
            x: 0.170,
            y: 0.797,
            Y: 1.0,
        },
        Blue: CIExyY {
            x: 0.131,
            y: 0.046,
            Y: 1.0,
        },
    };
    let linear_curve = ToneCurve::new(1.0);
    Profile::new_rgb(
        &d65,
        &primaries,
        &[&linear_curve, &linear_curve, &linear_curve],
    )
    .map_err(|_| ColorError::Lcms("Failed to create linear Rec.2020 profile".into()))
}

/// Creates a linear sRGB profile (D65 white point, Rec.709 primaries, linear gamma 1.0).
pub fn make_linear_srgb_profile() -> Result<Profile, ColorError> {
    let d65 = CIExyY {
        x: 0.3127,
        y: 0.3290,
        Y: 1.0,
    };
    let primaries = CIExyYTRIPLE {
        Red: CIExyY {
            x: 0.640,
            y: 0.330,
            Y: 1.0,
        },
        Green: CIExyY {
            x: 0.300,
            y: 0.600,
            Y: 1.0,
        },
        Blue: CIExyY {
            x: 0.150,
            y: 0.060,
            Y: 1.0,
        },
    };
    let linear_curve = ToneCurve::new(1.0);
    Profile::new_rgb(
        &d65,
        &primaries,
        &[&linear_curve, &linear_curve, &linear_curve],
    )
    .map_err(|_| ColorError::Lcms("Failed to create linear sRGB profile".into()))
}

/// Color transformation pipeline backed by LittleCMS 2.
pub struct ScannerColorPipeline {
    working_space: WorkingColorSpace,
    u16_to_working: Transform<[u16; 3], [f32; 3], GlobalContext, DisallowCache>,
    flt_to_working: Transform<[f32; 3], [f32; 3], GlobalContext, DisallowCache>,
    working_to_srgb: Transform<[f32; 3], [u8; 3], GlobalContext, DisallowCache>,
}

impl ScannerColorPipeline {
    /// Creates a pipeline using the bundled Nikon LS-4000 / LS-40 Negative ICC profile
    /// and linear Rec.2020 working space (matching Darktable).
    pub fn default_ls40() -> Result<Self, ColorError> {
        Self::from_icc_bytes(DEFAULT_LS40_ICC_BYTES, WorkingColorSpace::LinearRec2020)
    }

    /// Creates a pipeline from an ICC profile file path.
    pub fn from_icc_file(
        path: &Path,
        working_space: WorkingColorSpace,
    ) -> Result<Self, ColorError> {
        let bytes = std::fs::read(path).map_err(ColorError::Io)?;
        Self::from_icc_bytes(&bytes, working_space)
    }

    /// Creates a pipeline from in-memory ICC profile bytes and a specified working space.
    pub fn from_icc_bytes(
        icc_bytes: &[u8],
        working_space: WorkingColorSpace,
    ) -> Result<Self, ColorError> {
        let in_profile = Profile::new_icc(icc_bytes)
            .map_err(|_| ColorError::Lcms("Invalid input ICC profile".into()))?;

        let out_profile = match working_space {
            WorkingColorSpace::LinearRec2020 => make_linear_rec2020_profile()?,
            WorkingColorSpace::LinearSrgb => make_linear_srgb_profile()?,
            WorkingColorSpace::StandardSrgb => Profile::new_srgb(),
        };

        let srgb_profile = Profile::new_srgb();

        let u16_to_working = Transform::new_flags_context(
            GlobalContext::new(),
            &in_profile,
            PixelFormat::RGB_16,
            &out_profile,
            PixelFormat::RGB_FLT,
            Intent::Perceptual,
            Flags::NO_CACHE,
        )
        .map_err(|_| {
            ColorError::Lcms("Failed to build 16-bit to working space transform".into())
        })?;

        let flt_to_working = Transform::new_flags_context(
            GlobalContext::new(),
            &in_profile,
            PixelFormat::RGB_FLT,
            &out_profile,
            PixelFormat::RGB_FLT,
            Intent::Perceptual,
            Flags::NO_CACHE,
        )
        .map_err(|_| ColorError::Lcms("Failed to build float to working space transform".into()))?;

        let working_to_srgb = Transform::new_flags_context(
            GlobalContext::new(),
            &out_profile,
            PixelFormat::RGB_FLT,
            &srgb_profile,
            PixelFormat::RGB_8,
            Intent::Perceptual,
            Flags::NO_CACHE,
        )
        .map_err(|_| ColorError::Lcms("Failed to build working space to sRGB transform".into()))?;

        Ok(Self {
            working_space,
            u16_to_working,
            flt_to_working,
            working_to_srgb,
        })
    }

    /// The target working color space.
    pub fn working_space(&self) -> WorkingColorSpace {
        self.working_space
    }

    /// Transforms a working-space float RGB pixel into 8-bit display sRGB.
    pub fn working_to_display_srgb(&self, rgb: [f32; 3]) -> [u8; 3] {
        let mut out = [[0u8; 3]];
        self.working_to_srgb.transform_pixels(&[rgb], &mut out);
        out[0]
    }

    /// Bulk transforms working-space float RGB pixels into 8-bit display sRGB.
    pub fn bulk_working_to_display_srgb(&self, src: &[[f32; 3]], dst: &mut [[u8; 3]]) {
        self.working_to_srgb.transform_pixels(src, dst);
    }
}

impl ColorTransform for ScannerColorPipeline {
    fn transform_rgb(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut out = [[0.0f32; 3]];
        self.flt_to_working.transform_pixels(&[rgb], &mut out);
        out[0]
    }

    fn transform_u16_rgb(&self, rgb: [u16; 3]) -> [f32; 3] {
        let mut out = [[0.0f32; 3]];
        self.u16_to_working.transform_pixels(&[rgb], &mut out);
        out[0]
    }

    fn transform_u16_slice(&self, src: &[[u16; 3]], dst: &mut [[f32; 3]]) {
        self.u16_to_working.transform_pixels(src, dst);
    }

    fn transform_planar_samples(
        &self,
        samples: &Samples,
        pass: &Pass,
    ) -> Result<Vec<[f32; 3]>, ColorError> {
        if samples.colors.len() != 3 {
            return Err(ColorError::InvalidChannelCount(samples.colors.len()));
        }

        let pixels = pass
            .rows
            .checked_mul(pass.cols)
            .ok_or_else(|| ColorError::Lcms("Pass dimensions overflow".into()))?;

        for plane in &samples.colors {
            if plane.len() < pixels {
                return Err(ColorError::IncompletePlane {
                    expected: pixels,
                    found: plane.len(),
                });
            }
        }

        let red = &samples.colors[0];
        let green = &samples.colors[1];
        let blue = &samples.colors[2];

        let mut output = vec![[0.0f32; 3]; pixels];
        const CHUNK_SIZE: usize = 16384;
        let mut chunk_in = vec![[0u16; 3]; CHUNK_SIZE];

        for chunk_idx in 0..pixels.div_ceil(CHUNK_SIZE) {
            let start = chunk_idx * CHUNK_SIZE;
            let end = (start + CHUNK_SIZE).min(pixels);
            let len = end - start;

            for (i, dst) in chunk_in[..len].iter_mut().enumerate() {
                let p = start + i;
                *dst = [red[p], green[p], blue[p]];
            }

            self.u16_to_working
                .transform_pixels(&chunk_in[..len], &mut output[start..end]);
        }

        Ok(output)
    }
}

/// Simple identity transform (scaling u16 by 1/65535.0, keeping floats unchanged).
pub struct IdentityColorTransform;

impl ColorTransform for IdentityColorTransform {
    fn transform_rgb(&self, rgb: [f32; 3]) -> [f32; 3] {
        rgb
    }

    fn transform_u16_rgb(&self, rgb: [u16; 3]) -> [f32; 3] {
        [
            rgb[0] as f32 / 65535.0,
            rgb[1] as f32 / 65535.0,
            rgb[2] as f32 / 65535.0,
        ]
    }

    fn transform_u16_slice(&self, src: &[[u16; 3]], dst: &mut [[f32; 3]]) {
        for (s, d) in src.iter().zip(dst.iter_mut()) {
            *d = self.transform_u16_rgb(*s);
        }
    }

    fn transform_planar_samples(
        &self,
        samples: &Samples,
        pass: &Pass,
    ) -> Result<Vec<[f32; 3]>, ColorError> {
        if samples.colors.len() != 3 {
            return Err(ColorError::InvalidChannelCount(samples.colors.len()));
        }

        let pixels = pass
            .rows
            .checked_mul(pass.cols)
            .ok_or_else(|| ColorError::Lcms("Pass dimensions overflow".into()))?;

        for plane in &samples.colors {
            if plane.len() < pixels {
                return Err(ColorError::IncompletePlane {
                    expected: pixels,
                    found: plane.len(),
                });
            }
        }

        let red = &samples.colors[0];
        let green = &samples.colors[1];
        let blue = &samples.colors[2];

        let mut output = Vec::with_capacity(pixels);
        for p in 0..pixels {
            output.push([
                red[p] as f32 / 65535.0,
                green[p] as f32 / 65535.0,
                blue[p] as f32 / 65535.0,
            ]);
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ls40_pipeline_creation() {
        let pipeline = ScannerColorPipeline::default_ls40().unwrap();
        assert_eq!(pipeline.working_space(), WorkingColorSpace::LinearRec2020);
    }

    #[test]
    fn transform_known_ls40_pixel() {
        let pipeline = ScannerColorPipeline::default_ls40().unwrap();
        // 16-bit D-min approx: R=58752, G=59590, B=57775
        let transformed = pipeline.transform_u16_rgb([58752, 59590, 57775]);
        // Parity check against LittleCMS reference calculation:
        // Expected approx: R ~ 0.8999, G ~ 0.9077, B ~ 0.8851
        assert!(
            (transformed[0] - 0.899875).abs() < 1e-3,
            "R: {}",
            transformed[0]
        );
        assert!(
            (transformed[1] - 0.907703).abs() < 1e-3,
            "G: {}",
            transformed[1]
        );
        assert!(
            (transformed[2] - 0.885087).abs() < 1e-3,
            "B: {}",
            transformed[2]
        );
    }

    #[test]
    fn bulk_planar_sample_transformation() {
        let pipeline = ScannerColorPipeline::default_ls40().unwrap();
        let pass = Pass {
            layout: nkscan::protocol::image::Layout::single_line(2, 2, vec![1]),
            cooperation: Vec::new(),
            complete: true,
            blocks: 1,
            rows: 2,
            cols: 2,
        };
        let samples = Samples {
            colors: vec![
                vec![58752, 58752, 58752, 58752],
                vec![59590, 59590, 59590, 59590],
                vec![57775, 57775, 57775, 57775],
            ],
            ir: None,
        };

        let result = pipeline.transform_planar_samples(&samples, &pass).unwrap();
        assert_eq!(result.len(), 4);
        for px in result {
            assert!((px[0] - 0.899875).abs() < 1e-3);
            assert!((px[1] - 0.907703).abs() < 1e-3);
            assert!((px[2] - 0.885087).abs() < 1e-3);
        }
    }

    #[test]
    fn identity_transform_behavior() {
        let identity = IdentityColorTransform;
        let px = identity.transform_u16_rgb([65535, 32768, 0]);
        assert!((px[0] - 1.0).abs() < 1e-5);
        assert!((px[1] - (32768.0 / 65535.0)).abs() < 1e-5);
        assert!((px[2] - 0.0).abs() < 1e-5);
    }
}
