import numpy as np
from PIL import Image

def test_vertical_crop():
    for f, path, l, r in [
        ('Frame 2', 'investigation/strip-b-09-autocrop/frame-2-uncropped.bmp', 29, 1059),
        ('Frame 3', 'investigation/strip-b-10-autocrop/frame-3-uncropped.bmp', 18, 1045),
        ('Frame 4', 'investigation/strip-b-12-autocrop/frame-4-uncropped.bmp', 39, 1063),
    ]:
        im = Image.open(path)
        arr = np.array(im)
        
        # 5 column inset along travel (width)
        w_start = l + 5
        w_end = r - 5
        
        # Method 1: Just crop black bars (rows 24..691)
        top = 24
        bottom = 691
        crop_aperture = arr[top:bottom, w_start:w_end, :]
        h1, w1 = crop_aperture.shape[:2]
        
        # Method 2: Exact 2:3 aspect ratio (ratio 1.5)
        # Since available height is 667 rows, 3/2 * 667 = 1000.5 columns.
        # If we take height = 667, width = round(667 * 1.5) = 1000 cols.
        target_w = int(round(h1 * 1.5))
        # Center target_w within [w_start, w_end]
        available_w = w_end - w_start
        excess_w = available_w - target_w
        if excess_w >= 0:
            w_start_23 = w_start + excess_w // 2
            w_end_23 = w_start_23 + target_w
            crop_23 = arr[top:bottom, w_start_23:w_end_23, :]
        else:
            # If width is narrower than 1.5*h, reduce height to match 2:3
            target_h = int(round(available_w / 1.5))
            excess_h = h1 - target_h
            top_23 = top + excess_h // 2
            bottom_23 = top_23 + target_h
            crop_23 = arr[top_23:bottom_23, w_start:w_end, :]
            
        h2, w2 = crop_23.shape[:2]
        
        out1_path = path.replace("-uncropped.bmp", "-aperture-inset5.bmp")
        out2_path = path.replace("-uncropped.bmp", "-ratio2x3-inset5.bmp")
        Image.fromarray(crop_aperture).save(out1_path)
        Image.fromarray(crop_23).save(out2_path)
        
        print(f"=== {f} ===")
        print(f"Aperture crop: {w1}x{h1} ({w1/725*25.4:.2f} x {h1/725*25.4:.2f} mm), ratio={w1/h1:.3f} -> {out1_path}")
        print(f"2:3 exact crop: {w2}x{h2} ({w2/725*25.4:.2f} x {h2/725*25.4:.2f} mm), ratio={w2/h2:.3f} -> {out2_path}")

test_vertical_crop()
