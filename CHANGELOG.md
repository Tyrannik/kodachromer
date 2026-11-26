# Changelog

## Unreleased

- **A new look for the app.** The library is a strip of slide film: frames sit between sprocket
  holes with their frame number printed along the edge, and the frame you're editing is outlined.
  Everything around the photo is now strictly neutral grey, so nothing on screen tints how you judge
  its colour. The edit panel is grouped into Film, Adjust, Tune, Copy settings and Export, with
  labels in one column and value boxes of one width. Each Tune slider is centred on the film's
  Kodak-data value, so an untouched film shows its handles in the middle. Compare and zoom moved to a bar under the photo. Type is Barlow.
- **Fix:** opening **Tune** no longer marks the film as changed. The sliders snapped some values to
  their step just by being shown (Kodachrome's 0.003 screen black became 0), so saving changed
  the film without you touching it.
- **Highlights fitted to each film.** The camera's brightest recorded tone (about three stops over
  mid-grey on the M9) now lands on the film's white, with a smooth shoulder from one stop over grey.
  Ektachrome and Kodachrome whites reach white instead of stopping at a dull light grey, and the top
  stop keeps more separation. Mid-tones are unchanged. Turn off **Fit camera highlights to film**
  (or pass `--no-fit` to the CLI) for the Kodak datasheet curve exactly as published.

## 0.1.0-alpha.5

- **Two new films: Ektachrome E100 and Portra 400.** Each is its own model built from its Kodak
  datasheet, not a re-tuned Kodachrome:
  - A new *spectral* engine fits each film's layer sensitivities from the published spectral
    sensitivity curves, inverts the Status A or Status M characteristic curves through the
    published dye spectra, and computes the colour you see from the dyes' spectra under 5000 K
    light (CIE 1931 observer).
  - **Ektachrome E100** (E-4000) is a slide film with its own E-6 dyes, scanned or projected like
    Kodachrome, with its published rms granularity of 8.
  - **Portra 400** (E-4050) is a colour negative, shown as an optical print on **Endura Premier**
    paper (E-4070): the negative's printing densities, computed from its measured spectra and the
    paper's sensitivity, expose the paper through its own curves and dyes.
  - Both use each layer's published MTF (including Portra's adjacency boost) for film sharpness.
    Like grain and halation, it is a size on the film, so it scales with each camera's pixel pitch.
- For negatives, the Output menu shows the print paper instead of Scanned / Projected slide.
- **Warning for files that aren't from a Leica M9.** The library flags them ("⚠ not M9", with a
  count at the top), and the preview shows a banner naming the camera. They still open and develop
  as before. The M9-P, M9 Titanium and M-E count as M9s; Monochrom bodies don't.
- `remjet-cli info`, `render` and `export` print the same warning on stderr for each such file.
- **Exported JPGs are now tagged as sRGB** with an embedded ICC profile, so colour-managed viewers and
  wide-gamut displays show them as intended instead of guessing (and oversaturating). Pixels are
  unchanged, and camera metadata (EXIF) is still carried over when enabled.

## 0.1.0-alpha.4

- **Film looks scale to each camera's sensor.** Grain, grain clump size and halation are physical
  sizes on the film, so they are now converted to pixels with each file's own pixel pitch instead of
  the M9's. The pitch comes from the crop factor (35 mm-equivalent focal length) or the EXIF
  focal-plane resolution, and falls back to full frame. Leica M9-family files render exactly as before.
- **More raw files open:** the app and CLI list all raw formats rawler reads (CR2, CR3, NEF, ARW,
  RAF, RW2, ORF, PEF and others), not only DNG.
- **Fujifilm X-Trans** sensors and **linear DNGs** now develop.
- Files without a D65 colour matrix use their closest daylight matrix instead of failing.
- `remjet-cli info` prints each file's sensor pitch and where it came from.

## 0.1.0-alpha.3

- **Renamed to Remjet** (formerly Kodachromer): the executables are now `remjet.exe` and
  `remjet-cli.exe`, and the repository is <https://github.com/Tyrannik/remjet>.
- Your edits, settings and tuned films move automatically from `%APPDATA%\Kodachromer` to
  `%APPDATA%\Remjet` on first start. Settings copied to the clipboard by Kodachromer still paste.
- `REMJET_DATA_DIR` replaces `KODACHROMER_DATA_DIR` (the old name still works).

## 0.1.0-alpha.2

- **Projected slide output** alongside the scanned look: full density range, flare-limited deep
  blacks, warm lamp and corner falloff (tunable under *Tune look*).
- **Edits no longer clutter photo folders:** they live in `%APPDATA%\Kodachromer\edits`, matched by
  file content so they survive moves and renames. Old sidecar files are imported and removed
  automatically. *Reset all* now returns an image to unedited.
- **Undo / redo** per image (Ctrl+Z, Ctrl+Y / Ctrl+Shift+Z, toolbar buttons).
- **Copy / paste settings** shortcuts are more robust, also use the system clipboard (paste
  settings across sessions), and every action shows a short confirmation message.
- **Fixed:** a crash when Windows or another tool reported an invalid window size during a resize.
- `KODACHROMER_DATA_DIR` environment variable to keep app data elsewhere (portable use).
- Releases are now built and published by GitHub Actions from the tagged source.

## 0.1.0-alpha.1 — first public alpha

First release of Kodachromer, a Windows app that develops Leica M9 DNGs with a Kodachrome 64
(scanned slide) look.

### Film emulation
- Kodachrome 64 model built on Kodak's published data (E-55): characteristic curves and dye
  cross-absorption, with a scanner stage for the scanned-slide look.
- Per-image controls: exposure, warmth, tint, highlights, halation, sharpen, grain and grain size.
- **Tune look**: adjust the film itself (brightness, contrast, saturation, shadow lift, black depth,
  scanner colour, grain) with live preview; save it, restore Kodak defaults, or save variants as
  new films.
- Film-like grain that matches between the 100% preview and the export.

### App
- Library with thumbnails, live preview, before/after (hold **B**), reference image beside the
  preview.
- Zoom: click for 100%, Ctrl+scroll up to 800%, smooth panning.
- Non-destructive edits saved beside each DNG; copy/paste settings across selected images.
- JPG export (full size or resized, quality, EXIF) for one, selected, edited or all images, with
  progress and cancel. Command-line export for scripting.

### Known limitations
See the *Known limitations* section of the README.
