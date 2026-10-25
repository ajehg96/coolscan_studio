//! Physical Nikon LS-40 Hardware Acceptance Suite (Phase 8 & Phase 9).
//!
//! Standard CI executes `hardware_acceptance_harness_compiles_and_validates_control_flow` to ensure
//! the hardware test harness remains type-checked and functional.
//!
//! To execute the physical hardware-in-the-loop tests with a Nikon LS-40 connected via WinUSB
//! and a 35mm film strip loaded in the SA-21 adapter, run:
//!
//! ```powershell
//! cargo test --test hardware_acceptance -- --ignored --test-threads=1 --nocapture
//! ```

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use coolscan_studio::diagnostics::SystemDiagnostics;
use coolscan_studio::processing::color::ScannerColorPipeline;
use coolscan_studio::processing::roll::{PreparedFrame, RollProfile};
use coolscan_studio::scanner::pipeline::{discover_strip, scan_strip_streaming};
use coolscan_studio::scanner::types::{
    FrameSelection, ScanControl, ScanError, ScanEvent, ScanRequest,
};
use coolscan_studio::tiff::read_tiff;
use coolscan_studio::ui::review::ReviewSession;

fn open_first_scanner() -> (nkscan::device::Device, nkscan::session::Session) {
    let scanners = nkscan::device::list();
    let scanner = scanners
        .into_iter()
        .next()
        .expect("No Nikon Coolscan scanner found via USB");
    let transport = scanner.open().expect("Failed to open USB transport");
    let session = nkscan::session::Session::open(transport).expect("Failed to open Session");
    (scanner, session)
}

#[test]
fn hardware_acceptance_harness_compiles_and_validates_control_flow() {
    let control = ScanControl::default();
    assert!(!control.is_cancel_requested());
    assert!(!control.is_stop_after_current_requested());

    let req = ScanRequest::new(FrameSelection::Specific(1), 725, 1, false, true)
        .with_roll(RollProfile::pro_image_100());
    assert!(req.validate().is_ok());

    let diag = SystemDiagnostics::probe();
    assert!(!diag.color_engine_info.is_empty());
}

#[test]
#[ignore = "requires Nikon LS-40 hardware"]
fn ls40_device_discovery_and_status() {
    let diag = SystemDiagnostics::probe();
    println!("{}", diag.format_summary());
    assert!(
        diag.scanner_detected,
        "Expected physical Nikon Coolscan scanner via USB; check power and WinUSB driver (Zadig)"
    );

    let (scanner, mut session) = open_first_scanner();
    println!("Opened scanner: {scanner}");

    let discovered = discover_strip(&mut session, |evt| println!("  [Discovery] {evt:?}"))
        .expect("Failed to discover frames on film strip");
    println!(
        "Discovered {} frames on film strip (DPI: {:?})",
        discovered.frame_count(),
        discovered.optical_dpi
    );
    assert!(
        discovered.frame_count() > 0,
        "Expected at least 1 frame on the loaded 35mm strip in SA-21 adapter"
    );
}

#[test]
#[ignore = "requires Nikon LS-40 hardware"]
fn ls40_single_frame_725_dpi_streaming_and_export() {
    let (scanner, session) = open_first_scanner();
    let roll = RollProfile::pro_image_100();
    let req =
        ScanRequest::new(FrameSelection::Specific(1), 725, 1, false, true).with_roll(roll.clone());
    let control = ScanControl::default();
    let streamed_count = Arc::new(AtomicUsize::new(0));
    let streamed_count_cb = Arc::clone(&streamed_count);
    let mut streamed_artifacts = Vec::new();

    let summary = scan_strip_streaming(
        &scanner,
        session,
        &req,
        None,
        &control,
        |evt| {
            if let ScanEvent::FrameStarted { frame_number, .. }
            | ScanEvent::FrameCompleted { frame_number } = evt
            {
                println!("  [Frame {frame_number}] {evt:?}");
            }
        },
        |artifact| {
            println!(
                "  [Streamed] Frame {} ready ({}x{} @ {} DPI)",
                artifact.frame_number, artifact.pass.cols, artifact.pass.rows, artifact.dpi
            );
            streamed_count_cb.fetch_add(1, Ordering::SeqCst);
            streamed_artifacts.push(artifact);
            ControlFlow::Continue(())
        },
    )
    .expect("Single-frame 725 DPI scan failed");

    assert_eq!(streamed_count.load(Ordering::SeqCst), 1);
    assert_eq!(summary.frames_acquired, 1);
    assert_eq!(summary.frame_numbers, vec![1]);
    assert_eq!(streamed_artifacts.len(), 1);

    let pipeline = ScannerColorPipeline::default_ls40().unwrap();
    let prepared = PreparedFrame::from_artifact(
        streamed_artifacts.into_iter().next().unwrap(),
        roll,
        &pipeline,
    )
    .expect("Failed to prepare scanned frame");
    let review = ReviewSession::from_prepared_frames(vec![prepared], pipeline).unwrap();

    let temp_dir = std::env::temp_dir().join(format!(
        "coolscan_hw_acceptance_export_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&temp_dir);

    let (master_path, working_path, xmp_path) = review
        .save_current_complete_bundle(&temp_dir)
        .expect("Failed to export complete bundle");
    let master_tiff = read_tiff(&master_path).expect("Valid master TIFF");
    assert!(master_tiff.icc_profile.is_some());
    assert!(working_path.is_file());
    assert!(xmp_path.is_file());

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
#[ignore = "requires Nikon LS-40 hardware"]
fn ls40_cancel_current_frame_and_recover() {
    let (scanner, session) = open_first_scanner();
    let req = ScanRequest::new(FrameSelection::Specific(1), 725, 1, false, true)
        .with_roll(RollProfile::pro_image_100());
    let control = ScanControl::default();
    let control_cb = control.clone();

    // 1. Trigger mid-pass cancellation as soon as progress events begin
    let cancel_outcome = scan_strip_streaming(
        &scanner,
        session,
        &req,
        None,
        &control,
        move |evt| {
            if let ScanEvent::Progress { .. } = evt {
                control_cb.request_cancel_current();
            }
        },
        |_artifact| ControlFlow::Continue(()),
    );

    assert!(
        matches!(cancel_outcome, Err(ScanError::Cancelled { .. })),
        "Expected ScanError::Cancelled on mid-pass abort, got: {cancel_outcome:?}"
    );
    println!("Mid-pass cancellation succeeded cleanly; verifying immediate recovery scan...");

    // 2. Immediately execute a follow-up scan to verify the scanner recovers cleanly
    let fresh_control = ScanControl::default();
    let (recovery_scanner, recovery_session) = open_first_scanner();
    let recovery_summary = scan_strip_streaming(
        &recovery_scanner,
        recovery_session,
        &req,
        None,
        &fresh_control,
        |_evt| {},
        |_artifact| ControlFlow::Continue(()),
    )
    .expect("Follow-up scan after cancellation must succeed");
    assert_eq!(recovery_summary.frames_acquired, 1);
}

#[test]
#[ignore = "requires Nikon LS-40 hardware"]
fn ls40_stop_after_current_frame_multi_frame() {
    let (scanner, session) = open_first_scanner();
    let req = ScanRequest::new(FrameSelection::List(vec![1, 2]), 725, 1, false, true)
        .with_roll(RollProfile::pro_image_100());
    let control = ScanControl::default();
    let control_on_frame = control.clone();

    let summary = scan_strip_streaming(
        &scanner,
        session,
        &req,
        None,
        &control,
        |_evt| {},
        move |artifact| {
            println!(
                "  [Streamed] Frame {} complete; requesting stop_after_current",
                artifact.frame_number
            );
            control_on_frame.request_stop_after_current();
            ControlFlow::Continue(())
        },
    )
    .expect("Stop-after-current scan should return partial StripSummary");

    assert_eq!(
        summary.frames_acquired, 1,
        "Stop-after-current must preserve Frame 1 and skip Frame 2"
    );
    assert_eq!(summary.frame_numbers, vec![1]);
}
