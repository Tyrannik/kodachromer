//! Film preset registry: built-in presets (compiled in) plus the user's own
//! presets and tuned overrides, stored as TOML in
//! `%APPDATA%\Remjet\presets\<id>.toml`. A user file with a built-in's id
//! overrides that built-in everywhere (preview, thumbnails, export, CLI).

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::film::FilmPreset;

/// (id, display name) of the presets compiled into the app.
pub const BUILTINS: &[(&str, &str)] = &[("kodachrome64", "Kodachrome 64")];

pub fn user_dir() -> Option<PathBuf> {
    crate::paths::data_dir().map(|d| d.join("presets"))
}

fn user_file(id: &str) -> Option<PathBuf> {
    user_dir().map(|d| d.join(format!("{id}.toml")))
}

/// Load a preset by id: the user's version if there is one, else built-in.
pub fn load(id: &str) -> Result<FilmPreset> {
    if let Some(path) = user_file(id).filter(|p| p.exists()) {
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        return FilmPreset::from_toml(&text).with_context(|| format!("in {}", path.display()));
    }
    FilmPreset::builtin(id)
}

pub fn is_builtin(id: &str) -> bool {
    BUILTINS.iter().any(|(b, _)| *b == id)
}

/// True if a built-in has been tuned (a user override exists).
pub fn is_overridden(id: &str) -> bool {
    is_builtin(id) && user_file(id).is_some_and(|p| p.exists())
}

/// All presets as (id, name), built-ins first, then user presets by name.
pub fn list() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> =
        BUILTINS.iter().map(|(id, _)| (id.to_string(), load(id).map(|p| p.name).unwrap_or_default())).collect();
    let mut user = Vec::new();
    if let Some(dir) = user_dir() {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let path = e.path();
                let Some(id) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
                if path.extension().is_some_and(|x| x == "toml") && !is_builtin(&id) {
                    if let Ok(p) = load(&id) {
                        user.push((id, p.name));
                    }
                }
            }
        }
    }
    user.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    out.extend(user);
    out
}

pub fn save_user(preset: &FilmPreset) -> Result<PathBuf> {
    let path = user_file(&preset.id).context("APPDATA is not set")?;
    std::fs::create_dir_all(path.parent().expect("has parent"))?;
    std::fs::write(&path, preset.to_toml()?).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Remove a user preset or a built-in's override.
pub fn delete_user(id: &str) -> Result<()> {
    if let Some(path) = user_file(id).filter(|p| p.exists()) {
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

/// A file-name-safe id from a display name, unique among existing presets.
pub fn new_id(name: &str) -> String {
    let base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let base = if base.is_empty() { "preset".to_string() } else { base };
    let taken: Vec<String> = list().into_iter().map(|(id, _)| id).collect();
    let mut id = base.clone();
    let mut n = 2;
    while taken.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_round_trip_preserves_preset() {
        let p = FilmPreset::builtin("kodachrome64").unwrap();
        let back = FilmPreset::from_toml(&p.to_toml().unwrap()).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn ids_are_slugs() {
        assert!(new_id("My K64 — warmer!").starts_with("my-k64-warmer"));
        assert!(new_id("???").starts_with("preset"));
    }
}
