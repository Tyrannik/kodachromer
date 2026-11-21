# Development notes

How Kodachromer is put together, for anyone reading or extending the code.

## Workspace

| Crate | Role |
|---|---|
| `crates/kodachromer-core` | Everything that touches pixels: DNG decoding, colour science, the film model, grain, halation, edits/sidecars, export, EXIF. No UI. |
| `crates/kodachromer` | The desktop app (egui/eframe). Library, preview worker, editor, film tuning, batch export. |
| `crates/kodachromer-cli` | Command-line `info`, `render` and `export` (uses the app's saved edits). |
| `presets/` | Film presets as TOML data. `kodachrome64.toml` is compiled in as the built-in film. |
| `assets/` | Icon. |

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
5. **Output** (`output.rs`, `exif.rs`): sRGB encode, orientation, JPEG (4:4:4, optimised tables) with
   a minimal EXIF block built from the DNG's metadata.

## The app

- A preview worker thread renders three kinds of output: a downscaled *fit* preview (fast enough for
  live sliders), a quick *patch* of the visible area at 100%, and the *full* frame, which becomes a
  mipmapped GPU texture so panning and zooming (Ctrl+scroll up to 800%) never re-render.
- Edits auto-save to `<name>.DNG.kodachromer.json` beside each DNG (pre-release `.m9film.json` files
  are read and migrated on save). App settings and tuned films live in `%APPDATA%\Kodachromer`.
- "Tune look" edits a copy of the film preset live; Save writes a user override, which preview,
  thumbnails, export and the CLI all pick up.

## Film data sources

- Kodak publication **E-55**, *KODACHROME 25, 64 and 200 Professional Film* (06/2009), Kodachrome 64
  page: characteristic curves (Status A, K-14, daylight), spectral dye-density curves and diffuse rms
  granularity. The PDF's plots are vector paths; they were digitised from the path coordinates.
- Look parameters marked as tunable in the preset (sensitivity overlap, interimage, scanner gamma and
  balance, grain strength, halation) were set by eye against scanned K64 slides.

## Tests

`cargo test --release` covers the colour maths, the film model (neutral exposures reproduce the
datasheet curve; grey scans neutral; highlight shoulder; halation; near-black noise is not
amplified; cropped renders have identical grain), sidecars and their migration, EXIF structure, and
the orientation maths used by zoom.

## Ideas / roadmap

- More films as data presets (Ektachrome, Kodachrome 25, Velvia, Portra, Tri-X).
- Highlight reconstruction for clipped raw channels.
- Lens corrections (M9 vignetting/colour shading), crop and straighten.
- Embedded ICC profile in exports; TIFF export.
- Installer and code signing; macOS/iOS builds (the core is UI-free for that reason).
