# Changelog

## Unreleased

- **Warning for files that aren't from a Leica M9.** The library flags them ("⚠ not M9", with a
  count at the top), and the preview shows a banner naming the camera. They still open and export
  as before. The M9-P, M9 Titanium and M-E count as M9s; Monochrom bodies don't.
- `remjet-cli info`, `render` and `export` print the same warning on stderr for each such file.

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
