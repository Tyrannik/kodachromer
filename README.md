# M9 Film Preset App

Windows desktop app (Rust) that develops Leica M9 DNGs with film emulation, starting with Kodachrome 64.
See [PLAN.md](PLAN.md) for the roadmap.

## Layout
- `crates/film-core` – DNG decoding, colour pipeline, film model, edits/sidecars (no UI)
- `crates/film-app` – the desktop app
- `crates/film-cli` – command-line tool
- `presets/` – film presets (data files)

## Run the app
```powershell
cargo run -p film-app --release -- samples
```
Edits are saved next to each DNG as `<name>.DNG.m9film.json`; exports go to `<folder>/exports`.

Shortcuts: ←/→ next/previous, hold B before/after, click image or Z for 100%, Ctrl+scroll to zoom (up to 800%),
drag to pan, Esc or click to fit,
Ctrl+A select all, Ctrl+C / Ctrl+V copy/paste settings, double-click a slider to reset it.

## Command line
```powershell
# Export using each DNG's saved app edits (default output: <folder>\exports)
cargo run -p film-cli --release -- export samples --max-edge 2048 --edited-only
# Render with a preset, ignoring saved edits
cargo run -p film-cli --release -- render samples --preset kodachrome64 --out exports/k64
```
`samples/` and `references/` hold personal images and are not committed.
