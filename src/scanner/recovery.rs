use std::time::Duration;
use nkscan::{device::Device, protocol::data::BoundaryType2, session::Session};

/// Maximum scan attempts per frame before skipping.
pub const MAX_SCAN_ATTEMPTS: usize = 2;

/// Wait time before retrying a failed USB scan attempt.
pub const RETRY_DELAY: Duration = Duration::from_millis(1000);

/// Wait time between frame scans when refreshing the USB transport.
pub const INTER_FRAME_DELAY: Duration = Duration::from_millis(300);

/// Re-opens the scanner transport and restores boundary table if present.
pub fn refresh_session(
    scanner: &Device,
    boundary_table: Option<&BoundaryType2>,
) -> Result<Session, nkscan::error::Error> {
    let transport = scanner.open()?;
    let mut session = Session::open(transport)?;
    if let Some(table) = boundary_table {
        let _ = session.set_boundaries_type2(table);
    }
    Ok(session)
}
