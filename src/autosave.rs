//! Autosave file naming, persistence, discovery, and cleanup.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use crate::document::Document;

/// One recoverable autosave and the project path it belongs to, if any.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recovery {
    /// Autosave file to load or discard.
    pub autosave_path: PathBuf,
    /// Original project path for sibling autosaves.
    pub project_path: Option<PathBuf>,
}

/// Returns the autosave path for a saved or untitled project.
pub fn path_for(project_path: Option<&Path>) -> PathBuf {
    if let Some(path) = project_path {
        let mut name: OsString = path.as_os_str().to_owned();
        name.push(".autosave");
        return PathBuf::from(name);
    }
    crate::settings::settings_dir()
        .join("autosave")
        .join(format!("untitled-{}.ductile", std::process::id()))
}

/// Writes an autosave immediately, creating the untitled directory when needed.
pub fn write(document: &mut Document, project_path: Option<&Path>) -> Result<PathBuf, String> {
    let path = path_for(project_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    document.save_to(&path)?;
    Ok(path)
}

/// Removes the sibling autosave after a successful manual save.
pub fn clean(project_path: &Path) -> Result<(), String> {
    let path = path_for(Some(project_path));
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Finds newer sibling autosaves and orphaned untitled autosaves.
pub fn discover(projects: &[PathBuf]) -> Vec<Recovery> {
    let mut recoveries = Vec::new();
    for project in projects {
        let autosave = path_for(Some(project));
        let Ok(autosave_meta) = fs::metadata(&autosave) else {
            continue;
        };
        let newer = fs::metadata(project)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .zip(autosave_meta.modified().ok())
            .is_none_or(|(original, backup)| backup > original);
        if newer {
            recoveries.push(Recovery {
                autosave_path: autosave,
                project_path: Some(project.clone()),
            });
        }
    }
    let untitled_dir = crate::settings::settings_dir().join("autosave");
    if let Ok(entries) = fs::read_dir(untitled_dir) {
        recoveries.extend(entries.flatten().filter_map(|entry| {
            let path = entry.path();
            crate::app::is_project_file(&path).then_some(Recovery {
                autosave_path: path,
                project_path: None,
            })
        }));
    }
    recoveries.sort_by_key(|recovery| {
        fs::metadata(&recovery.autosave_path)
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    recoveries.reverse();
    recoveries
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use glam::DVec3;

    use super::*;
    use crate::history::PrimitiveKind;

    #[test]
    fn writes_discovers_and_cleans_autosaves() {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let directory = tempfile::tempdir().unwrap();
        // SAFETY: this test serializes access to the process environment.
        unsafe { std::env::set_var("DUCTILE_SETTINGS_DIR", directory.path()) };
        let project = directory.path().join("part.ductile");
        let mut document = Document::new();
        document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::splat(10.0),
        });
        let sibling = write(&mut document, Some(&project)).unwrap();
        assert_eq!(
            sibling,
            PathBuf::from(format!("{}.autosave", project.display()))
        );
        assert!(
            discover(std::slice::from_ref(&project))
                .iter()
                .any(|item| item.autosave_path == sibling)
        );
        clean(&project).unwrap();
        assert!(!sibling.exists());

        let untitled = write(&mut document, None).unwrap();
        assert!(untitled.exists());
        assert!(
            discover(&[])
                .iter()
                .any(|item| item.autosave_path == untitled)
        );
        // SAFETY: protected by the same process-local test mutex.
        unsafe { std::env::remove_var("DUCTILE_SETTINGS_DIR") };
    }
}
