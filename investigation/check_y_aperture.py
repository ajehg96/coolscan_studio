import glob
import numpy as np
from PIL import Image

def find_y_aperture():
    bmps = sorted(glob.glob("investigation/strip-b-*/frame*.bmp"))
    for path in bmps:
        if "crop" in path or "inset" in path:
            continue
        im = Image.open(path)
        arr = np.array(im, dtype=float)
        if arr.shape[0] < 500:
            continue
        row_mean = arr.mean(axis=(1, 2))
        
        # Find top mask edge (where brightness rises above ~15)
        top_idx = 0
        for r in range(50):
            if row_mean[r] > 15.0:
                top_idx = r
                break
        
        # Find bottom mask edge (where brightness drops below ~15)
        bottom_idx = arr.shape[0] - 1
        for r in range(arr.shape[0] - 1, arr.shape[0] - 50, -1):
            if row_mean[r] > 15.0:
                bottom_idx = r
                break
        
        print(f"{path}: shape={arr.shape}, top edge ~ row {top_idx} (val={row_mean[top_idx]:.1f}), bottom edge ~ row {bottom_idx} (val={row_mean[bottom_idx]:.1f}), height={bottom_idx - top_idx}")

find_y_aperture()
