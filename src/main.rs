use std::{io::Write, path::Path};
use coolscan_studio::{
    cli,
    frame_position::{self, FramePosition},
    output::{self, OutputPolicy},
    processing::RollProfile,
    scanner::{
        discover_strip, dots_to_mm, scan_strip_with_session,
        types::{
            CropFallbackReason, FrameSelection, ScanEvent, ScanPhase, ScanRequest,
        },
    },
};
use nkscan::{device, session::Session};

fn main() {
    let options = match cli::parse(std::env::args().skip(1)) {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{}", cli::HELP);
            return;
        }
        Err(error) => {
            eprintln!("{error}\n\n{}", cli::HELP);
            std::process::exit(2);
        }
    };
    println!("Coolscan Studio");
    println!("---------------");

    let scanners = device::list();

    let Some(scanner) = scanners.first() else {
        println!("No Nikon Coolscan scanners found.");
        return;
    };

    println!("Found: {scanner}");
    let scanner = scanner.clone();

    let transport = match scanner.open() {
        Ok(transport) => transport,
        Err(error) => {
            println!("Failed to open scanner: {error}");
            return;
        }
    };

    let mut session = match Session::open(transport) {
        Ok(session) => session,
        Err(error) => {
            println!("Failed to start scanner session: {error}");
            return;
        }
    };

    println!("Scanner session ready.");

    if options.eject {
        print!("Ejecting film... ");
        let _ = std::io::stdout().flush();
        match session.eject() {
            Ok(true) => println!("done. Film ejected."),
            Ok(false) => println!("notice: scanner or adapter reported no eject support."),
            Err(error) => eprintln!("Failed to eject: {error}"),
        }
        return;
    }

    match session.media_loaded() {
        Ok(false) => {
            println!("No film loaded.");
            return;
        }
        Ok(true) => println!("Film loaded."),
        Err(error) => {
            println!("Could not determine film state: {error}");
            return;
        }
    }

    let discovery = match discover_strip(&mut session, |event| match event {
        ScanEvent::DiscoveryStarted { method } => {
            println!("Frame detection method: {method:?}");
        }
        ScanEvent::DiscoveryCompleted { detected_count } => {
            println!("Detected {detected_count} frames.");
        }
        _ => {}
    }) {
        Ok(d) => d,
        Err(error) => {
            println!("Frame detection failed: {error}");
            return;
        }
    };

    let (x_dpi, y_dpi) = discovery.optical_dpi;

    if options.scan {
        let out_dir = options.output.as_deref().unwrap_or(".");
        let scan_dpi = options.dpi.unwrap_or(if options.high_fidelity { 2900 } else { 725 });
        let do_clean = options.clean || options.high_fidelity;
        let do_tiff = options.tiff || options.high_fidelity;
        let requested_samples = options.samples.unwrap_or(if options.high_fidelity { 16 } else { 1 });

        let frames = match options.frame {
            Some(f) => FrameSelection::Specific(f),
            None => FrameSelection::All,
        };

        let roll_profile = match options.roll.as_deref() {
            Some("pro-image-100" | "proimage100" | "pro-image") => {
                let p = RollProfile::pro_image_100();
                println!("Roll profile: {} ({})", p.name, p.film_stock);
                println!(
                    "  D-min: R={:.4}, G={:.4}, B={:.4}",
                    p.dmin[0], p.dmin[1], p.dmin[2]
                );
                Some(p)
            }
            Some(path) => {
                match RollProfile::load_from_file(Path::new(path)) {
                    Ok(p) => {
                        println!("Roll profile: {} ({})", p.name, p.film_stock);
                        println!(
                            "  D-min: R={:.4}, G={:.4}, B={:.4}",
                            p.dmin[0], p.dmin[1], p.dmin[2]
                        );
                        Some(p)
                    }
                    Err(e) => {
                        eprintln!("Failed to load roll profile from {path}: {e}");
                        std::process::exit(2);
                    }
                }
            }
            None => None,
        };

        let mut request = ScanRequest::new(frames, scan_dpi, requested_samples, do_clean, options.auto_crop);
        if let Some(p) = roll_profile {
            request = request.with_roll(p);
        }
        let output_policy = OutputPolicy::new(out_dir, true, do_tiff);

        let progress = |event: ScanEvent| match event {
            ScanEvent::MultiSamplingConfigured {
                software_passes,
                noise_reduction_db,
                ..
            } => {
                println!(
                    "Multi-sampling: using {software_passes}x software multi-pass averaging (noise reduction: +{noise_reduction_db:.1} dB)."
                );
            }
            ScanEvent::FrameStarted {
                frame_number,
                total_frames,
                dpi,
                is_super_fine,
                features,
            } => {
                let mut display_features = features;
                if do_tiff {
                    display_features.push("16-bit linear TIFF master".into());
                }
                let features_str = if display_features.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", display_features.join(", "))
                };
                println!(
                    "\nScanning Frame {frame_number} of {total_frames} ({dpi} DPI{}){features_str}...",
                    if is_super_fine { " Super Fine" } else { "" }
                );
            }
            ScanEvent::FrameAttemptStarted {
                frame_number,
                attempt,
                max_attempts,
            } => {
                if attempt > 1 {
                    println!("  [USB] Retrying Frame {frame_number} (attempt {attempt} of {max_attempts})...");
                }
            }
            ScanEvent::FramePassStarted {
                pass,
                total_passes,
                ..
            } => {
                if total_passes > 1 {
                    println!("  Pass {pass} of {total_passes}:");
                }
            }
            ScanEvent::Progress {
                phase,
                percent,
                total_passes,
                ..
            } => {
                let indent = if total_passes > 1 { "    " } else { "  " };
                let label = match phase {
                    ScanPhase::Metering => "Focus & Exposure Metering",
                    ScanPhase::Acquiring => "Acquiring Image Data",
                };
                print!("{indent}[{label}] {percent:>3}%\r");
                let _ = std::io::stdout().flush();
            }
            ScanEvent::ProgressPhaseEnd => {
                println!();
            }
            ScanEvent::UsbResetting { frame_number } => {
                println!(
                    "  [USB] Connection issue detected; resetting scanner transport and retrying Frame {frame_number}..."
                );
            }
            ScanEvent::UsbReconnectFailed { error, .. } => {
                eprintln!("  [USB] Reconnect failed: {error}");
            }
            ScanEvent::CleaningStarted { .. } => {
                print!("  Running OpenICE dust & scratch removal... ");
                let _ = std::io::stdout().flush();
            }
            ScanEvent::CleaningFinished { cleaned_pixels, .. } => {
                println!("Cleaned {cleaned_pixels} defect pixels.");
            }
            ScanEvent::CleaningNotice { message, .. } => {
                println!("OpenICE notice: {message}");
            }
            ScanEvent::CropAccepted {
                width_cols,
                height_rows,
                width_mm,
                height_mm,
                aspect_ratio,
                ..
            } => {
                println!(
                    "  Auto-cropped: {width_cols}x{height_rows} pixels ({width_mm:.2} x {height_mm:.2} mm, ratio: {aspect_ratio:.3})"
                );
            }
            ScanEvent::CropFallback { reason, .. } => match reason {
                CropFallbackReason::Ambiguous => {
                    println!("  Frame edges ambiguous; retaining full overscan bounds.");
                }
                CropFallbackReason::Error(e) => {
                    println!("  Crop decision error: {e}; retaining full bounds.");
                }
            },
            ScanEvent::UsbRefreshStarting { .. } => {
                print!("  [USB] Refreshing connection for next frame... ");
                let _ = std::io::stdout().flush();
            }
            ScanEvent::UsbRefreshReady { .. } => {
                println!("ready.");
            }
            ScanEvent::UsbRefreshRetrying { error, .. } => {
                println!("retry in 1s ({error})...");
            }
            ScanEvent::UsbRefreshReconnected { .. } => {
                println!("  [USB] Reconnected successfully.");
            }
            ScanEvent::UsbRefreshFailed { error, .. } => {
                eprintln!("  [USB] Failed to reconnect: {error}");
            }
            ScanEvent::FrameSkipped { frame_number, reason } => {
                eprintln!("Skipping frame {frame_number} due to: {reason}");
            }
            ScanEvent::FrameCompleted { .. } => {}
            _ => {}
        };

        let scan_result = match scan_strip_with_session(
            &scanner,
            session,
            &request,
            Some(discovery),
            progress,
        ) {
            Ok(res) => res,
            Err(e) => {
                eprintln!("Scan error: {e}");
                std::process::exit(2);
            }
        };

        for artifact in &scan_result.frames {
            match output::save_frame_outputs(artifact, &output_policy) {
                Ok(files) => {
                    for f in files {
                        let path_str = f.path.display();
                        match (f.format, f.crop_status) {
                            (output::FileFormat::Bmp, output::SavedCropStatus::Cropped) => {
                                println!("  Saved {path_str}");
                            }
                            (output::FileFormat::Bmp, output::SavedCropStatus::AmbiguousFallback) => {
                                println!("  Saved {path_str} (uncropped fallback)");
                            }
                            (output::FileFormat::Bmp, output::SavedCropStatus::Uncropped) => {
                                println!("  Saved {path_str} (uncropped)");
                            }
                            (output::FileFormat::Tiff, output::SavedCropStatus::Cropped) => {
                                println!("  Saved {path_str} (16-bit linear master)");
                            }
                            (output::FileFormat::Tiff, output::SavedCropStatus::AmbiguousFallback) => {
                                println!("  Saved {path_str} (16-bit master fallback)");
                            }
                            (output::FileFormat::Tiff, output::SavedCropStatus::Uncropped) => {
                                println!("  Saved {path_str} (16-bit master uncropped)");
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("  Failed to write outputs for frame {}: {e}", artifact.frame_number);
                }
            }
        }
        return;
    }

    if let Some(number) = options.frame {
        let Some(&detected) = discovery.frames().get(number - 1) else {
            eprintln!(
                "Frame {number} is unavailable; discovered {} frames.",
                discovery.frame_count()
            );
            std::process::exit(2);
        };
        let mut position = FramePosition::new(detected);
        let capabilities = session.capabilities();
        let x = &capabilities.address.x_axis;
        let y = &capabilities.address.y_axis;
        let Some(right) = frame_position::axis_end(
            x.address_range.start,
            x.address_range.last,
            x.boundary,
        ) else {
            eprintln!("Invalid scanner X limits.");
            std::process::exit(2);
        };
        let Some(bottom) = frame_position::axis_end(
            y.address_range.start,
            y.address_range.last,
            y.boundary,
        ) else {
            eprintln!("Invalid scanner Y limits.");
            std::process::exit(2);
        };
        let bounds = nkscan::protocol::data::Rect {
            left: x.address_range.start,
            right,
            top: y.address_range.start,
            bottom,
        };
        println!("Frame {number}: detected {detected:?}");
        println!("Requested offset: {:+.3} mm", options.offset_mm);
        println!(
            "Scanner address limits: {bounds:?}; maximum dimensions: {} x {} dots",
            x.boundary, y.boundary
        );
        let result = position
            .set_offset_mm(options.offset_mm, y_dpi)
            .and_then(|()| position.adjusted_within(bounds, x.boundary, y.boundary));
        match result {
            Ok(adjusted) => {
                println!(
                    "Applied offset: {:+} dots ({:+.3} mm)",
                    position.offset_dots,
                    f64::from(position.offset_dots) * 25.4 / f64::from(y_dpi)
                );
                println!("Proposed position: {adjusted:?}");
                println!(
                    "Position is within reported limits. No preview scanned or image written."
                );
            }
            Err(error) => {
                eprintln!("Position rejected: {error}");
                std::process::exit(2);
            }
        }
        return;
    }

    let frames: Vec<_> = discovery
        .frames()
        .iter()
        .copied()
        .map(FramePosition::new)
        .collect();
    for (index, position) in frames.iter().enumerate() {
        let frame = &position.detected;
        let width = frame.right - frame.left;
        let height = frame.bottom - frame.top;

        println!(
            "Frame {}: left={:.2} mm, top={:.2} mm, width={:.2} mm, height={:.2} mm \
             (raw: left={}, top={}, right={}, bottom={})",
            index + 1,
            dots_to_mm(frame.left, x_dpi),
            dots_to_mm(frame.top, y_dpi),
            dots_to_mm(width, x_dpi),
            dots_to_mm(height, y_dpi),
            frame.left,
            frame.top,
            frame.right,
            frame.bottom,
        );
        let offset_mm = f64::from(position.offset_dots) * 25.4 / f64::from(y_dpi);
        match position.adjusted() {
            Some(adjusted) => println!(
                "  Offset={offset_mm:+.2} mm; adjusted: left={}, top={}, right={}, bottom={}",
                adjusted.left, adjusted.top, adjusted.right, adjusted.bottom,
            ),
            None => println!("  Offset={offset_mm:+.2} mm; adjusted position is invalid."),
        }
    }

    if let (Some(thumbnail), Some(thumbnail_samples)) = (
        discovery.thumbnail.as_ref(),
        discovery.thumbnail_samples.as_ref(),
    ) {
        match output::save_discovery_thumbnail(thumbnail, thumbnail_samples, Path::new("discovery-strip.bmp")) {
            Ok(()) => println!(
                "Saved discovery-strip.bmp ({} x {} pixels).",
                thumbnail.cols, thumbnail.rows
            ),
            Err(error) => println!("Could not save discovery thumbnail: {error}"),
        }
    }
}
