# Phase 9: Darktable Sidecar Research Spike

## 1. Overview and Objective

The goal of Phase 9 is to definitively establish the structure, encoding, module versioning, parameter binary layout, and history semantics of Darktable's XMP sidecars. This replaces assumptions with empirical data derived from Darktable 5.6.0 (installed on the host system) and actual Darktable sidecars generated for scans in `scans-strip-02` through `scans-strip-06`.

---

## 2. XMP Document Structure & Namespaces

Darktable writes XMP files with the following XML envelope:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 4.4.0-Exiv2">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:exif="http://ns.adobe.com/exif/1.0/"
    xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:darktable="http://darktable.sf.net/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:lr="http://ns.adobe.com/lightroom/1.0/"
   exif:DateTimeOriginal=""
   xmpMM:DerivedFrom="frame-1.tif"
   xmp:Rating="1"
   darktable:import_timestamp="63924743275725256"
   darktable:change_timestamp="63924826490374288"
   darktable:export_timestamp="-1"
   darktable:print_timestamp="-1"
   darktable:xmp_version="5"
   darktable:raw_params="0"
   darktable:auto_presets_applied="1"
   darktable:history_end="7"
   darktable:iop_order_version="5"
   darktable:history_basic_hash="33e4711b8f6644f5f8c2a164fa3f94cd"
   darktable:history_current_hash="d678c30cc80d2a6de87a924eeda08d99">
   <darktable:masks_history>
    <rdf:Seq/>
   </darktable:masks_history>
   <darktable:history>
    <rdf:Seq>
      <!-- History items: rdf:li -->
    </rdf:Seq>
   </darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
```

### Key Top-Level Attributes:
- `xmpMM:DerivedFrom`: Name of the source TIFF image (e.g. `frame-1.tif`).
- `darktable:xmp_version`: `5` for modern Darktable releases.
- `darktable:iop_order_version`: `5` (uses default pipeline processing order; no manual legacy `iop_order` float reordering required).
- `darktable:raw_params`: `0` for raster images / TIFF files.
- `darktable:auto_presets_applied`: `1`.
- `darktable:history_end`: Total number of active history items in `<darktable:history>`.
- `darktable:history_basic_hash` / `darktable:history_current_hash`: Optional MD5 hex hashes. Source code inspection of `src/common/exif.cc` confirms that when absent or generated externally, Darktable automatically computes hashes from history upon import (`dt_history_hash_write_from_history`).

---

## 3. Parameter Encoding: Hex vs Compressed Base64 ("gz")

In Darktable (`src/common/exif.cc`, lines 3410–3540):
- If data is uncompressed, it is encoded as lowercase hexadecimal ASCII (2 characters per byte).
- If compressed, it is prefixed with `"gz"` followed by a 2-digit decimal compression factor `10 * (c0 - '0') + (c1 - '0')`, followed by standard Base64 of `zlib` compressed data (RFC 1950 deflate with adler32 checksum).
- When decoding:
  ```c
  if (!strncmp(input, "gz", 2)) {
    int factor = 10 * (input[2] - '0') + (input[3] - '0');
    // base64 decode input + 4
    // uncompress with initial buffer size = factor * compressed_size
  } else {
    // hex decode
  }
  ```
- Darktable's decoder accepts both formats interchangeably for any module.

---

## 4. Module Specifications & Binary Struct Layouts

### 4.1. `colorin` (Input Color Profile)
- **Module Identifier**: `colorin`
- **Module Version**: `7` (`modversion="7"`)
- **C Struct**: `dt_iop_colorin_params_t` (from `src/iop/colorin.c`):
  ```c
  #define DT_IOP_COLOR_ICC_LEN 512
  typedef struct dt_iop_colorin_params_t {
    dt_colorspaces_color_profile_type_t type;       // int32: 0 = DT_COLORSPACE_FILE
    char filename[DT_IOP_COLOR_ICC_LEN];            // 512 bytes: null-padded ICC profile filename/path
    dt_iop_color_intent_t intent;                   // int32: 0 = DT_INTENT_PERCEPTUAL
    dt_iop_color_normalize_t normalize;             // int32: 0 = DT_NORMALIZE_OFF
    gboolean blue_mapping;                          // int32: 0 = FALSE
    dt_colorspaces_color_profile_type_t type_work;  // int32: 4 = DT_COLORSPACE_LIN_REC2020
    char filename_work[DT_IOP_COLOR_ICC_LEN];       // 512 bytes: zeroes
  } dt_iop_colorin_params_t;
  ```
  Total size: `4 + 512 + 4 + 4 + 4 + 4 + 512 = 1044` bytes.
- When pointing to `NKLS4000LS40_N.icc`:
  - `type`: `0` (`DT_COLORSPACE_FILE`)
  - `filename`: `"C:\\Users\\AJEHG\\AppData\\Local\\darktable\\color\\in\\NKLS4000LS40_N.icc"` (or relative filename `NKLS4000LS40_N.icc`)
  - `type_work`: `4` (`DT_COLORSPACE_LIN_REC2020`)
  - Serialized as `gz12eJxjYGBgcLaKCS1OLSqOcfRy9XCPcSwocEksSYzxyU9OzIlJSSzKLklMykmNSc7PyS+KycyL8fP2CTYxMDAAkfF+epnJyQyjYJgAloF2wCgYcAAAncoXAg==` (88 compressed bytes, factor 12).

### 4.2. `colorout` (Output Color Profile)
- **Module Identifier**: `colorout`
- **Module Version**: `5` (`modversion="5"`)
- **Serialized Params**: `gz35eJxjZBgFo4CBAQAEEAAC` (default standard sRGB export profile).

### 4.3. `gamma` (Display Gamma)
- **Module Identifier**: `gamma`
- **Module Version**: `1` (`modversion="1"`)
- **Serialized Params**: `0000000000000000` (8 bytes of zeroes).

### 4.4. `flip` (Orientation)
- **Module Identifier**: `flip`
- **Module Version**: `2` (`modversion="2"`)
- **C Struct**: `dt_iop_flip_params_t` (from `src/iop/flip.c`):
  ```c
  typedef struct dt_iop_flip_params_t {
    dt_image_orientation_t orientation; // uint32 little-endian
  } dt_iop_flip_params_t;
  ```
  Total size: `4` bytes.
- Values:
  - `00000000`: Normal (0°)
  - `06000000`: 90° clockwise (`ORIENTATION_SWAP_XY | ORIENTATION_FLIP_Y`)
  - `03000000`: 180° (`ORIENTATION_FLIP_X | ORIENTATION_FLIP_Y`)
  - `05000000`: 270° clockwise (`ORIENTATION_SWAP_XY | ORIENTATION_FLIP_X`)
  - `ffffffff`: Auto orientation from image metadata (`_builtin_auto`).

### 4.5. `negadoctor` (Negative Inversion and Tone Curve)
- **Module Identifier**: `negadoctor`
- **Module Version**: `2` (`modversion="2"`)
- **C Struct**: `dt_iop_negadoctor_params_t` (from `src/iop/negadoctor.c`):
  ```c
  typedef struct dt_iop_negadoctor_params_t {
    int32_t film_stock;     // 1 = color negative, 2 = B&W
    float dmin[4];          // [R, G, B, 1.0] unexposed film base density
    float wb_high[4];       // [R, G, B, 1.0] highlight WB multipliers
    float wb_low[4];        // [R, G, B, 1.0] shadow WB multipliers
    float dmax;             // dynamic range / D-max
    float offset;           // scan bias
    float black;            // paper black density
    float gamma;            // paper grade (contrast)
    float soft_clip;        // paper gloss (highlights roll-off)
    float exposure;         // print exposure
  } dt_iop_negadoctor_params_t;
  ```
  Total size: `4 + 16 + 16 + 16 + 4 + 4 + 4 + 4 + 4 + 4 = 76` bytes (little-endian).
- Hex Encoding: `76 * 2 = 152` lowercase hex characters.
- Example verified against `scans-strip-02/frame-1.tif.xmp`:
  `010000000581653fe1c7683f88b0613f0000803f4eadf93f20e2ce3f0000803f0000803f0000803f0000803f0000803f0000803f736551405219d93dd0cccc3dffff8f400000403f08ac6c3f`
  Unpacked values:
  - `film_stock`: 1
  - `dmin`: [0.8965, 0.9093, 0.8816, 1.0]
  - `wb_high`: [1.9506, 1.6163, 1.0, 1.0]
  - `wb_low`: [1.0, 1.0, 1.0, 1.0]
  - `dmax`: 3.2718
  - `offset`: 0.1060
  - `black`: 0.1000
  - `gamma`: 4.5
  - `soft_clip`: 0.75
  - `exposure`: 0.9245

### 4.6. Blendop Parameters
Every operation element `<rdf:li>` includes blending options:
- `blendop_version="14"`
- `blendop_params="gz11eJxjYIAACQYYOOHEgAZY0QWAgBGLGANDgz0Ej1Q+dcF/IADRAGpyHQU="`
Inspection confirms this string is identical across all operations in all Darktable 5.x XMPs (representing default normal blend with opacity 1.0 and no mask).

---

## 5. History Pipeline Sequence for LS-40 Scans

When Coolscan Studio generates an XMP for Darktable import, the canonical minimal history sequence consists of:

| `num` | `operation` | `modversion` | `params` | Notes |
|:---|:---|:---|:---|:---|
| 0 | `colorin` | 7 | `gz48eJxjZBgFowABWAbaAaNgwAEAEDgABg==` | Darktable initial default input profile |
| 1 | `colorout` | 5 | `gz35eJxjZBgFo4CBAQAEEAAC` | Darktable default output sRGB profile |
| 2 | `gamma` | 1 | `0000000000000000` | Default gamma |
| 3 | `flip` | 2 | `ffffffff` | Initial auto-orientation (`_builtin_auto`) |
| 4 | `colorin` | 7 | `gz12eJxjYGBgcLaKCS1OLSqOcfRy9XCPcSwocEksSYzxyU9OzIlJSSzKLklMykmNSc7PyS+KycyL8fP2CTYxMDAAkfF+epnJyQyjYJgAloF2wCgYcAAAncoXAg==` | Scanner profile (`NKLS4000LS40_N.icc`) |
| 5 | `negadoctor` | 2 | `152 hex chars` | Calibrated Negadoctor inversion |
| 6 | `flip` | 2 | e.g. `03000000` | User orientation (if rotated from Normal) |

If orientation is `Normal` (0°), item 6 can be omitted (`history_end="6"`), or included as `00000000`.

---

## 6. Real CLI Validation

Tested running `darktable-cli.exe` (v5.6.0) on `scans-strip-02/frame-1.tif` with its sidecar:
- Successfully parsed history, applied input profile, and executed Negadoctor inversion.
- Confirmed zero errors during headless CLI rendering.
