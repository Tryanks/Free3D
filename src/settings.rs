//! Small JSON persistence layer for application-level preferences.

use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{i18n::LangChoice, nav::NavPreset, units::Units};

/// Preferences persisted independently from a design document.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Settings {
    /// Whether the dark theme is active.
    pub dark_theme: bool,
    /// Active navigation mapping.
    pub nav_preset: NavPreset,
    /// Active display/input unit.
    pub units: Units,
    /// Interface language selection.
    pub language: LangChoice,
    /// Autosave cadence in seconds; zero disables autosave.
    pub autosave_interval_secs: u64,
    /// Whether the viewport tool banner is collapsed to its title.
    pub tool_banner_collapsed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark_theme: false,
            nav_preset: NavPreset::default(),
            units: Units::default(),
            language: LangChoice::Auto,
            autosave_interval_secs: 180,
            tool_banner_collapsed: false,
        }
    }
}

pub(crate) fn settings_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("DUCTILE_SETTINGS_DIR") {
        return PathBuf::from(path);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Ductile")
}

/// Moves the small persisted application state into the renamed config folder once.
pub(crate) fn migrate_legacy_config() {
    if std::env::var_os("DUCTILE_SETTINGS_DIR").is_some() {
        return;
    }
    let Some(config) = dirs::config_dir() else {
        return;
    };
    let old = config.join("Free3D");
    let new = config.join("Ductile");
    if new.exists() || !old.is_dir() {
        return;
    }
    let _ = fs::create_dir_all(&new);
    for name in ["recent.json", "settings.json"] {
        let _ = fs::copy(old.join(name), new.join(name));
    }
    copy_directory_best_effort(&old.join("autosave"), &new.join("autosave"));
}

fn copy_directory_best_effort(source: &std::path::Path, destination: &std::path::Path) {
    let Ok(entries) = fs::read_dir(source) else {
        return;
    };
    let _ = fs::create_dir_all(destination);
    for entry in entries.flatten() {
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_directory_best_effort(&source_path, &destination_path);
        } else {
            let _ = fs::copy(source_path, destination_path);
        }
    }
}

fn settings_path() -> PathBuf {
    settings_dir().join("settings.json")
}

/// Loads preferences, falling back to defaults for missing or invalid JSON.
pub fn load() -> Settings {
    fs::read(settings_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Persists preferences. Failures are returned so callers can log without crashing.
pub fn save(settings: Settings) -> Result<(), String> {
    let directory = settings_dir();
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(&settings).map_err(|error| error.to_string())?;
    fs::write(settings_path(), bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use super::*;

    #[test]
    fn persistence_roundtrip_uses_override_directory() {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let directory = tempfile::tempdir().unwrap();
        // SAFETY: this test serializes access to the process environment.
        unsafe { std::env::set_var("DUCTILE_SETTINGS_DIR", directory.path()) };
        let expected = Settings {
            dark_theme: true,
            nav_preset: NavPreset::Blender,
            units: Units::Inch,
            language: LangChoice::ZhCn,
            autosave_interval_secs: 90,
            tool_banner_collapsed: true,
        };
        save(expected).unwrap();
        assert_eq!(load(), expected);
        // SAFETY: protected by the same process-local test mutex.
        unsafe { std::env::remove_var("DUCTILE_SETTINGS_DIR") };
    }

    #[test]
    fn language_serde_roundtrip_and_legacy_default() {
        let settings = Settings {
            language: LangChoice::ZhCn,
            ..Settings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(
            serde_json::from_str::<Settings>(&json).unwrap().language,
            LangChoice::ZhCn
        );
        let legacy = r#"{"dark_theme":false,"nav_preset":"DuctileDefault","units":"Millimeter","autosave_interval_secs":180}"#;
        assert_eq!(
            serde_json::from_str::<Settings>(legacy).unwrap().language,
            LangChoice::Auto
        );
    }
}
