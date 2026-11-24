//! Core image processing for Remjet: DNG decoding, colour
//! pipeline and film emulation. Deliberately free of any UI code so it can be
//! reused by the desktop app, the CLI, and (later) other platforms.

pub mod color;
pub mod develop;
pub mod edit;
pub mod exif;
pub mod film;
pub mod icc;
pub mod output;
pub mod paths;
pub mod presets;
pub mod raw;
