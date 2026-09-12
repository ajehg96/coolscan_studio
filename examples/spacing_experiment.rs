//! Known image edges, controlled spacing and sampling. No hardware.
#[allow(dead_code)]
#[path = "support/boundaries_probe.rs"]
mod probe;
use nkscan::{
    protocol::decode::Image,
    scan::boundaries::{self, Polarity},
};
fn main() {
    let mut failed = false;
    for (name, starts) in [
        ("regular", vec![30, 162, 294, 426, 558, 690]),
        ("uneven", vec![30, 165, 294, 429, 558, 690]),
    ] {
        for scale in [1, 4] {
            let cols = 870 * scale;
            let rows = 64;
            let length = 120 * scale;
            let actual: Vec<_> = starts.iter().map(|x| x * scale).collect();
            let mut plane = vec![30000; rows * cols];
            for x in 0..cols {
                if actual
                    .iter()
                    .any(|start| (*start..*start + length).contains(&x))
                {
                    for y in 0..rows {
                        let swing = ((y * 7 + (x / scale) * 3) % 11) as f32 / 11.0 - 0.5;
                        plane[y * cols + x] = (8000.0 * (1.0 + 0.55 * swing)) as u16;
                    }
                }
            }
            let image = Image {
                colors: vec![&plane, &plane, &plane],
                ir: &[],
                rows,
                cols,
                bits: 16,
            };
            let result = if std::env::args().any(|arg| arg == "--preserve-edges") {
                let p = probe::preserve_edges(&image, length, probe::Polarity::Negative);
                boundaries::Detected {
                    frames: p.frames,
                    length: p.length,
                    pitch: p.pitch,
                }
            } else {
                boundaries::detect(&image, length, Polarity::Negative)
            };
            let errors: Vec<_> = result
                .frames
                .iter()
                .zip(&actual)
                .map(|(g, w)| (*g as f64 - *w as f64) / scale as f64)
                .collect();
            println!(
                "{name} scale={scale}: actual={actual:?}, detected={:?}, errors_in_coarse_pixels={errors:?}",
                result.frames
            );
            if result.frames.len() != actual.len() || errors.iter().any(|e| e.abs() > 1.0) {
                failed = true;
            }
        }
    }
    if std::env::args().any(|arg| arg == "--assert-edges") {
        assert!(
            !failed,
            "Detector moved known image boundaries more than one coarse pixel"
        );
    }
}
