//! Offline detector replay. No scanner access.
#[allow(dead_code)]
#[path = "support/boundaries_probe.rs"]
mod probe;
use nkscan::{protocol::decode::Image, scan::boundaries};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 5 {
        return Err("Usage: replay_boundaries RAW ROWS COLS BITS NOMINAL_LENGTH".into());
    }
    let rows: usize = args[1].parse()?;
    let cols: usize = args[2].parse()?;
    let bits = args[3].parse()?;
    let length = args[4].parse()?;
    let bytes = std::fs::read(&args[0])?;
    if bytes.len() % 2 != 0 || rows == 0 || cols == 0 {
        return Err("Invalid raw dimensions or byte count".into());
    }
    let samples: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    if samples.len() != rows * cols * 3 {
        return Err("Incorrect raw sample dimensions".into());
    }
    let image = Image {
        colors: samples.chunks_exact(rows * cols).collect(),
        ir: &[],
        rows,
        cols,
        bits,
    };
    let official = boundaries::detect(&image, length, boundaries::Polarity::Negative);
    let traced = probe::detect(&image, length, probe::Polarity::Negative);
    assert_eq!(official.frames, traced.frames);
    assert_eq!(official.length, traced.length);
    println!("Official: {official:?}");
    println!(
        "Preserve local edges: {:?}",
        probe::preserve_edges(&image, length, probe::Polarity::Negative)
    );
    Ok(())
}
