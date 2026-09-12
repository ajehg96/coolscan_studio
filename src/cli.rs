pub const HELP: &str = "Usage: coolscan-studio [OPTIONS]\n\
    --scan           Perform scanning and write cropped image(s).\n\
    --eject          Eject loaded film from the scanner.\n\
    --frame N        Select one frame (numbered 1 to 6); defaults to all frames when scanning.\n\
    --dpi DPI        Scan resolution in DPI (90 to 2900); defaults to 725.\n\
    --samples N      Multi-pass averaging count for noise reduction (1 to 16); defaults to 1.\n\
    --high-fidelity  Ultimate fidelity mode: 2900 DPI, 16x multi-sampling, 16-bit TIFF.\n\
    --clean          Enable infrared dust and scratch removal (OpenICE).\n\
    --tiff           Write uncompressed 16-bit linear RGB TIFF master image.\n\
    --output DIR     Directory to write output images; defaults to current directory.\n\
    --no-auto-crop   Disable automatic border detection and keep raw overscan images.\n\
    --roll PROFILE   Roll profile file path or preset name (e.g. 'pro-image-100').\n\
    --offset-mm MM   Shift along film travel; defaults to 0.\n\
    --help           Show help without opening the scanner.\n\
\n\
With --scan, coolscan-studio runs discovery, acquires frames with safety overscan,\n\
detects frame edges using film chromaticity and gradients, and outputs exact 2:3\n\
aspect ratio images with borders eliminated.\n\
\n\
Without --scan, frame selection runs discovery, prints proposed position, and exits.";

#[derive(Debug, PartialEq)]
pub struct Options {
    pub frame: Option<usize>,
    pub offset_mm: f64,
    pub scan: bool,
    pub eject: bool,
    pub output: Option<String>,
    pub auto_crop: bool,
    pub dpi: Option<u16>,
    pub samples: Option<u8>,
    pub high_fidelity: bool,
    pub clean: bool,
    pub tiff: bool,
    pub roll: Option<String>,
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Options>, String> {
    let mut args = args.into_iter().peekable();
    if args
        .peek()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        args.next();
        return if args.next().is_none() {
            Ok(None)
        } else {
            Err("Use --help alone.".into())
        };
    }
    let mut frame = None;
    let mut offset = None;
    let mut scan = false;
    let mut eject = false;
    let mut output = None;
    let mut auto_crop = true;
    let mut dpi = None;
    let mut samples = None;
    let mut high_fidelity = false;
    let mut clean = false;
    let mut tiff = false;
    let mut roll = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--scan" => {
                scan = true;
            }
            "--eject" => {
                eject = true;
            }
            "--no-auto-crop" => {
                auto_crop = false;
            }
            "--high-fidelity" | "--hq" => {
                high_fidelity = true;
            }
            "--clean" | "--ice" => {
                clean = true;
            }
            "--tiff" | "--16bit" => {
                tiff = true;
            }
            "--roll" if roll.is_none() => {
                let value = args.next().ok_or("Missing roll profile name or path.")?;
                roll = Some(value);
            }
            "--output" if output.is_none() => {
                let dir = args.next().ok_or("Missing output directory.")?;
                output = Some(dir);
            }
            "--dpi" if dpi.is_none() => {
                let value = args.next().ok_or("Missing DPI value.")?;
                let number = value
                    .parse::<u16>()
                    .map_err(|_| "DPI must be a positive integer.")?;
                if !(90..=2900).contains(&number) {
                    return Err("DPI must be between 90 and 2900.".into());
                }
                dpi = Some(number);
            }
            "--samples" if samples.is_none() => {
                let value = args.next().ok_or("Missing samples value.")?;
                let number = value
                    .parse::<u8>()
                    .map_err(|_| "Samples must be an integer between 1 and 16.")?;
                if !(1..=16).contains(&number) {
                    return Err("Samples must be between 1 and 16.".into());
                }
                samples = Some(number);
            }
            "--frame" if frame.is_none() => {
                let value = args.next().ok_or("Missing frame number.")?;
                let number = value
                    .parse::<usize>()
                    .map_err(|_| "Frame must be a positive integer.")?;
                if number == 0 {
                    return Err("Frame numbers start at 1.".into());
                }
                if number > 6 {
                    return Err("Frame number must be between 1 and 6.".into());
                }
                frame = Some(number);
            }
            "--offset-mm" if offset.is_none() => {
                let value = args.next().ok_or("Missing offset in millimetres.")?;
                let mm = value
                    .parse::<f64>()
                    .map_err(|_| "Offset must be a finite number.")?;
                if !mm.is_finite() {
                    return Err("Offset must be a finite number.".into());
                }
                offset = Some(mm);
            }
            _ => return Err(format!("Unknown or repeated argument: {arg}")),
        }
    }
    if offset.is_some() && frame.is_none() {
        return Err("--offset-mm requires --frame.".into());
    }
    Ok(Some(Options {
        frame,
        offset_mm: offset.unwrap_or(0.0),
        scan,
        eject,
        output,
        auto_crop,
        dpi,
        samples,
        high_fidelity,
        clean,
        tiff,
        roll,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse_words(words: &str) -> Result<Option<Options>, String> {
        parse(words.split_whitespace().map(str::to_owned))
    }
    #[test]
    fn accepts_selection_and_signed_offsets() {
        assert_eq!(
            parse_words("--frame 2 --offset-mm +1.7").unwrap(),
            Some(Options {
                frame: Some(2),
                offset_mm: 1.7,
                scan: false,
                eject: false,
                output: None,
                auto_crop: true,
                dpi: None,
                samples: None,
                high_fidelity: false,
                clean: false,
                tiff: false,
                roll: None,
            })
        );
        assert_eq!(
            parse_words("--scan --frame 3 --output ./out --dpi 2900 --samples 16 --clean --tiff")
                .unwrap(),
            Some(Options {
                frame: Some(3),
                offset_mm: 0.0,
                scan: true,
                eject: false,
                output: Some("./out".into()),
                auto_crop: true,
                dpi: Some(2900),
                samples: Some(16),
                high_fidelity: false,
                clean: true,
                tiff: true,
                roll: None,
            })
        );
        assert_eq!(
            parse_words("--scan --high-fidelity")
                .unwrap(),
            Some(Options {
                frame: None,
                offset_mm: 0.0,
                scan: true,
                eject: false,
                output: None,
                auto_crop: true,
                dpi: None,
                samples: None,
                high_fidelity: true,
                clean: false,
                tiff: false,
                roll: None,
            })
        );
        assert_eq!(
            parse_words("--scan --no-auto-crop")
                .unwrap(),
            Some(Options {
                frame: None,
                offset_mm: 0.0,
                scan: true,
                eject: false,
                output: None,
                auto_crop: false,
                dpi: None,
                samples: None,
                high_fidelity: false,
                clean: false,
                tiff: false,
                roll: None,
            })
        );
        assert_eq!(
            parse_words("--eject").unwrap(),
            Some(Options {
                frame: None,
                offset_mm: 0.0,
                scan: false,
                eject: true,
                output: None,
                auto_crop: true,
                dpi: None,
                samples: None,
                high_fidelity: false,
                clean: false,
                tiff: false,
                roll: None,
            })
        );
        assert_eq!(
            parse_words("--scan --roll pro-image-100").unwrap(),
            Some(Options {
                frame: None,
                offset_mm: 0.0,
                scan: true,
                eject: false,
                output: None,
                auto_crop: true,
                dpi: None,
                samples: None,
                high_fidelity: false,
                clean: false,
                tiff: false,
                roll: Some("pro-image-100".into()),
            })
        );
        assert_eq!(
            parse_words("--offset-mm -1.8 --frame 3")
                .unwrap()
                .unwrap()
                .offset_mm,
            -1.8
        );
        assert_eq!(parse_words("--frame 1").unwrap().unwrap().offset_mm, 0.0);
        assert_eq!(parse_words("").unwrap().unwrap().frame, None);
        assert_eq!(parse_words("--help").unwrap(), None);
    }
    #[test]
    fn rejects_invalid_arguments() {
        for input in [
            "--frame 0",
            "--frame 7",
            "--frame -1",
            "--frame",
            "--frame 1.5",
            "--output",
            "--dpi",
            "--dpi 50",
            "--dpi 3000",
            "--dpi abc",
            "--samples",
            "--samples 0",
            "--samples 17",
            "--samples abc",
            "--roll",
            "--offset-mm 1",
            "--frame 1 --offset-mm",
            "--frame 1 --offset-mm NaN",
            "--frame 1 --offset-mm inf",
            "--frame 1 --frame 2",
            "--preview",
            "--help --frame 1",
        ] {
            assert!(parse_words(input).is_err(), "{input}");
        }
    }
}
