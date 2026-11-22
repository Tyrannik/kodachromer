//! Where Remjet keeps its own files: `%APPDATA%\Remjet`.
//!
//! The app was previously called Kodachromer (`%APPDATA%\Kodachromer`), and
//! earlier development builds used `%APPDATA%\M9Film`. On first use the
//! newest of those folders is moved across so edits, settings and tuned
//! films carry over.

use std::path::PathBuf;
use std::sync::OnceLock;

pub const APP_NAME: &str = "Remjet";
const LEGACY_DIRS: [&str; 2] = ["Kodachromer", "M9Film"];

/// The app's data folder (not created here). `REMJET_DATA_DIR` (or the
/// older `KODACHROMER_DATA_DIR`) overrides it (portable use, testing).
pub fn data_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        for var in ["REMJET_DATA_DIR", "KODACHROMER_DATA_DIR"] {
            if let Some(dir) = std::env::var_os(var).filter(|d| !d.is_empty()) {
                return Some(PathBuf::from(dir));
            }
        }
        let base = PathBuf::from(std::env::var_os("APPDATA")?);
        let dir = base.join(APP_NAME);
        if !dir.exists() {
            if let Some(legacy) = LEGACY_DIRS.iter().map(|d| base.join(d)).find(|d| d.is_dir()) {
                // Best effort: if the move fails (e.g. the old app is still
                // running), start fresh rather than failing to start.
                let _ = std::fs::rename(&legacy, &dir);
            }
        }
        Some(dir)
    })
    .clone()
}
