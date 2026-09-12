import numpy as np
from PIL import Image

def check_f4():
    im = Image.open('investigation/strip-b-12-autocrop/frame-4-uncropped.bmp')
    arr = np.array(im, dtype=float)
    # Check row-by-row or image content:
    # Let's see what the image is on frame 4
    print("Frame 4 uncropped size:", arr.shape)
    for c in range(1040, 1075, 2):
        col_slice = arr[:, c, :]
        print(f"Col {c}: mean={col_slice.mean():.1f}, min={col_slice.min():.1f}, max={col_slice.max():.1f}, std={col_slice.std():.1f}")

check_f4()
