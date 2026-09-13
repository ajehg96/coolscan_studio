pub const HELP: &str = "Usage: coolscan-studio [OPTIONS]\n\
    --gui            Launch interactive desktop GUI review and scan studio.\n\
    --mock           Run GUI in simulation mode with 6 canned mock frames.\n\
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
    --diagnose       Run system environment, USB driver, and Darktable diagnostics.\n\
    --version, -V    Show program version.\n\
    --help           Show help without opening the scanner.\n\
\n\
With --gui, coolscan-studio starts the Darktable-integrated graphical review studio.\n\
With --scan, coolscan-studio runs discovery, acquires frames with safety overscan,\n\
detects frame edges using film chromaticity and gradients, and outputs exact 2:3\n\
aspect ratio images with borders eliminated.\n\
\n\
Without flags, coolscan-studio launches the GUI studio if a display is available.";

#[derive(Debug, Clone, PartialEq)]
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
    pub gui: bool,
    pub mock: bool,
    pub diagnose: bool,
    pub version: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            frame: None,
            offset_mm: 0.0,
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
            gui: false,
            mock: false,
            diagnose: false,
            version: false,
        }
    }
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
    let mut options = Options::default();
    let mut offset_specified = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--gui" => {
                options.gui = true;
            }
            "--mock" => {
                options.mock = true;
                options.gui = true;
            }
            "--diagnose" => {
                options.diagnose = true;
            }
            "--version" | "-V" => {
                options.version = true;
            }
            "--scan" => {
                options.scan = true;
            }
            "--eject" => {
                options.eject = true;
            }
            "--no-auto-crop" => {
                options.auto_crop = false;
            }
            "--high-fidelity" | "--hq" => {
                options.high_fidelity = true;
            }
            "--clean" | "--ice" => {
                options.clean = true;
            }
            "--tiff" | "--16bit" => {
                options.tiff = true;
            }
            "--roll" if options.roll.is_none() => {
                let value = args.next().ok_or("Missing roll profile name or path.")?;
                options.roll = Some(value);
            }
            "--output" if options.output.is_none() => {
                let dir = args.next().ok_or("Missing output directory.")?;
                options.output = Some(dir);
            }
            "--dpi" if options.dpi.is_none() => {
                let value = args.next().ok_or("Missing DPI value.")?;
                let number = value
                    .parse::<u16>()
                    .map_err(|_| "DPI must be a positive integer.")?;
                if !(90..=2900).contains(&number) {
                    return Err("DPI must be between 90 and 2900.".into());
                }
                options.dpi = Some(number);
            }
            "--samples" if options.samples.is_none() => {
                let value = args.next().ok_or("Missing samples value.")?;
                let number = value
                    .parse::<u8>()
                    .map_err(|_| "Samples must be an integer between 1 and 16.")?;
                if !(1..=16).contains(&number) {
                    return Err("Samples must be between 1 and 16.".into());
                }
                options.samples = Some(number);
            }
            "--frame" if options.frame.is_none() => {
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
                options.frame = Some(number);
            }
            "--offset-mm" if !offset_specified => {
                let value = args.next().ok_or("Missing offset in millimetres.")?;
                let mm = value
                    .parse::<f64>()
                    .map_err(|_| "Offset must be a finite number.")?;
                if !mm.is_finite() {
                    return Err("Offset must be a finite number.".into());
                }
                options.offset_mm = mm;
                offset_specified = true;
            }
            _ => return Err(format!("Unknown or repeated argument: {arg}")),
        }
    }
    if offset_specified && options.frame.is_none() {
        return Err("--offset-mm requires --frame.".into());
    }
    Ok(Some(options))
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
                ..Options::default()
            })
        );
        assert_eq!(
            parse_words("--scan --frame 3 --output ./out --dpi 2900 --samples 16 --clean --tiff")
                .unwrap(),
            Some(Options {
                frame: Some(3),
                offset_mm: 0.0,
                scan: true,
                output: Some("./out".into()),
                dpi: Some(2900),
                samples: Some(16),
                clean: true,
                tiff: true,
                ..Options::default()
            })
        );
        assert_eq!(
            parse_words("--scan --high-fidelity").unwrap(),
            Some(Options {
                scan: true,
                high_fidelity: true,
                ..Options::default()
            })
        );
        assert_eq!(
            parse_words("--scan --no-auto-crop").unwrap(),
            Some(Options {
                scan: true,
                auto_crop: false,
                ..Options::default()
            })
        );
        assert_eq!(
            parse_words("--eject").unwrap(),
            Some(Options {
                eject: true,
                ..Options::default()
            })
        );
        assert_eq!(
            parse_words("--scan --roll pro-image-100").unwrap(),
            Some(Options {
                scan: true,
                roll: Some("pro-image-100".into()),
                ..Options::default()
            })
        );
        assert_eq!(
            parse_words("--gui").unwrap(),
            Some(Options {
                gui: true,
                ..Options::default()
            })
        );
        assert_eq!(
            parse_words("--mock").unwrap(),
            Some(Options {
                gui: true,
                mock: true,
                ..Options::default()
            })
        );
        assert_eq!(
            parse_words("--offset-mm -1.8 --frame 3")
                .unwrap()
                .unwrap()
                .offset_mm,
            -1.8
        );
        assert_eq!(
            parse_words("--diagnose").unwrap(),
            Some(Options {
                diagnose: true,
                ..Options::default()
            })
        );
        assert_eq!(
            parse_words("--version").unwrap(),
            Some(Options {
                version: true,
                ..Options::default()
            })
        );
        assert_eq!(
            parse_words("-V").unwrap(),
            Some(Options {
                version: true,
                ..Options::default()
            })
        );
        assert_eq!(parse_words("--frame 1").unwrap().unwrap().offset_mm, 0.0);
        assert_eq!(parse_words("").unwrap().unwrap().frame, None);
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
