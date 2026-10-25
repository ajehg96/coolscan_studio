# Scanner ICC Profiles (`profiles/`)

`NKLS4000LS40_N.icc` is derived from the Nikon Scan 4 installer's ICM profile for the Nikon Super Coolscan LS-4000 ED / Coolscan IV ED (LS-40 ED) negative workflow, adapted by `nkscan` (`scripts/profiles.py`):

1. The profile class header was changed from Nikon's private `nkpf` signature to standard `scnr` so LittleCMS 2 and Darktable `colorin` can open it directly.
2. The gamma 2.2 encoding expected by Nikon Scan's CMS was composed into the profile's curves so that the profile accepts linear 16-bit scanner samples directly.

## Provenance & License Notice

Derived from the ICM profiles in the Nikon Scan 4 installer, which carry the notice `"Nikon Inc. & Nikon Corporation 2003"`. That notice is retained inside the ICC profile file.

These profile files are **not** covered by Coolscan Studio's or `nkscan`'s MIT/Apache-2.0 license, and no license to them is granted here. Users may supply custom scanner ICC profiles or rebuild the original profile from their own Nikon Scan 4 installer copy.
