"""Synthetic checks for reference-distance classification."""
import numpy as np

def film_fraction(pixels, reference, colour_tol=.10, intensity_tol=.25):
    ratios = reference / reference.mean()
    means = pixels.mean(axis=1)
    colour = np.max(np.abs(pixels / np.maximum(means[:,None],1) - ratios), axis=1)
    intensity = means / reference.mean()
    return ((colour <= colour_tol) & (intensity >= 1-intensity_tol) & (intensity <= 1+intensity_tol)).mean()

rng = np.random.default_rng(7)
reference = np.array([56000., 57000., 55000.])
film = reference * rng.uniform(.94, 1.06, size=(1000,3))
picture = np.array([12000., 18000., 15000.]) * rng.uniform(.8, 1.2, size=(1000,3))
assert film_fraction(film, reference) > .95
assert film_fraction(picture, reference) < .05
print('film_reference checks passed')
