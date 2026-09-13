//! Offline replay of automatic cropping on saved capture directories.
#[allow(dead_code)]
#[path = "../src/bmp.rs"]
mod bmp;
#[allow(dead_code)]
#[path = "../src/crop.rs"]
mod crop;

use crop::{CropDecision, CropOptions};
use nkscan::protocol::decode::Samples;
use std::{fs, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return Err("Usage: replay_autocrop CAPTURE_DIR [FRAME_NUM]".into());
    }
    let dir = &args[0];
    let frame_num = args
        .get(1)
        .map(|s| s.parse::<usize>())
        .transpose()?
        .unwrap_or(2);

    let root = Path::new(dir);
    let raw_path = if root.join(format!("frame-{frame_num}.raw")).exists() {
        root.join(format!("frame-{frame_num}.raw"))
    } else if root
        .join(format!("frame-{frame_num}-uncropped.raw"))
        .exists()
    {
        root.join(format!("frame-{frame_num}-uncropped.raw"))
    } else {
        return Err(format!("Missing raw file in {dir} for frame {frame_num}").into());
    };

    let meta_path = root.join(format!("frame-{frame_num}.txt"));
    let (rows, cols, channels, top_dot) = if meta_path.exists() {
        let meta = fs::read_to_string(&meta_path)?;
        let r: usize = meta
            .lines()
            .find(|l| l.starts_with("rows="))
            .and_then(|l| l.split_whitespace().find(|w| w.starts_with("rows=")))
            .and_then(|s| s.strip_prefix("rows="))
            .ok_or("Missing rows in metadata")?
            .parse()?;
        let c: usize = meta
            .lines()
            .find(|l| l.starts_with("rows="))
            .and_then(|l| l.split_whitespace().find(|w| w.starts_with("cols=")))
            .and_then(|s| s.strip_prefix("cols="))
            .ok_or("Missing cols in metadata")?
            .parse()?;
        let ch: usize = meta
            .lines()
            .find(|l| l.starts_with("rows="))
            .and_then(|l| l.split_whitespace().find(|w| w.starts_with("channels=")))
            .and_then(|s| s.strip_prefix("channels="))
            .ok_or("Missing channels in metadata")?
            .parse()?;
        let top: u32 = meta
            .lines()
            .find(|l| l.starts_with("rect=Rect"))
            .and_then(|l| l.split("top: ").nth(1))
            .and_then(|s| s.split(',').next())
            .ok_or("Missing top in rect")?
            .parse()?;
        (r, c, ch, top)
    } else {
        // Fallback to reading from uncropped BMP header if available
        let bmp_path = root.join(format!("frame-{frame_num}-uncropped.bmp"));
        let bytes = fs::read(&bmp_path)?;
        let c = u32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]) as usize;
        let r_raw = i32::from_le_bytes([bytes[22], bytes[23], bytes[24], bytes[25]]);
        let r = r_raw.unsigned_abs() as usize;
        // Parse top from decision json or local-registration.txt
        let dec_path = root.join(format!("frame-{frame_num}-decision.json"));
        let top = if dec_path.exists() {
            let dec_str = fs::read_to_string(dec_path)?;
            dec_str
                .lines()
                .find(|l| l.contains("\"travel_dots\":"))
                .and_then(|l| l.split('(').nth(1))
                .and_then(|s| s.split(',').next())
                .and_then(|s| s.trim().parse::<u32>().ok())
                .unwrap_or(0)
        } else {
            0
        };
        (r, c, 3, top)
    };
    let bytes = fs::read(&raw_path)?;
    let expected_bytes = rows * cols * channels * 2;
    if bytes.len() < expected_bytes {
        return Err("Raw file is incomplete".into());
    }

    let mut colors = Vec::with_capacity(channels);
    let plane_samples = rows * cols;
    for c in 0..channels {
        let start = c * plane_samples * 2;
        let mut plane = Vec::with_capacity(plane_samples);
        for i in 0..plane_samples {
            let offset = start + i * 2;
            let sample = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
            plane.push(sample);
        }
        colors.push(plane);
    }
    let samples = Samples { colors, ir: None };

    let pass = nkscan::scan::pass::Pass {
        layout: nkscan::protocol::image::Layout::single_line(
            rows as u32,
            cols as u32,
            (1..=channels as u8).collect(),
        ),
        cooperation: Vec::new(),
        complete: true,
        blocks: 1,
        rows,
        cols,
    };

    let dots_per_col = 2900.0 / 725.0; // 4.0
    let crop_options = CropOptions::default();
    let decision =
        CropDecision::from_preview_samples(&samples, &pass, top_dot, dots_per_col, &crop_options)?;

    println!("=== Replay Autocrop for {} (Frame {}) ===", dir, frame_num);
    println!("  Accepted: {}", decision.accepted);
    println!("  Leading edge: {:?}", decision.leading);
    println!("  Trailing edge: {:?}", decision.trailing);
    println!("  Chosen columns: {:?}", decision.columns);
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
        frame_num,
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
        root.join(format!("frame-{frame_num}-decision.json")),
        &decision_json,
    )?;

    if decision.accepted {
        let (cropped_samples, cropped_pass) =
            bmp::crop_samples(&samples, &pass, decision.rows, decision.columns)?;
        let cropped_bmp = root.join(format!("frame-{frame_num}-cropped.bmp"));
        let _ = fs::remove_file(&cropped_bmp);
        bmp::write_bmp(
            cropped_bmp.to_str().unwrap(),
            &cropped_samples,
            &cropped_pass,
        )?;
        println!("  Saved cropped BMP: {}", cropped_bmp.display());
    } else {
        println!("  Conservative fallback retained full bounds.");
    }

    Ok(())
}
