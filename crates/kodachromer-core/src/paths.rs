//! Where Kodachromer keeps its own files: `%APPDATA%\Kodachromer`.
//!
//! Earlier development builds used `%APPDATA%\M9Film`; on first use that
//! folder is moved across so settings and tuned films carry over.

use std::path::PathBuf;
use std::sync::OnceLock;

pub const APP_NAME: &str = "Kodachromer";
const LEGACY_DIR: &str = "M9Film";

/// The app's data folder (not created here). `KODACHROMER_DATA_DIR`
/// overrides it (portable use, testing).
pub fn data_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Some(dir) = std::env::var_os("KODACHROMER_DATA_DIR").filter(|d| !d.is_empty()) {
            return Some(PathBuf::from(dir));
        }
        let base = PathBuf::from(std::env::var_os("APPDATA")?);
        let dir = base.join(APP_NAME);
        let legacy = base.join(LEGACY_DIR);
        if !dir.exists() && legacy.is_dir() {
            // Best effort: if the move fails, fall back to copying nothing and
            // starting fresh rather than failing to start.
            let _ = std::fs::rename(&legacy, &dir);
        }
        Some(dir)
    })
    .clone()
}
