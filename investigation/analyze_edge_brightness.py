import numpy as np
from PIL import Image

def analyze(name, path):
    im = Image.open(path)
    arr = np.array(im, dtype=float) # shape (H, W, 3)
    col_mean = arr.mean(axis=(0, 2))
    print(f"=== {name} (W={arr.shape[1]}, H={arr.shape[0]}) ===")
    print("Left 12 columns brightness: ", [round(x, 1) for x in col_mean[:12]])
    print("Right 12 columns brightness:", [round(x, 1) for x in col_mean[-12:]])
    r_mean = arr[:, :, 0].mean(axis=0)
    g_mean = arr[:, :, 1].mean(axis=0)
    b_mean = arr[:, :, 2].mean(axis=0)
    print("Left 5 cols RGB:")
    for i in range(5):
        print(f"  col {i}: R={r_mean[i]:.1f}, G={g_mean[i]:.1f}, B={b_mean[i]:.1f}")
    print("Right 5 cols RGB:")
    for i in range(arr.shape[1]-5, arr.shape[1]):
        print(f"  col {i} ({i-arr.shape[1]}): R={r_mean[i]:.1f}, G={g_mean[i]:.1f}, B={b_mean[i]:.1f}")
    print()

analyze("Frame 2", "investigation/strip-b-09-autocrop/frame-2-cropped.bmp")
analyze("Frame 3", "investigation/strip-b-10-autocrop/frame-3-cropped.bmp")
analyze("Frame 4", "investigation/strip-b-12-autocrop/frame-4-cropped.bmp")
