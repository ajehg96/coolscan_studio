use coolscan_studio::{
    processing::{
        Orientation, RollProfile, ScannerColorPipeline,
        analysis::{SampleRect, TechnicalAnalysis, WorkingImage},
    },
    ui::{ReviewFrameState, SelectionStatus},
};
use std::time::Instant;

#[test]
fn phase13_downscaled_preview_and_instant_rotation_caching() {
    let pipeline = ScannerColorPipeline::default_ls40().expect("Failed to create color pipeline");
    let roll = RollProfile::pro_image_100();

    // 1. Create a large simulated master image (e.g. 2400 x 1600 = 3.84 million pixels)
    let master_w = 2400;
    let master_h = 1600;
    let mut pixels = Vec::with_capacity(master_w * master_h);
    for y in 0..master_h {
        for x in 0..master_w {
            let fx = x as f32 / master_w as f32;
            let fy = y as f32 / master_h as f32;
            let factor = (fx + fy) * 0.5;
            let val = 0.005 + factor * (0.40 - 0.005);
            pixels.push([val, val * 1.01, val * 0.99]);
        }
    }
    let working_image = WorkingImage::new(master_w, master_h, pixels);

    let technical = TechnicalAnalysis {
        dmax: 3.27,
        scan_bias: 0.10,
    };

    let mut frame = ReviewFrameState::new(1, working_image, &roll, technical);

    // Verify preview image was downscaled appropriately (max dimension <= 1440)
    assert!(frame.preview_image.width <= 1440);
    assert!(frame.preview_image.height <= 1440);
    assert_eq!(frame.preview_image.width, 1200);
    assert_eq!(frame.preview_image.height, 800);
    assert_eq!(frame.working_image.width, 2400);
    assert_eq!(frame.working_image.height, 1600);

    // 2. Measure first render time (Negadoctor + LittleCMS + transpose)
    let t0 = Instant::now();
    let (pw1, ph1, _) = frame.get_or_render_preview(&pipeline);
    let initial_render_time = t0.elapsed();
    assert_eq!(pw1, 1200);
    assert_eq!(ph1, 800);

    // 3. Rotate orientation and measure second render time (Transpose only, cached sRGB)
    frame.set_orientation(Orientation::Rotate90);
    let t1 = Instant::now();
    let (pw2, ph2, _) = frame.get_or_render_preview(&pipeline);
    let rotation_render_time = t1.elapsed();

    assert_eq!(pw2, 800);
    assert_eq!(ph2, 1200);
    println!(
        "Initial preview render: {:?}, Orientation rotation render: {:?}",
        initial_render_time, rotation_render_time
    );

    // Rotation transposition from cached sRGB must be dramatically faster than full color conversion
    assert!(
        rotation_render_time < initial_render_time / 2 || rotation_render_time.as_millis() < 10,
        "Expected rotation to use cached sRGB, but it took {:?}",
        rotation_render_time
    );
}

#[test]
fn phase13_highlight_wb_selection_preserves_master_precision() {
    let pipeline = ScannerColorPipeline::default_ls40().expect("Failed to create color pipeline");
    let roll = RollProfile::pro_image_100();

    let master_w = 2000;
    let master_h = 1000;
    let mut pixels = Vec::with_capacity(master_w * master_h);
    for y in 0..master_h {
        for x in 0..master_w {
            // Highlights in top-left: transmission ~ 0.005
            let fx = x as f32 / master_w as f32;
            let fy = y as f32 / master_h as f32;
            let factor = (fx + fy) * 0.5;
            let val = 0.005 + factor * (0.40 - 0.005);
            pixels.push([val, val * 1.02, val * 0.98]);
        }
    }
    let working_image = WorkingImage::new(master_w, master_h, pixels);

    let technical = TechnicalAnalysis {
        dmax: 3.27,
        scan_bias: 0.10,
    };

    let mut frame = ReviewFrameState::new(1, working_image, &roll, technical);

    // Ensure preview was created
    assert_eq!(frame.preview_image.width, 1000);
    assert_eq!(frame.preview_image.height, 500);

    // Select a highlight rectangle in preview coordinates (e.g. x=10, y=10, w=20, h=20)
    let preview_selection = SampleRect::new(10, 10, 20, 20);
    frame.apply_highlight_wb_selection(preview_selection);

    assert_eq!(frame.selection_status, SelectionStatus::Valid);
    assert!(frame.highlight_wb_rect.is_some());

    // Highlight WB channel minimum must be normalized to 1.0
    let min_wb = frame.params.wb_high[0]
        .min(frame.params.wb_high[1])
        .min(frame.params.wb_high[2]);
    assert!((min_wb - 1.0).abs() < 1e-4);

    // Master calculation produced valid, finite paper black and exposure
    assert!(frame.params.paper_black.is_finite());
    assert!(frame.params.print_exposure > 0.0);

    // After applying WB, get_or_render_preview successfully produces valid RGBA display pixels
    let (pw, ph, raw) = frame.get_or_render_preview(&pipeline);
    assert_eq!(pw, 1000);
    assert_eq!(ph, 500);
    assert_eq!(raw.len(), pw * ph * 4);
}
