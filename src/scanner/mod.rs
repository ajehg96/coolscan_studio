pub mod device;
pub mod pipeline;
pub mod recovery;
pub mod types;
pub mod worker;

pub use device::{first_scanner, list_scanners};
pub use pipeline::{configure_overscan, discover_strip, dots_to_mm, scan_strip, scan_strip_with_session};
pub use types::{
    CropFallbackReason, EffectiveImage, FrameArtifact, FrameSelection, ScanError, ScanEvent,
    ScanMetadata, ScanPhase, ScanRequest, ScanRequestError, StripDiscovery, StripScanResult,
};
pub use worker::{
    MockScannerBackend, ScanCommand, ScannerBackend, ScannerWorkerHandle, WorkerMessage,
};
