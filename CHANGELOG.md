# Changelog

## Unreleased

- **Android app (experimental).** Open an M9 DNG on a phone, pick any film (or neutral), and export
  a JPEG at full (18 MP) or half (4.5 MP) size, with the camera's EXIF kept. It uses the same
  processing as the desktop app. Android 7.0 or newer; releases include
  `Remjet-<version>-android.apk`. See [docs/ANDROID.md](docs/ANDROID.md).

## 0.1.0-alpha.8

- **Five new films from Kodak datasheets**, each digitised from its own publication's vector plots
  and built on the same spectral models as the existing films:
  - **Portra 160** (E-4051, 2025), a colour negative like Portra 400: lab scan by default, Endura
    print optional. Grain estimated from its Print Grain Index (28 at 4x6). Pull 1, push 1 and
    push 2 are modelled like Portra 400's.
  - **T-Max 100** (F-4016) and **T-Max 400** (F-4043) in black and white, with D-76 curves,
    panchromatic sensitivity, MTF and rms granularity (8 and 10). T-Max 400's pull 1, push 1 and
    push 2 use Kodak's measured 6-, 8- and 11-minute curves. T-Max 100's pushes use its measured
    7 1/2- and 10-minute curves; it has no shorter curve, so its pull is modelled (marked).
  - **Ektachrome E100VS** (E-163) and **Ektachrome E200** (E-28), both discontinued slide films.
    Their pushes are modelled (marked); E200's follow Kodak's push table up to EI 800 with
    a gentler contrast rise. E100VS's datasheet curves are close to E100's, so it renders only a
    little more saturated: Kodak's "very saturated" look came largely from interlayer effects the
    datasheet doesn't plot.
- **Compare views.** The bar under the photo has a **Compare** switch (or press **C** to cycle):
  **Split** shows one image with a divider you drag, the comparison on the left and your edit on
  the right; **Side by side** shows the two next to each other, zoomed and panned together (click
  for 100%, then drag). Compare **with** the frame before the film look, or with any other film
  using the same adjustments. **Use film** switches the frame to the compared film and keeps the old
  one on the other side, so you can go back and forth. Hold **B** for before still works anywhere.
- **Export a whole shoot in one look.** The export panel's new **Look** menu sets what the export
  buttons (this frame, selected, edited, whole folder) render with: each frame's own edit (as
  before), the settings of the frame you're editing for all of them, or any film with each frame's
  own exposure and colour adjustments. Saved edits aren't changed. **Film in file name** writes
  `L1002695_portra400.jpg`, so one shoot can be exported in several looks into the same folder.
  The CLI gets the same as `export --film <id> --film-in-name`.

## 0.1.0-alpha.7

- **Saturated colours no longer wash out.** Bright, saturated colours (a yellow car most of all)
  came out pale and creamy on the colour negatives and Kodachrome 64. The cause was a missing
  scanner step: the film's layers see colour through broad, overlapping sensitivities, and their
  scanner readings were shown as screen colours directly, which cost about a fifth of the chroma
  of every saturated colour. Scans now get the colour calibration a lab scanner applies (layer
  primaries back to sRGB, with colours outside sRGB desaturated toward their own grey rather than
  clipped). Neutrals are unchanged. The new **Scanner colour** checkbox (on by default; CLI:
  `--no-scanner-colour`) brings back the old look. Ektachrome E100 and the Endura print are
  viewed through their real dyes and are unaffected.
- **New colour negative: Portra 800.** Built from Kodak publication E-4040 (2025) like Portra 400:
  characteristic curves, layer sensitivities, the negative's measured spectra and MTF. Its
  **Development** menu offers push 1 (EI 1600) and push 2 (EI 3200) from Kodak's own measured
  curves, not modelled. The push curves' slightly different fog per layer gives pushed shadows a
  faint cool-magenta shift, as Kodak's data shows. Grain is estimated from Kodak's Print Grain
  Index (48 at 4x6).
- **Push and pull development.** A new **Development** menu (CLI: `--push <stops>`, negative to
  pull) rates the film faster or slower and develops it to match: pushes get contrastier and lose
  shadow detail, pulls get softer. Exposure follows the rating, and the lab scanner keeps its
  normal-development setup so the push shows.
  - Tri-X 400 uses Kodak's measured D-76 curves from F-4017: pull 1 (6 min), push 1 (normal
    development at EI 800), push 2 (10 min) and push 3 (12 min).
  - Portra 400, Ektar 100, Gold 200 and Ultramax 400 have pull 1, push 1 and push 2, **modelled**
    on the measured Portra 800 push curves (E-4040), since Kodak publishes none for these films.
  - Ektachrome E100 has pull 1, push 1 (Kodak's EI 200 / 8 min first developer) and push 2, with
    modelled curves: steeper about grey and thinner blacks.
  - Kodachrome 64 has none: Kodak recommends against pushing it.
  - Modelled developments are labelled "modelled" in the menu, and its (i) explains the sources.
- **A new look for the app.** The library is a strip of slide film: frames sit between sprocket
  holes with their frame number printed along the edge, and the frame you're editing is outlined.
  Everything around the photo is now strictly neutral grey, so nothing on screen tints how you judge
  its colour. The edit panel is grouped into Film, Adjust, Tune, Copy settings and Export, with
  labels in one column and value boxes of one width. Each Tune slider is centred on the film's
  Kodak-data value, so an untouched film shows its handles in the middle. An (i) beside less obvious settings
  (Output, Fit camera highlights, Halation, the Tune sliders and more) explains them on hover. Compare and zoom moved to a bar under the photo. Type is Barlow.
- **Fix:** opening **Tune** no longer marks the film as changed. The sliders snapped some values to
  their step just by being shown (Kodachrome's 0.003 screen black became 0), so saving changed
  the film without you touching it.
- **First black-and-white film: Tri-X 400.** A new black-and-white model built from Kodak
  publication F-4017: the 400TX characteristic curve for D-76 (8 minutes, Kodak's contrast aim),
  the film's panchromatic spectral sensitivity, its MTF and its published rms granularity of 17.
  One silver image, so the result is exactly neutral and the grain has no colour. Kodak publishes
  no paper for it, so it is shown as a lab scan of the negative; the scanner colour sliders work
  as toning.

## 0.1.0-alpha.6

- **Three new colour negatives: Ektar 100, Gold 200 and Ultramax 400.** Each is built from its own
  Kodak datasheet (E-4046, E-7022, E-7023) the same way as Portra 400: characteristic curves,
  layer sensitivities and the negative's measured spectra, digitised from the PDFs' vector plots.
  Like Portra they are shown as a lab scan by default, or as an optical print on Endura Premier
  paper (E-4070). Ektar uses its published MTF for film sharpness; Kodak
  publishes none for Gold or Ultramax. Grain strengths are estimates from Kodak's Print Grain Index.
  All three use the camera-to-film highlight fit.
- **Highlights fitted to each film.** The camera's brightest recorded tone (about three stops over
  mid-grey on the M9) now lands on the film's white, with a smooth shoulder from one stop over grey.
  Ektachrome and Kodachrome whites reach white instead of stopping at a dull light grey, and the top
  stop keeps more separation. Mid-tones are unchanged. Turn off **Fit camera highlights to film**
  (or pass `--no-fit` to the CLI) for the Kodak datasheet curve exactly as published.
- **Portra 400 is now shown as a lab scan of the negative** by default, the way most people see
  Portra today: a minilab scanner reads the negative above its orange mask, grey-balances it and
  applies a soft tone curve, so the negative's long highlight latitude survives. The optical print
  on Endura Premier is still there: choose **Optical print on Endura Premier** in the Output menu
  (CLI: `--print`).

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
