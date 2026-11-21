# M9 Film Preset App — Plan

Native Windows desktop app in Rust that develops Leica M9 DNGs with film emulation.
First stock: **Kodachrome 64**. More stocks later as data-only presets. iOS deferred.

**Target look: a scanned Kodachrome 64 slide** (not projected): scanner-style highlight roll-off and
lifted-but-dense shadows rather than a lightbox's full Dmax.

## Status
- Phase 1 (neutral raw render) — done: `film-cli render` develops M9 DNGs colorimetrically
  (DNG dual-illuminant matrices, Bradford to D50, linear ProPhoto working space, PPG demosaic, sRGB JPEG).
  Known gaps: no highlight reconstruction (channels clip at white), simple per-channel gamut clip on output.
- Phase 2 (Kodachrome 64 engine) — first version done: `presets/kodachrome64.toml` + `film-core/src/film.rs`.
  Characteristic curves and dye cross-absorptions digitised from Kodak E-55 (K64 page); layer
  sensitivity, interimage, scanner gamma/balance and grain strength are tuned by eye and still open
  for your feedback (Phase 5). `film-cli render ... --preset kodachrome64 [--ev] [--grain]`.
- Phase 3 (app) — done: `film-app` (egui). Library with thumbnails, live preview, film / exposure /
  warmth / tint / grain controls, hold-B before/after, ←/→ navigation, edits auto-saved to
  `<file>.DNG.m9film.json` sidecars, full-res JPG export to `<folder>/exports`.
  Not yet: EXIF copied into JPGs, zoom to 100%, remembering the last folder.

## Requirements
- Open M9 DNGs (compressed and uncompressed).
- Film look with adjustable exposure, white balance, look strength, grain, vignette.
- **Non-destructive, re-editable** edits: settings saved to a sidecar next to each DNG.
- **Batch**: apply/copy settings across many images, export in the background.
- **Export JPG** (sRGB, embedded ICC profile, EXIF kept, quality + long-edge size options).

## Architecture (Cargo workspace)
| Crate | Role |
|---|---|
| `film-core` | DNG decode, colour pipeline, film model, grain. No UI. Reusable later for iOS. |
| `film-presets` | Preset files (TOML/JSON: curves, matrices, grain params, version). |
| `film-app` | GUI (egui/eframe): library grid, editor, before/after, batch queue. |
| `film-cli` | Command-line batch export (useful for testing and scripting). |

Key choices (to confirm in Phase 1):
- Raw decode: `rawler` (pure Rust, from dnglab) — fallback: LibRaw bindings.
- Processing: CPU + `rayon` for full-res export (accuracy first); GPU (`wgpu`) for live preview later if needed.
- Sidecar: `L1000123.DNG.m9film.json` storing preset id + version and all slider values.

## Image pipeline
1. Decode DNG → linearise (M9 compressed table), black/white levels.
2. White balance (AsShotNeutral or user), demosaic (fast for preview, high quality for export).
3. Camera RGB → XYZ via DNG ColorMatrix/ForwardMatrix → linear wide-gamut working space.
4. Film model: per-layer exposure → characteristic (H&D) curves → inter-layer colour coupling → dye densities → display render. Baked into a 3D LUT per preset.
5. Grain (density-dependent), vignette, output transform → sRGB JPG.

## Phases
0. **Foundations** — workspace, sample DNGs, Kodachrome references, Kodak datasheet data.
1. **Raw pipeline** — neutral, accurate render; validate against Lightroom/RawTherapee.
2. **Kodachrome 64 engine** — film model + LUT + grain; tune against references.
3. **App** — library, editor, sidecars, JPG export.
4. **Batch** — copy/paste settings, export queue with progress.
5. **Tuning with your eye** — side-by-side iterations.
6. **More stocks.**

## Kodachrome reference sources
- Wikimedia Commons, [Photographs taken on Kodachrome 64 film](https://commons.wikimedia.org/wiki/Category:Photographs_taken_on_Kodachrome_64_film) — 52 freely licensed, labelled K64 scans (mixed scanners).
- Library of Congress, John Margolies Roadside America — ~11,700 slide scans, no known restrictions; mostly Kodachrome 25 (same K-14 family).
- Flickr tag/group "Kodachrome 64" — many; personal reference only unless CC-licensed.
- Kodak datasheets: E-55 (Kodachrome 25/64/200 Professional, 2009) and E-88 (consumer KR 64), indexed at 125px.com/techdocs/kodak.
- Best ground truth: buy a lot of K64 slides and copy them with the M9 on a light table (same camera pipeline as our inputs).

## Your to-dos
- [ ] 20–30 sample M9 DNGs in `samples/` (varied light, skin, sky, reds, foliage, contrast, compressed + uncompressed, ideally a ColorChecker).
- [ ] Kodachrome 64 references (your scans ideally) in `references/` + note: projected-slide look or scanned look?
- [ ] Optional: install VS Code + rust-analyzer extension if you want to read/edit code yourself.
