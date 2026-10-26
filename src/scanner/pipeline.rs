use nkscan::{
    device::Device,
    protocol::{
        caps::set_window::{ColorInterleaving, ScanMode},
        data::{BoundaryType2, Rect},
        decode::Samples,
    },
    scan::{boundaries::Polarity, clean, frame, framing, window::Recipe},
    session::Session,
};
use std::thread::sleep;

use crate::{
    boundaries,
    crop::{CropDecision, CropOptions},
    scanner::{
        recovery::{INTER_FRAME_DELAY, MAX_SCAN_ATTEMPTS, RETRY_DELAY, refresh_session},
        types::{
            CropFallbackReason, FrameArtifact, ScanControl, ScanError, ScanEvent, ScanMetadata,
            ScanPhase, ScanRequest, StripDiscovery, StripScanResult, StripSummary,
        },
    },
};

/// Converts scanner dots to millimetres at a given DPI.
pub fn dots_to_mm(dots: u32, dpi: u16) -> f64 {
    f64::from(dots) * 25.4 / f64::from(dpi)
}

/// Computes the clamped overscan window length and per-frame top dot positions
/// within `[y_origin, y_end]` so a final frame near `y_end` clamps cleanly
/// rather than aborting overscan for the entire strip.
pub fn compute_clamped_overscan_positions(
    frame_cols: &[usize],
    raw_len_dots: u32,
    pitch: u32,
    y_origin: u32,
    y_end: u32,
    y_limit: u32,
) -> Option<(u32, Vec<u32>)> {
    let max_span = y_end.saturating_sub(y_origin);
    let length = raw_len_dots.saturating_add(240).min(y_limit).min(max_span);
    if length == 0 || frame_cols.is_empty() {
        return None;
    }
    let max_top = y_end.saturating_sub(length).max(y_origin);
    let tops = frame_cols
        .iter()
        .map(|&col| {
            y_origin
                .saturating_add((col as u32).saturating_mul(pitch))
                .saturating_sub(60)
                .clamp(y_origin, max_top)
        })
        .collect();
    Some((length, tops))
}

/// Sets up overscan frame boundaries using perforation registration (Type2) if supported.
#[allow(clippy::too_many_arguments)]
pub fn setup_overscan_frames(
    discovery: &mut framing::Discovery,
    session: &mut Session,
    samples: &Samples,
    y_dpi: u16,
    y_origin: u32,
    y_end: u32,
    y_limit: u32,
    x_start: u32,
    x_width: u32,
) -> Option<BoundaryType2> {
    if !matches!(
        discovery.table,
        nkscan::protocol::data::FrameTable::BoundaryType2(_)
    ) {
        return None;
    }
    let pass = discovery.thumbnail.as_ref()?;
    let image = nkscan::protocol::decode::Image::new(&pass.layout, samples).ok()?;
    let pitch = pass.layout.line_pitch;
    let nominal = (36.0 * f64::from(y_dpi) / 25.4) as u32 / pitch;
    let found =
        boundaries::preserve_edges(&image, nominal as usize, boundaries::Polarity::Negative);

    if found.frames.len() != discovery.frames.len() {
        return None;
    }
    let raw_len = u32::try_from(found.length).ok()?;
    let raw_len_dots = raw_len.checked_mul(pitch)?;
    let (length, tops) = compute_clamped_overscan_positions(
        &found.frames,
        raw_len_dots,
        pitch,
        y_origin,
        y_end,
        y_limit,
    )?;
    let perfs = session.read_perforations().ok()?;
    let mut entries = Vec::with_capacity(found.frames.len());
    for (&col, top) in found.frames.iter().zip(tops) {
        let p = perfs.at(col)?;
        entries.push(nkscan::protocol::data::FramePosition::new(top, p));
    }
    let table = BoundaryType2 { frames: entries };
    discovery.frames = table
        .frames
        .iter()
        .map(|p| p.rect(x_start, x_width, length))
        .collect();
    let _ = session.set_boundaries_type2(&table);
    Some(table)
}

/// Discovers frames on a loaded negative strip.
pub fn discover_strip(
    session: &mut Session,
    mut progress: impl FnMut(ScanEvent),
) -> Result<StripDiscovery, ScanError> {
    let framing = framing::Framing::choose(session.capabilities());
    progress(ScanEvent::DiscoveryStarted { method: framing });

    let mut samples = Samples::default();
    let discovery = framing::discover(session, None, Polarity::Negative, &mut samples)
        .map_err(ScanError::FramingDiscovery)?;

    let x_dpi = session.capabilities().address.x_axis.optical_dpi;
    let y_dpi = session.capabilities().address.y_axis.optical_dpi;

    let detected_count = discovery.frames.len();
    progress(ScanEvent::DiscoveryCompleted { detected_count });

    let thumbnail = discovery.thumbnail.clone();
    let thumbnail_samples = if thumbnail.is_some() {
        Some(samples)
    } else {
        None
    };

    Ok(StripDiscovery {
        framing,
        detected_frames: discovery.frames,
        overscan_frames: Vec::new(),
        optical_dpi: (x_dpi, y_dpi),
        boundary_table: None,
        thumbnail,
        thumbnail_samples,
    })
}

/// Applies overscan frame boundaries to an existing discovery result if supported.
pub fn configure_overscan(
    strip: &mut StripDiscovery,
    session: &mut Session,
    mut progress: impl FnMut(ScanEvent),
) -> Option<BoundaryType2> {
    let thumbnail = strip.thumbnail.as_ref()?;
    let samples = strip.thumbnail_samples.as_ref()?;
    let image = nkscan::protocol::decode::Image::new(&thumbnail.layout, samples).ok()?;

    let (_x_dpi, y_dpi) = strip.optical_dpi;
    let y_origin = session.capabilities().address.y_axis.address_range.start;
    let y_end = session.capabilities().address.y_axis.address_range.last;
    let y_limit = session.capabilities().address.y_axis.boundary;
    let x_start = session.capabilities().address.x_axis.address_range.start;
    let x_width = session.capabilities().address.x_axis.boundary;

    let pitch = thumbnail.layout.line_pitch;
    let nominal = (36.0 * f64::from(y_dpi) / 25.4) as u32 / pitch;
    let found =
        boundaries::preserve_edges(&image, nominal as usize, boundaries::Polarity::Negative);

    if found.frames.len() != strip.detected_frames.len() {
        return None;
    }
    let raw_len = u32::try_from(found.length).ok()?;
    let raw_len_dots = raw_len.checked_mul(pitch)?;
    let (length, tops) = compute_clamped_overscan_positions(
        &found.frames,
        raw_len_dots,
        pitch,
        y_origin,
        y_end,
        y_limit,
    )?;
    let perfs = session.read_perforations().ok()?;
    let mut entries = Vec::with_capacity(found.frames.len());
    for (&col, top) in found.frames.iter().zip(tops) {
        let p = perfs.at(col)?;
        entries.push(nkscan::protocol::data::FramePosition::new(top, p));
    }
    let table = BoundaryType2 { frames: entries };
    let overscan_rects: Vec<Rect> = table
        .frames
        .iter()
        .map(|p| p.rect(x_start, x_width, length))
        .collect();
    let _ = session.set_boundaries_type2(&table);

    let frame_count = overscan_rects.len();
    strip.overscan_frames = overscan_rects;
    strip.boundary_table = Some(table.clone());
    progress(ScanEvent::OverscanConfigured { frame_count });

    Some(table)
}

/// Scoped exposure state for a single frame scan attempt.
///
/// Ensures exposure state is strictly local to an attempt and does not
/// persist or leak into subsequent USB retry attempts.
#[derive(Default)]
pub struct AttemptExposureState {
    pub(crate) first_scanned: Option<frame::Scanned>,
}

impl AttemptExposureState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn exposures(&self) -> Option<&nkscan::scan::autoexpose::Exposures> {
        self.first_scanned.as_ref().map(|s| &s.exposures)
    }

    pub fn record_pass(&mut self, scanned: frame::Scanned) {
        if self.first_scanned.is_none() {
            self.first_scanned = Some(scanned);
        }
    }

    pub fn into_exposures(self) -> Option<nkscan::scan::autoexpose::Exposures> {
        self.first_scanned.map(|s| s.exposures)
    }
}

/// Validates that acquired artifacts are non-empty.
pub fn check_acquired_artifacts(acquired: &[FrameArtifact]) -> Result<(), ScanError> {
    if acquired.is_empty() {
        Err(ScanError::AllFramesFailed)
    } else {
        Ok(())
    }
}

/// Checks whether the active pass should continue or abort inside the `nkscan` progress callback.
pub fn check_pass_progress_control(control: &ScanControl) -> std::ops::ControlFlow<()> {
    if control.is_cancel_requested() {
        std::ops::ControlFlow::Break(())
    } else {
        std::ops::ControlFlow::Continue(())
    }
}

/// Outcome of a single scan pass, distinguishing intentional cancellation from transport/incomplete errors.
pub enum PassOutcome {
    Completed(frame::Scanned),
    Cancelled,
    Failed,
}

/// Classifies the result of `frame::scan_frame_with` alongside the `ScanControl` state.
///
/// Ensures user cancellation (`Error::Cancelled` or `cancel_current` set) never triggers USB retry loops.
pub fn classify_pass_result(
    result: Result<frame::Scanned, nkscan::error::Error>,
    control: &ScanControl,
) -> PassOutcome {
    if control.is_cancel_requested() {
        return PassOutcome::Cancelled;
    }
    match result {
        Ok(scanned) if scanned.pass.complete => PassOutcome::Completed(scanned),
        Err(nkscan::error::Error::Cancelled) => PassOutcome::Cancelled,
        Ok(_) | Err(_) => PassOutcome::Failed,
    }
}

/// Resolves and validates the adjusted frame rectangle for a given frame and travel offset (`offset_mm`).
pub fn resolve_frame_rect(
    frame_number: usize,
    detected: Rect,
    offset_mm: f64,
    y_dpi: u16,
    scanner_bounds: Rect,
    max_width: u32,
    max_height: u32,
) -> Result<(Rect, i32), ScanError> {
    crate::frame_position::apply_offset_within(
        detected,
        offset_mm,
        y_dpi,
        scanner_bounds,
        max_width,
        max_height,
    )
    .map_err(|reason| ScanError::InvalidOffset {
        frame_number,
        offset_mm,
        reason: reason.to_string(),
    })
}

/// Shifts a `BoundaryType2` table's frame top coordinates by `offset_dots`, validating `[y_start, y_end]`.
pub fn adjust_boundary_table(
    table: &BoundaryType2,
    offset_dots: i32,
    y_start: u32,
    y_end: u32,
) -> Option<BoundaryType2> {
    if offset_dots == 0 {
        return Some(table.clone());
    }
    let mut adjusted = Vec::with_capacity(table.frames.len());
    for entry in &table.frames {
        let new_top = entry.top.checked_add_signed(offset_dots)?;
        if new_top < y_start || new_top > y_end {
            return None;
        }
        let mut updated = *entry;
        updated.top = new_top;
        adjusted.push(updated);
    }
    Some(BoundaryType2 { frames: adjusted })
}

/// Executes a complete strip scan using an already established scanner session.
pub fn scan_strip_with_session(
    scanner: &Device,
    session: Session,
    request: &ScanRequest,
    pre_discovery: Option<StripDiscovery>,
    progress: impl FnMut(ScanEvent),
) -> Result<StripScanResult, ScanError> {
    scan_strip_with_control(
        scanner,
        session,
        request,
        pre_discovery,
        &ScanControl::default(),
        progress,
    )
}

/// Executes a complete strip scan with explicit live `ScanControl` (cancellation and stop-after-current).
pub fn scan_strip_with_control(
    scanner: &Device,
    session: Session,
    request: &ScanRequest,
    pre_discovery: Option<StripDiscovery>,
    control: &ScanControl,
    progress: impl FnMut(ScanEvent),
) -> Result<StripScanResult, ScanError> {
    let mut acquired_artifacts = Vec::new();
    let summary = scan_strip_streaming(
        scanner,
        session,
        request,
        pre_discovery,
        control,
        progress,
        |artifact| {
            acquired_artifacts.push(artifact);
            std::ops::ControlFlow::Continue(())
        },
    )?;

    Ok(StripScanResult {
        frames: acquired_artifacts,
        discovery: summary.discovery,
        roll: summary.roll,
        stop_reason: summary.stop_reason,
    })
}

/// Outcome of acquiring a single frame during a streaming strip scan.
pub enum FrameAcquisitionOutcome {
    Acquired(Box<FrameArtifact>),
    Skipped { reason: String },
    Cancelled { reason: String },
}

/// Executes the per-frame streaming control loop, delivering each acquired `FrameArtifact`
/// immediately via `on_frame` before initiating inter-frame USB refresh or subsequent frames.
#[allow(clippy::too_many_arguments)]
pub fn execute_streaming_frames<S>(
    state: &mut S,
    frames_to_scan: &[usize],
    total_discovered_frames: usize,
    control: &ScanControl,
    mut on_event: impl FnMut(ScanEvent),
    mut acquire_frame: impl FnMut(
        &mut S,
        usize,
        &ScanControl,
        &mut dyn FnMut(ScanEvent),
    ) -> Result<FrameAcquisitionOutcome, ScanError>,
    mut on_frame: impl FnMut(FrameArtifact) -> std::ops::ControlFlow<()>,
    mut inter_frame_refresh: impl FnMut(&mut S, usize, &mut dyn FnMut(ScanEvent)) -> bool,
) -> Result<(usize, Vec<usize>, Option<String>), ScanError> {
    let mut frames_acquired = 0usize;
    let mut frame_numbers = Vec::new();
    let mut stop_reason: Option<String> = None;
    let mut cancelled_frame: Option<usize> = None;

    for (seq_idx, &frame_num) in frames_to_scan.iter().enumerate() {
        // Checkpoint 1: Check cancellation before starting frame
        if control.take_cancel_requested() {
            let reason = format!("Frame {frame_num} cancelled");
            on_event(ScanEvent::FrameCancelled {
                frame_number: frame_num,
                reason: reason.clone(),
            });
            cancelled_frame = Some(frame_num);
            stop_reason = Some(reason);
            break;
        }

        let frame_idx = frame_num
            .checked_sub(1)
            .ok_or(ScanError::FrameUnavailable(frame_num))?;
        if frame_idx >= total_discovered_frames {
            on_event(ScanEvent::FrameSkipped {
                frame_number: frame_num,
                reason: format!("Frame {frame_num} is outside discovered bounds"),
            });
            continue;
        }

        match acquire_frame(state, frame_num, control, &mut on_event)? {
            FrameAcquisitionOutcome::Acquired(artifact) => {
                on_event(ScanEvent::FrameCompleted {
                    frame_number: frame_num,
                });
                frames_acquired += 1;
                frame_numbers.push(frame_num);

                // Deliver completed frame artifact immediately to consumer before next frame
                let flow = on_frame(*artifact);

                // Checkpoint 5: Check stop-after-current, consumer break, or late cancel before next frame
                if flow.is_break() || control.take_stop_after_current_requested() {
                    control.take_stop_after_current_requested();
                    on_event(ScanEvent::ScanStoppedAfterFrame {
                        frame_number: frame_num,
                    });
                    stop_reason = Some("Stopped after current frame".into());
                    break;
                }
                if control.take_cancel_requested() {
                    let reason = format!("Stopped after frame {frame_num} (cancel requested)");
                    on_event(ScanEvent::FrameCancelled {
                        frame_number: frame_num,
                        reason: reason.clone(),
                    });
                    stop_reason = Some(reason);
                    break;
                }
            }
            FrameAcquisitionOutcome::Skipped { reason } => {
                on_event(ScanEvent::FrameSkipped {
                    frame_number: frame_num,
                    reason,
                });
                continue;
            }
            FrameAcquisitionOutcome::Cancelled { reason } => {
                on_event(ScanEvent::FrameCancelled {
                    frame_number: frame_num,
                    reason: reason.clone(),
                });
                cancelled_frame = Some(frame_num);
                stop_reason = Some(reason);
                break;
            }
        }

        if seq_idx + 1 < frames_to_scan.len() {
            let next_num = frames_to_scan[seq_idx + 1];
            if !inter_frame_refresh(state, next_num, &mut on_event) {
                break;
            }
        }
    }

    if frames_acquired == 0 {
        if let Some(reason) = stop_reason {
            return Err(ScanError::Cancelled {
                frame_number: cancelled_frame,
                reason,
            });
        }
        return Err(ScanError::AllFramesFailed);
    }

    Ok((frames_acquired, frame_numbers, stop_reason))
}

/// Executes a strip scan, emitting each `FrameArtifact` incrementally via `on_frame`
/// as soon as that individual frame finishes acquisition, OpenICE cleaning, and auto-crop.
pub fn scan_strip_streaming(
    scanner: &Device,
    mut session: Session,
    request: &ScanRequest,
    pre_discovery: Option<StripDiscovery>,
    control: &ScanControl,
    mut progress: impl FnMut(ScanEvent),
    on_frame: impl FnMut(FrameArtifact) -> std::ops::ControlFlow<()>,
) -> Result<StripSummary, ScanError> {
    request.validate().map_err(ScanError::Request)?;

    let mut strip_discovery = match pre_discovery {
        Some(d) => d,
        None => {
            let mut d = discover_strip(&mut session, &mut progress)?;
            let _ = configure_overscan(&mut d, &mut session, &mut progress);
            d
        }
    };

    if strip_discovery.boundary_table.is_none() {
        let _ = configure_overscan(&mut strip_discovery, &mut session, &mut progress);
    }

    let y_dpi = strip_discovery.optical_dpi.1;
    let scan_dpi = request.dpi;
    let do_clean = request.clean;

    let (scanner_bounds, x_boundary, y_boundary, y_start, y_end) = {
        let caps = session.capabilities();
        let x = &caps.address.x_axis;
        let y = &caps.address.y_axis;
        let bounds = crate::frame_position::scanner_bounds(
            x.address_range.start,
            x.address_range.last,
            x.boundary,
            y.address_range.start,
            y.address_range.last,
            y.boundary,
        )
        .ok_or_else(|| ScanError::InvalidOffset {
            frame_number: 0,
            offset_mm: request.offset_mm,
            reason: "Invalid scanner address limits.".into(),
        })?;
        (
            bounds,
            x.boundary,
            y.boundary,
            y.address_range.start,
            bounds.bottom,
        )
    };

    let supports_multi_reading = session
        .capabilities()
        .set_window
        .mode
        .contains(ScanMode::MULTI_READING);

    let (hardware_samples, software_passes) = if supports_multi_reading {
        (request.samples, 1)
    } else if request.samples > 1 {
        (1, request.samples)
    } else {
        (1, 1)
    };

    if software_passes > 1 {
        let noise_reduction_db = 10.0 * (software_passes as f64).log10();
        progress(ScanEvent::MultiSamplingConfigured {
            hardware_samples,
            software_passes,
            noise_reduction_db,
        });
    }

    let recipe = Recipe {
        dpi: scan_dpi,
        samples: hardware_samples,
        interleaving: ColorInterleaving::LINE_WITHOUT_DISTANCE,
        infrared: do_clean,
    };
    recipe
        .supported(session.capabilities())
        .map_err(|e| ScanError::UnsupportedRecipe(e.to_string()))?;

    let frames_to_scan = request.frames.resolve(strip_discovery.frame_count());

    // Pre-validate travel offset for all requested frames within the discovered strip
    let mut adjusted_frame_rects = std::collections::HashMap::new();
    let mut applied_offset_dots = 0i32;
    for &frame_num in &frames_to_scan {
        let frame_idx = frame_num
            .checked_sub(1)
            .ok_or(ScanError::FrameUnavailable(frame_num))?;
        if let Some(&detected_rect) = strip_discovery.frames().get(frame_idx) {
            let (adjusted_rect, offset_dots) = resolve_frame_rect(
                frame_num,
                detected_rect,
                request.offset_mm,
                y_dpi,
                scanner_bounds,
                x_boundary,
                y_boundary,
            )?;
            applied_offset_dots = offset_dots;
            adjusted_frame_rects.insert(frame_num, (adjusted_rect, offset_dots));
        }
    }

    let mut boundary_table = strip_discovery.boundary_table.clone();
    if applied_offset_dots != 0
        && let Some(ref table) = boundary_table
        && let Some(shifted) = adjust_boundary_table(table, applied_offset_dots, y_start, y_end)
    {
        let _ = session.set_boundaries_type2(&shifted);
        boundary_table = Some(shifted);
    }

    let mut features = Vec::new();
    if software_passes > 1 {
        features.push(format!("{software_passes}x multi-pass averaging"));
    } else if hardware_samples > 1 {
        features.push(format!("{hardware_samples}x hardware multi-sampling"));
    }
    if do_clean {
        features.push("OpenICE IR cleaning".into());
    }
    if applied_offset_dots != 0 {
        features.push(format!(
            "offset {:+.2} mm ({:+} dots)",
            request.offset_mm, applied_offset_dots
        ));
    }

    let mut current_session = Some(session);
    let total_discovered_frames = strip_discovery.frame_count();
    let discovered_rects: Vec<Rect> = strip_discovery.frames().to_vec();

    let (frames_acquired, frame_numbers, stop_reason) = execute_streaming_frames(
        &mut current_session,
        &frames_to_scan,
        total_discovered_frames,
        control,
        &mut progress,
        |current_session, frame_num, control, progress| {
            let frame_idx = frame_num - 1;
            let detected_rect = discovered_rects[frame_idx];
            let (rect, offset_dots) = adjusted_frame_rects
                .get(&frame_num)
                .copied()
                .unwrap_or((detected_rect, 0));

            if offset_dots != 0 {
                progress(ScanEvent::FrameOffsetApplied {
                    frame_number: frame_num,
                    offset_mm: request.offset_mm,
                    offset_dots,
                    adjusted_rect: rect,
                });
            }

            progress(ScanEvent::FrameStarted {
                frame_number: frame_num,
                total_frames: total_discovered_frames,
                dpi: scan_dpi,
                is_super_fine: scan_dpi == 2900,
                features: features.clone(),
            });

            let mut final_samples = None;
            let mut final_pass = None;
            let mut final_exposures = None;
            let mut cancelled_during_frame = false;

            for attempt in 1..=MAX_SCAN_ATTEMPTS {
                if control.is_cancel_requested() {
                    cancelled_during_frame = true;
                    break;
                }

                progress(ScanEvent::FrameAttemptStarted {
                    frame_number: frame_num,
                    attempt,
                    max_attempts: MAX_SCAN_ATTEMPTS,
                });

                let mut accum_colors: Vec<Vec<u64>> = Vec::new();
                let mut accum_ir: Option<Vec<u64>> = None;
                let mut current_final_pass = None;
                let mut pass_succeeded = true;
                let mut attempt_exposure = AttemptExposureState::new();

                for pass_idx in 1..=software_passes {
                    // Checkpoint 2: Check cancellation before each pass
                    if control.is_cancel_requested() {
                        cancelled_during_frame = true;
                        pass_succeeded = false;
                        break;
                    }

                    progress(ScanEvent::FramePassStarted {
                        frame_number: frame_num,
                        pass: pass_idx as usize,
                        total_passes: software_passes as usize,
                    });

                    let mut pass_samples = Samples::default();
                    let mut current_phase = None;
                    let mut last_pct = None;
                    let scan_opts = frame::Options {
                        exposures: attempt_exposure.exposures(),
                        lock_white_balance: false,
                        clean: false,
                    };

                    let Some(active_session) = current_session.as_mut() else {
                        pass_succeeded = false;
                        break;
                    };

                    let scan_result = frame::scan_frame_with(
                        active_session,
                        &recipe,
                        rect,
                        scan_opts,
                        &mut pass_samples,
                        |phase, progress_info| {
                            // Checkpoint 3: Check cancellation inside nkscan progress callback
                            check_pass_progress_control(control)?;

                            let mapped_phase = match phase {
                                frame::Phase::Meter(_) => ScanPhase::Metering,
                                frame::Phase::Scan => ScanPhase::Acquiring,
                            };
                            if current_phase != Some(mapped_phase) {
                                if current_phase.is_some() {
                                    progress(ScanEvent::ProgressPhaseEnd);
                                }
                                current_phase = Some(mapped_phase);
                                last_pct = None;
                            }
                            let pct =
                                (progress_info.bytes * 100 / progress_info.total.max(1)) as u8;
                            if last_pct != Some(pct / 10) {
                                last_pct = Some(pct / 10);
                                progress(ScanEvent::Progress {
                                    frame_number: frame_num,
                                    phase: mapped_phase,
                                    percent: pct,
                                    pass: pass_idx as usize,
                                    total_passes: software_passes as usize,
                                });
                            }
                            std::ops::ControlFlow::Continue(())
                        },
                    );

                    if current_phase.is_some() {
                        progress(ScanEvent::ProgressPhaseEnd);
                    }

                    // Checkpoint 4: Classify pass outcome and check cancellation after pass
                    let scanned = match classify_pass_result(scan_result, control) {
                        PassOutcome::Completed(scanned) => scanned,
                        PassOutcome::Cancelled => {
                            cancelled_during_frame = true;
                            pass_succeeded = false;
                            break;
                        }
                        PassOutcome::Failed => {
                            pass_succeeded = false;
                            break;
                        }
                    };

                    if accum_colors.is_empty() {
                        accum_colors = pass_samples
                            .colors
                            .iter()
                            .map(|plane| plane.iter().map(|&v| v as u64).collect())
                            .collect();
                    } else {
                        for (acc_plane, plane) in accum_colors.iter_mut().zip(&pass_samples.colors)
                        {
                            for (acc, &v) in acc_plane.iter_mut().zip(plane) {
                                *acc += v as u64;
                            }
                        }
                    }

                    if let Some(ir) = pass_samples.ir.as_ref() {
                        if let Some(acc_ir) = accum_ir.as_mut() {
                            for (acc, &v) in acc_ir.iter_mut().zip(ir) {
                                *acc += v as u64;
                            }
                        } else {
                            accum_ir = Some(ir.iter().map(|&v| v as u64).collect());
                        }
                    }

                    current_final_pass = Some(scanned.pass.clone());
                    attempt_exposure.record_pass(scanned);
                }

                if cancelled_during_frame || control.is_cancel_requested() {
                    cancelled_during_frame = true;
                    break;
                }

                if pass_succeeded && current_final_pass.is_some() {
                    let n = software_passes as u64;
                    final_samples = Some(Samples {
                        colors: accum_colors
                            .into_iter()
                            .map(|plane| {
                                plane
                                    .into_iter()
                                    .map(|sum| ((sum + n / 2) / n) as u16)
                                    .collect()
                            })
                            .collect(),
                        ir: accum_ir.map(|ir_plane| {
                            ir_plane
                                .into_iter()
                                .map(|sum| ((sum + n / 2) / n) as u16)
                                .collect()
                        }),
                    });
                    final_pass = current_final_pass;
                    final_exposures = attempt_exposure.into_exposures();
                    break;
                }

                if attempt < MAX_SCAN_ATTEMPTS {
                    progress(ScanEvent::UsbResetting {
                        frame_number: frame_num,
                    });
                    drop(current_session.take());
                    sleep(RETRY_DELAY);
                    match refresh_session(scanner, boundary_table.as_ref()) {
                        Ok(new_session) => {
                            *current_session = Some(new_session);
                        }
                        Err(e) => {
                            progress(ScanEvent::UsbReconnectFailed {
                                frame_number: frame_num,
                                error: e.to_string(),
                            });
                            break;
                        }
                    }
                }
            }

            if cancelled_during_frame || control.take_cancel_requested() {
                control.take_cancel_requested();
                return Ok(FrameAcquisitionOutcome::Cancelled {
                    reason: format!("Frame {frame_num} cancelled during scan"),
                });
            }

            let (mut final_samples, final_pass) = match (final_samples, final_pass) {
                (Some(s), Some(p)) => (s, p),
                _ => {
                    return Ok(FrameAcquisitionOutcome::Skipped {
                        reason: "Failed all scan attempts".into(),
                    });
                }
            };

            let mut cleaned_pixels_count = None;
            if do_clean {
                progress(ScanEvent::CleaningStarted {
                    frame_number: frame_num,
                });
                let model = current_session
                    .as_ref()
                    .and_then(|s| s.capabilities().identity.model());
                match clean::clean_frame(&mut final_samples, &final_pass, model) {
                    Ok(cleaned) => {
                        cleaned_pixels_count = Some(cleaned);
                        progress(ScanEvent::CleaningFinished {
                            frame_number: frame_num,
                            cleaned_pixels: cleaned,
                        });
                    }
                    Err(e) => {
                        progress(ScanEvent::CleaningNotice {
                            frame_number: frame_num,
                            message: e.to_string(),
                        });
                    }
                }
            }
            // Release raw infrared channel memory immediately after OpenICE cleaning
            final_samples.ir = None;

            let mut crop_decision = None;
            if request.auto_crop {
                let dots_per_col = f64::from(y_dpi) / f64::from(scan_dpi);
                let edge_inset_columns = (5.0 * f64::from(scan_dpi) / 725.0).round() as usize;
                let crop_options = CropOptions {
                    edge_inset_columns,
                    ..CropOptions::default()
                };
                match CropDecision::from_preview_samples(
                    &final_samples,
                    &final_pass,
                    rect.top,
                    dots_per_col,
                    &crop_options,
                ) {
                    Ok(decision) if decision.accepted => {
                        let width_cols = decision.columns.1 - decision.columns.0;
                        let height_rows = decision.rows.1 - decision.rows.0;
                        let width_mm = width_cols as f64 * 25.4 / f64::from(scan_dpi);
                        let height_mm = height_rows as f64 * 25.4 / f64::from(scan_dpi);
                        let aspect_ratio = width_cols as f64 / height_rows.max(1) as f64;
                        progress(ScanEvent::CropAccepted {
                            frame_number: frame_num,
                            crop: decision.clone(),
                            width_cols,
                            height_rows,
                            width_mm,
                            height_mm,
                            aspect_ratio,
                        });
                        crop_decision = Some(decision);
                    }
                    Ok(_) => {
                        progress(ScanEvent::CropFallback {
                            frame_number: frame_num,
                            reason: CropFallbackReason::Ambiguous,
                        });
                    }
                    Err(e) => {
                        progress(ScanEvent::CropFallback {
                            frame_number: frame_num,
                            reason: CropFallbackReason::Error(e.to_string()),
                        });
                    }
                }
            }

            let scan_metadata = ScanMetadata {
                scanner_model: current_session
                    .as_ref()
                    .and_then(|s| s.capabilities().identity.model().map(|m| format!("{m:?}"))),
                focus_position: None,
                exposures: final_exposures,
                hardware_samples,
                software_passes,
                infrared_cleaned_pixels: cleaned_pixels_count,
            };

            Ok(FrameAcquisitionOutcome::Acquired(Box::new(FrameArtifact {
                frame_number: frame_num,
                total_frames: total_discovered_frames,
                dpi: scan_dpi,
                raw_rect: rect,
                samples: final_samples,
                pass: final_pass,
                crop: crop_decision,
                scan_metadata,
                roll: request.roll.clone(),
            })))
        },
        on_frame,
        |current_session, next_num, progress| {
            progress(ScanEvent::UsbRefreshStarting {
                next_frame_number: next_num,
            });
            drop(current_session.take());
            sleep(INTER_FRAME_DELAY);
            match refresh_session(scanner, boundary_table.as_ref()) {
                Ok(new_session) => {
                    *current_session = Some(new_session);
                    progress(ScanEvent::UsbRefreshReady {
                        next_frame_number: next_num,
                    });
                    true
                }
                Err(e) => {
                    progress(ScanEvent::UsbRefreshRetrying {
                        next_frame_number: next_num,
                        error: e.to_string(),
                    });
                    sleep(RETRY_DELAY);
                    match refresh_session(scanner, boundary_table.as_ref()) {
                        Ok(new_session) => {
                            *current_session = Some(new_session);
                            progress(ScanEvent::UsbRefreshReconnected {
                                next_frame_number: next_num,
                            });
                            true
                        }
                        Err(e) => {
                            progress(ScanEvent::UsbRefreshFailed {
                                next_frame_number: next_num,
                                error: e.to_string(),
                            });
                            false
                        }
                    }
                }
            }
        },
    )?;

    Ok(StripSummary {
        frames_acquired,
        frame_numbers,
        discovery: strip_discovery,
        roll: request.roll.clone(),
        stop_reason,
    })
}

/// Discovers and scans a strip starting directly from the scanner Device.
pub fn scan_strip(
    scanner: &Device,
    request: &ScanRequest,
    progress: impl FnMut(ScanEvent),
) -> Result<StripScanResult, ScanError> {
    let transport = scanner.open().map_err(ScanError::DeviceOpen)?;
    let session = Session::open(transport).map_err(ScanError::SessionStart)?;
    scan_strip_with_session(scanner, session, request, None, progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::types::FrameSelection;

    fn dummy_scanned(red_exposure: u32) -> frame::Scanned {
        let mut exp = nkscan::scan::autoexpose::Exposures::default();
        exp.set(nkscan::protocol::window::Channel::Red, red_exposure);
        frame::Scanned {
            pass: nkscan::scan::pass::Pass {
                layout: nkscan::protocol::image::Layout::single_line(100, 100, vec![1, 2, 3]),
                cooperation: Vec::new(),
                complete: true,
                blocks: 1,
                rows: 100,
                cols: 100,
            },
            exposures: exp,
            cleaned: None,
        }
    }

    #[test]
    fn test_retry_exposure_state_resets_between_attempts() {
        // Attempt 1 starts fresh with no cached exposures
        let mut attempt1 = AttemptExposureState::new();
        assert!(attempt1.exposures().is_none());

        // Attempt 1, Pass 1 meters and records exposure
        attempt1.record_pass(dummy_scanned(1200));
        assert_eq!(
            attempt1
                .exposures()
                .unwrap()
                .get(nkscan::protocol::window::Channel::Red),
            Some(1200)
        );

        // Attempt 1, subsequent pass preserves locked exposure
        assert_eq!(
            attempt1
                .exposures()
                .unwrap()
                .get(nkscan::protocol::window::Channel::Red),
            Some(1200)
        );

        // Attempt 1 fails (e.g. USB transport reset).
        // Attempt 2 starts fresh: must NOT leak attempt 1's exposure state!
        let mut attempt2 = AttemptExposureState::new();
        assert!(
            attempt2.exposures().is_none(),
            "Retry attempt must start with fresh exposure state"
        );

        // Attempt 2, Pass 1 meters fresh exposure (e.g. 1800)
        attempt2.record_pass(dummy_scanned(1800));
        assert_eq!(
            attempt2
                .exposures()
                .unwrap()
                .get(nkscan::protocol::window::Channel::Red),
            Some(1800)
        );

        let final_exp = attempt2.into_exposures().unwrap();
        assert_eq!(
            final_exp.get(nkscan::protocol::window::Channel::Red),
            Some(1800)
        );
    }

    #[test]
    fn test_zero_acquired_artifacts_returns_all_frames_failed() {
        assert!(matches!(
            check_acquired_artifacts(&[]),
            Err(ScanError::AllFramesFailed)
        ));
    }

    #[test]
    fn test_zero_discovered_frames_resolves_to_empty_and_fails() {
        let req = ScanRequest::new(FrameSelection::All, 2900, 1, false, false);
        let frames_to_scan = req.frames.resolve(0);
        assert!(frames_to_scan.is_empty());
        // An empty acquisition list resulting from 0 discovered frames produces AllFramesFailed
        let acquired: Vec<FrameArtifact> = Vec::new();
        assert!(matches!(
            check_acquired_artifacts(&acquired),
            Err(ScanError::AllFramesFailed)
        ));
    }

    #[test]
    fn test_discovered_frames_resolves_all() {
        let req = ScanRequest::new(FrameSelection::All, 2900, 1, false, false);
        let frames_to_scan = req.frames.resolve(6);
        assert_eq!(frames_to_scan, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn test_pass_progress_control_and_classification() {
        let control = ScanControl::default();
        assert_eq!(
            check_pass_progress_control(&control),
            std::ops::ControlFlow::Continue(())
        );

        // Completed pass when not cancelled
        let completed = classify_pass_result(Ok(dummy_scanned(1000)), &control);
        assert!(matches!(completed, PassOutcome::Completed(_)));

        // Incomplete pass when not cancelled -> Failed (eligible for retry)
        let mut incomplete_scan = dummy_scanned(1000);
        incomplete_scan.pass.complete = false;
        let failed = classify_pass_result(Ok(incomplete_scan), &control);
        assert!(matches!(failed, PassOutcome::Failed));

        // nkscan Error::Cancelled -> Cancelled (must NOT retry)
        let nk_cancelled = classify_pass_result(Err(nkscan::error::Error::Cancelled), &control);
        assert!(matches!(nk_cancelled, PassOutcome::Cancelled));

        // Atomic cancel_current flag set -> Break in callback and Cancelled outcome
        control.request_cancel_current();
        assert_eq!(
            check_pass_progress_control(&control),
            std::ops::ControlFlow::Break(())
        );
        let flag_cancelled = classify_pass_result(Ok(dummy_scanned(1000)), &control);
        assert!(matches!(flag_cancelled, PassOutcome::Cancelled));
        assert!(control.take_cancel_requested());
        assert!(!control.is_cancel_requested());
    }

    #[test]
    fn test_resolve_frame_rect_and_boundary_cases() {
        let bounds = crate::frame_position::scanner_bounds(0, 0, 2870, 0, 34656, 4332).unwrap();
        let frame_1 = Rect {
            left: 0,
            right: 2869,
            top: 90,
            bottom: 4259,
        };
        let frame_6 = Rect {
            left: 0,
            right: 2869,
            top: 30387,
            bottom: 34556,
        };

        // Valid +0.5 mm offset on Frame 1
        let (adj, dots) = resolve_frame_rect(1, frame_1, 0.5, 2900, bounds, 2870, 4332).unwrap();
        assert_eq!(dots, 57);
        assert_eq!(adj.top, 147);
        assert_eq!(adj.bottom, 4316);

        // Invalid -1.0 mm offset on Frame 1 (underflows top = 0)
        let err_f1 = resolve_frame_rect(1, frame_1, -1.0, 2900, bounds, 2870, 4332).unwrap_err();
        assert!(matches!(
            err_f1,
            ScanError::InvalidOffset {
                frame_number: 1,
                ..
            }
        ));

        // Invalid +1.0 mm offset on Frame 6 (exceeds bottom = 34656)
        let err_f6 = resolve_frame_rect(6, frame_6, 1.0, 2900, bounds, 2870, 4332).unwrap_err();
        assert!(matches!(
            err_f6,
            ScanError::InvalidOffset {
                frame_number: 6,
                ..
            }
        ));
    }

    #[test]
    fn test_adjust_boundary_table() {
        let perf = nkscan::protocol::data::PerforationInformation {
            perf_number: 8,
            count_switching_flag: false,
            perf_decimal: 1,
            pulse_number: 6,
        };
        let table = BoundaryType2 {
            frames: vec![
                nkscan::protocol::data::FramePosition::new(90, &perf),
                nkscan::protocol::data::FramePosition::new(4380, &perf),
            ],
        };

        let shifted = adjust_boundary_table(&table, 57, 0, 34656).unwrap();
        assert_eq!(shifted.frames[0].top, 147);
        assert_eq!(shifted.frames[1].top, 4437);

        // Shifting frame 1 below 0 returns None
        assert!(adjust_boundary_table(&table, -100, 0, 34656).is_none());
    }

    fn dummy_artifact(frame_number: usize) -> FrameArtifact {
        FrameArtifact {
            frame_number,
            total_frames: 6,
            dpi: 725,
            raw_rect: Rect {
                left: 0,
                right: 2,
                top: 0,
                bottom: 2,
            },
            samples: nkscan::protocol::decode::Samples {
                colors: vec![vec![1000; 4]; 3],
                ir: None,
            },
            pass: nkscan::scan::pass::Pass {
                layout: nkscan::protocol::image::Layout::single_line(2, 2, vec![1]),
                cooperation: Vec::new(),
                complete: true,
                blocks: 1,
                rows: 2,
                cols: 2,
            },
            crop: None,
            scan_metadata: ScanMetadata {
                scanner_model: None,
                focus_position: None,
                exposures: None,
                hardware_samples: 1,
                software_passes: 1,
                infrared_cleaned_pixels: None,
            },
            roll: None,
        }
    }

    #[test]
    fn test_execute_streaming_frames_emits_before_next_frame_and_preserves_sparse_order() {
        let control = ScanControl::default();
        let trace = std::cell::RefCell::new(Vec::new());
        let mut emitted = Vec::new();

        let (count, nums, stop_reason) = execute_streaming_frames(
            &mut (),
            &[2, 4, 6],
            6,
            &control,
            |_event| {},
            |_state, frame_num, _ctrl, _progress| {
                trace.borrow_mut().push(format!("acquire:{frame_num}"));
                Ok(FrameAcquisitionOutcome::Acquired(Box::new(dummy_artifact(
                    frame_num,
                ))))
            },
            |artifact| {
                trace
                    .borrow_mut()
                    .push(format!("emit:{}", artifact.frame_number));
                emitted.push(artifact.frame_number);
                std::ops::ControlFlow::Continue(())
            },
            |_state, next_num, _progress| {
                trace
                    .borrow_mut()
                    .push(format!("refresh_before:{next_num}"));
                true
            },
        )
        .unwrap();

        assert_eq!(count, 3);
        assert_eq!(nums, vec![2, 4, 6]);
        assert_eq!(emitted, vec![2, 4, 6]);
        assert!(stop_reason.is_none());
        assert_eq!(
            *trace.borrow(),
            vec![
                "acquire:2",
                "emit:2",
                "refresh_before:4",
                "acquire:4",
                "emit:4",
                "refresh_before:6",
                "acquire:6",
                "emit:6",
            ]
        );
    }

    #[test]
    fn test_execute_streaming_frames_cancel_during_second_frame_preserves_first() {
        let control = ScanControl::default();
        let mut emitted = Vec::new();

        let (count, nums, stop_reason) = execute_streaming_frames(
            &mut (),
            &[1, 2, 3],
            6,
            &control,
            |_event| {},
            |_state, frame_num, _ctrl, _progress| {
                if frame_num == 2 {
                    return Ok(FrameAcquisitionOutcome::Cancelled {
                        reason: "Frame 2 cancelled mid-pass".into(),
                    });
                }
                Ok(FrameAcquisitionOutcome::Acquired(Box::new(dummy_artifact(
                    frame_num,
                ))))
            },
            |artifact| {
                emitted.push(artifact.frame_number);
                std::ops::ControlFlow::Continue(())
            },
            |_state, _next_num, _progress| true,
        )
        .unwrap();

        assert_eq!(count, 1);
        assert_eq!(nums, vec![1]);
        assert_eq!(emitted, vec![1]);
        assert_eq!(stop_reason.as_deref(), Some("Frame 2 cancelled mid-pass"));
    }

    #[test]
    fn test_execute_streaming_frames_stop_after_current_skips_inter_frame_refresh() {
        let control = ScanControl::default();
        let mut refresh_called = false;
        let mut emitted = Vec::new();

        let (count, nums, stop_reason) = execute_streaming_frames(
            &mut (),
            &[1, 2, 3],
            6,
            &control,
            |_event| {},
            |_state, frame_num, ctrl, _progress| {
                if frame_num == 1 {
                    ctrl.request_stop_after_current();
                }
                Ok(FrameAcquisitionOutcome::Acquired(Box::new(dummy_artifact(
                    frame_num,
                ))))
            },
            |artifact| {
                emitted.push(artifact.frame_number);
                std::ops::ControlFlow::Continue(())
            },
            |_state, _next_num, _progress| {
                refresh_called = true;
                true
            },
        )
        .unwrap();

        assert_eq!(count, 1);
        assert_eq!(nums, vec![1]);
        assert_eq!(emitted, vec![1]);
        assert!(!refresh_called);
        assert_eq!(stop_reason.as_deref(), Some("Stopped after current frame"));
    }

    #[test]
    fn test_compute_clamped_overscan_positions_preserves_strip_when_final_frame_near_y_end() {
        // Real strip layout: starts=[5, 148, 292, 436, 581, 709], length=137 cols, pitch=30 dots
        // raw_len_dots = 137 * 30 = 4110; overscan length = 4110 + 240 = 4350 (clamped to y_limit=4332)
        // Frame 6 raw top = 709 * 30 - 60 = 21210; if y_end = 25500, 21210 + 4332 = 25542 > 25500.
        let starts = [5usize, 148, 292, 436, 581, 709];
        let (length, tops) =
            compute_clamped_overscan_positions(&starts, 137 * 30, 30, 0, 25500, 4332).unwrap();

        assert_eq!(length, 4332);
        assert_eq!(tops.len(), 6);
        assert_eq!(tops[0], 5 * 30 - 60);
        assert_eq!(tops[4], 581 * 30 - 60);
        // Frame 6 is clamped cleanly so top + length == y_end (25500 - 4332 = 21168)
        assert_eq!(tops[5], 25500 - 4332);
        for &top in &tops {
            assert!(top + length <= 25500);
        }
    }
}
