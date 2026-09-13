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
            CropFallbackReason, FrameArtifact, ScanError, ScanEvent, ScanMetadata, ScanPhase,
            ScanRequest, StripDiscovery, StripScanResult,
        },
    },
};

/// Converts scanner dots to millimetres at a given DPI.
pub fn dots_to_mm(dots: u32, dpi: u16) -> f64 {
    f64::from(dots) * 25.4 / f64::from(dpi)
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

    let raw_len = u32::try_from(found.length).ok()?;
    let raw_len_dots = raw_len.checked_mul(pitch)?;
    let length = raw_len_dots.saturating_add(240).min(y_limit);
    if length == 0 || found.frames.len() != discovery.frames.len() {
        return None;
    }
    let perfs = session.read_perforations().ok()?;
    let mut entries = Vec::new();
    for col in &found.frames {
        let top = y_origin
            .saturating_add((*col as u32).saturating_mul(pitch))
            .saturating_sub(60);
        if top.saturating_add(length) > y_end {
            return None;
        }
        let p = perfs.at(*col)?;
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

    let raw_len = u32::try_from(found.length).ok()?;
    let raw_len_dots = raw_len.checked_mul(pitch)?;
    let length = raw_len_dots.saturating_add(240).min(y_limit);
    if length == 0 || found.frames.len() != strip.detected_frames.len() {
        return None;
    }
    let perfs = session.read_perforations().ok()?;
    let mut entries = Vec::new();
    for col in &found.frames {
        let top = y_origin
            .saturating_add((*col as u32).saturating_mul(pitch))
            .saturating_sub(60);
        if top.saturating_add(length) > y_end {
            return None;
        }
        let p = perfs.at(*col)?;
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

/// Executes a complete strip scan using an already established scanner session.
pub fn scan_strip_with_session(
    scanner: &Device,
    mut session: Session,
    request: &ScanRequest,
    pre_discovery: Option<StripDiscovery>,
    mut progress: impl FnMut(ScanEvent),
) -> Result<StripScanResult, ScanError> {
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

    let mut features = Vec::new();
    if software_passes > 1 {
        features.push(format!("{software_passes}x multi-pass averaging"));
    } else if hardware_samples > 1 {
        features.push(format!("{hardware_samples}x hardware multi-sampling"));
    }
    if do_clean {
        features.push("OpenICE IR cleaning".into());
    }

    let boundary_table = strip_discovery.boundary_table.clone();
    let mut current_session = Some(session);
    let mut acquired_artifacts = Vec::new();

    for (seq_idx, &frame_num) in frames_to_scan.iter().enumerate() {
        let frame_idx = frame_num
            .checked_sub(1)
            .ok_or(ScanError::FrameUnavailable(frame_num))?;
        let Some(&rect) = strip_discovery.frames().get(frame_idx) else {
            progress(ScanEvent::FrameSkipped {
                frame_number: frame_num,
                reason: format!("Frame {frame_num} is outside discovered bounds"),
            });
            continue;
        };

        progress(ScanEvent::FrameStarted {
            frame_number: frame_num,
            total_frames: strip_discovery.frame_count(),
            dpi: scan_dpi,
            is_super_fine: scan_dpi == 2900,
            features: features.clone(),
        });

        let mut final_samples = None;
        let mut final_pass = None;
        let mut final_exposures = None;

        for attempt in 1..=MAX_SCAN_ATTEMPTS {
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
                        let pct = (progress_info.bytes * 100 / progress_info.total.max(1)) as u8;
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

                let scanned = match scan_result {
                    Ok(scanned) if scanned.pass.complete => scanned,
                    Ok(_) => {
                        pass_succeeded = false;
                        break;
                    }
                    Err(_) => {
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
                    for (acc_plane, plane) in accum_colors.iter_mut().zip(&pass_samples.colors) {
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
                        current_session = Some(new_session);
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

        let (mut final_samples, final_pass) = match (final_samples, final_pass) {
            (Some(s), Some(p)) => (s, p),
            _ => {
                progress(ScanEvent::FrameSkipped {
                    frame_number: frame_num,
                    reason: "Failed all scan attempts".into(),
                });
                continue;
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

        let artifact = FrameArtifact {
            frame_number: frame_num,
            total_frames: strip_discovery.frame_count(),
            dpi: scan_dpi,
            raw_rect: rect,
            samples: final_samples,
            pass: final_pass,
            crop: crop_decision,
            scan_metadata,
            roll: request.roll.clone(),
        };

        progress(ScanEvent::FrameCompleted {
            frame_number: frame_num,
        });

        acquired_artifacts.push(artifact);

        // Reset USB session between frames to maintain fresh transport state
        if seq_idx + 1 < frames_to_scan.len() {
            let next_num = frames_to_scan[seq_idx + 1];
            progress(ScanEvent::UsbRefreshStarting {
                next_frame_number: next_num,
            });
            drop(current_session.take());
            sleep(INTER_FRAME_DELAY);
            match refresh_session(scanner, boundary_table.as_ref()) {
                Ok(new_session) => {
                    current_session = Some(new_session);
                    progress(ScanEvent::UsbRefreshReady {
                        next_frame_number: next_num,
                    });
                }
                Err(e) => {
                    progress(ScanEvent::UsbRefreshRetrying {
                        next_frame_number: next_num,
                        error: e.to_string(),
                    });
                    sleep(RETRY_DELAY);
                    match refresh_session(scanner, boundary_table.as_ref()) {
                        Ok(new_session) => {
                            current_session = Some(new_session);
                            progress(ScanEvent::UsbRefreshReconnected {
                                next_frame_number: next_num,
                            });
                        }
                        Err(e) => {
                            progress(ScanEvent::UsbRefreshFailed {
                                next_frame_number: next_num,
                                error: e.to_string(),
                            });
                            break;
                        }
                    }
                }
            }
        }
    }

    check_acquired_artifacts(&acquired_artifacts)?;

    Ok(StripScanResult {
        frames: acquired_artifacts,
        discovery: strip_discovery,
        roll: request.roll.clone(),
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
}
