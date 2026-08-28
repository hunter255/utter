//! Stable application identity and one-time storage migration.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;

pub const APP_IDENTIFIER: &str = "io.github.hunter255.utter";
pub const LEGACY_STORAGE_IDENTIFIER: &str = "dev.utter.utter";

const CONFIG_FILE: &str = "config.toml";
const HISTORY_FILE: &str = "history.sqlite3";
const MODELS_DIR: &str = "models";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct StorageMigrationReport {
    pub migrated: Vec<&'static str>,
    pub warnings: Vec<String>,
}

pub fn config_path() -> PathBuf {
    current_dirs()
        .map(|dirs| dirs.config_dir().join(CONFIG_FILE))
        .unwrap_or_else(|| PathBuf::from(CONFIG_FILE))
}

pub fn data_dir() -> Result<PathBuf> {
    current_dirs()
        .map(|dirs| dirs.data_dir().to_path_buf())
        .context("failed to resolve the platform data directory")
}

/// Migrates only the three artifacts the app owns. Existing destinations
/// always win; each failure is reported without blocking the other moves.
/// Settings and history stay behind as small backups, while models are
/// renamed so multi-gigabyte downloads are not duplicated.
pub fn migrate_legacy_storage() -> StorageMigrationReport {
    let (Some(legacy), Some(current)) = (legacy_dirs(), current_dirs()) else {
        return StorageMigrationReport::default();
    };
    migrate_paths(
        &legacy.config_dir().join(CONFIG_FILE),
        &current.config_dir().join(CONFIG_FILE),
        legacy.data_dir(),
        current.data_dir(),
    )
}

fn current_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("io.github", "hunter255", "utter")
}

fn legacy_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("dev", "utter", "utter")
}

fn migrate_paths(
    legacy_config: &Path,
    current_config: &Path,
    legacy_data: &Path,
    current_data: &Path,
) -> StorageMigrationReport {
    let mut report = StorageMigrationReport::default();
    migrate_artifact(
        "settings",
        legacy_config,
        current_config,
        false,
        &mut report,
    );
    migrate_artifact(
        "history",
        &legacy_data.join(HISTORY_FILE),
        &current_data.join(HISTORY_FILE),
        false,
        &mut report,
    );
    migrate_artifact(
        "models",
        &legacy_data.join(MODELS_DIR),
        &current_data.join(MODELS_DIR),
        true,
        &mut report,
    );
    report
}

fn migrate_artifact(
    label: &'static str,
    source: &Path,
    destination: &Path,
    move_instead_of_copy: bool,
    report: &mut StorageMigrationReport,
) {
    if source == destination || !source.exists() || destination.exists() {
        return;
    }

    let result = destination
        .parent()
        .context("destination has no parent")
        .and_then(|parent| fs::create_dir_all(parent).context("failed to create destination"))
        .and_then(|()| {
            if move_instead_of_copy {
                fs::rename(source, destination).context("failed to move artifact")
            } else {
                fs::copy(source, destination)
                    .map(|_| ())
                    .context("failed to copy artifact")
            }
        });

    match result {
        Ok(()) => report.migrated.push(label),
        Err(error) => {
            // A failed copy may have left a new partial file. It cannot be a
            // pre-existing user file because that case returned above.
            if !move_instead_of_copy {
                let _ = fs::remove_file(destination);
            }
            report.warnings.push(format!(
                "Could not migrate {label} from {} to {}: {error:#}",
                source.display(),
                destination.display()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn migrate(legacy: &Path, current: &Path) -> StorageMigrationReport {
        migrate_paths(
            &legacy.join(CONFIG_FILE),
            &current.join(CONFIG_FILE),
            legacy,
            current,
        )
    }

    #[test]
    fn fresh_install_creates_nothing() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current");
        assert_eq!(
            migrate(&root.path().join("legacy"), &current),
            StorageMigrationReport::default()
        );
        assert!(!current.exists());
    }

    #[test]
    fn migrates_existing_data_and_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("legacy");
        let current = root.path().join("current");
        write(&legacy.join(CONFIG_FILE), "settings");
        write(&legacy.join(HISTORY_FILE), "history");
        write(&legacy.join(MODELS_DIR).join("model.bin"), "model");

        assert_eq!(
            migrate(&legacy, &current).migrated,
            ["settings", "history", "models"]
        );
        assert!(migrate(&legacy, &current).migrated.is_empty());
        assert_eq!(
            fs::read_to_string(current.join(CONFIG_FILE)).unwrap(),
            "settings"
        );
        assert_eq!(
            fs::read_to_string(current.join(HISTORY_FILE)).unwrap(),
            "history"
        );
        assert!(current.join(MODELS_DIR).join("model.bin").exists());
    }

    #[test]
    fn existing_new_data_is_never_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("legacy");
        let current = root.path().join("current");
        write(&legacy.join(CONFIG_FILE), "old");
        write(&current.join(CONFIG_FILE), "new");

        assert!(migrate(&legacy, &current).migrated.is_empty());
        assert_eq!(
            fs::read_to_string(current.join(CONFIG_FILE)).unwrap(),
            "new"
        );
    }

    #[test]
    fn one_failure_does_not_block_other_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let legacy_data = root.path().join("legacy-data");
        let current_data = root.path().join("current-data");
        let blocker = root.path().join("not-a-directory");
        write(&root.path().join("legacy-config.toml"), "settings");
        fs::write(&blocker, "file").unwrap();
        write(&legacy_data.join(HISTORY_FILE), "history");
        write(&legacy_data.join(MODELS_DIR).join("model.bin"), "model");

        let report = migrate_paths(
            &root.path().join("legacy-config.toml"),
            &blocker.join(CONFIG_FILE),
            &legacy_data,
            &current_data,
        );

        assert_eq!(report.migrated, ["history", "models"]);
        assert_eq!(report.warnings.len(), 1);
    }
}
