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

    let mut frame = ReviewFrameState::new(1, working_image, &roll, technical).unwrap();

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

    let mut frame = ReviewFrameState::new(1, working_image, &roll, technical).unwrap();

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

#[test]
fn phase13_dependency_model_recalculation_chain() {
    use coolscan_studio::ui::review::WhiteBalanceMode;
    let roll = RollProfile::pro_image_100();

    // Create an image with known stats so recalculation is deterministic
    let master_w = 100;
    let master_h = 100;
    let mut pixels = Vec::with_capacity(master_w * master_h);
    for _ in 0..(master_w * master_h) {
        pixels.push([0.5, 0.5, 0.5]); // mean = 0.5, min = 0.5, max = 0.5
    }
    let working_image = WorkingImage::new(master_w, master_h, pixels);

    let technical = TechnicalAnalysis {
        dmax: 3.27,
        scan_bias: 0.10,
    };

    let mut frame = ReviewFrameState::new(1, working_image.clone(), &roll, technical).unwrap();

    // Initial auto values
    let initial_offset = frame.params.offset;
    let initial_paper_black = frame.params.paper_black;
    let initial_print_exposure = frame.params.print_exposure;

    // 1. D-max change -> recompute auto scan bias, paper black, print exposure
    frame.modes.dmax_auto = false;
    frame.params.dmax = 2.0;
    frame.recalculate_downstream();

    assert_ne!(
        frame.params.offset, initial_offset,
        "Offset should recalculate when D-max changes"
    );
    assert_ne!(
        frame.params.paper_black, initial_paper_black,
        "Paper black should recalculate"
    );
    assert_ne!(
        frame.params.print_exposure, initial_print_exposure,
        "Print exposure should recalculate"
    );

    // 2. Manual downstream values remain unchanged
    frame.modes.offset_auto = false;
    frame.params.offset = 0.25;
    let saved_offset = frame.params.offset;

    frame.modes.paper_black_auto = false;
    frame.params.paper_black = 0.55;
    let saved_paper_black = frame.params.paper_black;

    // Now change D-max again. Auto downstreams would change, but these are manual
    frame.params.dmax = 2.5;
    frame.recalculate_downstream();

    assert_eq!(
        frame.params.offset, saved_offset,
        "Manual offset should not be overwritten"
    );
    assert_eq!(
        frame.params.paper_black, saved_paper_black,
        "Manual paper black should not be overwritten"
    );
    assert_ne!(
        frame.params.print_exposure, initial_print_exposure,
        "Auto print exposure should still change"
    );

    // 3. Reset-to-auto recalculates correctly
    frame.modes.offset_auto = true;
    frame.modes.paper_black_auto = true;
    frame.recalculate_downstream();

    assert_ne!(
        frame.params.offset, saved_offset,
        "Offset should snap back to auto"
    );
    assert_ne!(
        frame.params.paper_black, saved_paper_black,
        "Paper black should snap back to auto"
    );

    // 4. Sampled WB -> recompute paper black
    let pre_wb_paper_black = frame.params.paper_black;
    frame.modes.wb_high_mode = WhiteBalanceMode::SampledAuto(SampleRect::new(0, 0, 10, 10));
    // simulate picker setting a new wb_high value, though recalculate_downstream will re-run the sampler
    frame.recalculate_downstream();

    // sample_highlight_wb will calculate a new wb_high.
    assert_ne!(
        frame.params.wb_high,
        [1.0, 1.0, 1.0],
        "Sampled WB should not be neutral"
    );
    assert_ne!(
        frame.params.paper_black, pre_wb_paper_black,
        "Paper black should recalculate after WB change"
    );

    // 5. Reset WB to neutral
    frame.modes.wb_high_mode = WhiteBalanceMode::Neutral;
    frame.recalculate_downstream();
    assert_eq!(frame.params.wb_high, [1.0, 1.0, 1.0]);
}

#[test]
fn phase13_mock_mode_non_zero_real_image_stats() {
    use coolscan_studio::scanner::types::{FrameSelection, ScanEvent, ScanRequest};
    use coolscan_studio::scanner::worker::{ScanCommand, ScannerWorkerHandle};
    use coolscan_studio::ui::ReviewSession;

    let roll = RollProfile::pro_image_100();
    let handle = ScannerWorkerHandle::spawn_mock(1);
    handle.send(ScanCommand::CheckMedia);
    handle.send(ScanCommand::DiscoverStrip);

    let req = ScanRequest::new(FrameSelection::All, 2900, 1, false, true);
    handle.send(ScanCommand::StartScan(Box::new(req)));

    let mut prepared_frame = None;
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(5) {
        if let Some(msg) = handle.try_recv() {
            if let WorkerMessage::FrameReady(p) = msg {
                prepared_frame = Some(*p);
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let prepared_frame = prepared_frame.expect("Mock backend should yield a FramePrepared event");

    let pipeline = ScannerColorPipeline::default_ls40().unwrap();
    let mut session = ReviewSession::from_prepared_frames(vec![prepared_frame], pipeline).unwrap();
    let frame = session.current_frame_mut().unwrap();

    // The image stats MUST be populated from the actual working image, NOT zero.
    assert!(
        frame.image_stats.max[0] > 0.0,
        "Real image stats must not be zero"
    );
    assert!(
        frame.image_stats.max[1] > 0.0,
        "Real image stats must not be zero"
    );
    assert!(
        frame.image_stats.max[2] > 0.0,
        "Real image stats must not be zero"
    );

    let initial_dmax = frame.params.dmax;
    frame.modes.dmax_auto = true; // force recalculation
    frame.recalculate_downstream();

    assert!(
        (frame.params.dmax - initial_dmax).abs() < 2.0,
        "Dmax jumped wildly, likely due to zero stats"
    );
}
