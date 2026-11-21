# Changelog

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
