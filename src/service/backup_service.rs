//! Display configuration backup, atomic writing, and restore.
//!
//! The display configuration file is user-owned. Before the manager changes it
//! the first time, a pristine snapshot (`display.kdl.bak`) is created. Every
//! write is atomic: content goes to a temporary file in the same directory, is
//! flushed to disk, and is renamed over the target, so a crash can never leave
//! a truncated config behind.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::infrastructure::kdl_parser;

/// Result of creating the pristine snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotOutcome {
    Created,
    AlreadyExists,
}

/// Result of restoring the pristine state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreOutcome {
    RestoredFromBackup,
    RemovedManagedSection,
    NoChange,
}

/// Errors raised while reading or writing configuration files.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to back up {} to {}: {source}", source_path.display(), backup_path.display())]
    Backup {
        source_path: PathBuf,
        backup_path: PathBuf,
        source: std::io::Error,
    },
}

/// Abstraction over the display configuration file.
pub trait ConfigStore: Send + Sync {
    fn read_display_config(&self) -> Result<String, ConfigError>;

    /// Create the one-time pristine snapshot if it does not exist yet.
    fn snapshot(&self) -> Result<SnapshotOutcome, ConfigError>;

    /// Write the managed section, replacing any previous managed section.
    fn write_managed_section(&self, section: &str) -> Result<(), ConfigError>;

    /// Restore the pristine snapshot, or remove the managed section when no
    /// snapshot exists. This operation is idempotent.
    fn restore_snapshot(&self) -> Result<RestoreOutcome, ConfigError>;

    fn display_config_path(&self) -> &Path;

    fn main_config_path(&self) -> &Path;

    /// Whether the display file is registered through `include` in the main
    /// niri configuration.
    fn is_registered_in_main_config(&self) -> Result<bool, ConfigError>;
}

/// Production store backed by the filesystem.
#[derive(Debug, Clone)]
pub struct FileConfigStore {
    display_path: PathBuf,
    backup_path: PathBuf,
    main_config_path: PathBuf,
}

impl FileConfigStore {
    pub fn new(display_path: PathBuf, backup_path: PathBuf, main_config_path: PathBuf) -> Self {
        Self {
            display_path,
            backup_path,
            main_config_path,
        }
    }

    /// Build paths from the environment.
    ///
    /// Precedence: `$NDM_CONFIG_DIR` (tests and advanced setups),
    /// `$XDG_CONFIG_HOME`, `$HOME/.config`, then the temporary directory.
    pub fn from_environment() -> Self {
        let base = std::env::var_os("NDM_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(std::env::temp_dir);
        let niri = base.join("niri");
        Self::new(
            niri.join("cfg").join("display.kdl"),
            niri.join("cfg").join("display.kdl.bak"),
            niri.join("config.kdl"),
        )
    }

    fn read_or_empty(path: &Path) -> Result<String, ConfigError> {
        match fs::read_to_string(path) {
            Ok(contents) => Ok(contents),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }
}

impl ConfigStore for FileConfigStore {
    fn read_display_config(&self) -> Result<String, ConfigError> {
        Self::read_or_empty(&self.display_path)
    }

    fn snapshot(&self) -> Result<SnapshotOutcome, ConfigError> {
        if self.backup_path.exists() {
            return Ok(SnapshotOutcome::AlreadyExists);
        }
        let contents = Self::read_or_empty(&self.display_path)?;
        atomic_write(&self.backup_path, &contents)?;
        Ok(SnapshotOutcome::Created)
    }

    fn write_managed_section(&self, section: &str) -> Result<(), ConfigError> {
        let original = Self::read_or_empty(&self.display_path)?;
        let updated = kdl_parser::splice(&original, section);
        atomic_write(&self.display_path, &updated)
    }

    fn restore_snapshot(&self) -> Result<RestoreOutcome, ConfigError> {
        if self.backup_path.exists() {
            let contents = Self::read_or_empty(&self.backup_path)?;
            atomic_write(&self.display_path, &contents)?;
            return Ok(RestoreOutcome::RestoredFromBackup);
        }
        let original = Self::read_or_empty(&self.display_path)?;
        let (updated, removed) = kdl_parser::remove_section(&original);
        if removed {
            atomic_write(&self.display_path, &updated)?;
            return Ok(RestoreOutcome::RemovedManagedSection);
        }
        Ok(RestoreOutcome::NoChange)
    }

    fn display_config_path(&self) -> &Path {
        &self.display_path
    }

    fn main_config_path(&self) -> &Path {
        &self.main_config_path
    }

    fn is_registered_in_main_config(&self) -> Result<bool, ConfigError> {
        let main_config = Self::read_or_empty(&self.main_config_path)?;
        Ok(kdl_parser::is_included_by(&main_config, "display.kdl"))
    }
}

/// Write `contents` to `path` atomically, preserving existing permissions.
fn atomic_write(path: &Path, contents: &str) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("display.kdl");
    let temporary_path = path.with_file_name(format!("{file_name}.tmp"));
    let existing_permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());

    let write_result = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temporary_path)?;
        if let Some(permissions) = existing_permissions {
            file.set_permissions(permissions)?;
        }
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary_path, path)?;
        Ok(())
    })();

    if let Err(source) = write_result {
        let _ = fs::remove_file(&temporary_path);
        return Err(ConfigError::Write {
            path: path.to_path_buf(),
            source,
        });
    }

    // Flush the directory entry so the rename survives a crash.
    if let Some(parent) = path.parent() {
        let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::display::OutputId;
    use crate::domain::profile::{LayoutPlan, OutputPlan, ProfileKind};

    fn plan() -> LayoutPlan {
        LayoutPlan {
            profile: ProfileKind::ExtendRight,
            outputs: vec![OutputPlan {
                output: OutputId::new("HDMI-A-1"),
                enabled: true,
                position: Some((1920, 0)),
                scale: Some(1.0),
                expect_present: true,
            }],
            focus_output: None,
        }
    }

    fn store(root: &Path) -> FileConfigStore {
        FileConfigStore::new(
            root.join("display.kdl"),
            root.join("display.kdl.bak"),
            root.join("config.kdl"),
        )
    }

    #[test]
    fn snapshot_is_created_once_and_kept_pristine() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(directory.path());
        fs::write(
            store.display_config_path(),
            "// original\noutput \"DP-1\" { scale 2 }\n",
        )
        .unwrap();

        assert_eq!(store.snapshot().unwrap(), SnapshotOutcome::Created);
        assert_eq!(store.snapshot().unwrap(), SnapshotOutcome::AlreadyExists);

        let section = crate::infrastructure::kdl_parser::render_section(&plan());
        store.write_managed_section(&section).unwrap();
        assert!(
            store
                .read_display_config()
                .unwrap()
                .contains("managed section")
        );

        let backup = fs::read_to_string(directory.path().join("display.kdl.bak")).unwrap();
        assert_eq!(backup, "// original\noutput \"DP-1\" { scale 2 }\n");
    }

    #[test]
    fn restore_prefers_backup_and_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(directory.path());
        let original = "// original\n";
        fs::write(store.display_config_path(), original).unwrap();
        store.snapshot().unwrap();
        store
            .write_managed_section(&crate::infrastructure::kdl_parser::render_section(&plan()))
            .unwrap();

        assert_eq!(
            store.restore_snapshot().unwrap(),
            RestoreOutcome::RestoredFromBackup
        );
        assert_eq!(store.read_display_config().unwrap(), original);
        assert_eq!(
            store.restore_snapshot().unwrap(),
            RestoreOutcome::RestoredFromBackup
        );
    }

    #[test]
    fn restore_without_backup_removes_the_managed_section() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(directory.path());
        fs::write(store.display_config_path(), "// user content\n").unwrap();
        store
            .write_managed_section(&crate::infrastructure::kdl_parser::render_section(&plan()))
            .unwrap();

        assert_eq!(
            store.restore_snapshot().unwrap(),
            RestoreOutcome::RemovedManagedSection
        );
        assert_eq!(store.read_display_config().unwrap(), "// user content\n");
        assert_eq!(store.restore_snapshot().unwrap(), RestoreOutcome::NoChange);
    }

    #[test]
    fn atomic_writes_preserve_permissions_and_leave_no_temporary_file() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let store = store(directory.path());
        fs::write(store.display_config_path(), "// original\n").unwrap();
        fs::set_permissions(
            store.display_config_path(),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();

        store
            .write_managed_section(&crate::infrastructure::kdl_parser::render_section(&plan()))
            .unwrap();

        let mode = fs::metadata(store.display_config_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!directory.path().join("display.kdl.tmp").exists());
    }

    #[test]
    fn include_registration_is_detected() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(directory.path());
        assert!(!store.is_registered_in_main_config().unwrap());

        fs::write(store.main_config_path(), "include \"./cfg/display.kdl\"\n").unwrap();
        assert!(store.is_registered_in_main_config().unwrap());
    }
}
