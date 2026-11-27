//! Saved looks: a named combination of film, output, development and
//! adjustments (highlight fit, scanner colour, grain, ...) that can be applied
//! to other frames and used for batch export.
//!
//! Each look is a small JSON file in `%APPDATA%\Remjet\looks\<id>.json`.
//! Settings missing from a file (because it was saved before they existed)
//! take their defaults, and settings a file has that this version doesn't
//! know are ignored, so looks stay readable across versions.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::edit::Edit;

const LOOK_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    pub version: u32,
    pub name: String,
    /// Applying the look also sets exposure, warmth and tint. Off (the
    /// default) keeps each frame's own, since those usually differ per shot.
    pub include_exposure: bool,
    pub settings: Edit,
}

impl Default for Look {
    fn default() -> Self {
        Self { version: LOOK_VERSION, name: String::new(), include_exposure: false, settings: Edit::default() }
    }
}

impl Look {
    pub fn new(name: &str, settings: &Edit, include_exposure: bool) -> Self {
        Self { name: name.trim().to_string(), include_exposure, settings: settings.clone(), ..Default::default() }
    }

    /// `base` (a frame's edit) with this look applied.
    pub fn apply(&self, base: &Edit) -> Edit {
        let mut e = Edit { version: base.version, ..self.settings.clone() };
        if !self.include_exposure {
            e.exposure_ev = base.exposure_ev;
            e.warmth = base.warmth;
            e.tint = base.tint;
        }
        e
    }

    /// True if applying the look would leave `edit` unchanged.
    pub fn matches(&self, edit: &Edit) -> bool {
        self.apply(edit) == *edit
    }
}

pub fn dir() -> Option<PathBuf> {
    crate::paths::data_dir().map(|d| d.join("looks"))
}

/// A file-name-safe id from a look's name ("Portra, warm!" -> "portra-warm").
pub fn slug(name: &str) -> String {
    let s = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if s.is_empty() { "look".into() } else { s }
}

fn file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// All looks in `dir` as (id, look), sorted by name. Unreadable files are skipped.
pub fn list_in(dir: &Path) -> Vec<(String, Look)> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for path in rd.flatten().map(|e| e.path()) {
            if path.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let Some(id) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
            if let Ok(look) = load_in(dir, &id) {
                out.push((id, look));
            }
        }
    }
    out.sort_by(|a, b| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()).then_with(|| a.0.cmp(&b.0)));
    out
}

pub fn load_in(dir: &Path, id: &str) -> Result<Look> {
    let path = file(dir, id);
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut look: Look = serde_json::from_str(&text).with_context(|| format!("in {}", path.display()))?;
    look.settings = look.settings.migrated();
    if look.name.trim().is_empty() {
        look.name = id.to_string();
    }
    Ok(look)
}

/// Save a look and return its id. A look with the same name (ignoring case)
/// is replaced; otherwise a new file is made.
pub fn save_in(dir: &Path, look: &Look) -> Result<String> {
    let existing = list_in(dir);
    let key = look.name.trim().to_lowercase();
    let id = match existing.iter().find(|(_, l)| l.name.trim().to_lowercase() == key) {
        Some((id, _)) => id.clone(),
        None => {
            let base = slug(&look.name);
            let mut id = base.clone();
            let mut n = 2;
            while existing.iter().any(|(i, _)| *i == id) {
                id = format!("{base}-{n}");
                n += 1;
            }
            id
        }
    };
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let look = Look { version: LOOK_VERSION, name: look.name.trim().to_string(), ..look.clone() };
    let path = file(dir, &id);
    std::fs::write(&path, serde_json::to_string_pretty(&look)?).with_context(|| format!("writing {}", path.display()))?;
    Ok(id)
}

pub fn delete_in(dir: &Path, id: &str) -> Result<()> {
    let path = file(dir, id);
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

/// Find a look by id or by name (ignoring case).
pub fn find_in(dir: &Path, id_or_name: &str) -> Option<(String, Look)> {
    let key = id_or_name.trim().to_lowercase();
    let all = list_in(dir);
    let by_id = all.iter().position(|(id, _)| *id == key);
    let by_name = || all.iter().position(|(_, l)| l.name.trim().to_lowercase() == key);
    by_id.or_else(by_name).map(|i| all[i].clone())
}

const NO_DIR: &str = "no app data folder (APPDATA is not set)";

pub fn list() -> Vec<(String, Look)> {
    dir().map(|d| list_in(&d)).unwrap_or_default()
}

pub fn save(look: &Look) -> Result<String> {
    save_in(&dir().context(NO_DIR)?, look)
}

pub fn delete(id: &str) -> Result<()> {
    delete_in(&dir().context(NO_DIR)?, id)
}

pub fn find(id_or_name: &str) -> Option<(String, Look)> {
    find_in(&dir()?, id_or_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::OutputMode;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("remjet-looks-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn portra_push() -> Edit {
        Edit {
            preset: Some("portra800".into()),
            push: 1,
            output: OutputMode::Print,
            fit_highlights: false,
            grain: 1.6,
            exposure_ev: 0.7,
            warmth: 0.3,
            tint: -0.1,
            ..Default::default()
        }
    }

    #[test]
    fn applying_keeps_the_frames_exposure_and_white_balance() {
        let look = Look::new("Portra pushed", &portra_push(), false);
        let frame = Edit { exposure_ev: -0.5, warmth: -0.2, tint: 0.1, preset: Some("kodachrome64".into()), ..Default::default() };
        let e = look.apply(&frame);
        assert_eq!(e.preset.as_deref(), Some("portra800"));
        assert_eq!((e.push, e.output, e.fit_highlights, e.grain), (1, OutputMode::Print, false, 1.6));
        assert_eq!((e.exposure_ev, e.warmth, e.tint), (-0.5, -0.2, 0.1));
        assert!(look.matches(&e) && !look.matches(&frame));

        let with_exp = Look::new("Portra pushed", &portra_push(), true);
        assert_eq!(with_exp.apply(&frame), portra_push());
    }

    #[test]
    fn looks_round_trip_and_same_name_replaces() {
        let dir = temp("store");
        assert!(list_in(&dir).is_empty());
        let id = save_in(&dir, &Look::new("  Portra, pushed! ", &portra_push(), false)).unwrap();
        assert_eq!(id, "portra-pushed");
        let loaded = load_in(&dir, &id).unwrap();
        assert_eq!(loaded.name, "Portra, pushed!");
        assert_eq!(loaded.settings, portra_push());

        // Same name (any case) updates the same file.
        let k64 = Edit { preset: Some("kodachrome64".into()), ..Default::default() };
        assert_eq!(save_in(&dir, &Look::new("portra, PUSHED!", &k64, false)).unwrap(), id);
        assert_eq!(list_in(&dir).len(), 1);
        // A different name with the same slug gets its own file.
        assert_eq!(save_in(&dir, &Look::new("Portra pushed", &k64, false)).unwrap(), "portra-pushed-2");
        assert_eq!(find_in(&dir, "PORTRA PUSHED").unwrap().0, "portra-pushed-2");
        assert_eq!(find_in(&dir, "portra-pushed").unwrap().1.name, "portra, PUSHED!");

        delete_in(&dir, &id).unwrap();
        assert_eq!(list_in(&dir).len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn old_and_newer_files_still_load() {
        let dir = temp("compat");
        std::fs::create_dir_all(&dir).unwrap();
        // Saved before most settings existed: they take their defaults.
        std::fs::write(file(&dir, "old"), r#"{"name": "Old", "settings": {"preset": "portra400", "grain": 2.0}}"#).unwrap();
        // Saved by a later version with settings this one doesn't know.
        std::fs::write(
            file(&dir, "new"),
            r#"{"version": 9, "name": "New", "future_option": true, "settings": {"preset": "trix400", "bloom": 0.4}}"#,
        )
        .unwrap();
        std::fs::write(file(&dir, "broken"), "not json").unwrap();
        let all = list_in(&dir);
        assert_eq!(all.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["new", "old"]);
        let old = &all[1].1;
        assert_eq!((old.settings.grain, old.settings.scanner_colour, old.include_exposure), (2.0, true, false));
        assert_eq!(all[0].1.settings.preset.as_deref(), Some("trix400"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
