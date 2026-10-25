# Coolscan Studio — Notices & Licensing

## Application Information

- **Name:** Coolscan Studio
- **Version:** 0.1.0
- **License:** MIT OR Apache-2.0
- **Repository:** https://github.com/ajehg96/coolscan_studio

Coolscan Studio is a post-scan review studio and scanner controller designed specifically for Nikon Coolscan film scanners (LS-40 ED / IV ED, LS-50 ED / V ED, LS-4000 ED, LS-5000 ED), featuring direct Darktable Negadoctor mathematical compatibility.

---

## Hardware & Driver Prerequisites (Windows)

Nikon Coolscan scanners utilize standard USB bulk endpoints for bidirectional command and image transfer.

### WinUSB Driver Configuration

To communicate with the scanner on Windows without proprietary Nikon Scan software:

1. Download **Zadig** (Universal USB Driver Installer) from [https://zadig.akeo.ie](https://zadig.akeo.ie).
2. Power on your Nikon Coolscan scanner and connect it via USB.
3. In Zadig, select **Options -> List All Devices**.
4. Select your Nikon scanner from the dropdown list (e.g. `Nikon LS-40 ED` or `Nikon Coolscan`).
5. Select **WinUSB** as the target driver and click **Replace Driver** (or **Install Driver**).
6. Once installation completes, Coolscan Studio will automatically discover and interface with the scanner without rebooting.

---

## Third-Party Components & Licenses

### 1. LittleCMS 2 (lcms2)

Coolscan Studio utilizes LittleCMS 2 for ICC color management, including converting raw scanner-calibrated RGB sensor data into linear Rec.2020 working space and display sRGB.

- **Copyright:** (c) 1998-2024 Marti Maria Saguer
- **License:** MIT License
- **Website:** https://www.littlecms.com

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the standard MIT conditions.

### 2. Darktable Compatibility Notice (Mathematical Specification)

Coolscan Studio implements the mathematical equations and binary parameter structures specified by Darktable's **Negadoctor** (`negadoctor` modversion 2) color film negative inversion module and **Colorin** (`colorin` modversion 7) input profile module for interoperability with Darktable 5.x.

- Coolscan Studio writes standard XML sidecars (`.tif.xmp`) conforming to Darktable's open sidecar format.
- The Rust implementation in `src/processing/negadoctor.rs` and `src/darktable/xmp.rs` is written in Rust against Darktable's documented mathematical formulas and C struct layouts (`dt_iop_negadoctor_params_t`, `dt_iop_colorin_params_t`, `dt_iop_flip_params_t`) without linking Darktable C/C++ libraries.
- Darktable is developed by the Darktable Team (https://www.darktable.org) and licensed under the GNU General Public License v3.0 or later.

### 3. Nikon Coolscan ICC Profiles (`profiles/NKLS4000LS40_N.icc`)

The scanner input profile `profiles/NKLS4000LS40_N.icc` (sourced via `nkscan`) is derived from the ICM profiles in the Nikon Scan 4 installer, which carry the notice `"Nikon Inc. & Nikon Corporation 2003"` (retained inside the profile header).

- **License Carve-Out:** As documented by `nkscan`, these Nikon-derived ICC profile files are **not** covered by Coolscan Studio's or `nkscan`'s MIT/Apache-2.0 license, and no license to those profile tables is granted here.
- Two structural adjustments were made by `nkscan/scripts/profiles.py` for LittleCMS / Darktable compatibility: the profile class header was changed from Nikon's private `nkpf` tag to standard `scnr`, and the gamma 2.2 encoding expected by Nikon Scan's CMS was composed into the profile's curves so that the profile accepts linear 16-bit scanner samples directly.
- Users may also supply their own custom scanner ICC profiles or rebuild the profile from their own Nikon Scan 4 installer copy.

---

## Diagnostic Commands

You can verify your scanner connection, WinUSB driver status, and Darktable integration at any time by running:

```powershell
coolscan-studio.exe --diagnose
```

To run the interactive desktop review interface in simulation mode without physical hardware:

```powershell
coolscan-studio.exe --mock
```
