use coolscan_studio::{
    cli::{self, Options},
    processing::{RollProfile, ScannerColorPipeline},
    scanner::{types::FrameSelection, worker::ScannerWorkerHandle},
    ui::{ReviewApp, ReviewSession, app::QualityPreset},
};
use std::path::PathBuf;

#[test]
fn phase12_gui_setup_and_custom_scan_request_builder() {
    let pipeline =
        ScannerColorPipeline::default_ls40().expect("Failed to initialize color pipeline");
    let roll = RollProfile::pro_image_100();
    let session = ReviewSession::empty(roll, pipeline);
    let mut app = ReviewApp::new(session);

    // 1. Verify default setup configuration (Basic Mode defaults)
    assert_eq!(app.scan_setup.resolution_dpi, 2900);
    assert!(app.scan_setup.dust_removal);
    assert_eq!(app.scan_setup.quality, QualityPreset::Standard);
    assert_eq!(app.scan_setup.samples, 1);
    assert_eq!(app.scan_setup.film_stock_index, 0); // Pro Image 100
    assert!(app.scan_setup.auto_crop);
    assert_eq!(app.scan_setup.scanner_offset_mm, 0.0);
    assert!(app.scan_setup.all_frames);

    let default_req = app.build_scan_request();
    assert_eq!(default_req.dpi, 2900);
    assert_eq!(default_req.samples, 1);
    assert!(default_req.clean);
    assert!(default_req.auto_crop);
    assert_eq!(default_req.frames, FrameSelection::All);
    assert_eq!(default_req.offset_mm, 0.0);
    assert_eq!(
        default_req.roll.as_ref().unwrap().film_stock,
        "Kodak Pro Image 100"
    );

    // 2. Configure Quality Preset: Fine (4x multi-sample)
    app.scan_setup.quality = QualityPreset::Fine;
    let fine_req = app.build_scan_request();
    assert_eq!(fine_req.samples, 4);

    // 3. Configure Quality Preset: Ultimate (16x multi-sample)
    app.scan_setup.quality = QualityPreset::Ultimate;
    let ultimate_req = app.build_scan_request();
    assert_eq!(ultimate_req.samples, 16);

    // 4. Configure Film Profile: Kodak Portra 400
    app.scan_setup.film_stock_index = 1;
    let portra_req = app.build_scan_request();
    assert_eq!(
        portra_req.roll.as_ref().unwrap().film_stock,
        "Kodak Portra 400"
    );
    assert_eq!(portra_req.roll.as_ref().unwrap().id.0, "kodak-portra-400");

    // 5. Configure Film Profile: Kodak Gold 200
    app.scan_setup.film_stock_index = 2;
    let gold_req = app.build_scan_request();
    assert_eq!(gold_req.roll.as_ref().unwrap().film_stock, "Kodak Gold 200");
    assert_eq!(gold_req.roll.as_ref().unwrap().id.0, "kodak-gold-200");

    // 6. Advanced Controls: Custom samples slider (e.g. 8x), travel offset (+1.5 mm), manual frames (frames 2 and 5)
    app.scan_setup.quality = QualityPreset::Custom;
    app.scan_setup.samples = 8;
    app.scan_setup.resolution_dpi = 1450;
    app.scan_setup.dust_removal = false;
    app.scan_setup.auto_crop = false;
    app.scan_setup.scanner_offset_mm = 1.5;
    app.scan_setup.all_frames = false;
    app.scan_setup.frame_selected = [false, true, false, false, true, false]; // frames 2 and 5

    let advanced_req = app.build_scan_request();
    assert_eq!(advanced_req.dpi, 1450);
    assert_eq!(advanced_req.samples, 8);
    assert!(!advanced_req.clean);
    assert!(!advanced_req.auto_crop);
    assert_eq!(advanced_req.offset_mm, 1.5);
    assert_eq!(advanced_req.frames, FrameSelection::List(vec![2, 5]));
}

#[test]
fn phase12_start_scan_syncs_session_roll_and_dispatches_to_worker() {
    let pipeline =
        ScannerColorPipeline::default_ls40().expect("Failed to initialize color pipeline");
    let initial_roll = RollProfile::pro_image_100();
    let session = ReviewSession::empty(initial_roll, pipeline);
    let worker = ScannerWorkerHandle::spawn_mock(6);

    let mut app = ReviewApp::new(session)
        .with_worker(worker)
        .with_output_dir(PathBuf::from("/tmp/scans"));

    assert_eq!(app.session.roll.film_stock, "Kodak Pro Image 100");

    // Select Kodak Portra 400 and start scan
    app.scan_setup.film_stock_index = 1;
    app.scan_setup.show_setup_modal = true;
    app.start_scan();

    // Verify session roll was updated to Portra 400 and scanning flag set
    assert_eq!(app.session.roll.film_stock, "Kodak Portra 400");
    assert!(app.is_scanning);
    assert!(!app.scan_setup.show_setup_modal);
    assert_eq!(app.status_message, "Starting scan...");
}

#[test]
fn phase12_all_cli_scan_capabilities_remain_reachable() {
    // 1. --gui flag
    let parsed_gui = cli::parse(["--gui".to_string()]).unwrap().unwrap();
    assert!(parsed_gui.gui);
    assert!(!parsed_gui.mock);
    assert!(!parsed_gui.scan);

    // 2. --mock flag
    let parsed_mock = cli::parse(["--mock".to_string()]).unwrap().unwrap();
    assert!(parsed_mock.mock);
    assert!(parsed_mock.gui);

    // 3. Full CLI scan flag combinations
    let parsed_full = cli::parse([
        "--scan".to_string(),
        "--frame".to_string(),
        "3".to_string(),
        "--dpi".to_string(),
        "2900".to_string(),
        "--samples".to_string(),
        "4".to_string(),
        "--clean".to_string(),
        "--tiff".to_string(),
        "--roll".to_string(),
        "portra-400".to_string(),
        "--offset-mm".to_string(),
        "+0.8".to_string(),
        "--output".to_string(),
        "/scans/roll-1".to_string(),
    ])
    .unwrap()
    .unwrap();

    assert!(parsed_full.scan);
    assert_eq!(parsed_full.frame, Some(3));
    assert_eq!(parsed_full.dpi, Some(2900));
    assert_eq!(parsed_full.samples, Some(4));
    assert!(parsed_full.clean);
    assert!(parsed_full.tiff);
    assert_eq!(parsed_full.roll.as_deref(), Some("portra-400"));
    assert_eq!(parsed_full.offset_mm, 0.8);
    assert_eq!(parsed_full.output.as_deref(), Some("/scans/roll-1"));

    // 4. High-fidelity shortcut flag
    let parsed_hq = cli::parse(["--scan".to_string(), "--high-fidelity".to_string()])
        .unwrap()
        .unwrap();
    assert!(parsed_hq.scan);
    assert!(parsed_hq.high_fidelity);

    // 5. Eject flag
    let parsed_eject = cli::parse(["--eject".to_string()]).unwrap().unwrap();
    assert!(parsed_eject.eject);

    // 6. Default execution (no args)
    let parsed_default = cli::parse(Vec::<String>::new()).unwrap().unwrap();
    assert_eq!(parsed_default, Options::default());
}
