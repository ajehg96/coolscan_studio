//! Phase 6 & Phase 7 Acceptance Suite:
//! - Phase 6: Darktable 5.6.0 Parity Fixture (Stages A, B, C, D) on real Nikon LS-40 Kodak Pro Image 100 data.
//! - Phase 7: Export Hardening (XML-safe XMP writer, ICC profile verification/provisioning, Master vs Working TIFF policy).
#![allow(clippy::needless_range_loop)]

use std::path::Path;

use coolscan_studio::crop::CropDecision;
use coolscan_studio::darktable::xmp::{
    ColorinParams, DarktableNegadoctorExt, DarktableParamCodec, DarktableXmp, FlipParams,
};
use coolscan_studio::diagnostics::{
    DEFAULT_LS40_ICC_FILENAME, DEFAULT_LS40_ICC_FNV1A64, DarktableIccStatus,
    ensure_darktable_icc_for_export, probe_darktable_icc_status_in_dir,
};
use coolscan_studio::processing::analysis::{
    ImageSampleStats, SampleRect, WorkingImage, analyse_pre_white_balance,
    finish_after_white_balance, sample_highlight_wb,
};
use coolscan_studio::processing::color::{
    ColorTransform, DEFAULT_LS40_ICC_BYTES, ScannerColorPipeline,
};
use coolscan_studio::processing::negadoctor::NegadoctorParams;
use coolscan_studio::processing::orientation::Orientation;
use coolscan_studio::processing::roll::{PreparedFrame, RollProfile};
use coolscan_studio::scanner::types::{FrameArtifact, ScanMetadata};
use coolscan_studio::tiff::read_tiff;
use coolscan_studio::ui::review::ReviewSession;
use serde::{Deserialize, Serialize};

const FIXTURE_TIFF_PATH: &str = "tests/fixtures/pro_image_100_frame4_crop.tif";
const FIXTURE_JSON_PATH: &str = "tests/fixtures/pro_image_100_parity_fixture.json";
const FIXTURE_FRAME1_XMP_PATH: &str = "tests/fixtures/pro_image_100_frame1.tif.xmp";
const FIXTURE_FRAME4_XMP_PATH: &str = "tests/fixtures/pro_image_100_frame4.tif.xmp";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ParityPixelFixture {
    name: String,
    source_coord: [usize; 2],
    scanner_u16: [u16; 3],
    linear_rec2020: [f32; 3],
    rendered_linear_rgb: [f32; 3],
    rendered_srgb_u8: [u8; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DarktableReferenceValues {
    dmax: f32,
    scan_bias: f32,
    wb_high: [f32; 3],
    wb_low: [f32; 3],
    paper_black: f32,
    paper_grade: f32,
    paper_gloss: f32,
    print_exposure_manual: f32,
    print_exposure_auto: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ParityTolerances {
    icc_linear_rec2020: f32,
    picker_stats: f32,
    dmax: f32,
    scan_bias: f32,
    wb_high: f32,
    paper_black: f32,
    print_exposure: f32,
    rendered_linear_rgb: f32,
    rendered_srgb_u8: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DarktableParityFixture {
    darktable_version: String,
    scanner_model: String,
    film_stock: String,
    frame_number: usize,
    source_dimensions: [usize; 2],
    crop_bounds_inset_rect: SampleRect,
    wb_picker_rect_in_source: SampleRect,
    wb_picker_rect_in_fixture: SampleRect,
    fixture_dimensions: [usize; 2],
    dmin: [f32; 3],
    full_fixture_stats: ImageSampleStats,
    wb_patch_stats: ImageSampleStats,
    darktable_reference: DarktableReferenceValues,
    tolerances: ParityTolerances,
    reference_pixels: Vec<ParityPixelFixture>,
}

fn load_parity_fixture() -> &'static DarktableParityFixture {
    static FIXTURE: std::sync::OnceLock<DarktableParityFixture> = std::sync::OnceLock::new();
    FIXTURE.get_or_init(|| {
        assert!(
            Path::new(FIXTURE_TIFF_PATH).exists(),
            "Missing committed parity TIFF fixture at {FIXTURE_TIFF_PATH}"
        );
        let content = std::fs::read_to_string(FIXTURE_JSON_PATH)
            .expect("Missing committed parity JSON fixture");
        serde_json::from_str(&content).expect("Valid parity JSON fixture")
    })
}

#[test]
fn stage_a_icc_input_transform_parity() {
    let fixture = load_parity_fixture();
    let pipeline = ScannerColorPipeline::default_ls40().unwrap();
    let tol = fixture.tolerances.icc_linear_rec2020;

    for px in &fixture.reference_pixels {
        let actual = pipeline.transform_u16_rgb(px.scanner_u16);
        for c in 0..3 {
            let diff = (actual[c] - px.linear_rec2020[c]).abs();
            assert!(
                diff <= tol,
                "Stage A ICC transform mismatch on pixel '{}' ch {}: actual={} vs expected={} (diff={} > tol={})",
                px.name,
                c,
                actual[c],
                px.linear_rec2020[c],
                diff,
                tol
            );
        }
    }
}

#[test]
fn stage_b_picker_region_statistics_parity() {
    let fixture = load_parity_fixture();
    let parsed = read_tiff(Path::new(FIXTURE_TIFF_PATH)).unwrap();
    assert_eq!(
        [parsed.pass.cols, parsed.pass.rows],
        fixture.fixture_dimensions
    );

    let pipeline = ScannerColorPipeline::default_ls40().unwrap();
    let working =
        WorkingImage::from_scanner_samples(&parsed.samples, &parsed.pass, &pipeline).unwrap();
    let tol = fixture.tolerances.picker_stats;

    // 1. Full fixture extrema stats
    let full_stats = working.sample_region(None);
    assert_eq!(full_stats.count, fixture.full_fixture_stats.count);
    for c in 0..3 {
        assert!((full_stats.min[c] - fixture.full_fixture_stats.min[c]).abs() <= tol);
        assert!((full_stats.max[c] - fixture.full_fixture_stats.max[c]).abs() <= tol);
        assert!((full_stats.mean[c] - fixture.full_fixture_stats.mean[c]).abs() <= tol);
    }

    // 2. 32x32 Highlight-WB picker region stats
    let wb_stats = working.sample_region(Some(fixture.wb_picker_rect_in_fixture));
    assert_eq!(wb_stats.count, 32 * 32);
    assert_eq!(wb_stats.count, fixture.wb_patch_stats.count);
    for c in 0..3 {
        assert!((wb_stats.min[c] - fixture.wb_patch_stats.min[c]).abs() <= tol);
        assert!((wb_stats.max[c] - fixture.wb_patch_stats.max[c]).abs() <= tol);
        assert!((wb_stats.mean[c] - fixture.wb_patch_stats.mean[c]).abs() <= tol);
    }
}

#[test]
fn stage_c_negadoctor_parameter_and_xmp_structural_parity() {
    let fixture = load_parity_fixture();
    let parsed = read_tiff(Path::new(FIXTURE_TIFF_PATH)).unwrap();
    let pipeline = ScannerColorPipeline::default_ls40().unwrap();
    let working =
        WorkingImage::from_scanner_samples(&parsed.samples, &parsed.pass, &pipeline).unwrap();
    let roll = RollProfile::pro_image_100();

    // 1. Stage 1 & Stage 2 numerical parity against Darktable 5.6.0
    let tech = analyse_pre_white_balance(&working, &roll).unwrap();
    let dt = &fixture.darktable_reference;
    let tol = &fixture.tolerances;

    assert!(
        (tech.dmax - dt.dmax).abs() <= tol.dmax,
        "D-max parity failed: actual={} vs Darktable 5.6.0={}",
        tech.dmax,
        dt.dmax
    );
    assert!(
        (tech.scan_bias - dt.scan_bias).abs() <= tol.scan_bias,
        "Scan bias parity failed: actual={} vs Darktable 5.6.0={}",
        tech.scan_bias,
        dt.scan_bias
    );

    let mut params = NegadoctorParams::from_dmin(fixture.dmin);
    params.dmax = tech.dmax;
    params.offset = tech.scan_bias;
    params.wb_high =
        sample_highlight_wb(&working, &params, Some(fixture.wb_picker_rect_in_fixture));

    for c in 0..3 {
        assert!(
            (params.wb_high[c] - dt.wb_high[c]).abs() <= tol.wb_high,
            "Chained Highlight WB ch {} parity failed: actual={} vs Darktable 5.6.0={}",
            c,
            params.wb_high[c],
            dt.wb_high[c]
        );
    }

    // Also verify isolated Stage 2 WB formula parity when given Darktable's exact Stage 1 dmax & scan_bias
    let mut isolated_wb_params = NegadoctorParams::from_dmin(fixture.dmin);
    isolated_wb_params.dmax = dt.dmax;
    isolated_wb_params.offset = dt.scan_bias;
    let isolated_wb_high = sample_highlight_wb(
        &working,
        &isolated_wb_params,
        Some(fixture.wb_picker_rect_in_fixture),
    );
    for c in 0..3 {
        assert!(
            (isolated_wb_high[c] - dt.wb_high[c]).abs() <= 2.5e-4,
            "Isolated Highlight WB ch {} parity failed: actual={} vs Darktable 5.6.0={}",
            c,
            isolated_wb_high[c],
            dt.wb_high[c]
        );
    }

    finish_after_white_balance(&working, &mut params);
    assert!(
        (params.paper_black - dt.paper_black).abs() <= tol.paper_black,
        "Paper black parity failed: actual={} vs Darktable 5.6.0={}",
        params.paper_black,
        dt.paper_black
    );
    assert!(
        (params.print_exposure - dt.print_exposure_auto).abs() <= tol.print_exposure,
        "Print exposure parity failed: actual={} vs reference={}",
        params.print_exposure,
        dt.print_exposure_auto
    );

    // 2. Exact / structural parity against committed Darktable 5.6.0 XMP sidecars
    for (xmp_path, expected_derived) in [
        (FIXTURE_FRAME1_XMP_PATH, "frame-1.tif"),
        (FIXTURE_FRAME4_XMP_PATH, "frame-4.tif"),
    ] {
        let content = std::fs::read_to_string(xmp_path).unwrap();
        let xmp = DarktableXmp::parse(&content).unwrap();
        assert_eq!(xmp.derived_from, expected_derived);
        assert_eq!(xmp.xmp_version, 5);
        assert_eq!(xmp.iop_order_version, 5);
        assert_eq!(xmp.extract_orientation(), Orientation::Rotate180);

        // Verify exact binary struct sizes on history items
        for item in &xmp.history {
            match item.operation.as_str() {
                "colorin" => {
                    let bytes = DarktableParamCodec::decode_param(&item.params).unwrap();
                    if item.num >= 4 {
                        assert_eq!(bytes.len(), 1044);
                        let colorin = ColorinParams::from_bytes(&bytes).unwrap();
                        assert!(colorin.filename.ends_with(DEFAULT_LS40_ICC_FILENAME));
                    }
                }
                "negadoctor" => {
                    assert_eq!(item.params.len(), 152);
                    let bytes = DarktableParamCodec::decode_param(&item.params).unwrap();
                    assert_eq!(bytes.len(), 76);
                    let neg = NegadoctorParams::from_darktable_bytes(&bytes).unwrap();
                    assert_eq!(neg.to_darktable_hex(), item.params);
                }
                "flip" if item.params != "ffffffff" => {
                    let bytes = DarktableParamCodec::decode_param(&item.params).unwrap();
                    assert_eq!(bytes.len(), 4);
                    let flip = FlipParams::from_hex(&item.params).unwrap();
                    assert_eq!(flip.to_orientation(), Some(Orientation::Rotate180));
                }
                _ => {}
            }
        }
    }
}

#[test]
fn stage_d_render_output_pixel_parity() {
    let fixture = load_parity_fixture();
    let pipeline = ScannerColorPipeline::default_ls40().unwrap();

    let xmp4_str = std::fs::read_to_string(FIXTURE_FRAME4_XMP_PATH).unwrap();
    let xmp4 = DarktableXmp::parse(&xmp4_str).unwrap();
    let dt_params = xmp4.extract_negadoctor_params().unwrap();
    let prepared = dt_params.prepare();

    for px in &fixture.reference_pixels {
        let work_rgb = pipeline.transform_u16_rgb(px.scanner_u16);
        let linear_out = prepared.invert_pixel(work_rgb);
        let srgb_out = pipeline.working_to_display_srgb(linear_out);

        for c in 0..3 {
            let flt_diff = (linear_out[c] - px.rendered_linear_rgb[c]).abs();
            assert!(
                flt_diff <= fixture.tolerances.rendered_linear_rgb,
                "Stage D linear render mismatch on '{}' ch {}: actual={} vs expected={}",
                px.name,
                c,
                linear_out[c],
                px.rendered_linear_rgb[c]
            );

            let u8_diff = srgb_out[c].abs_diff(px.rendered_srgb_u8[c]);
            assert!(
                u8_diff <= fixture.tolerances.rendered_srgb_u8,
                "Stage D sRGB u8 mismatch on '{}' ch {}: actual={} vs expected={}",
                px.name,
                c,
                srgb_out[c],
                px.rendered_srgb_u8[c]
            );
        }
    }
}

#[test]
fn phase7_clean_machine_export_bundle_and_icc_verification() {
    let _ = load_parity_fixture();
    let parsed = read_tiff(Path::new(FIXTURE_TIFF_PATH)).unwrap();
    let pipeline = ScannerColorPipeline::default_ls40().unwrap();
    let roll = RollProfile::pro_image_100();

    // Build a FrameArtifact with an accepted sub-crop (rows 2..30, cols 0..32)
    let artifact = FrameArtifact {
        frame_number: 4,
        total_frames: 6,
        dpi: 2900,
        raw_rect: nkscan::protocol::data::Rect {
            left: 0,
            right: parsed.pass.cols as u32,
            top: 0,
            bottom: parsed.pass.rows as u32,
        },
        samples: parsed.samples,
        pass: parsed.pass,
        crop: Some(CropDecision {
            leading: coolscan_studio::crop::EdgeConfidence::Confident { column: 0, dots: 0 },
            trailing: coolscan_studio::crop::EdgeConfidence::Confident {
                column: 32,
                dots: 32,
            },
            columns: (0, 32),
            rows: (2, 30),
            travel_dots: (0, 32),
            accepted: true,
            is_blank: false,
        }),
        scan_metadata: ScanMetadata {
            scanner_model: Some("Nikon LS-40 ED".into()),
            focus_position: None,
            exposures: None,
            hardware_samples: 1,
            software_passes: 1,
            infrared_cleaned_pixels: None,
        },
        roll: Some(roll.clone()),
    };

    let prepared = PreparedFrame::from_artifact(artifact, roll, &pipeline).unwrap();
    let session = ReviewSession::from_prepared_frames(vec![prepared], pipeline).unwrap();

    let temp_root = std::env::temp_dir().join(format!(
        "coolscan_phase7_bundle_test_{}",
        std::process::id()
    ));
    let clean_color_in = temp_root.join("darktable_color_in");
    let export_dir = temp_root.join("exported_scans");
    let _ = std::fs::remove_dir_all(&temp_root);

    // 1. Verify ICC provisioning into a clean machine's color/in directory
    assert!(matches!(
        probe_darktable_icc_status_in_dir(&clean_color_in),
        DarktableIccStatus::Missing { .. }
    ));
    let provisioned_name =
        ensure_darktable_icc_for_export(&export_dir, Some(&clean_color_in)).unwrap();
    assert_eq!(provisioned_name, DEFAULT_LS40_ICC_FILENAME);
    assert_eq!(
        probe_darktable_icc_status_in_dir(&clean_color_in),
        DarktableIccStatus::Verified {
            path: clean_color_in.join(DEFAULT_LS40_ICC_FILENAME),
            size_bytes: DEFAULT_LS40_ICC_BYTES.len(),
            fnv1a64: DEFAULT_LS40_ICC_FNV1A64,
        }
    );

    // 2. Export complete bundle: (frame-4-master.tif, frame-4.tif, frame-4.tif.xmp)
    let (master_path, working_path, xmp_path) =
        session.save_current_complete_bundle(&export_dir).unwrap();
    assert_eq!(master_path.file_name().unwrap(), "frame-4-master.tif");
    assert_eq!(working_path.file_name().unwrap(), "frame-4.tif");
    assert_eq!(xmp_path.file_name().unwrap(), "frame-4.tif.xmp");

    // 3. Master TIFF is uncropped (36x32) and embeds NKLS4000LS40_N.icc
    let master_tiff = read_tiff(&master_path).unwrap();
    assert_eq!([master_tiff.pass.cols, master_tiff.pass.rows], [36, 32]);
    assert_eq!(
        master_tiff.icc_profile.as_deref(),
        Some(DEFAULT_LS40_ICC_BYTES)
    );

    // 4. Working TIFF is cropped (32x28)
    let working_tiff = read_tiff(&working_path).unwrap();
    assert_eq!([working_tiff.pass.cols, working_tiff.pass.rows], [32, 28]);
    assert!(working_tiff.icc_profile.is_none());

    // 5. Companion XMP sidecar is valid XML and references frame-4.tif and NKLS4000LS40_N.icc
    let xmp_parsed = DarktableXmp::parse(&std::fs::read_to_string(&xmp_path).unwrap()).unwrap();
    assert_eq!(xmp_parsed.derived_from, "frame-4.tif");
    assert_eq!(
        xmp_parsed.extract_colorin_profile().as_deref(),
        Some(DEFAULT_LS40_ICC_FILENAME)
    );

    let _ = std::fs::remove_dir_all(&temp_root);
}
