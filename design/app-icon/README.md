# lam icon

`lam-mark.svg` is the editable source for the approved `icon-lam` design.
The curved stem, ring, and attention dot use paths with no font dependency.
The colors are amber `#D5A64D` and graphite `#24272E`.

Android foreground and monochrome vectors use the same paths, scaled by 0.6
and translated by `(14.7, 15.3)` within the 108 dp adaptive canvas. This centers
the mark's visible bounds and keeps every pixel inside the central 66 dp safe
circle. The notification vector scales the mark by 0.25 and centers it in a
24 dp transparent canvas. `res/raw/keep.xml` retains that resource during
release shrinking until notification delivery uses it.
Adaptive resources live in `mipmap-anydpi` because the app's minimum API 29
already supports adaptive icons; the `v26` qualifier would be redundant.

From the repository root, with Python 3, `rsvg-convert`, and ImageMagick installed:

```sh
python3 android/scripts/check-icon-assets.py --previews /tmp/lam-icon-previews
python3 android/scripts/test-icon-assets.py
```

The first command checks the rendered artwork against the approved geometry,
resolves manifest/adaptive resource links, checks the safe circle and themed
silhouette, and verifies notification detail at 24 dp and all ten density PNGs.
It exports circle, squircle, rounded-square, themed, and notification previews.
The second command verifies that the checks reject missing assets, broken
references, a missing dot, and swapped density artwork using temporary copies.

After building, also inspect the packaged resources, including the shrunk
release APK. This catches stale resource-merge output after folder moves:

```sh
python3 android/scripts/check-icon-assets.py --apk android/app/build/outputs/apk/release/app-release.apk --aapt2 /path/to/android-sdk/build-tools/36.0.0/aapt2
```

After an approved artwork change, update the SVG, Android vectors, and approved
rendering reference in the check script, then regenerate the legacy PNGs:

```sh
python3 android/scripts/check-icon-assets.py --generate-legacy --previews /tmp/lam-icon-previews
```

Legacy launcher sizes are 48, 72, 96, 144, and 192 px. Both rounded-square and
round PNGs use the adaptive foreground cropped to its 72 dp visible area.
Preview masks approximate launcher shapes. They do not replace device checks
of launcher, recent apps, Settings, themed icons, notification shade, or display
scaling. No command here installs an app or modifies device state.
