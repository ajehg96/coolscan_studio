#[allow(dead_code)]
#[path = "../src/bmp.rs"]
mod bmp;
#[allow(dead_code)]
#[path = "../src/crop.rs"]
mod crop;
#[allow(dead_code)]
#[path = "support/boundaries_probe.rs"]
mod probe;

use crop::{CropDecision, CropOptions};
use nkscan::{
    device,
    protocol::{caps::set_window::ColorInterleaving, decode::Samples},
    scan::{boundaries::Polarity, frame, framing, window::Recipe},
    session::Session,
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
};

fn save_raw(path: &str, samples: &Samples) -> std::io::Result<()> {
    let mut file =
        std::io::BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
    for plane in &samples.colors {
        for sample in plane {
            file.write_all(&sample.to_le_bytes())?;
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.is_empty() || args.len() > 3 {
        return Err("Usage: scan_autocrop NEW_DIRECTORY [--frame N] (N between 1 and 6)".into());
    }
    let dir = &args[0];
    let selected_frame = if args.len() >= 3 && args[1] == "--frame" {
        let n = args[2].parse::<usize>()?;
        if !(1..=6).contains(&n) {
            return Err("Frame number must be between 1 and 6".into());
        }
        Some(n)
    } else {
        Some(2) // Default to frame 2 for testing
    };

    let scanners = device::list();
    let scanner = scanners.first().ok_or("No scanner found")?;
    println!("Opening {scanner}");
    let mut session = Session::open(scanner.open()?)?;
    if !session.media_loaded()? {
        return Err("No film loaded in scanner. Please feed the film strip into the adapter.".into());
    }
    fs::create_dir(dir)?;

    println!("Running discovery pass...");
    let mut samples = Samples::default();
    let mut discovery = framing::discover(&mut session, None, Polarity::Negative, &mut samples)?;
    let caps = session.capabilities();
    let pass = discovery.thumbnail.as_ref().ok_or("No thumbnail")?;
    let info = format!(
        "caps={caps:#?}\nframes={:#?}\nlayout={:#?}\nrows={} cols={} channels={}\n",
        discovery.frames,
        pass.layout,
        pass.rows,
        pass.cols,
        samples.colors.len()
    );
    fs::write(format!("{dir}/discovery.txt"), &info)?;
    println!(
        "Discovery found {} frames; rows={} cols={} pitch={} bits={}",
        discovery.frames.len(),
        pass.rows,
        pass.cols,
        pass.layout.line_pitch,
        pass.layout.bits_per_sample
    );

    save_raw(&format!("{dir}/discovery.raw"), &samples)?;
    let mut display = samples.clone();
    display.to_full_scale(pass.layout.bits_per_sample);
    bmp::write_bmp(&format!("{dir}/discovery.bmp"), &display, pass)?;

    if !matches!(
        discovery.table,
        nkscan::protocol::data::FrameTable::BoundaryType2(_)
    ) {
        return Err("Local experiment requires Type2 perforation registration".into());
    }

    // Apply local-edge preservation with verified safe overscan
    let image = nkscan::protocol::decode::Image::new(&pass.layout, &samples)?;
    let pitch = pass.layout.line_pitch;
    let nominal = (36.0 * f64::from(caps.address.y_axis.optical_dpi) / 25.4) as u32 / pitch;
    let found = probe::preserve_edges(&image, nominal as usize, probe::Polarity::Negative);
    let origin = caps.address.y_axis.address_range.start;
    let end = caps.address.y_axis.address_range.last;
    let limit = caps.address.y_axis.boundary;
    let x_start = caps.address.x_axis.address_range.start;
    let x_width = caps.address.x_axis.boundary;

    // Use 60 dots leading overscan and trailing overscan capped strictly at the hardware limit (4332 dots).
    let raw_length = u32::try_from(found.length)?
        .checked_mul(pitch)
        .ok_or("length overflow")?;
    let length = raw_length.saturating_add(240).min(limit);

    if length == 0 || length > limit || found.frames.len() != 6 {
        return Err("Unexpected local frame geometry".into());
    }

    let perfs = session.read_perforations()?;
    let mut entries = Vec::new();
    for col in &found.frames {
        let mut top = origin
            .checked_add(
                u32::try_from(*col)?
                    .checked_mul(pitch)
                    .ok_or("position overflow")?,
            )
            .ok_or("position overflow")?;
        top = top.saturating_sub(60);
        if top.checked_add(length).is_none_or(|v| v > end) {
            return Err("Local frame exceeds travel".into());
        }
        entries.push(nkscan::protocol::data::FramePosition::new(
            top,
            perfs.at(*col).ok_or("Missing perforation registration")?,
        ));
    }

    let table = nkscan::protocol::data::BoundaryType2 { frames: entries };
    discovery.frames = table
        .frames
        .iter()
        .map(|p| p.rect(x_start, x_width, length))
        .collect();
    fs::write(
        format!("{dir}/local-registration.txt"),
        format!("{table:#?}\nframes={:#?}", discovery.frames),
    )?;
    session.set_boundaries_type2(&table)?;

    let target_frame = selected_frame.unwrap_or(2);
    let target_idx = target_frame - 1;
    let detected = discovery
        .frames
        .get(target_idx)
        .copied()
        .ok_or("Selected frame not in discovery")?;

    let recipe = Recipe {
        dpi: 725,
        samples: 1,
        interleaving: ColorInterleaving::LINE_WITHOUT_DISTANCE,
        infrared: false,
    };
    recipe.supported(session.capabilities())?;

    let caps = session.capabilities();
    let y = &caps.address.y_axis;
    let rect = detected;
    if rect.bottom > y.address_range.last || rect.bottom - rect.top > y.boundary {
        return Err("Unsafe preview extent".into());
    }

    println!(
        "\nScanning Frame {} at 725 DPI with overscan: {rect:?}",
        target_frame
    );
    let mut last_progress = None;
    let scanned = frame::scan_frame_with(
        &mut session,
        &recipe,
        rect,
        frame::Options::default(),
        &mut samples,
        |phase, progress| {
            let step = progress.bytes.saturating_mul(4) / progress.total.max(1);
            if last_progress != Some((phase, step)) {
                println!("{phase:?}: {} / {} bytes", progress.bytes, progress.total);
                last_progress = Some((phase, step));
            }
            std::ops::ControlFlow::Continue(())
        },
    )?;

    if !scanned.pass.complete {
        return Err("Incomplete preview scan".into());
    }

    // Save full uncropped overscan acquisition
    let stem_uncropped = format!("{dir}/frame-{target_frame}-uncropped");
    save_raw(&format!("{stem_uncropped}.raw"), &samples)?;
    bmp::write_bmp(&format!("{stem_uncropped}.bmp"), &samples, &scanned.pass)?;
    println!("Saved uncropped overscan image: {stem_uncropped}.bmp");

    // Run automatic crop decision
    let dots_per_col = 2900.0 / 725.0; // 4.0
    let crop_options = CropOptions::default();
    let decision = CropDecision::from_preview_samples(
        &samples,
        &scanned.pass,
        rect.top,
        dots_per_col,
        &crop_options,
    )?;

    println!("\n=== Crop Decision for Frame {} ===", target_frame);
    println!("  Accepted: {}", decision.accepted);
    println!("  Leading edge: {:?}", decision.leading);
    println!("  Trailing edge: {:?}", decision.trailing);
    println!("  Chosen columns: {:?}", decision.columns);
    println!("  Chosen rows: {:?}", decision.rows);
    println!("  Travel dots: {:?}", decision.travel_dots);

    let width_cols = decision.columns.1 - decision.columns.0;
    let width_mm = width_cols as f64 * 25.4 / 725.0;
    let height_rows = decision.rows.1 - decision.rows.0;
    let height_mm = height_rows as f64 * 25.4 / 725.0;
    println!(
        "  Crop dimensions: {}x{} (cols x rows), {:.2} x {:.2} mm (ratio: {:.3})",
        width_cols,
        height_rows,
        width_mm,
        height_mm,
        width_cols as f64 / height_rows.max(1) as f64,
    );

    let decision_json = format!(
        "{{\n  \"frame\": {},\n  \"accepted\": {},\n  \"is_blank\": {},\n  \"columns\": {:?},\n  \"rows\": {:?},\n  \"travel_dots\": {:?},\n  \"width_columns\": {},\n  \"width_mm\": {:.3},\n  \"height_rows\": {},\n  \"height_mm\": {:.3},\n  \"leading\": \"{:?}\",\n  \"trailing\": \"{:?}\"\n}}\n",
        target_frame,
        decision.accepted,
        decision.is_blank,
        decision.columns,
        decision.rows,
        decision.travel_dots,
        width_cols,
        width_mm,
        height_rows,
        height_mm,
        decision.leading,
        decision.trailing,
    );
    fs::write(
        format!("{dir}/frame-{target_frame}-decision.json"),
        &decision_json,
    )?;

    // If accepted, crop the image and write cropped BMP
    if decision.accepted {
        let (cropped_samples, cropped_pass) = bmp::crop_samples(
            &samples,
            &scanned.pass,
            decision.rows,
            decision.columns,
        )?;
        let stem_cropped = format!("{dir}/frame-{target_frame}-cropped");
        save_raw(&format!("{stem_cropped}.raw"), &cropped_samples)?;
        bmp::write_bmp(
            &format!("{stem_cropped}.bmp"),
            &cropped_samples,
            &cropped_pass,
        )?;
        println!("Saved automatically cropped image: {stem_cropped}.bmp");
        println!("Visual verification: compare {stem_uncropped}.bmp with {stem_cropped}.bmp.");
    } else {
        println!("Decision fell back to full overscan bounds (no image trimmed).");
    }

    Ok(())
}
