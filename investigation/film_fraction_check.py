"""Small executable checks for the 5% classifier rule."""
import numpy as np

def first_run(values, n=8):
    for i in range(len(values)-n+1):
        if values[i:i+n].all(): return i
    return None

assert first_run(np.array([True]*8+[False]*20)) == 0
assert first_run(np.array([True]*3+[False]*20)) is None
assert first_run(np.array([False]*12+[True]*8)) == 12
print('film_fraction checks passed')
