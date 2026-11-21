# M9 Film Preset App

Windows desktop app (Rust) that develops Leica M9 DNGs with film emulation, starting with Kodachrome 64.
See [PLAN.md](PLAN.md) for the roadmap.

## Layout
- `crates/film-core` – DNG decoding, colour pipeline, film model (no UI)
- `crates/film-cli` – command-line tool

## Try it
```powershell
cargo run -p film-cli -- info samples
```
`samples/` and `references/` hold personal images and are not committed.
