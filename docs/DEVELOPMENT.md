# Development notes

How Remjet is put together, for anyone reading or extending the code.

## Workspace

| Crate | Role |
|---|---|
| `crates/remjet-core` | Everything that touches pixels: DNG decoding, colour science, the film model, grain, halation, edits/sidecars, export, EXIF. No UI. |
| `crates/remjet` | The desktop app (egui/eframe). Library, preview worker, editor, film tuning, batch export. |
| `crates/remjet-cli` | Command-line `info`, `render` and `export` (uses the app's saved edits). |
| `presets/` | Film presets as TOML data, compiled in as the built-in films: `kodachrome64.toml` (matrix model), `ektachrome-e100.toml`, `ektachrome-e100vs.toml`, `ektachrome-e200.toml`, `portra160.toml`, `portra400.toml`, `portra800.toml`, `ektar100.toml`, `gold200.toml`, `ultramax400.toml`, `trix400.toml`, `tmax100.toml` and `tmax400.toml` (spectral model; Tri-X and T-Max are `process = "monochrome"`). |
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
     a black-point density, scanner gamma and colour balance → display-linear sRGB. Lab-scanned
     negatives and Kodachrome then get a scanner colour matrix (`FilmParams::scanner_colour`):
     the readings are in the layers' wide primaries, so they are mapped back to sRGB
     (inverse of the layer-sensitivity fit for negatives, Rec.2020 → sRGB for Kodachrome), with
     out-of-gamut colours pulled toward the grey of the same luminance.
5. **Output** (`output.rs`, `exif.rs`, `xmp.rs`, `icc.rs`): sRGB encode, orientation, JPEG (4:4:4,
   optimised tables) with a minimal EXIF block built from the DNG's metadata (IFD0, Exif and GPS
   IFDs; the look's summary as ImageDescription), an XMP packet with the look's caption
   (`dc:description`) and every edit setting under the `remjet:` namespace, and an embedded sRGB
   ICC profile (generated in code, ICC v2.1).

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
  titles and the edge print; egui's own fonts remain as fallbacks for symbols. Give any setting that
  isn't self-explanatory an (i): pass its explanation as `info` to `slider`/`row_info`, or call
  `theme::info` beside a checkbox.
- Sliders round their value to their step every frame they are shown (egui clamps and snaps), so a
  slider's step must be finer than the values it displays, or showing it silently edits them.
- "Tune" edits a copy of the film preset live; Save writes a user override, which preview,
  thumbnails, export and the CLI all pick up.

## Spectral film model

Ektachrome E100, E100VS and E200, the colour negatives (Portra 160, Portra 400, Portra 800, Ektar 100, Gold 200, Ultramax 400) and the black-and-white films (Tri-X 400, T-Max 100, T-Max 400) use `spectral.rs` instead of the Kodachrome matrix model. A preset
with `process = "reversal"`, `"negative"` or `"monochrome"` carries its datasheet as `[datasheet]` (and, for a
negative, its paper as `[print]`); everything else is computed from those spectra when the film is
first used, and cached:

1. **Layer exposure.** The datasheet's log spectral sensitivities, lit by CIE D55, are integrated
   over a set of synthetic reflectances (Gaussian bumps, smooth edges, a flat grey). A 3x3 matrix
   from ProPhoto (D50) to relative layer exposure is fitted by weighted least squares; white
   exposes every layer equally. The grey card lands on `exposure.log_h_ref`: ISO speed with
   standard meter calibration for E100 (log10 8/100), the datasheet's *Log H Ref* for Portra.
2. **Black and white.** A `monochrome` preset gives one `neutral` characteristic curve (diffuse
   visual density), one `neutral` sensitivity and one `neutral` MTF; each is used for all three
   channels, so layer exposures, density, grain and the scan are identical and the image is
   exactly neutral. Its grey card uses ISO speed with standard meter calibration (log10 8/400).
   There is no paper data, so it always goes through the lab scanner.
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
   **Lab scan (the default output for negatives).** Instead of printing, a minilab-style scanner
   reads the same printing densities above the mask, normalises each channel at the reference grey
   by its slope there (so the film's own toe and shoulder are kept), and maps the result through a
   logistic tone curve (`LabScan` in `spectral.rs`: grey at density 0.70, mid contrast 1.15, max
   2.4). It is softer than paper, so the negative's highlight latitude survives.
   **Push and pull.** `[[datasheet.development]]` entries list the processes a film supports
   (`push` in stops, a `label` and a `source`). Each gives measured `curves` (Tri-X from F-4017),
   a `model`, or neither (normal development, rated faster). A model derives each channel's curve
   from normal development as `pivot + contrast * (D(log_h + shift) - pivot)`, easing back to the
   film's minimum density below the pivot and optionally capping the maximum lower
   (`max_density_drop`). The C-41 values are fitted to Portra 800's measured Status M push 1 and
   push 2 curves (E-4040; RMS under 0.03 D); E100's are estimates, rated at Kodak's EI. The film is
   rated `push` stops faster (`log_h_ref - push * log10 2`); the lab scanner keeps the slope of
   normal development so the push's contrast shows, and balances grey on the film actually scanned.
4. **Viewing.** Transmittance (or reflectance) spectra under D50 with the CIE 1931 observer give
   XYZ, held in a 41³ table of log XYZ over dye amounts. That is adapted to D65 (Bradford), shown as
   linear sRGB with the film base or paper white as white, and handed to the same scanner stage as
   Kodachrome.
5. **Sharpness.** Each layer's MTF is fitted with `(1 + k) G(σ) − k G(σ_wide)` (the second term
   produces Portra's response above 100%) and applied to that layer's exposure at the camera's pixel
   pitch.

Reference tables are in `cie.rs`: CIE 015:2018 colour-matching functions and daylight basis, and
the ISO 5-3 Status A and M responses. The negatives' grain strengths (Kodak gives only Print Grain Index)
and the print grey aim are not in the datasheets; both are marked as estimates in the presets.

Grain and halation:
- Grain strength comes from the datasheet's diffuse rms granularity (48 µm aperture), converted to
  the camera's pixel size by Selwyn's law. A downscaled render (the fit preview, a small export)
  generates the grain one output pixel would average: std scaled by b / sqrt(b² + s²) and clump size
  sqrt(b² + s²), where b is the grain blur and s = f / (2 sqrt(pi)) is the box filter of the scale
  factor f as a Gaussian. Pushed and pulled developments scale grain by their contrast (mean
  gradient between 0.3 and 1.3 above Dmin) relative to normal development.
- Halation: `[halation] strength` is each film's own (0..1, 1 = no anti-halation protection),
  an estimate by type of protection (rem-jet backing, undercoat, dyed base), since Kodak publishes
  no figure. The glow's sigma is 1.45 x the base thickness (`base_um`, from the datasheet where it
  gives one): totally reflected light returns about 2 t tan 42° from where it entered.

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
- Kodak publication **E-4046**, *KODAK PROFESSIONAL EKTAR 100 Film*: characteristic curves
  (Status M, C-41, with Log H Ref), spectral sensitivity, spectral density of a mid-scale neutral
  and of minimum density, MTF.
- Kodak publications **E-7022**, *KODAK GOLD 200 Film*, and **E-7023**, *KODAK ULTRA MAX 400
  Film*: the same plots as E-4046 except MTF, which they don't publish.
- Kodak publication **F-4017**, *KODAK PROFESSIONAL TRI-X 320 and 400 Films* (12/2016): 400TX
  characteristic curve (diffuse visual, D-76, 8 min large tank), spectral sensitivity, MTF,
  diffuse rms granularity.
- Kodak publication **E-4040**, *KODAK PROFESSIONAL PORTRA 800 Film* (2025): Status M curves at
  EI 800 normal, EI 1600 (push 1) and EI 3200 (push 2) (Portra 800's own developments, and the fit
  for the other colour negatives' push model), spectral sensitivity, spectral density of a
  mid-scale neutral and of minimum density, MTF.
  Other push sources checked: E-116 (Ektapress PJ400/PJ800 push 1 and 2 curves), E-147 (Ektachrome
  P1600 push 2, Status A), E-28 (E200 push times only) and E-55/E-88 (Kodachrome 200 push ratings;
  Kodak does not recommend pushing Kodachrome 64).
- Kodak publication **E-4051**, *KODAK PROFESSIONAL PORTRA 160 Film* (01/2025): the same plots as
  E-4050 (Log H Ref -1.051).
- Kodak publications **F-4016**, *KODAK PROFESSIONAL T-MAX 100 Film* (06/2018), and **F-4043**,
  *KODAK PROFESSIONAL T-MAX 400 Film* (02/2016): characteristic curves (diffuse visual, D-76 small
  tank at 6/7.5/10 and 6/8/11 min), spectral sensitivity (D = 1.0 above D-min curve), MTF, diffuse
  rms granularity.
- Kodak publications **E-163**, *KODAK PROFESSIONAL EKTACHROME Film E100VS*, and **E-28**, *KODAK
  PROFESSIONAL EKTACHROME Film E200* (2005, also its push table): the same plots as E-4000. These
  older PDFs draw several curves in one path, and E-28's green MTF as many small filled pieces.
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

- More films as data presets (Kodachrome 25, Portra 160, T-Max 100/400); the catalogue of
  Kodak datasheets is a good starting point.
- Highlight reconstruction for clipped raw channels.
- Lens corrections (M9 vignetting/colour shading), crop and straighten.
- Embedded ICC profile in exports; TIFF export.
- Installer and code signing; macOS/iOS builds (the core is UI-free for that reason).
