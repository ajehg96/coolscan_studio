#[allow(dead_code)]
#[path = "../src/bmp.rs"]
mod bmp;
#[allow(dead_code)]
#[path = "support/boundaries_probe.rs"]
mod probe;
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
    if args.len() != 1
        && !(args.len() == 2
            && ["--frame2", "--local-frame2", "--overscan-frame2"].contains(&args[1].as_str()))
    {
        return Err(
            "Usage: capture_strip NEW_DIRECTORY [--frame2|--local-frame2|--overscan-frame2]".into(),
        );
    }
    let dir = &args[0];
    fs::create_dir(dir)?;
    let scanners = device::list();
    let scanner = scanners.first().ok_or("No scanner")?;
    println!("Opening {scanner}");
    let mut session = Session::open(scanner.open()?)?;
    if !session.media_loaded()? {
        return Err("No film loaded".into());
    }
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
        "frames={:?}; rows={} cols={} pitch={} bits={}",
        discovery.frames, pass.rows, pass.cols, pass.layout.line_pitch, pass.layout.bits_per_sample
    );
    save_raw(&format!("{dir}/discovery.raw"), &samples)?;
    let mut display = samples.clone();
    display.to_full_scale(pass.layout.bits_per_sample);
    bmp::write_bmp(&format!("{dir}/discovery.bmp"), &display, pass)?;
    let local = args
        .get(1)
        .is_some_and(|arg| arg == "--local-frame2" || arg == "--overscan-frame2");
    let overscan = args.get(1).is_some_and(|arg| arg == "--overscan-frame2");
    if local {
        if !matches!(
            discovery.table,
            nkscan::protocol::data::FrameTable::BoundaryType2(_)
        ) {
            return Err("Local experiment requires Type2 registration".into());
        }
        let image = nkscan::protocol::decode::Image::new(&pass.layout, &samples)?;
        let pitch = pass.layout.line_pitch;
        let nominal = (36.0 * f64::from(caps.address.y_axis.optical_dpi) / 25.4) as u32 / pitch;
        let found = probe::preserve_edges(&image, nominal as usize, probe::Polarity::Negative);
        let origin = caps.address.y_axis.address_range.start;
        let end = caps.address.y_axis.address_range.last;
        let limit = caps.address.y_axis.boundary;
        let x_start = caps.address.x_axis.address_range.start;
        let x_width = caps.address.x_axis.boundary;
        let mut length = u32::try_from(found.length)?
            .checked_mul(pitch)
            .ok_or("length overflow")?;
        if overscan {
            // Use 60 dots before and trailing overscan capped strictly at limit (4332 dots).
            length = length.saturating_add(240).min(limit);
        }
        if length == 0 || length > limit || found.frames.len() != 6 {
            return Err("Unexpected local frame geometry".into());
        }
        let perfs = session.read_perforations()?;
        let mut entries = Vec::new();
        for col in found.frames {
            let mut top = origin
                .checked_add(
                    u32::try_from(col)?
                        .checked_mul(pitch)
                        .ok_or("position overflow")?,
                )
                .ok_or("position overflow")?;
            if overscan {
                top = top.saturating_sub(60);
            }
            if top.checked_add(length).is_none_or(|v| v > end) {
                return Err("Local frame exceeds travel".into());
            }
            entries.push(nkscan::protocol::data::FramePosition::new(
                top,
                perfs.at(col).ok_or("Missing perforation registration")?,
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
    }
    if args.len() == 2 {
        let recipe = Recipe {
            dpi: 725,
            samples: 1,
            interleaving: ColorInterleaving::LINE_WITHOUT_DISTANCE,
            infrared: false,
        };
        recipe.supported(session.capabilities())?;
        for (i, detected) in discovery.frames.iter().enumerate() {
            if i != 1 {
                continue;
            }
            // Keep the discovery registration intact; shifted origins can select another Type2 entry.
            let caps = session.capabilities();
            let y = &caps.address.y_axis;
            let rect = *detected;
            if rect.bottom > y.address_range.last || rect.bottom - rect.top > y.boundary {
                return Err("Unsafe preview extent".into());
            }
            let windows = recipe.windows(caps, rect)?;
            for window in &windows {
                if window
                    .origin
                    .1
                    .checked_add(window.size.1)
                    .is_none_or(|end| end > y.address_range.last)
                    || window.size.1 > y.boundary
                {
                    return Err("Unsafe scan window".into());
                }
            }
            println!("Preview {}: {rect:?}", i + 1);
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
                return Err("Incomplete preview; not accepted as evidence".into());
            }
            let stem = format!("{dir}/frame-{}", i + 1);
            save_raw(&format!("{stem}.raw"), &samples)?;
            bmp::write_bmp(&format!("{stem}.bmp"), &samples, &scanned.pass)?;
            fs::write(
                format!("{stem}.txt"),
                format!(
                    "rect={rect:?}\nwindows={windows:#?}\nlayout={:#?}\nrows={} cols={} channels={}\n",
                    scanned.pass.layout,
                    scanned.pass.rows,
                    scanned.pass.cols,
                    samples.colors.len()
                ),
            )?;
            println!("Saved frame {}", i + 1);
        }
    }
    Ok(())
}
