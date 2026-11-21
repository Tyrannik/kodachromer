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

## Command line
```powershell
cargo run -p film-cli --release -- render samples --preset kodachrome64 --out exports/k64
```
`samples/` and `references/` hold personal images and are not committed.
