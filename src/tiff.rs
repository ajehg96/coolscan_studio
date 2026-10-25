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
    write_tiff_with_icc(path, samples, pass, dpi, None)
}

/// Writes planar 16-bit `samples` to an uncompressed 16-bit RGB TIFF file,
/// optionally embedding an ICC profile in Tag 34675 (`InterColorProfile`).
pub fn write_tiff_with_icc(
    path: &str,
    samples: &Samples,
    pass: &nkscan::scan::pass::Pass,
    dpi: u16,
    icc_profile: Option<&[u8]>,
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

    let icc_bytes = icc_profile.filter(|b| !b.is_empty());
    let icc_len = match icc_bytes {
        Some(b) => u32::try_from(b.len()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "ICC profile is too large for TIFF",
            )
        })?,
        None => 0,
    };

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

    // IFD Directory entries: 13 standard + 1 optional InterColorProfile (Tag 34675)
    let num_entries: u16 = if icc_bytes.is_some() { 14 } else { 13 };
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

    // Optional ICC profile (Tag 34675, UNDEFINED = 7)
    let icc_offset = extra_data_offset;
    extra_data_offset += icc_len;

    // Align image data to 4-byte boundary
    let pixel_data_offset = (extra_data_offset + 3) & !3;

    writer.write_all(&num_entries.to_le_bytes())?;

    // Helper to write a 12-byte IFD entry (Tag, Type, Count, Value/Offset)
    // Types: 1=BYTE, 2=ASCII, 3=SHORT, 4=LONG, 5=RATIONAL, 7=UNDEFINED
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
    // Tag 34675 (0x8773) InterColorProfile (UNDEFINED = 7)
    if icc_bytes.is_some() {
        write_entry(34675, 7, icc_len, icc_offset)?;
    }

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

    // Optional ICC profile data
    if let Some(icc) = icc_bytes {
        writer.write_all(icc)?;
    }

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

/// Parsed representation of a 16-bit uncompressed Baseline TIFF 6.0 file.
#[derive(Debug, Clone)]
pub struct ParsedTiff {
    pub samples: Samples,
    pub pass: nkscan::scan::pass::Pass,
    pub dpi: u16,
    pub icc_profile: Option<Vec<u8>>,
}

/// Reads an uncompressed 16-bit RGB or grayscale Baseline TIFF 6.0 file into planar `Samples`.
pub fn read_tiff(path: &std::path::Path) -> std::io::Result<ParsedTiff> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 16 || &bytes[0..2] != b"II" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Invalid or unsupported TIFF header (expected little-endian II)",
        ));
    }
    let magic = u16::from_le_bytes([bytes[2], bytes[3]]);
    if magic != 42 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Invalid TIFF magic number: {magic}"),
        ));
    }
    let ifd_offset = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if ifd_offset + 2 > bytes.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "IFD offset out of bounds",
        ));
    }
    let num_entries = u16::from_le_bytes([bytes[ifd_offset], bytes[ifd_offset + 1]]) as usize;
    if ifd_offset + 2 + num_entries * 12 > bytes.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "IFD directory entries truncated",
        ));
    }

    let mut width: Option<usize> = None;
    let mut height: Option<usize> = None;
    let mut strip_offset: Option<usize> = None;
    let mut samples_per_pixel: usize = 3;
    let mut dpi: u16 = 725;
    let mut icc_profile: Option<Vec<u8>> = None;

    for i in 0..num_entries {
        let off = ifd_offset + 2 + i * 12;
        let tag = u16::from_le_bytes([bytes[off], bytes[off + 1]]);
        let tag_type = u16::from_le_bytes([bytes[off + 2], bytes[off + 3]]);
        let count = u32::from_le_bytes([
            bytes[off + 4],
            bytes[off + 5],
            bytes[off + 6],
            bytes[off + 7],
        ]) as usize;
        let val_or_off = u32::from_le_bytes([
            bytes[off + 8],
            bytes[off + 9],
            bytes[off + 10],
            bytes[off + 11],
        ]) as usize;

        let scalar_val = if tag_type == 3 {
            u16::from_le_bytes([bytes[off + 8], bytes[off + 9]]) as usize
        } else {
            val_or_off
        };

        match tag {
            256 => width = Some(scalar_val),
            257 => height = Some(scalar_val),
            273 => strip_offset = Some(scalar_val),
            277 => samples_per_pixel = scalar_val,
            282 => {
                if val_or_off + 8 <= bytes.len() {
                    let num = u32::from_le_bytes([
                        bytes[val_or_off],
                        bytes[val_or_off + 1],
                        bytes[val_or_off + 2],
                        bytes[val_or_off + 3],
                    ]);
                    let den = u32::from_le_bytes([
                        bytes[val_or_off + 4],
                        bytes[val_or_off + 5],
                        bytes[val_or_off + 6],
                        bytes[val_or_off + 7],
                    ]);
                    if let Some(q) = num.checked_div(den) {
                        dpi = q as u16;
                    }
                }
            }
            34675 if val_or_off + count <= bytes.len() => {
                icc_profile = Some(bytes[val_or_off..val_or_off + count].to_vec());
            }
            _ => {}
        }
    }

    let w = width.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing TIFF ImageWidth")
    })?;
    let h = height.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing TIFF ImageLength")
    })?;
    let data_off = strip_offset.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing TIFF StripOffsets")
    })?;

    if samples_per_pixel != 1 && samples_per_pixel != 3 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Unsupported SamplesPerPixel: {samples_per_pixel}"),
        ));
    }

    let total_pixels = w * h;
    let data_bytes = total_pixels * samples_per_pixel * 2;
    if data_off + data_bytes > bytes.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "TIFF pixel data truncated",
        ));
    }

    let raw_data = &bytes[data_off..data_off + data_bytes];
    let colors = if samples_per_pixel == 3 {
        let mut r = Vec::with_capacity(total_pixels);
        let mut g = Vec::with_capacity(total_pixels);
        let mut b = Vec::with_capacity(total_pixels);
        for px in raw_data.as_chunks::<6>().0 {
            r.push(u16::from_le_bytes([px[0], px[1]]));
            g.push(u16::from_le_bytes([px[2], px[3]]));
            b.push(u16::from_le_bytes([px[4], px[5]]));
        }
        vec![r, g, b]
    } else {
        let mut gray = Vec::with_capacity(total_pixels);
        for px in raw_data.as_chunks::<2>().0 {
            gray.push(u16::from_le_bytes([px[0], px[1]]));
        }
        vec![gray]
    };

    let channels = if samples_per_pixel == 3 {
        vec![1, 2, 3]
    } else {
        vec![1]
    };
    let pass = nkscan::scan::pass::Pass {
        layout: nkscan::protocol::image::Layout::single_line(h as u32, w as u32, channels),
        cooperation: Vec::new(),
        complete: true,
        blocks: 1,
        rows: h,
        cols: w,
    };

    Ok(ParsedTiff {
        samples: Samples { colors, ir: None },
        pass,
        dpi,
        icc_profile,
    })
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
