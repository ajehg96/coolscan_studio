# Coolscan Studio — Notices & Licensing

## Application Information

- **Name:** Coolscan Studio
- **Version:** 0.1.0
- **License:** MIT OR Apache-2.0
- **Repository:** https://github.com/activexray/coolscan-studio

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

### 2. Darktable Compatibility Notice

Coolscan Studio implements the exact mathematical equations used in Darktable's **Negadoctor** color film negative inversion module and **Colorin** input profile modules.

- Coolscan Studio writes standard XML sidecars (`.tif.xmp`) conforming to Darktable's open specification.
- Coolscan Studio is an independent clean-room implementation and does not link Darktable code or libraries.
- Darktable is developed by the Darktable Team (https://www.darktable.org) and licensed under the GNU General Public License v3.0 or later.

### 3. Nikon Coolscan Calibration Profiles

The default color pipeline profile for the Nikon Super Coolscan LS-40 ED is derived from standardized scanner characterization data targeting CIE standard illuminant D50 and standard 35mm color film substrate. Users can supply custom ICC scanner profiles or film calibration profiles in standard JSON format.

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
