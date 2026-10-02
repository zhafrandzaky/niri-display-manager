//! Display configuration backup, atomic writing, and restore.
//!
//! Two layouts are supported:
//!
//! - **Modular**: a dedicated display file (for example `cfg/display.kdl`)
//!   that the main configuration includes. Used when such an include is
//!   present, preserving setups with modular `cfg/*.kdl` directories.
//! - **Inline**: a fenced managed section appended to the main configuration
//!   itself. Used when no dedicated display file is wired into niri, which is
//!   the common case on vanilla niri installations.
//!
//! In both layouts the same guarantees hold: the managed file is snapshotted
//! before the first change, every write is atomic, and restore is idempotent.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::config_paths::{Environment, ResolvedPaths};
use crate::infrastructure::kdl_parser;

/// How the managed fenced section reaches niri.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfigMode {
    /// A dedicated display file that the main configuration includes.
    Modular,
    /// A fenced managed section inside the main configuration file.
    #[default]
    Inline,
}

impl ConfigMode {
    pub fn is_modular(self) -> bool {
        matches!(self, ConfigMode::Modular)
    }
}

/// Result of creating the pristine snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotOutcome {
    Created,
    AlreadyExists,
    /// The managed file does not exist yet; no backup was written and rollback
    /// only needs to remove the managed section.
    FileAbsent,
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
    #[error("invalid path {}: {source}", path.display())]
    InvalidPath {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(
        "cannot determine the configuration directory: set HOME, XDG_CONFIG_HOME, NDM_CONFIG_DIR, or NIRI_CONFIG, or pass --config"
    )]
    NoConfigBase,
}

/// Abstraction over the managed configuration file.
pub trait ConfigStore: Send + Sync {
    fn mode(&self) -> ConfigMode;

    fn main_config_path(&self) -> &Path;

    /// File that carries the managed fenced section.
    fn managed_config_path(&self) -> &Path;

    /// Backup of the managed file, created once before the first change.
    fn backup_path(&self) -> &Path;

    fn read_managed_config(&self) -> Result<String, ConfigError>;

    /// Create the one-time pristine snapshot if it does not exist yet.
    fn snapshot(&self) -> Result<SnapshotOutcome, ConfigError>;

    /// Write the managed section, replacing any previous managed section.
    fn write_managed_section(&self, section: &str) -> Result<(), ConfigError>;

    /// Restore the pristine snapshot, or remove the managed section when no
    /// snapshot exists. Idempotent.
    fn restore_snapshot(&self) -> Result<RestoreOutcome, ConfigError>;

    /// Whether the managed file is wired into the main configuration.
    ///
    /// Always true in inline mode, where the section lives in the main file.
    fn display_file_registered(&self) -> Result<bool, ConfigError>;
}

/// Production store backed by the filesystem.
#[derive(Debug, Clone)]
pub struct FileConfigStore {
    mode: ConfigMode,
    main_config: PathBuf,
    managed_config: PathBuf,
    backup_path: PathBuf,
    home: Option<PathBuf>,
}

impl FileConfigStore {
    /// Discover the layout for resolved paths.
    ///
    /// Modular mode is selected when the display candidate is included by the
    /// main configuration (directly or through one level of indirection) or
    /// explicitly requested with `--display-config`; otherwise the portable
    /// inline mode is used.
    pub fn discover(
        resolved: ResolvedPaths,
        environment: &Environment,
    ) -> Result<Self, ConfigError> {
        let registered = display_file_is_included(
            &resolved.main_config,
            &resolved.display_candidate,
            environment.home.as_deref(),
        )?;
        let mode = if resolved.display_explicit || registered {
            ConfigMode::Modular
        } else {
            ConfigMode::Inline
        };
        Ok(Self::for_layout(
            mode,
            resolved.main_config,
            resolved.display_candidate,
            environment.home.clone(),
        ))
    }

    /// Construct a store for an explicit layout (tests and embedders).
    pub fn for_layout(
        mode: ConfigMode,
        main_config: PathBuf,
        display_config: PathBuf,
        home: Option<PathBuf>,
    ) -> Self {
        let managed_config = match mode {
            ConfigMode::Modular => display_config,
            ConfigMode::Inline => main_config.clone(),
        };
        let backup_path = backup_path_for(&managed_config);
        Self {
            mode,
            main_config,
            managed_config,
            backup_path,
            home,
        }
    }
}

impl ConfigStore for FileConfigStore {
    fn mode(&self) -> ConfigMode {
        self.mode
    }

    fn main_config_path(&self) -> &Path {
        &self.main_config
    }

    fn managed_config_path(&self) -> &Path {
        &self.managed_config
    }

    fn backup_path(&self) -> &Path {
        &self.backup_path
    }

    fn read_managed_config(&self) -> Result<String, ConfigError> {
        read_or_empty(&self.managed_config)
    }

    fn snapshot(&self) -> Result<SnapshotOutcome, ConfigError> {
        if self.backup_path.exists() {
            return Ok(SnapshotOutcome::AlreadyExists);
        }
        if !self.managed_config.exists() {
            return Ok(SnapshotOutcome::FileAbsent);
        }
        let contents = read_or_empty(&self.managed_config)?;
        atomic_write(&self.backup_path, &contents)?;
        Ok(SnapshotOutcome::Created)
    }

    fn write_managed_section(&self, section: &str) -> Result<(), ConfigError> {
        let original = read_or_empty(&self.managed_config)?;
        let updated = kdl_parser::splice(&original, section);
        atomic_write(&self.managed_config, &updated)
    }

    fn restore_snapshot(&self) -> Result<RestoreOutcome, ConfigError> {
        if self.backup_path.exists() {
            let contents = read_or_empty(&self.backup_path)?;
            atomic_write(&self.managed_config, &contents)?;
            return Ok(RestoreOutcome::RestoredFromBackup);
        }
        let original = read_or_empty(&self.managed_config)?;
        let (updated, removed) = kdl_parser::remove_section(&original);
        if removed {
            atomic_write(&self.managed_config, &updated)?;
            return Ok(RestoreOutcome::RemovedManagedSection);
        }
        Ok(RestoreOutcome::NoChange)
    }

    fn display_file_registered(&self) -> Result<bool, ConfigError> {
        match self.mode {
            ConfigMode::Inline => Ok(true),
            ConfigMode::Modular => display_file_is_included(
                &self.main_config,
                &self.managed_config,
                self.home.as_deref(),
            ),
        }
    }
}

fn backup_path_for(path: &Path) -> PathBuf {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(name) => path.with_file_name(format!("{name}.bak")),
        None => path.with_extension("bak"),
    }
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

/// Whether `display` is included by `main_config`, following one level of
/// indirection through other included files.
fn display_file_is_included(
    main_config: &Path,
    display: &Path,
    home: Option<&Path>,
) -> Result<bool, ConfigError> {
    let contents = read_or_empty(main_config)?;
    let main_dir = main_config
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/"));
    let target = kdl_parser::normalize_path(display);
    if kdl_parser::includes_target(&contents, &main_dir, &target, home) {
        return Ok(true);
    }
    for include in kdl_parser::collect_include_paths(&contents) {
        let resolved = kdl_parser::resolve_include(&include, &main_dir, home);
        if resolved == target {
            continue;
        }
        let Ok(nested) = fs::read_to_string(&resolved) else {
            continue;
        };
        let nested_dir = resolved
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));
        if kdl_parser::includes_target(&nested, &nested_dir, &target, home) {
            return Ok(true);
        }
    }
    Ok(false)
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
        .unwrap_or("config.kdl");
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

    fn modular_store(root: &Path) -> FileConfigStore {
        FileConfigStore::for_layout(
            ConfigMode::Modular,
            root.join("config.kdl"),
            root.join("cfg").join("display.kdl"),
            None,
        )
    }

    fn inline_store(root: &Path) -> FileConfigStore {
        FileConfigStore::for_layout(
            ConfigMode::Inline,
            root.join("config.kdl"),
            root.join("cfg").join("display.kdl"),
            None,
        )
    }

    fn resolved(main_config: PathBuf, display_explicit: bool) -> ResolvedPaths {
        let display_candidate = main_config
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"))
            .join("cfg")
            .join("display.kdl");
        ResolvedPaths {
            main_config,
            display_candidate,
            display_explicit,
        }
    }

    #[test]
    fn modular_snapshot_is_created_once_and_kept_pristine() {
        let directory = tempfile::tempdir().unwrap();
        let store = modular_store(directory.path());
        fs::create_dir_all(store.managed_config_path().parent().unwrap()).unwrap();
        fs::write(
            store.managed_config_path(),
            "// original\noutput \"DP-1\" { scale 2 }\n",
        )
        .unwrap();

        assert_eq!(store.snapshot().unwrap(), SnapshotOutcome::Created);
        assert_eq!(store.snapshot().unwrap(), SnapshotOutcome::AlreadyExists);

        store
            .write_managed_section(&crate::infrastructure::kdl_parser::render_section(&plan()))
            .unwrap();
        assert!(
            store
                .read_managed_config()
                .unwrap()
                .contains("managed section")
        );

        let backup = fs::read_to_string(store.backup_path()).unwrap();
        assert_eq!(backup, "// original\noutput \"DP-1\" { scale 2 }\n");
    }

    #[test]
    fn inline_mode_writes_and_restores_the_main_config() {
        let directory = tempfile::tempdir().unwrap();
        let store = inline_store(directory.path());
        let original = "// user config\noutput \"DP-1\" { scale 2 }\n";
        fs::write(store.main_config_path(), original).unwrap();

        assert_eq!(store.mode(), ConfigMode::Inline);
        assert_eq!(store.managed_config_path(), store.main_config_path());
        assert_eq!(store.snapshot().unwrap(), SnapshotOutcome::Created);

        store
            .write_managed_section(&crate::infrastructure::kdl_parser::render_section(&plan()))
            .unwrap();
        let written = store.read_managed_config().unwrap();
        assert!(written.starts_with(original));
        assert!(written.contains("// profile: extend-right"));
        assert_eq!(fs::read_to_string(store.backup_path()).unwrap(), original);

        assert_eq!(
            store.restore_snapshot().unwrap(),
            RestoreOutcome::RestoredFromBackup
        );
        assert_eq!(
            fs::read_to_string(store.main_config_path()).unwrap(),
            original
        );
    }

    #[test]
    fn restore_without_backup_removes_the_managed_section() {
        let directory = tempfile::tempdir().unwrap();
        let store = inline_store(directory.path());
        fs::write(store.main_config_path(), "// user content\n").unwrap();
        store
            .write_managed_section(&crate::infrastructure::kdl_parser::render_section(&plan()))
            .unwrap();

        assert_eq!(
            store.restore_snapshot().unwrap(),
            RestoreOutcome::RemovedManagedSection
        );
        assert_eq!(
            fs::read_to_string(store.main_config_path()).unwrap(),
            "// user content\n"
        );
        assert_eq!(store.restore_snapshot().unwrap(), RestoreOutcome::NoChange);
    }

    #[test]
    fn snapshot_reports_absent_files_without_creating_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let store = modular_store(directory.path());

        assert_eq!(store.snapshot().unwrap(), SnapshotOutcome::FileAbsent);
        assert!(!store.backup_path().exists());

        store
            .write_managed_section(&crate::infrastructure::kdl_parser::render_section(&plan()))
            .unwrap();
        assert_eq!(
            store.restore_snapshot().unwrap(),
            RestoreOutcome::RemovedManagedSection
        );
        assert_eq!(store.read_managed_config().unwrap(), "");
        assert!(!store.backup_path().exists());
    }

    #[test]
    fn atomic_writes_preserve_permissions_and_leave_no_temporary_file() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let store = inline_store(directory.path());
        fs::write(store.main_config_path(), "// original\n").unwrap();
        fs::set_permissions(store.main_config_path(), fs::Permissions::from_mode(0o600)).unwrap();

        store
            .write_managed_section(&crate::infrastructure::kdl_parser::render_section(&plan()))
            .unwrap();

        let mode = fs::metadata(store.main_config_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!directory.path().join("config.kdl.tmp").exists());
    }

    #[test]
    fn discovery_selects_inline_for_vanilla_layouts() {
        let directory = tempfile::tempdir().unwrap();
        let main_config = directory.path().join("config.kdl");
        fs::write(
            &main_config,
            "// vanilla config\noutput \"DP-1\" { scale 2 }\n",
        )
        .unwrap();

        let store = FileConfigStore::discover(
            resolved(main_config.clone(), false),
            &Environment::default(),
        )
        .unwrap();
        assert_eq!(store.mode(), ConfigMode::Inline);
        assert_eq!(store.managed_config_path(), main_config);
        assert_eq!(store.backup_path(), directory.path().join("config.kdl.bak"));
    }

    #[test]
    fn discovery_selects_modular_when_included() {
        let directory = tempfile::tempdir().unwrap();
        let main_config = directory.path().join("config.kdl");
        fs::write(&main_config, "include \"./cfg/display.kdl\"\n").unwrap();

        let store =
            FileConfigStore::discover(resolved(main_config, false), &Environment::default())
                .unwrap();
        assert_eq!(store.mode(), ConfigMode::Modular);
        assert_eq!(
            store.managed_config_path(),
            directory.path().join("cfg").join("display.kdl")
        );
    }

    #[test]
    fn discovery_honors_an_explicit_display_config() {
        let directory = tempfile::tempdir().unwrap();
        let main_config = directory.path().join("config.kdl");
        fs::write(&main_config, "// no includes here\n").unwrap();

        let store = FileConfigStore::discover(resolved(main_config, true), &Environment::default())
            .unwrap();
        assert_eq!(store.mode(), ConfigMode::Modular);
        assert!(!store.display_file_registered().unwrap());
    }

    #[test]
    fn display_file_registration_handles_indirection_and_false_positives() {
        let directory = tempfile::tempdir().unwrap();
        let main_config = directory.path().join("config.kdl");
        let display = directory.path().join("cfg").join("display.kdl");

        // Indirect: config includes another file, which includes the target.
        let other = directory.path().join("modular.kdl");
        fs::write(&other, "include \"./cfg/display.kdl\"\n").unwrap();
        fs::write(&main_config, "include \"modular.kdl\"\n").unwrap();
        let store = FileConfigStore::for_layout(
            ConfigMode::Modular,
            main_config.clone(),
            display.clone(),
            None,
        );
        assert!(store.display_file_registered().unwrap());

        // False positive: same basename, different directory.
        fs::write(&main_config, "include \"old/display.kdl\"\n").unwrap();
        let store = FileConfigStore::for_layout(
            ConfigMode::Modular,
            main_config.clone(),
            display.clone(),
            None,
        );
        assert!(!store.display_file_registered().unwrap());

        // Direct include.
        fs::write(&main_config, "include \"./cfg/display.kdl\"\n").unwrap();
        let store = FileConfigStore::for_layout(ConfigMode::Modular, main_config, display, None);
        assert!(store.display_file_registered().unwrap());
    }

    #[test]
    fn inline_mode_is_always_registered() {
        let directory = tempfile::tempdir().unwrap();
        let store = inline_store(directory.path());
        assert!(store.display_file_registered().unwrap());
    }
}
