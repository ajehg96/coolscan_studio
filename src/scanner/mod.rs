pub mod device;
pub mod pipeline;
pub mod recovery;
pub mod types;
pub mod worker;

pub use device::{first_scanner, list_scanners};
pub use pipeline::{
    FrameAcquisitionOutcome, configure_overscan, discover_strip, dots_to_mm,
    execute_streaming_frames, resolve_frame_rect, scan_strip, scan_strip_streaming,
    scan_strip_with_control, scan_strip_with_session,
};
pub use types::{
    CropFallbackReason, EffectiveImage, FrameArtifact, FrameSelection, ScanControl, ScanError,
    ScanEvent, ScanMetadata, ScanPhase, ScanRequest, ScanRequestError, StripDiscovery,
    StripScanResult, StripSummary,
};
pub use worker::{
    MockScannerBackend, ScanCommand, ScannerBackend, ScannerWorkerHandle, WorkerMessage,
};
