import numpy as np
from PIL import Image

def examine_uncropped(name, path, lead_col, trail_col):
    im = Image.open(path)
    arr = np.array(im, dtype=float)
    col_mean = arr.mean(axis=(0, 2))
    print(f"=== {name} uncropped (shape={arr.shape}) ===")
    print(f"Leading edge chosen: col {lead_col}")
    print("Cols around leading [lead-5 : lead+8]:")
    for c in range(lead_col - 5, lead_col + 8):
        print(f"  col {c:4d}: mean={col_mean[c]:.1f}, R={arr[:, c, 0].mean():.1f}, G={arr[:, c, 1].mean():.1f}, B={arr[:, c, 2].mean():.1f}")
    
    print(f"Trailing edge chosen: col {trail_col}")
    print("Cols around trailing [trail-8 : trail+5]:")
    for c in range(trail_col - 8, trail_col + 5):
        print(f"  col {c:4d}: mean={col_mean[c]:.1f}, R={arr[:, c, 0].mean():.1f}, G={arr[:, c, 1].mean():.1f}, B={arr[:, c, 2].mean():.1f}")
    print()

examine_uncropped("Frame 2", "investigation/strip-b-09-autocrop/frame-2-uncropped.bmp", 29, 1059)
examine_uncropped("Frame 3", "investigation/strip-b-10-autocrop/frame-3-uncropped.bmp", 18, 1045)
examine_uncropped("Frame 4", "investigation/strip-b-12-autocrop/frame-4-uncropped.bmp", 39, 1063)
