use nkscan::protocol::decode::Samples;
use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
};
pub fn write_bmp(
    path: &str,
    samples: &Samples,
    pass: &nkscan::scan::pass::Pass,
) -> std::io::Result<()> {
    if !matches!(samples.colors.len(), 1 | 3) {
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
            "preview is too wide for BMP",
        )
    })?;
    let height = u32::try_from(pass.rows).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "preview is too tall for BMP",
        )
    })?;
    let pixels = pass.rows.checked_mul(pass.cols).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "preview dimensions overflow",
        )
    })?;
    if samples.colors.iter().any(|plane| plane.len() < pixels) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "scanner returned an incomplete colour plane",
        ));
    }

    let row_bytes = width.checked_mul(3).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "BMP row size overflow")
    })?;
    let padding = (4 - row_bytes % 4) % 4;
    let image_bytes = (row_bytes + padding).checked_mul(height).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "BMP image size overflow")
    })?;
    let file_bytes = 54u32.checked_add(image_bytes).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "BMP file size overflow")
    })?;

    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    let mut file = BufWriter::new(file);

    file.write_all(b"BM")?;
    file.write_all(&file_bytes.to_le_bytes())?;
    file.write_all(&[0; 4])?;
    file.write_all(&54u32.to_le_bytes())?;
    file.write_all(&40u32.to_le_bytes())?;
    file.write_all(&(width as i32).to_le_bytes())?;
    file.write_all(&(-(height as i32)).to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&24u16.to_le_bytes())?;
    file.write_all(&0u32.to_le_bytes())?;
    file.write_all(&image_bytes.to_le_bytes())?;
    file.write_all(&[0; 16])?;

    for row in 0..pass.rows {
        for column in 0..pass.cols {
            let pixel = row * pass.cols + column;
            match samples.colors.as_slice() {
                [gray] => {
                    let value = (gray[pixel] >> 8) as u8;
                    file.write_all(&[value, value, value])?;
                }
                [red, green, blue] => file.write_all(&[
                    (blue[pixel] >> 8) as u8,
                    (green[pixel] >> 8) as u8,
                    (red[pixel] >> 8) as u8,
                ])?,
                _ => unreachable!("channel count checked above"),
            }
        }
        file.write_all(&[0; 3][..padding as usize])?;
    }

    Ok(())
}

/// Crops planar samples along rows and columns from `row_range` and `col_range`.
#[allow(dead_code)]
pub fn crop_samples(
    samples: &Samples,
    pass: &nkscan::scan::pass::Pass,
    row_range: (usize, usize),
    col_range: (usize, usize),
) -> Result<(Samples, nkscan::scan::pass::Pass), &'static str> {
    let (start_row, end_row) = row_range;
    let (start_col, end_col) = col_range;
    if start_row >= end_row || end_row > pass.rows {
        return Err("Invalid row crop range");
    }
    if start_col >= end_col || end_col > pass.cols {
        return Err("Invalid column crop range");
    }
    let new_rows = end_row - start_row;
    let new_cols = end_col - start_col;
    let mut cropped_colors = Vec::with_capacity(samples.colors.len());
    for plane in &samples.colors {
        if plane.len() < pass.rows * pass.cols {
            return Err("Incomplete color plane");
        }
        let mut new_plane = Vec::with_capacity(new_rows * new_cols);
        for row in start_row..end_row {
            let row_offset = row * pass.cols;
            new_plane.extend_from_slice(&plane[row_offset + start_col..row_offset + end_col]);
        }
        cropped_colors.push(new_plane);
    }
    let mut new_pass = pass.clone();
    new_pass.rows = new_rows;
    new_pass.cols = new_cols;
    Ok((
        Samples {
            colors: cropped_colors,
            ir: samples.ir.clone(),
        },
        new_pass,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_pass(rows: usize, cols: usize) -> nkscan::scan::pass::Pass {
        nkscan::scan::pass::Pass {
            layout: nkscan::protocol::image::Layout::single_line(rows as u32, cols as u32, vec![1]),
            cooperation: Vec::new(),
            complete: true,
            blocks: 1,
            rows,
            cols,
        }
    }

    #[test]
    fn crop_samples_slices_rows_and_columns_correctly() {
        // 3 rows, 5 columns, 1 channel:
        // row 0: [10, 20, 30, 40, 50]
        // row 1: [60, 70, 80, 90, 100]
        // row 2: [110, 120, 130, 140, 150]
        let channel = vec![
            10, 20, 30, 40, 50,
            60, 70, 80, 90, 100,
            110, 120, 130, 140, 150,
        ];
        let samples = Samples {
            colors: vec![channel],
            ir: None,
        };
        let pass = mock_pass(3, 5);

        // Crop rows 1..3 and columns 1..4 (rows 1,2; cols 1,2,3 -> 2 rows x 3 cols):
        // row 1: [70, 80, 90]
        // row 2: [120, 130, 140]
        let (cropped, new_pass) = crop_samples(&samples, &pass, (1, 3), (1, 4)).unwrap();
        assert_eq!(new_pass.rows, 2);
        assert_eq!(new_pass.cols, 3);
        assert_eq!(cropped.colors[0], vec![70, 80, 90, 120, 130, 140]);
    }

    #[test]
    fn crop_samples_rejects_out_of_bounds() {
        let samples = Samples {
            colors: vec![vec![0; 10]],
            ir: None,
        };
        let pass = mock_pass(2, 5);

        assert!(crop_samples(&samples, &pass, (0, 2), (4, 3)).is_err());
        assert!(crop_samples(&samples, &pass, (0, 2), (0, 6)).is_err());
        assert!(crop_samples(&samples, &pass, (0, 2), (2, 2)).is_err());
        assert!(crop_samples(&samples, &pass, (2, 1), (0, 3)).is_err());
        assert!(crop_samples(&samples, &pass, (0, 3), (0, 3)).is_err());
    }
}
