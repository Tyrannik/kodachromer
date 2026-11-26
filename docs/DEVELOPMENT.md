# Development notes

How Remjet is put together, for anyone reading or extending the code.

## Workspace

| Crate | Role |
|---|---|
| `crates/remjet-core` | Everything that touches pixels: DNG decoding, colour science, the film model, grain, halation, edits/sidecars, export, EXIF. No UI. |
| `crates/remjet` | The desktop app (egui/eframe). Library, preview worker, editor, film tuning, batch export. |
| `crates/remjet-cli` | Command-line `info`, `render` and `export` (uses the app's saved edits). |
| `presets/` | Film presets as TOML data, compiled in as the built-in films: `kodachrome64.toml` (matrix model), `ektachrome-e100.toml` and `portra400.toml` (spectral model). |
| `assets/` | Icon, and the Barlow fonts the app embeds (`assets/fonts`, SIL OFL 1.1). |

## Image pipeline

1. **Decode** (`raw.rs`, via [rawler](https://github.com/dnglab/dnglab)): Bayer data, black/white
   levels, as-shot white balance, DNG colour matrices, orientation.
2. **Develop** (`develop.rs`): levels and white balance on the mosaic (channels clipped at white so
   blown highlights stay neutral), PPG demosaic (full) or superpixel (half-size previews), chroma
   noise reduction.
3. **Colour** (`color.rs`): camera RGB → XYZ using the DNG's dual-illuminant ColorMatrix, interpolated
   by the white point's CCT, Bradford-adapted to D50, into **linear ProPhoto RGB** as the working space.
   Exposure and the warmth/tint sliders are applied here; optional luminance sharpening follows.
4. **Film** (`film.rs`):
   - Layer exposures from linear Rec.2020 through a sensitivity matrix; veiling-flare pedestal.
   - Halation (optional): exposure above a threshold is blurred wide and added mostly to the red layer.
   - log10 exposure, contrast and highlight shoulder around mid-grey, interimage coupling.
   - **Analytical dye densities** from the measured Status A curves with dye cross-absorption removed
     (so a neutral exposure reproduces Kodak's curves exactly), then integral density via the dye
     matrix digitised from Kodak's spectral dye-density curves.
   - Grain in density space: clumpy, density-dependent, seeded from absolute frame coordinates so any
     crop renders identical grain to the full frame.
   - **Scanner**: grey balance at the mid-grey density, transmittance with white at the film base and
     a black-point density, scanner gamma and colour balance → display-linear sRGB.
5. **Output** (`output.rs`, `exif.rs`, `icc.rs`): sRGB encode, orientation, JPEG (4:4:4, optimised
   tables) with a minimal EXIF block built from the DNG's metadata and an embedded sRGB ICC profile
   (generated in code, ICC v2.1).

## The app

- A preview worker thread renders three kinds of output: a downscaled *fit* preview (fast enough for
  live sliders), a quick *patch* of the visible area at 100%, and the *full* frame, which becomes a
  mipmapped GPU texture so panning and zooming (Ctrl+scroll up to 800%) never re-render.
- Edits auto-save to an edit store, `%APPDATA%\Remjet\edits\<fingerprint>.json`, keyed by a
  content fingerprint (FNV-1a of file size + first 64 KiB) so they follow moved/renamed files.
  Sidecars written beside DNGs by earlier versions are imported and removed. An edit equal to the
  defaults is deleted (the image is unedited). `REMJET_DATA_DIR` overrides the data folder.
- Undo/redo: each commit (slider release, reset, paste, film change) pushes the previous edit onto
  a per-image, per-session history. Toasts confirm copy, paste, undo and redo.
- Output modes: *Scan* (scanner black point, gamma, balance) and *Projection* (transmittance shown
  directly, screen/room flare as the black floor, lamp colour, radial falloff from frame position).
- The wgpu device's uncaptured-error handler logs instead of panicking, so a bogus window size
  during a resize can't crash the app.
- Look (`theme.rs`): every grey in the UI is neutral (R = G = B) and the photo sits on a slightly
  lighter surround than the panels, so the interface never tints colour judgement. Colour is used in
  one place, the library's film strip (edge-print amber), plus red for warnings. Selected buttons
  and slider fills stay grey for the same reason. Fonts: Barlow, with Barlow Condensed for section
  titles and the edge print; egui's own fonts remain as fallbacks for symbols.
- Sliders round their value to their step every frame they are shown (egui clamps and snaps), so a
  slider's step must be finer than the values it displays, or showing it silently edits them.
- "Tune" edits a copy of the film preset live; Save writes a user override, which preview,
  thumbnails, export and the CLI all pick up.

## Spectral film model

Ektachrome E100 and Portra 400 use `spectral.rs` instead of the Kodachrome matrix model. A preset
with `process = "reversal"` or `"negative"` carries its datasheet as `[datasheet]` (and, for a
negative, its paper as `[print]`); everything else is computed from those spectra when the film is
first used, and cached:

1. **Layer exposure.** The datasheet's log spectral sensitivities, lit by CIE D55, are integrated
   over a set of synthetic reflectances (Gaussian bumps, smooth edges, a flat grey). A 3x3 matrix
   from ProPhoto (D50) to relative layer exposure is fitted by weighted least squares; white
   exposes every layer equally. The grey card lands on `exposure.log_h_ref`: ISO speed with
   standard meter calibration for E100 (log10 8/100), the datasheet's *Log H Ref* for Portra.
2. **Reversal (E-6).** At each log exposure, Newton's method finds the cyan, magenta and yellow dye
   amounts whose combined spectrum reads as the published Status A densities through the ISO 5-3
   Status A response. A neutral exposure reproduces the curves exactly. Grain is added to the dye
   amounts.
3. **Negative (C-41) + print (RA-4).** Per-layer Status M density comes straight from the curves.
   Printing density is Status M times a per-channel gain computed from the negative's mid-scale
   neutral and minimum-density spectra, read through the paper's spectral sensitivity instead of
   Status M. Paper log exposure is printer light minus printing density; printer lights are solved
   so the reference grey prints neutral at `print.grey_density`. The paper's dye amounts come from
   its Status A curves and dye spectra, as in step 2, over a flat base.
4. **Viewing.** Transmittance (or reflectance) spectra under D50 with the CIE 1931 observer give
   XYZ, held in a 41³ table of log XYZ over dye amounts. That is adapted to D65 (Bradford), shown as
   linear sRGB with the film base or paper white as white, and handed to the same scanner stage as
   Kodachrome.
5. **Sharpness.** Each layer's MTF is fitted with `(1 + k) G(σ) − k G(σ_wide)` (the second term
   produces Portra's response above 100%) and applied to that layer's exposure at the camera's pixel
   pitch.

Reference tables are in `cie.rs`: CIE 015:2018 colour-matching functions and daylight basis, and
the ISO 5-3 Status A and M responses. Portra's grain strength and the print grey aim are not in the
datasheets; both are marked as estimates in the preset.

## Film data sources

- Kodak publication **E-55**, *KODACHROME 25, 64 and 200 Professional Film* (06/2009), Kodachrome 64
  page: characteristic curves (Status A, K-14, daylight), spectral dye-density curves and diffuse rms
  granularity. The PDF's plots are vector paths; they were digitised from the path coordinates.
- Kodak publication **E-4000**, *KODAK PROFESSIONAL EKTACHROME Film E100*: characteristic curves
  (Status A, E-6), spectral sensitivity, spectral dye density (and visual neutral), MTF, diffuse
  rms granularity.
- Kodak publication **E-4050**, *KODAK PROFESSIONAL PORTRA 400 Film*: characteristic curves
  (Status M, C-41, with Log H Ref), spectral sensitivity, spectral density of a mid-scale neutral
  and of minimum density, MTF.
- Kodak publication **E-4070**, *KODAK PROFESSIONAL ENDURA PREMIER Paper*: characteristic curves
  (Status A reflection), spectral sensitivity, spectral dye density.
- **ISO 5-3:1995** (ANSI/NAPM IT2.18), Tables 3 and 4: Status A and Status M spectral products.
- **CIE 015:2018**: CIE 1931 2° observer and the daylight basis functions S0, S1, S2.
- All Kodak plots are vector paths in the PDFs and were digitised from the path coordinates.
- Look parameters marked as tunable in the preset (sensitivity overlap, interimage, scanner gamma and
  balance, grain strength, halation) were set by eye against scanned K64 slides.

## Tests

`cargo test --release` covers the colour maths, the film model (neutral exposures reproduce the
datasheet curve; grey scans neutral; highlight shoulder; halation; near-black noise is not
amplified; cropped renders have identical grain), the spectral films (E100 and paper neutrals
reproduce their Status A curves; E100's dyes sum to the published visual neutral of 1.0; Portra's
grey prints neutral at the aim; monotonic ramps from black to white), sidecars and their migration, EXIF structure, and
the orientation maths used by zoom.

## Ideas / roadmap

- More films as data presets (Kodachrome 25, Ektar 100, Portra 160, Tri-X); the catalogue of
  Kodak datasheets is a good starting point.
- A *scanned negative* output for Portra alongside the optical print.
- Highlight reconstruction for clipped raw channels.
- Lens corrections (M9 vignetting/colour shading), crop and straighten.
- Embedded ICC profile in exports; TIFF export.
- Installer and code signing; macOS/iOS builds (the core is UI-free for that reason).
