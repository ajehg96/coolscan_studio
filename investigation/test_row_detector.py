import numpy as np
from PIL import Image

def test_detect_row_aperture(path):
    im = Image.open(path)
    arr = np.array(im, dtype=float) # shape (H, W, 3), 8-bit from BMP (0..255)
    rows, cols = arr.shape[:2]
    col_start = cols // 4
    col_end = cols - cols // 4
    
    row_means = arr[:, col_start:col_end, :].mean(axis=(1, 2))
    max_mean = row_means.max()
    threshold = max(max_mean * 0.15, 6.0)
    
    # Top
    search_top = rows // 5
    top = 0
    if row_means[0] < threshold:
        for r in range(search_top):
            if row_means[r] >= threshold:
                top = min(r + 2, rows // 2)
                break
                
    # Bottom
    search_bottom = rows - rows // 5
    bottom = rows
    if row_means[-1] < threshold:
        for r in range(rows - 1, search_bottom - 1, -1):
            if row_means[r] >= threshold:
                bottom = max(r - 1, top + 1)
                break
                
    print(f"{path}: top={top} (row_mean={row_means[top]:.1f}), bottom={bottom} (row_mean={row_means[bottom]:.1f}), height={bottom-top}")

test_detect_row_aperture("investigation/strip-b-09-autocrop/frame-2-uncropped.bmp")
test_detect_row_aperture("investigation/strip-b-10-autocrop/frame-3-uncropped.bmp")
test_detect_row_aperture("investigation/strip-b-12-autocrop/frame-4-uncropped.bmp")
test_detect_row_aperture("investigation/strip-b-08-overscan/frame-2.bmp")
test_detect_row_aperture("investigation/strip-b-06-overscan/frame-2.bmp")
