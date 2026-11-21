//! Core image processing for the M9 film preset app: DNG decoding, colour
//! pipeline and film emulation. Deliberately free of any UI code so it can be
//! reused by the desktop app, the CLI, and (later) other platforms.

pub mod color;
pub mod develop;
pub mod edit;
pub mod exif;
pub mod film;
pub mod output;
pub mod raw;
