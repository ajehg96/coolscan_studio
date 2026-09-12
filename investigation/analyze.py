"""Numerical boundary evidence from raw captures; outputs plots and CSV."""
from pathlib import Path
import re
import numpy as np


import sys
root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).parent / 'strip-b-01'
svg = ['<svg xmlns="http://www.w3.org/2000/svg" width="1400" height="1400"><rect width="1400" height="1400" fill="white"/>']
for i, stem in enumerate(['discovery'] + [f'frame-{n}' for n in range(1, 7)]):
    if not (root / f'{stem}.raw').exists():
        continue
    meta = (root / f'{stem}.txt').read_text()
    rows, cols, channels = map(int, re.search(r'rows=(\d+) cols=(\d+) channels=(\d+)', meta).groups())
    raw = np.fromfile(root / f'{stem}.raw', dtype='<u2').reshape(channels, rows, cols).astype(float)
    # Central film excludes holder overscan. Keep raw transmission, not inversion.
    profile = np.median(raw[1, rows//4:3*rows//4], axis=0)
    spread = np.std(raw[1, rows//4:3*rows//4], axis=0)
    pitch = int(re.search(r'line_pitch: (\d+)', meta).group(1))
    top = 0 if stem == 'discovery' else int(re.search(r'rect=Rect \{ top: (\d+)', meta).group(1))
    x = (top + np.arange(cols)*pitch)*25.4/2900
    np.savetxt(root / f'{stem}-profile.csv', np.column_stack([np.arange(cols),x,profile,spread]), delimiter=',', header='column,nominal_position_mm,green_median,green_std', comments='')
    points = ' '.join(f'{30+j*1330/max(1,cols-1):.1f},{i*195+170-v/max(1,profile.max())*135:.1f}' for j,v in enumerate(profile))
    svg.append(f'<text x="30" y="{i*195+22}" font-size="16">{stem}: central green transmission; {cols} columns, pitch {pitch} dots</text>')
    svg.append(f'<polyline points="{points}" fill="none" stroke="#176a9b" stroke-width="1"/>')
    if stem == 'discovery':
        # Diagnostic candidates, not trusted crops: scene edges can also win.
        frame_text = meta.split('frames=')[1].split('layout=')[0]
        tops = [int(v)//pitch for v in re.findall(r'top: (\d+)', frame_text)]
        bottoms = [int(v)//pitch for v in re.findall(r'bottom: (\d+)', frame_text)]
        log_rgb = np.log10(np.maximum(raw[:, rows//8:rows-rows//8].mean(axis=1), 1)).mean(axis=0)
        change = np.diff(log_rgb)
        candidates = []
        for n,(start,end) in enumerate(zip(tops,bottoms),1):
            lo,hi=max(0,start-8),min(cols-1,start+8)
            left = lo + int(np.argmin(change[lo:hi])) + 1
            lo,hi=max(0,end-10),min(cols-1,end+10)
            right = lo + int(np.argmax(change[lo:hi])) + 1
            candidates.append([n,start,left,change[left-1],end,right,change[right-1]])
        np.savetxt(root/'edge-candidates.csv', candidates, delimiter=',', header='frame,detected_start,candidate_start,start_log_step,detected_end,candidate_end,end_log_step', comments='')
        print('Diagnostic edge candidates:', candidates)
svg.append('</svg>')
(root/'profiles.svg').write_text('\n'.join(svg))
