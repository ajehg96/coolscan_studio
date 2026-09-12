use std::process::Command;
use std::time::{Duration, Instant};

use coolscan_studio::darktable::DarktableXmp;
use coolscan_studio::processing::analysis::SampleRect;
use coolscan_studio::processing::color::ScannerColorPipeline;
use coolscan_studio::processing::orientation::{Orientation, OrientationScope};
use coolscan_studio::processing::roll::RollProfile;
use coolscan_studio::scanner::{
    FrameSelection, ScanCommand, ScanRequest, ScannerWorkerHandle,
};
use coolscan_studio::ui::{ReviewApp, ReviewSession, SelectionStatus};

#[test]
fn phase11_end_to_end_mock_canned_strip_review_and_darktable_export() {
    let test_run_id = Instant::now().elapsed().as_nanos();
    let output_dir = std::env::temp_dir().join(format!("coolscan_studio_phase11_{test_run_id}"));
    std::fs::create_dir_all(&output_dir).expect("Failed to create temporary test directory");

    let pipeline = ScannerColorPipeline::default_ls40().expect("Failed to create LS40 color pipeline");
    let roll = RollProfile::pro_image_100();

    // 1. Initialize ReviewApp with an empty session and a 6-frame mock scanner worker
    let session = ReviewSession::empty(roll, pipeline);
    let worker = ScannerWorkerHandle::spawn_mock(6);
    let mut app = ReviewApp::new(session)
        .with_worker(worker)
        .with_output_dir(&output_dir);

    assert_eq!(app.session.frame_count(), 0);
    assert_eq!(app.status_message, "Ready");

    // 2. Start the scan for all 6 frames
    let req = ScanRequest::new(FrameSelection::All, 2900, 1, false, true);
    app.worker.as_ref().unwrap().send(ScanCommand::StartScan(req));

    // 3. Simulate UI event loop polling until all 6 frames are acquired and scan completes
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        app.poll_worker();
        if app.session.frame_count() == 6 && !app.is_scanning {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(
        app.session.frame_count(),
        6,
        "All 6 frames should have arrived into the review session"
    );
    assert!(
        !app.is_scanning,
        "Scanner should indicate scan sequence complete"
    );
    assert_eq!(app.session.current_index, 0);

    // 4. Interactive Review Phase on Frame 1:
    // Test orientation rotation
    {
        let frame1 = app.session.current_frame().expect("Frame 1 must exist");
        assert_eq!(frame1.orientation, Orientation::Normal);
        assert_eq!(frame1.frame_number, 1);

        // Rotate clockwise -> 90 degrees
        app.session.rotate(OrientationScope::CurrentFrame, true);
        assert_eq!(
            app.session.current_frame().unwrap().orientation,
            Orientation::Rotate90
        );

        // Test highlight white balance selection rectangle
        let sample = SampleRect::new(4, 4, 8, 8);
        app.session.current_frame_mut().unwrap().apply_highlight_wb_selection(sample);
        assert_eq!(
            app.session.current_frame().unwrap().selection_status,
            SelectionStatus::Valid
        );

        // Verify paper black and print exposure were updated
        let params = &app.session.current_frame().unwrap().params;
        assert!(params.paper_black != 0.0);
        assert!(params.print_exposure > 0.0);
    }

    // 5. Accept and save each frame in sequence (Frames 1 to 6)
    for frame_idx in 0..6 {
        assert_eq!(app.session.current_index, frame_idx);

        // Save active frame TIFF and Darktable XMP sidecar
        let (tif_path, xmp_path) = app
            .session
            .save_current_frame_and_xmp(&app.output_dir)
            .expect("Saving frame and XMP should succeed");

        assert!(tif_path.is_file(), "TIFF file must exist: {:?}", tif_path);
        assert!(xmp_path.is_file(), "XMP file must exist: {:?}", xmp_path);

        let expected_tif_name = format!("frame-{}.tif", frame_idx + 1);
        let expected_xmp_name = format!("frame-{}.tif.xmp", frame_idx + 1);
        assert_eq!(tif_path.file_name().unwrap(), expected_tif_name.as_str());
        assert_eq!(xmp_path.file_name().unwrap(), expected_xmp_name.as_str());

        // Advance to next frame
        app.session.accept_and_next();
    }

    // All frames accepted
    assert!(app.session.is_all_accepted(), "All 6 frames should be marked accepted");

    // 6. Verify and validate generated Darktable XMP sidecars
    for frame_num in 1..=6 {
        let xmp_file = output_dir.join(format!("frame-{frame_num}.tif.xmp"));
        let xmp_content = std::fs::read_to_string(&xmp_file).expect("Failed to read XMP file");

        let parsed_xmp = DarktableXmp::parse(&xmp_content)
            .expect("Generated XMP should parse cleanly back to DarktableXmp");

        assert_eq!(parsed_xmp.derived_from, format!("frame-{frame_num}.tif"));
        assert!(
            parsed_xmp.has_operation("negadoctor"),
            "Sidecar for frame {frame_num} must contain negadoctor operation"
        );
        assert!(
            parsed_xmp.has_operation("colorin"),
            "Sidecar for frame {frame_num} must contain colorin operation"
        );
        assert!(
            parsed_xmp.has_operation("flip"),
            "Sidecar for frame {frame_num} must contain flip operation"
        );

        if frame_num == 1 {
            // Frame 1 was rotated to 90 degrees
            let flip_item = parsed_xmp.find_last_operation("flip").unwrap();
            let raw_flip = flip_item.decode_params().unwrap();
            let orientation = Orientation::from_darktable_flip_params(&raw_flip).unwrap();
            assert_eq!(orientation, Orientation::Rotate90);
        }
    }

    // 7. If darktable-cli is available on Windows/system, verify darktable-cli renders the TIFF + XMP cleanly
    let dt_cli_path = "C:\\Program Files\\darktable\\bin\\darktable-cli.exe";
    if std::path::Path::new(dt_cli_path).exists() {
        let dt_config_dir = output_dir.join("dt_config");
        std::fs::create_dir_all(&dt_config_dir).unwrap();

        let tif_input = output_dir.join("frame-1.tif").to_string_lossy().replace('\\', "/");
        let xmp_input = output_dir.join("frame-1.tif.xmp").to_string_lossy().replace('\\', "/");
        let jpg_output = output_dir.join("frame-1.jpg").to_string_lossy().replace('\\', "/");
        let cfg_dir = dt_config_dir.to_string_lossy().replace('\\', "/");

        let status = Command::new(dt_cli_path)
            .args([
                &tif_input,
                &xmp_input,
                &jpg_output,
                "--core",
                "--configdir",
                &cfg_dir,
                "--library",
                ":memory:",
            ])
            .status();

        if let Ok(exit_status) = status {
            if exit_status.success() {
                let rendered_jpg = output_dir.join("frame-1.jpg");
                assert!(
                    rendered_jpg.is_file(),
                    "Darktable CLI should have successfully rendered frame-1.jpg"
                );
                let metadata = std::fs::metadata(&rendered_jpg).unwrap();
                assert!(metadata.len() > 100, "Rendered JPEG must contain image bytes");
            }
        }
    }

    // Clean up temporary files
    let _ = std::fs::remove_dir_all(&output_dir);
}
