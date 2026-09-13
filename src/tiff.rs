//! 16-bit uncompressed RGB TIFF exporter.
//!
//! Preserves the full 12-bit / 16-bit linear dynamic range from the Coolscan A/D converter
//! without downsampling to 8-bit. Standard Baseline TIFF 6.0 format compatible with
//! Lightroom, Photoshop, Negative Lab Pro, Darktable, RawTherapee, etc.

use nkscan::protocol::decode::Samples;
use std::{
    fs::File,
    io::{BufWriter, Write},
};

/// Writes planar 16-bit `samples` to an uncompressed 16-bit RGB TIFF file.
pub fn write_tiff(
    path: &str,
    samples: &Samples,
    pass: &nkscan::scan::pass::Pass,
    dpi: u16,
) -> std::io::Result<()> {
    if samples.colors.len() != 3 && samples.colors.len() != 1 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "expected 1 or 3 colour channels, received {}",
                samples.colors.len()
            ),
        ));
    }

    let width = u32::try_from(pass.cols).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "image is too wide for TIFF",
        )
    })?;
    let height = u32::try_from(pass.rows).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "image is too tall for TIFF",
        )
    })?;
    let pixels = pass.rows.checked_mul(pass.cols).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "dimensions overflow")
    })?;

    for plane in &samples.colors {
        if plane.len() < pixels {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "incomplete sample plane",
            ));
        }
    }

    let is_rgb = samples.colors.len() == 3;
    let samples_per_pixel: u16 = if is_rgb { 3 } else { 1 };
    let bytes_per_sample = 2u32; // 16-bit
    let image_data_bytes = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(samples_per_pixel as u32))
        .and_then(|s| s.checked_mul(bytes_per_sample))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "TIFF image data size overflow",
            )
        })?;

    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);

    // TIFF Header: Little-endian (II)
    // 0..2: "II" (0x49, 0x49)
    // 2..4: 42 (0x002A)
    // 4..8: Offset to first IFD (8)
    let ifd_offset: u32 = 8;
    writer.write_all(b"II")?;
    writer.write_all(&42u16.to_le_bytes())?;
    writer.write_all(&ifd_offset.to_le_bytes())?;

    // IFD Directory entries
    // Total entries: 13
    let num_entries: u16 = 13;
    let ifd_size = 2 + (num_entries as u32 * 12) + 4; // count + entries + next_ifd
    let mut extra_data_offset = ifd_offset + ifd_size;

    // Extra data structures:
    // BitsPerSample: if RGB -> 3 * u16 = 6 bytes
    let bits_per_sample_offset = extra_data_offset;
    extra_data_offset += if is_rgb { 6 } else { 0 };

    // XResolution: 2 * u32 = 8 bytes
    let x_res_offset = extra_data_offset;
    extra_data_offset += 8;

    // YResolution: 2 * u32 = 8 bytes
    let y_res_offset = extra_data_offset;
    extra_data_offset += 8;

    // Align image data to 4-byte boundary
    let pixel_data_offset = (extra_data_offset + 3) & !3;

    writer.write_all(&num_entries.to_le_bytes())?;

    // Helper to write a 12-byte IFD entry (Tag, Type, Count, Value/Offset)
    // Types: 1=BYTE, 2=ASCII, 3=SHORT, 4=LONG, 5=RATIONAL
    let mut write_entry =
        |tag: u16, tag_type: u16, count: u32, val_or_off: u32| -> std::io::Result<()> {
            writer.write_all(&tag.to_le_bytes())?;
            writer.write_all(&tag_type.to_le_bytes())?;
            writer.write_all(&count.to_le_bytes())?;
            writer.write_all(&val_or_off.to_le_bytes())?;
            Ok(())
        };

    // TIFF 6.0 specification REQUIRES tags to be strictly sorted in ascending order:
    // Tag 256 (0x0100) ImageWidth
    write_entry(256, 4, 1, width)?;
    // Tag 257 (0x0101) ImageLength
    write_entry(257, 4, 1, height)?;
    // Tag 258 (0x0102) BitsPerSample: 16-bit
    if is_rgb {
        write_entry(258, 3, 3, bits_per_sample_offset)?;
    } else {
        write_entry(258, 3, 1, 16)?;
    }
    // Tag 259 (0x0103) Compression: 1 = uncompressed
    write_entry(259, 3, 1, 1)?;
    // Tag 262 (0x0106) PhotometricInterpretation: 2 = RGB, 1 = BlackIsZero
    write_entry(262, 3, 1, if is_rgb { 2 } else { 1 })?;
    // Tag 273 (0x0111) StripOffsets
    write_entry(273, 4, 1, pixel_data_offset)?;
    // Tag 274 (0x0112) Orientation: 1 = TopLeft
    write_entry(274, 3, 1, 1)?;
    // Tag 277 (0x0115) SamplesPerPixel
    write_entry(277, 3, 1, samples_per_pixel as u32)?;
    // Tag 278 (0x0116) RowsPerStrip: height
    write_entry(278, 4, 1, height)?;
    // Tag 279 (0x0117) StripByteCounts
    write_entry(279, 4, 1, image_data_bytes)?;
    // Tag 282 (0x011A) XResolution (RATIONAL)
    write_entry(282, 5, 1, x_res_offset)?;
    // Tag 283 (0x011B) YResolution (RATIONAL)
    write_entry(283, 5, 1, y_res_offset)?;
    // Tag 296 (0x0128) ResolutionUnit: 2 = Inches
    write_entry(296, 3, 1, 2)?;

    // Next IFD offset: 0 (no more IFDs)
    writer.write_all(&0u32.to_le_bytes())?;

    // Extra data: BitsPerSample [16, 16, 16]
    if is_rgb {
        writer.write_all(&16u16.to_le_bytes())?;
        writer.write_all(&16u16.to_le_bytes())?;
        writer.write_all(&16u16.to_le_bytes())?;
    }

    // XResolution: [dpi as u32, 1u32]
    writer.write_all(&(dpi as u32).to_le_bytes())?;
    writer.write_all(&1u32.to_le_bytes())?;

    // YResolution: [dpi as u32, 1u32]
    writer.write_all(&(dpi as u32).to_le_bytes())?;
    writer.write_all(&1u32.to_le_bytes())?;

    // Padding to pixel_data_offset
    let current_pos = extra_data_offset;
    if pixel_data_offset > current_pos {
        let padding = (pixel_data_offset - current_pos) as usize;
        writer.write_all(&vec![0u8; padding])?;
    }

    // Write pixel samples
    if is_rgb {
        let red = &samples.colors[0];
        let green = &samples.colors[1];
        let blue = &samples.colors[2];
        for row in 0..pass.rows {
            let row_offset = row * pass.cols;
            for col in 0..pass.cols {
                let idx = row_offset + col;
                writer.write_all(&red[idx].to_le_bytes())?;
                writer.write_all(&green[idx].to_le_bytes())?;
                writer.write_all(&blue[idx].to_le_bytes())?;
            }
        }
    } else {
        let gray = &samples.colors[0];
        for row in 0..pass.rows {
            let row_offset = row * pass.cols;
            for col in 0..pass.cols {
                let idx = row_offset + col;
                writer.write_all(&gray[idx].to_le_bytes())?;
            }
        }
    }

    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_pass(rows: usize, cols: usize) -> nkscan::scan::pass::Pass {
        nkscan::scan::pass::Pass {
            layout: nkscan::protocol::image::Layout::single_line(
                rows as u32,
                cols as u32,
                vec![1, 2, 3],
            ),
            cooperation: Vec::new(),
            complete: true,
            blocks: 1,
            rows,
            cols,
        }
    }

    #[test]
    fn write_tiff_creates_valid_header_and_tags() {
        let pass = mock_pass(2, 3);
        let red = vec![1000u16, 2000, 3000, 4000, 5000, 6000];
        let green = vec![1100u16, 2100, 3100, 4100, 5100, 6100];
        let blue = vec![1200u16, 2200, 3200, 4200, 5200, 6200];
        let samples = Samples {
            colors: vec![red, green, blue],
            ir: None,
        };

        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("test_tiff.tif");
        let path_str = path.to_str().unwrap();

        write_tiff(path_str, &samples, &pass, 2900).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        // Magic
        assert_eq!(&bytes[0..2], b"II");
        assert_eq!(u16::from_le_bytes([bytes[2], bytes[3]]), 42);
        // Total size = header(8) + ifd(2 + 13*12 + 4 = 162) + extra(6+8+8 = 22) + padding(0) + data(2*3*3*2 = 36) = 228
        assert_eq!(bytes.len(), 228);

        let _ = std::fs::remove_file(path);
    }
}
