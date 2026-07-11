//! Small JSON persistence layer for application-level preferences.

use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{nav::NavPreset, units::Units};

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
    /// Autosave cadence in seconds; zero disables autosave.
    pub autosave_interval_secs: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark_theme: false,
            nav_preset: NavPreset::default(),
            units: Units::default(),
            autosave_interval_secs: 180,
        }
    }
}

pub(crate) fn settings_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("FREE3D_SETTINGS_DIR") {
        return PathBuf::from(path);
    }
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join("Library/Application Support/Free3D")
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
        unsafe { std::env::set_var("FREE3D_SETTINGS_DIR", directory.path()) };
        let expected = Settings {
            dark_theme: true,
            nav_preset: NavPreset::Blender,
            units: Units::Inch,
            autosave_interval_secs: 90,
        };
        save(expected).unwrap();
        assert_eq!(load(), expected);
        // SAFETY: protected by the same process-local test mutex.
        unsafe { std::env::remove_var("FREE3D_SETTINGS_DIR") };
    }
}
