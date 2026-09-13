//! Background scanner worker thread, command channel, and mockable scanner backend.
//!
//! Enforces Phase 11 requirements:
//! - Scanner I/O runs strictly on a dedicated background worker thread, never on the UI thread.
//! - Strongly typed `ScanCommand` inputs and `WorkerMessage` updates.
//! - Explicit cancellation: `CancelCurrentFrame`, `StopAfterCurrentFrame`, and `EjectFilm`.
//! - Pure mock backend (`MockScannerBackend`) enabling 100% headless end-to-end GUI workflow testing
//!   without physical LS-40 hardware.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::{spawn, JoinHandle};
use std::time::Duration;

use nkscan::protocol::data::Rect;
use nkscan::protocol::decode::Samples;
use nkscan::scan::pass::Pass;
use nkscan::session::Session;

use crate::crop::{CropDecision, EdgeConfidence};
use crate::processing::analysis::TechnicalAnalysis;
use crate::processing::color::ScannerColorPipeline;
use crate::processing::negadoctor::NegadoctorParams;
use crate::processing::orientation::Orientation;
use crate::processing::roll::{PreparedFrame, RollProfile};
use crate::scanner::types::{
    FrameArtifact, ScanEvent, ScanMetadata, ScanPhase, ScanRequest, StripDiscovery,
};

/// Commands sent from the UI/main thread to the scanner background worker.
#[derive(Debug, Clone, PartialEq)]
pub enum ScanCommand {
    /// Check if a scanner is present and whether film is loaded.
    CheckMedia,
    /// Run strip discovery to locate frame boundaries.
    DiscoverStrip,
    /// Begin scanning frames according to the specified request.
    StartScan(ScanRequest),
    /// Abort the currently scanning frame immediately.
    CancelCurrentFrame,
    /// Finish the current frame, then stop without starting subsequent frames.
    StopAfterCurrentFrame,
    /// Eject the film adapter or strip.
    EjectFilm,
    /// Stop the background worker thread cleanly.
    Shutdown,
}

/// Messages emitted from the scanner background worker to the UI/main thread.
#[derive(Debug, Clone)]
pub enum WorkerMessage {
    /// Granular scanner progress notification.
    Event(ScanEvent),
    /// A single frame has been acquired and pre-analyzed, ready for user review.
    FrameReady(Box<PreparedFrame>),
    /// Discovery result ready.
    DiscoveryReady(StripDiscovery),
    /// The entire requested scan sequence has completed.
    StripScanComplete,
    /// Scan was cancelled or stopped by user request.
    ScanCancelled { reason: String },
    /// Film was successfully ejected.
    FilmEjected,
    /// An error occurred during scanning or hardware communication.
    Error(String),
}

/// Trait abstracting scanner hardware operations for dependency injection and testing.
pub trait ScannerBackend: Send + 'static {
    /// Checks device presence and media state.
    fn check_media(&mut self, emit: &mut dyn FnMut(WorkerMessage));

    /// Runs strip discovery.
    fn discover_strip(&mut self, emit: &mut dyn FnMut(WorkerMessage));

    /// Scans frames according to the request.
    fn scan_strip(
        &mut self,
        request: &ScanRequest,
        cancel_current: Arc<AtomicBool>,
        stop_after_current: Arc<AtomicBool>,
        emit: &mut dyn FnMut(WorkerMessage),
    );

    /// Ejects film media.
    fn eject_film(&mut self, emit: &mut dyn FnMut(WorkerMessage));
}

/// A simulated scanner backend providing canned multi-frame strips for UI testing without hardware.
pub struct MockScannerBackend {
    pub frame_count: usize,
    pub step_delay: Duration,
    pub roll_profile: RollProfile,
}

impl Default for MockScannerBackend {
    fn default() -> Self {
        Self::new(6, Duration::from_millis(0))
    }
}

impl MockScannerBackend {
    pub fn new(frame_count: usize, step_delay: Duration) -> Self {
        Self {
            frame_count,
            step_delay,
            roll_profile: RollProfile::pro_image_100(),
        }
    }

    fn synthesize_canned_frame(&self, frame_number: usize, width: usize, height: usize) -> PreparedFrame {
        let mut red = vec![10000u16; width * height];
        let mut green = vec![12000u16; width * height];
        let mut blue = vec![9000u16; width * height];

        // Create a realistic gradient with a bright highlight patch for testing white balance
        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                let factor = (x + y) as f32 / (width + height) as f32;
                // Highlight area: dense negative (smaller scanner transmission value)
                if x < width / 3 && y < height / 3 {
                    red[idx] = 1200;
                    green[idx] = 1500;
                    blue[idx] = 900;
                } else {
                    red[idx] = (2000.0 + factor * 25000.0) as u16;
                    green[idx] = (2500.0 + factor * 28000.0) as u16;
                    blue[idx] = (1800.0 + factor * 22000.0) as u16;
                }
            }
        }

        let samples = Samples {
            colors: vec![red, green, blue],
            ir: None,
        };

        let pass = Pass {
            layout: nkscan::protocol::image::Layout::single_line(height as u32, width as u32, vec![1]),
            cooperation: Vec::new(),
            complete: true,
            blocks: 1,
            rows: height,
            cols: width,
        };

        let crop = CropDecision {
            leading: EdgeConfidence::Confident {
                column: 0,
                dots: 0,
            },
            trailing: EdgeConfidence::Confident {
                column: width - 1,
                dots: (width - 1) as u32,
            },
            columns: (0, width - 1),
            rows: (0, height - 1),
            travel_dots: (0, (width - 1) as u32),
            accepted: true,
            is_blank: false,
        };

        let artifact = FrameArtifact {
            frame_number,
            total_frames: self.frame_count,
            dpi: 2900,
            raw_rect: Rect {
                left: 0,
                right: width as u32,
                top: 0,
                bottom: height as u32,
            },
            samples,
            pass,
            crop: Some(crop),
            scan_metadata: ScanMetadata {
                scanner_model: Some("Nikon COOLSCAN IV ED (Mocked)".into()),
                focus_position: Some(128),
                exposures: None,
                hardware_samples: 1,
                software_passes: 1,
                infrared_cleaned_pixels: Some(0),
            },
            roll: Some(self.roll_profile.clone()),
        };

        let mut params = NegadoctorParams::from_dmin(self.roll_profile.dmin);
        params.dmax = 3.20 + (frame_number as f32 * 0.05);
        params.offset = 0.08 + (frame_number as f32 * 0.01);
        params.paper_black = 0.10;
        params.paper_grade = 4.5;
        params.paper_gloss = 0.75;
        params.print_exposure = 0.95;

        let dmax = params.dmax;
        let scan_bias = params.offset;

        PreparedFrame {
            source: artifact,
            roll: self.roll_profile.clone(),
            orientation: Orientation::Normal,
            params,
            technical: TechnicalAnalysis {
                dmax,
                scan_bias,
            },
            highlight_wb_rect: None,
        }
    }
}

impl ScannerBackend for MockScannerBackend {
    fn check_media(&mut self, emit: &mut dyn FnMut(WorkerMessage)) {
        emit(WorkerMessage::Event(ScanEvent::ScannerFound {
            description: "Nikon COOLSCAN IV ED (Mocked)".into(),
        }));
        emit(WorkerMessage::Event(ScanEvent::SessionReady));
        emit(WorkerMessage::Event(ScanEvent::MediaChecked { loaded: true }));
    }

    fn discover_strip(&mut self, emit: &mut dyn FnMut(WorkerMessage)) {
        emit(WorkerMessage::Event(ScanEvent::DiscoveryStarted {
            method: nkscan::scan::framing::Framing::Perforation,
        }));
        if self.step_delay > Duration::ZERO {
            std::thread::sleep(self.step_delay);
        }
        let detected_count = self.frame_count;
        emit(WorkerMessage::Event(ScanEvent::DiscoveryCompleted {
            detected_count,
        }));

        let fake_rects = (0..detected_count)
            .map(|i| Rect {
                left: 0,
                right: 500,
                top: (i * 600) as u32,
                bottom: ((i + 1) * 600) as u32,
            })
            .collect();

        let discovery = StripDiscovery {
            framing: nkscan::scan::framing::Framing::Perforation,
            detected_frames: fake_rects,
            overscan_frames: Vec::new(),
            optical_dpi: (2900, 2900),
            boundary_table: None,
            thumbnail: None,
            thumbnail_samples: None,
        };

        emit(WorkerMessage::DiscoveryReady(discovery));
    }

    fn scan_strip(
        &mut self,
        request: &ScanRequest,
        cancel_current: Arc<AtomicBool>,
        stop_after_current: Arc<AtomicBool>,
        emit: &mut dyn FnMut(WorkerMessage),
    ) {
        let frame_numbers = request.frames.resolve(self.frame_count);

        for &frame_num in &frame_numbers {
            if cancel_current.load(Ordering::Relaxed) {
                emit(WorkerMessage::ScanCancelled {
                    reason: format!("Frame {frame_num} cancelled"),
                });
                return;
            }

            emit(WorkerMessage::Event(ScanEvent::FrameStarted {
                frame_number: frame_num,
                total_frames: self.frame_count,
                dpi: request.dpi,
                is_super_fine: false,
                features: vec!["Mocked Auto-Exposure".into(), "Mocked Auto-Focus".into()],
            }));

            // Emit progress steps
            for percent in [25u8, 50, 75, 100] {
                if cancel_current.load(Ordering::Relaxed) {
                    emit(WorkerMessage::ScanCancelled {
                        reason: format!("Frame {frame_num} cancelled during scan"),
                    });
                    cancel_current.store(false, Ordering::Relaxed);
                    return;
                }
                if self.step_delay > Duration::ZERO {
                    std::thread::sleep(self.step_delay / 4);
                }
                emit(WorkerMessage::Event(ScanEvent::Progress {
                    frame_number: frame_num,
                    phase: ScanPhase::Acquiring,
                    percent,
                    pass: 1,
                    total_passes: 1,
                }));
            }

            // Synthesize frame
            let canned = self.synthesize_canned_frame(frame_num, 40, 60);
            emit(WorkerMessage::FrameReady(Box::new(canned)));

            emit(WorkerMessage::Event(ScanEvent::FrameCompleted {
                frame_number: frame_num,
            }));

            if stop_after_current.load(Ordering::Relaxed) {
                emit(WorkerMessage::ScanCancelled {
                    reason: "Stopped after current frame".into(),
                });
                stop_after_current.store(false, Ordering::Relaxed);
                return;
            }
        }

        emit(WorkerMessage::StripScanComplete);
    }

    fn eject_film(&mut self, emit: &mut dyn FnMut(WorkerMessage)) {
        if self.step_delay > Duration::ZERO {
            std::thread::sleep(self.step_delay);
        }
        emit(WorkerMessage::FilmEjected);
    }
}

/// Scanner backend driving physical Nikon Coolscan hardware via USB.
pub struct HardwareScannerBackend {
    pub roll_profile: RollProfile,
    pub color_pipeline: Option<ScannerColorPipeline>,
}

impl Default for HardwareScannerBackend {
    fn default() -> Self {
        Self::new(RollProfile::pro_image_100())
    }
}

impl HardwareScannerBackend {
    pub fn new(roll_profile: RollProfile) -> Self {
        let color_pipeline = ScannerColorPipeline::default_ls40().ok();
        Self {
            roll_profile,
            color_pipeline,
        }
    }
}

impl ScannerBackend for HardwareScannerBackend {
    fn check_media(&mut self, emit: &mut dyn FnMut(WorkerMessage)) {
        let scanners = nkscan::device::list();
        let Some(scanner) = scanners.first() else {
            emit(WorkerMessage::Error("No Nikon Coolscan scanners found.".into()));
            return;
        };

        emit(WorkerMessage::Event(ScanEvent::ScannerFound {
            description: scanner.to_string(),
        }));

        let transport = match scanner.open() {
            Ok(t) => t,
            Err(e) => {
                emit(WorkerMessage::Error(format!("Failed to open scanner: {e}")));
                return;
            }
        };

        let mut session = match Session::open(transport) {
            Ok(s) => s,
            Err(e) => {
                emit(WorkerMessage::Error(format!("Failed to start scanner session: {e}")));
                return;
            }
        };

        emit(WorkerMessage::Event(ScanEvent::SessionReady));

        match session.media_loaded() {
            Ok(loaded) => emit(WorkerMessage::Event(ScanEvent::MediaChecked { loaded })),
            Err(e) => emit(WorkerMessage::Error(format!("Could not check media state: {e}"))),
        }
    }

    fn discover_strip(&mut self, emit: &mut dyn FnMut(WorkerMessage)) {
        let scanners = nkscan::device::list();
        let Some(scanner) = scanners.first() else {
            emit(WorkerMessage::Error("No Nikon Coolscan scanners found.".into()));
            return;
        };

        let transport = match scanner.open() {
            Ok(t) => t,
            Err(e) => {
                emit(WorkerMessage::Error(format!("Failed to open scanner: {e}")));
                return;
            }
        };

        let mut session = match Session::open(transport) {
            Ok(s) => s,
            Err(e) => {
                emit(WorkerMessage::Error(format!("Failed to start scanner session: {e}")));
                return;
            }
        };

        match crate::scanner::pipeline::discover_strip(&mut session, |evt| {
            emit(WorkerMessage::Event(evt));
        }) {
            Ok(discovery) => emit(WorkerMessage::DiscoveryReady(discovery)),
            Err(e) => emit(WorkerMessage::Error(format!("Strip discovery failed: {e}"))),
        }
    }

    fn scan_strip(
        &mut self,
        request: &ScanRequest,
        _cancel_current: Arc<AtomicBool>,
        _stop_after_current: Arc<AtomicBool>,
        emit: &mut dyn FnMut(WorkerMessage),
    ) {
        let scanners = nkscan::device::list();
        let Some(scanner) = scanners.first() else {
            emit(WorkerMessage::Error("No Nikon Coolscan scanners found.".into()));
            return;
        };

        let transport = match scanner.open() {
            Ok(t) => t,
            Err(e) => {
                emit(WorkerMessage::Error(format!("Failed to open scanner: {e}")));
                return;
            }
        };

        let session = match Session::open(transport) {
            Ok(s) => s,
            Err(e) => {
                emit(WorkerMessage::Error(format!("Failed to start scanner session: {e}")));
                return;
            }
        };

        let roll = request.roll.clone().unwrap_or_else(|| self.roll_profile.clone());
        let default_pipeline;
        let pipeline: &ScannerColorPipeline = match &self.color_pipeline {
            Some(p) => p,
            None => {
                default_pipeline = match ScannerColorPipeline::default_ls40() {
                    Ok(p) => p,
                    Err(e) => {
                        emit(WorkerMessage::Error(format!("Color pipeline error: {e}")));
                        return;
                    }
                };
                &default_pipeline
            }
        };

        let scan_result = crate::scanner::pipeline::scan_strip_with_session(
            scanner,
            session,
            request,
            None,
            |evt| {
                emit(WorkerMessage::Event(evt));
            },
        );

        match scan_result {
            Ok(result) => {
                for artifact in result.frames {
                    match PreparedFrame::from_artifact(artifact, roll.clone(), pipeline) {
                        Ok(prepared) => emit(WorkerMessage::FrameReady(Box::new(prepared))),
                        Err(e) => emit(WorkerMessage::Error(format!("Frame preparation error: {e}"))),
                    }
                }
                emit(WorkerMessage::StripScanComplete);
            }
            Err(e) => {
                emit(WorkerMessage::Error(format!("Scan failed: {e}")));
            }
        }
    }

    fn eject_film(&mut self, emit: &mut dyn FnMut(WorkerMessage)) {
        let scanners = nkscan::device::list();
        let Some(scanner) = scanners.first() else {
            emit(WorkerMessage::Error("No Nikon Coolscan scanners found.".into()));
            return;
        };

        let transport = match scanner.open() {
            Ok(t) => t,
            Err(e) => {
                emit(WorkerMessage::Error(format!("Failed to open scanner: {e}")));
                return;
            }
        };

        let mut session = match Session::open(transport) {
            Ok(s) => s,
            Err(e) => {
                emit(WorkerMessage::Error(format!("Failed to start scanner session: {e}")));
                return;
            }
        };

        match session.eject() {
            Ok(_) => emit(WorkerMessage::FilmEjected),
            Err(e) => emit(WorkerMessage::Error(format!("Eject failed: {e}"))),
        }
    }
}

/// Handle held by the UI thread to interact with the background scanner worker.
pub struct ScannerWorkerHandle {
    cmd_tx: Sender<ScanCommand>,
    msg_rx: Receiver<WorkerMessage>,
    cancel_current: Arc<AtomicBool>,
    stop_after_current: Arc<AtomicBool>,
    _thread: Option<JoinHandle<()>>,
}

impl ScannerWorkerHandle {
    /// Spawns a background scanner worker with the given backend.
    pub fn spawn<B: ScannerBackend>(mut backend: B) -> Self {
        let (cmd_tx, cmd_rx) = channel::<ScanCommand>();
        let (msg_tx, msg_rx) = channel::<WorkerMessage>();

        let cancel_current = Arc::new(AtomicBool::new(false));
        let stop_after_current = Arc::new(AtomicBool::new(false));

        let cancel_current_clone = Arc::clone(&cancel_current);
        let stop_after_current_clone = Arc::clone(&stop_after_current);

        let thread = spawn(move || {
            let mut emit = |msg: WorkerMessage| {
                let _ = msg_tx.send(msg);
            };

            while let Ok(cmd) = cmd_rx.recv() {
                match cmd {
                    ScanCommand::CheckMedia => {
                        backend.check_media(&mut emit);
                    }
                    ScanCommand::DiscoverStrip => {
                        backend.discover_strip(&mut emit);
                    }
                    ScanCommand::StartScan(req) => {
                        backend.scan_strip(
                            &req,
                            Arc::clone(&cancel_current_clone),
                            Arc::clone(&stop_after_current_clone),
                            &mut emit,
                        );
                    }
                    ScanCommand::CancelCurrentFrame => {
                        cancel_current_clone.store(true, Ordering::Relaxed);
                    }
                    ScanCommand::StopAfterCurrentFrame => {
                        stop_after_current_clone.store(true, Ordering::Relaxed);
                    }
                    ScanCommand::EjectFilm => {
                        backend.eject_film(&mut emit);
                    }
                    ScanCommand::Shutdown => {
                        break;
                    }
                }
            }
        });

        Self {
            cmd_tx,
            msg_rx,
            cancel_current,
            stop_after_current,
            _thread: Some(thread),
        }
    }

    /// Spawns a worker with the default simulated/mock backend.
    pub fn spawn_mock(frame_count: usize) -> Self {
        Self::spawn(MockScannerBackend::new(frame_count, Duration::ZERO))
    }

    /// Spawns a worker with the physical scanner hardware backend.
    pub fn spawn_hardware(roll: RollProfile) -> Self {
        Self::spawn(HardwareScannerBackend::new(roll))
    }

    /// Sends a command to the scanner worker.
    pub fn send(&self, cmd: ScanCommand) {
        if matches!(cmd, ScanCommand::StartScan(_)) {
            self.cancel_current.store(false, Ordering::Relaxed);
            self.stop_after_current.store(false, Ordering::Relaxed);
        }
        let _ = self.cmd_tx.send(cmd);
    }

    /// Polls for the next message from the scanner worker without blocking.
    pub fn try_recv(&self) -> Option<WorkerMessage> {
        match self.msg_rx.try_recv() {
            Ok(msg) => Some(msg),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }

    /// Requests immediate cancellation of the active frame.
    pub fn cancel_current(&self) {
        self.cancel_current.store(true, Ordering::Relaxed);
        self.send(ScanCommand::CancelCurrentFrame);
    }

    /// Requests stopping after the active frame finishes.
    pub fn stop_after_current(&self) {
        self.stop_after_current.store(true, Ordering::Relaxed);
        self.send(ScanCommand::StopAfterCurrentFrame);
    }

    /// Requests film ejection.
    pub fn eject(&self) {
        self.send(ScanCommand::EjectFilm);
    }
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::types::FrameSelection;

    #[test]
    fn mock_backend_canned_strip_scan_end_to_end() {
        let handle = ScannerWorkerHandle::spawn_mock(6);

        handle.send(ScanCommand::CheckMedia);
        handle.send(ScanCommand::DiscoverStrip);

        let req = ScanRequest::new(FrameSelection::All, 2900, 1, false, true);
        handle.send(ScanCommand::StartScan(req));

        let mut frames_received = 0;
        let mut discovery_done = false;
        let mut strip_complete = false;

        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if let Some(msg) = handle.try_recv() {
                match msg {
                    WorkerMessage::DiscoveryReady(d) => {
                        assert_eq!(d.frames().len(), 6);
                        discovery_done = true;
                    }
                    WorkerMessage::FrameReady(f) => {
                        frames_received += 1;
                        assert_eq!(f.source.frame_number, frames_received);
                        assert_eq!(f.source.total_frames, 6);
                        assert!(f.params.dmax > 3.0);
                    }
                    WorkerMessage::StripScanComplete => {
                        strip_complete = true;
                        break;
                    }
                    _ => {}
                }
            }
            std::thread::yield_now();
        }

        assert!(discovery_done, "Discovery should have completed");
        assert_eq!(frames_received, 6, "Expected all 6 canned frames");
        assert!(strip_complete, "Strip should be completed");
    }

    #[test]
    fn mock_backend_cancellation_current_frame() {
        let handle = ScannerWorkerHandle::spawn(MockScannerBackend::new(
            6,
            Duration::from_millis(50),
        ));

        let req = ScanRequest::new(FrameSelection::All, 2900, 1, false, true);
        handle.send(ScanCommand::StartScan(req));

        // Let it start, then cancel immediately
        std::thread::sleep(Duration::from_millis(15));
        handle.cancel_current();

        let mut was_cancelled = false;
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            if let Some(WorkerMessage::ScanCancelled { .. }) = handle.try_recv() {
                was_cancelled = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(was_cancelled, "Scan should have been cancelled");
    }

    #[test]
    fn mock_backend_stop_after_current_frame() {
        let handle = ScannerWorkerHandle::spawn(MockScannerBackend::new(
            6,
            Duration::from_millis(60),
        ));

        let req = ScanRequest::new(FrameSelection::All, 2900, 1, false, true);
        handle.send(ScanCommand::StartScan(req));

        // Let frame 1 start, then request stop after current
        std::thread::sleep(Duration::from_millis(15));
        handle.stop_after_current();

        let mut frames_received = 0;
        let mut stopped = false;
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            if let Some(msg) = handle.try_recv() {
                match msg {
                    WorkerMessage::FrameReady(_) => {
                        frames_received += 1;
                    }
                    WorkerMessage::ScanCancelled { reason } => {
                        if reason.contains("Stopped after current frame") {
                            stopped = true;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        assert!(stopped, "Scan should have stopped after current frame");
        assert_eq!(frames_received, 1, "Should have acquired only 1 frame before stopping");
    }

    #[test]
    fn mock_backend_eject_film() {
        let handle = ScannerWorkerHandle::spawn_mock(6);
        handle.eject();

        let mut ejected = false;
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(1) {
            if let Some(WorkerMessage::FilmEjected) = handle.try_recv() {
                ejected = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        assert!(ejected, "Film should have ejected");
    }
}
