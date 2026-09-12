#[allow(dead_code)]
pub mod boundaries;
mod bmp;
use bmp::write_bmp;
mod cli;
pub mod crop;
pub mod tiff;
mod frame_position;

use std::io::Write;
use frame_position::FramePosition;
use nkscan::{
    device,
    protocol::{caps::set_window::ColorInterleaving, decode::Samples},
    scan::{boundaries::Polarity, frame, framing, window::Recipe},
    session::Session,
};

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

    let mut samples = Samples::default();

    let framing = framing::Framing::choose(session.capabilities());
    println!("Frame detection method: {framing:?}");

    match framing::discover(&mut session, None, Polarity::Negative, &mut samples) {
        Ok(discovery) => {
            println!("Detected {} frames.", discovery.frames.len());

            let x_dpi = session.capabilities().address.x_axis.optical_dpi;
            let y_dpi = session.capabilities().address.y_axis.optical_dpi;
            let y_origin = session.capabilities().address.y_axis.address_range.start;
            let y_end = session.capabilities().address.y_axis.address_range.last;
            let y_limit = session.capabilities().address.y_axis.boundary;
            let x_start = session.capabilities().address.x_axis.address_range.start;
            let x_width = session.capabilities().address.x_axis.boundary;

            if options.scan {
                let out_dir = options.output.as_deref().unwrap_or(".");
                if let Err(e) = std::fs::create_dir_all(out_dir) {
                    eprintln!("Failed to create output directory {out_dir}: {e}");
                    std::process::exit(2);
                }

                // Setup overscan boundaries if Type2 perforation registration is supported
                let mut scan_discovery = discovery;
                let boundary_table = setup_overscan_frames(
                    &mut scan_discovery,
                    &mut session,
                    &samples,
                    y_dpi,
                    y_origin,
                    y_end,
                    y_limit,
                    x_start,
                    x_width,
                );

                let scan_dpi = options.dpi.unwrap_or(if options.high_fidelity { 2900 } else { 725 });
                let do_clean = options.clean || options.high_fidelity;
                let do_tiff = options.tiff || options.high_fidelity;

                let supports_multi_reading = session
                    .capabilities()
                    .set_window
                    .mode
                    .contains(nkscan::protocol::caps::set_window::ScanMode::MULTI_READING);

                let requested_samples = options.samples.unwrap_or(if options.high_fidelity { 16 } else { 1 });

                let (hardware_samples, software_passes) = if supports_multi_reading {
                    (requested_samples, 1)
                } else if requested_samples > 1 {
                    (1, requested_samples)
                } else {
                    (1, 1)
                };

                if software_passes > 1 {
                    println!(
                        "Multi-sampling: using {software_passes}x software multi-pass averaging (noise reduction: +{:.1} dB).",
                        10.0 * (software_passes as f64).log10()
                    );
                }

                let recipe = Recipe {
                    dpi: scan_dpi,
                    samples: hardware_samples,
                    interleaving: ColorInterleaving::LINE_WITHOUT_DISTANCE,
                    infrared: do_clean,
                };
                if let Err(e) = recipe.supported(session.capabilities()) {
                    eprintln!("Scan recipe unsupported: {e}");
                    std::process::exit(2);
                }

                let frames_to_scan: Vec<usize> = match options.frame {
                    Some(f) => vec![f],
                    None => (1..=scan_discovery.frames.len()).collect(),
                };

                let mut features = Vec::new();
                if software_passes > 1 {
                    features.push(format!("{software_passes}x multi-pass averaging"));
                } else if hardware_samples > 1 {
                    features.push(format!("{hardware_samples}x hardware multi-sampling"));
                }
                if do_clean {
                    features.push("OpenICE IR cleaning".into());
                }
                if do_tiff {
                    features.push("16-bit linear TIFF master".into());
                }
                let features_str = if features.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", features.join(", "))
                };

                let mut current_session = Some(session);

                for (seq_idx, &frame_num) in frames_to_scan.iter().enumerate() {
                    let frame_idx = frame_num - 1;
                    let Some(&rect) = scan_discovery.frames.get(frame_idx) else {
                        eprintln!("Frame {frame_num} is unavailable.");
                        continue;
                    };

                    println!(
                        "\nScanning Frame {frame_num} of {} ({} DPI{}){}...",
                        scan_discovery.frames.len(),
                        scan_dpi,
                        if scan_dpi == 2900 { " Super Fine" } else { "" },
                        features_str
                    );

                    let mut final_samples = None;
                    let mut final_pass = None;
                    const MAX_ATTEMPTS: usize = 2;

                    for attempt in 1..=MAX_ATTEMPTS {
                        if attempt > 1 {
                            println!("  [USB] Retrying Frame {frame_num} (attempt {attempt} of {MAX_ATTEMPTS})...");
                        }

                        let mut accum_colors: Vec<Vec<u64>> = Vec::new();
                        let mut accum_ir: Option<Vec<u64>> = None;
                        let mut first_scanned = None;
                        let mut current_final_pass = None;
                        let mut pass_succeeded = true;

                        for pass_idx in 1..=software_passes {
                            if software_passes > 1 {
                                println!("  Pass {pass_idx} of {software_passes}:");
                            }
                            let mut pass_samples = Samples::default();
                            let mut current_phase = None;
                            let mut last_pct = None;
                            let scan_opts = frame::Options {
                                exposures: first_scanned.as_ref().map(|s: &frame::Scanned| &s.exposures),
                                lock_white_balance: false,
                                clean: false, // Clean after averaging
                            };

                            let Some(active_session) = current_session.as_mut() else {
                                eprintln!("  Scanner session unavailable.");
                                pass_succeeded = false;
                                break;
                            };

                            let scan_result = frame::scan_frame_with(
                                active_session,
                                &recipe,
                                rect,
                                scan_opts,
                                &mut pass_samples,
                                |phase, progress| {
                                    if current_phase != Some(phase) {
                                        if current_phase.is_some() {
                                            println!();
                                        }
                                        current_phase = Some(phase);
                                        last_pct = None;
                                    }
                                    let pct = progress.bytes * 100 / progress.total.max(1);
                                    if last_pct != Some(pct / 10) {
                                        last_pct = Some(pct / 10);
                                        let indent = if software_passes > 1 { "    " } else { "  " };
                                        let label = match phase {
                                            nkscan::scan::frame::Phase::Meter(_) => "Focus & Exposure Metering",
                                            nkscan::scan::frame::Phase::Scan => "Acquiring Image Data",
                                        };
                                        print!("{indent}[{label}] {pct:>3}%\r");
                                        let _ = std::io::stdout().flush();
                                    }
                                    std::ops::ControlFlow::Continue(())
                                },
                            );
                            println!();

                            let scanned = match scan_result {
                                Ok(scanned) if scanned.pass.complete => scanned,
                                Ok(_) => {
                                    eprintln!("  Scan incomplete for frame {frame_num} (pass {pass_idx}).");
                                    pass_succeeded = false;
                                    break;
                                }
                                Err(e) => {
                                    eprintln!("  Scan error on frame {frame_num} (pass {pass_idx}): {e}");
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
                            if first_scanned.is_none() {
                                first_scanned = Some(scanned);
                            }
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
                            break;
                        }

                        if attempt < MAX_ATTEMPTS {
                            println!("  [USB] Connection issue detected; resetting scanner transport and retrying Frame {frame_num}...");
                            drop(current_session.take());
                            std::thread::sleep(std::time::Duration::from_millis(1000));
                            match refresh_session(&scanner, boundary_table.as_ref()) {
                                Ok(new_session) => {
                                    current_session = Some(new_session);
                                }
                                Err(e) => {
                                    eprintln!("  [USB] Reconnect failed: {e}");
                                    break;
                                }
                            }
                        }
                    }

                    let (mut final_samples, final_pass) = match (final_samples, final_pass) {
                        (Some(s), Some(p)) => (s, p),
                        _ => {
                            eprintln!("Skipping frame {frame_num} due to scan errors.");
                            continue;
                        }
                    };

                    if do_clean {
                        print!("  Running OpenICE dust & scratch removal... ");
                        let _ = std::io::stdout().flush();
                        match nkscan::scan::clean::clean_frame(
                            &mut final_samples,
                            &final_pass,
                            current_session
                                .as_ref()
                                .and_then(|s| s.capabilities().identity.model()),
                        ) {
                            Ok(cleaned_pixels) => {
                                println!("Cleaned {cleaned_pixels} defect pixels.");
                            }
                            Err(e) => {
                                println!("OpenICE notice: {e}");
                            }
                        }
                    }

                    let base_file = format!("{out_dir}/frame-{frame_num}");
                    let bmp_file = format!("{base_file}.bmp");
                    let tiff_file = format!("{base_file}.tif");

                    if options.auto_crop {
                        let dots_per_col = f64::from(y_dpi) / f64::from(scan_dpi);
                        let edge_inset_columns = (5.0 * f64::from(scan_dpi) / 725.0).round() as usize;
                        let crop_options = crop::CropOptions {
                            edge_inset_columns,
                            ..crop::CropOptions::default()
                        };
                        match crop::CropDecision::from_preview_samples(
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
                                println!(
                                    "  Auto-cropped: {width_cols}x{height_rows} pixels ({width_mm:.2} x {height_mm:.2} mm, ratio: {:.3})",
                                    width_cols as f64 / height_rows.max(1) as f64
                                );
                                match bmp::crop_samples(&final_samples, &final_pass, decision.rows, decision.columns) {
                                    Ok((cropped_samples, cropped_pass)) => {
                                        if let Err(e) = write_bmp(&bmp_file, &cropped_samples, &cropped_pass) {
                                            eprintln!("  Failed to write {bmp_file}: {e}");
                                        } else {
                                            println!("  Saved {bmp_file}");
                                        }
                                        if do_tiff {
                                            if let Err(e) = tiff::write_tiff(&tiff_file, &cropped_samples, &cropped_pass, scan_dpi) {
                                                eprintln!("  Failed to write {tiff_file}: {e}");
                                            } else {
                                                println!("  Saved {tiff_file} (16-bit linear master)");
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("  Crop slicing error: {e}");
                                    }
                                }
                            }
                            Ok(_) => {
                                println!("  Frame edges ambiguous; retaining full overscan bounds.");
                                if let Err(e) = write_bmp(&bmp_file, &final_samples, &final_pass) {
                                    eprintln!("  Failed to write {bmp_file}: {e}");
                                } else {
                                    println!("  Saved {bmp_file} (uncropped fallback)");
                                }
                                if do_tiff {
                                    if let Err(e) = tiff::write_tiff(&tiff_file, &final_samples, &final_pass, scan_dpi) {
                                        eprintln!("  Failed to write {tiff_file}: {e}");
                                    } else {
                                        println!("  Saved {tiff_file} (16-bit master fallback)");
                                    }
                                }
                            }
                            Err(e) => {
                                println!("  Crop decision error: {e}; retaining full bounds.");
                                let _ = write_bmp(&bmp_file, &final_samples, &final_pass);
                                if do_tiff {
                                    let _ = tiff::write_tiff(&tiff_file, &final_samples, &final_pass, scan_dpi);
                                }
                            }
                        }
                    } else {
                        if let Err(e) = write_bmp(&bmp_file, &final_samples, &final_pass) {
                            eprintln!("  Failed to write {bmp_file}: {e}");
                        } else {
                            println!("  Saved {bmp_file} (uncropped)");
                        }
                        if do_tiff {
                            if let Err(e) = tiff::write_tiff(&tiff_file, &final_samples, &final_pass, scan_dpi) {
                                eprintln!("  Failed to write {tiff_file}: {e}");
                            } else {
                                println!("  Saved {tiff_file} (16-bit master uncropped)");
                            }
                        }
                    }

                    // Reset USB session between frames to maintain fresh transport state
                    if seq_idx + 1 < frames_to_scan.len() {
                        print!("  [USB] Refreshing connection for next frame... ");
                        let _ = std::io::stdout().flush();
                        drop(current_session.take());
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        match refresh_session(&scanner, boundary_table.as_ref()) {
                            Ok(new_session) => {
                                current_session = Some(new_session);
                                println!("ready.");
                            }
                            Err(e) => {
                                println!("retry in 1s ({e})...");
                                std::thread::sleep(std::time::Duration::from_millis(1000));
                                match refresh_session(&scanner, boundary_table.as_ref()) {
                                    Ok(new_session) => {
                                        current_session = Some(new_session);
                                        println!("  [USB] Reconnected successfully.");
                                    }
                                    Err(e) => {
                                        eprintln!("  [USB] Failed to reconnect: {e}");
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                return;
            }

            if let Some(number) = options.frame {
                let Some(detected) = discovery.frames.get(number - 1).copied() else {
                    eprintln!(
                        "Frame {number} is unavailable; discovered {} frames.",
                        discovery.frames.len()
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
                .frames
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

            if let Some(thumbnail) = discovery.thumbnail.as_ref() {
                let mut thumbnail_samples = samples.clone();
                thumbnail_samples.to_full_scale(thumbnail.layout.bits_per_sample);
                match write_bmp("discovery-strip.bmp", &thumbnail_samples, thumbnail) {
                    Ok(()) => println!(
                        "Saved discovery-strip.bmp ({} x {} pixels).",
                        thumbnail.cols, thumbnail.rows
                    ),
                    Err(error) => println!("Could not save discovery thumbnail: {error}"),
                }
            }
        }
        Err(error) => println!("Frame detection failed: {error}"),
    }
}

fn dots_to_mm(dots: u32, dpi: u16) -> f64 {
    f64::from(dots) * 25.4 / f64::from(dpi)
}

#[allow(clippy::too_many_arguments)]
fn setup_overscan_frames(
    discovery: &mut framing::Discovery,
    session: &mut Session,
    samples: &Samples,
    y_dpi: u16,
    y_origin: u32,
    y_end: u32,
    y_limit: u32,
    x_start: u32,
    x_width: u32,
) -> Option<nkscan::protocol::data::BoundaryType2> {
    if !matches!(
        discovery.table,
        nkscan::protocol::data::FrameTable::BoundaryType2(_)
    ) {
        return None;
    }
    let Some(pass) = discovery.thumbnail.as_ref() else {
        return None;
    };
    let Ok(image) = nkscan::protocol::decode::Image::new(&pass.layout, samples) else {
        return None;
    };
    let pitch = pass.layout.line_pitch;
    let nominal = (36.0 * f64::from(y_dpi) / 25.4) as u32 / pitch;
    let found = boundaries::preserve_edges(&image, nominal as usize, boundaries::Polarity::Negative);

    let Ok(raw_len) = u32::try_from(found.length) else {
        return None;
    };
    let Some(raw_len_dots) = raw_len.checked_mul(pitch) else {
        return None;
    };
    let length = raw_len_dots.saturating_add(240).min(y_limit);
    if length == 0 || found.frames.len() != discovery.frames.len() {
        return None;
    }
    let Ok(perfs) = session.read_perforations() else {
        return None;
    };
    let mut entries = Vec::new();
    for col in &found.frames {
        let top = y_origin
            .saturating_add((*col as u32).saturating_mul(pitch))
            .saturating_sub(60);
        if top.saturating_add(length) > y_end {
            return None;
        }
        let Some(p) = perfs.at(*col) else {
            return None;
        };
        entries.push(nkscan::protocol::data::FramePosition::new(top, p));
    }
    let table = nkscan::protocol::data::BoundaryType2 { frames: entries };
    discovery.frames = table
        .frames
        .iter()
        .map(|p| p.rect(x_start, x_width, length))
        .collect();
    let _ = session.set_boundaries_type2(&table);
    Some(table)
}

fn refresh_session(
    scanner: &device::Device,
    boundary_table: Option<&nkscan::protocol::data::BoundaryType2>,
) -> Result<Session, nkscan::error::Error> {
    let transport = scanner.open()?;
    let mut session = Session::open(transport)?;
    if let Some(table) = boundary_table {
        let _ = session.set_boundaries_type2(table);
    }
    Ok(session)
}
