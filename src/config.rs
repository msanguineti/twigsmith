//! Configuration inspection and create-only generation, independent of CLI input.
use std::{
    fmt, io,
    io::Write,
    path::{Path, PathBuf},
};

use crate::{
    repository::{self, InspectionError, RepositoryContext},
    settings::{self, LoadedSettings, SettingsError},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Toml,
    Yaml,
    Json,
}
impl Format {
    pub fn filename(self) -> &'static str {
        match self {
            Self::Toml => settings::CONFIG_FILES[0],
            Self::Yaml => settings::CONFIG_FILES[1],
            Self::Json => settings::CONFIG_FILES[3],
        }
    }
}

/// Render explicit built-in defaults, independent of Git and private settings.
pub fn render_defaults(format: Format) -> String {
    let defaults = settings::Settings::default();
    let pairs = [
        ("remote", defaults.remote.as_str()),
        ("base", defaults.base.as_str()),
        ("save-action", defaults.save_action.as_str()),
    ];
    // Built-in values are plain strings; JSON quoting also serves TOML and YAML.
    let quote = |s: &str| serde_json::to_string(s).expect("string serialization");
    match format {
        Format::Toml => pairs
            .iter()
            .map(|(k, v)| format!("{k} = {}\n", quote(v)))
            .collect(),
        Format::Yaml => pairs
            .iter()
            .map(|(k, v)| format!("{k}: {}\n", quote(v)))
            .collect(),
        Format::Json => format!(
            "{{\n{}\n}}\n",
            pairs
                .iter()
                .map(|(k, v)| format!("  {}: {}", quote(k), quote(v)))
                .collect::<Vec<_>>()
                .join(",\n")
        ),
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Repository(InspectionError),
    Settings(SettingsError),
    OutsideRepository,
    NoWorkingTree,
    ExistingShared(Vec<PathBuf>),
    ContextChanged,
    Write { path: PathBuf, source: io::Error },
}
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(e) => write!(f, "{e}"),
            Self::Settings(e) => write!(f, "{e}"),
            Self::OutsideRepository => f.write_str("current directory is outside a Git repository"),
            Self::NoWorkingTree => f.write_str("configuration generation requires a Git working tree; bare repositories have no shared configuration root"),
            Self::ExistingShared(paths) => write!(f, "shared configuration already exists: {}; keep or edit the existing file", paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")),
            Self::ContextChanged => f.write_str("repository context changed before configuration publication; retry from the intended working tree"),
            Self::Write { path, source } => write!(f, "cannot create configuration {}: {source}", path.display()),
        }
    }
}
impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Repository(e) => Some(e),
            Self::Settings(e) => Some(e),
            Self::Write { source, .. } => Some(source),
            _ => None,
        }
    }
}
impl From<InspectionError> for ConfigError {
    fn from(e: InspectionError) -> Self {
        Self::Repository(e)
    }
}
impl From<SettingsError> for ConfigError {
    fn from(e: SettingsError) -> Self {
        Self::Settings(e)
    }
}
fn discover(directory: &Path) -> Result<RepositoryContext, ConfigError> {
    repository::discover(directory)?.ok_or(ConfigError::OutsideRepository)
}

#[derive(Debug)]
pub struct Inspection {
    pub repository: RepositoryContext,
    pub loaded: LoadedSettings,
}
pub fn inspect(directory: &Path) -> Result<Inspection, ConfigError> {
    let repository = discover(directory)?;
    let loaded = settings::load_with_files(directory, &repository)?;
    Ok(Inspection { repository, loaded })
}

/// Captured repository selection, not an authorization to replace any file.
#[derive(Debug)]
pub struct GenerationPlan {
    repository: RepositoryContext,
    format: Format,
}
#[derive(Debug)]
pub struct Generated {
    pub path: PathBuf,
    /// Recognized private files at publication, including legacy overrides.
    /// Generation does not need to parse or copy private values.
    pub local_files: Vec<PathBuf>,
}
fn require_absent(root: &Path) -> Result<(), ConfigError> {
    let paths = settings::recognized_paths(root, false)?;
    if paths.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::ExistingShared(paths))
    }
}
pub fn prepare(directory: &Path, format: Format) -> Result<GenerationPlan, ConfigError> {
    let repository = discover(directory)?;
    let root = repository
        .working_tree_root
        .as_deref()
        .ok_or(ConfigError::NoWorkingTree)?;
    require_absent(root)?;
    Ok(GenerationPlan { repository, format })
}

/// Stage complete bytes in the destination directory. No recognized filename is
/// visible until publication. Dropping a staged file removes it after errors.
fn stage(
    root: &Path,
    path: &Path,
    contents: &[u8],
) -> Result<tempfile::NamedTempFile, ConfigError> {
    let write = || -> io::Result<tempfile::NamedTempFile> {
        let mut file = tempfile::Builder::new()
            .prefix(".git-oneflow-config-")
            .suffix(".tmp")
            .tempfile_in(root)?;
        file.write_all(contents)?;
        file.as_file().sync_all()?;
        Ok(file)
    };
    write().map_err(|source| ConfigError::Write {
        path: path.into(),
        source,
    })
}
fn publish(file: tempfile::NamedTempFile, path: &Path) -> Result<(), ConfigError> {
    // persist_noclobber atomically creates the destination without replacing an
    // existing entry (including dangling symlinks), unlike ordinary rename.
    file.persist_noclobber(path)
        .map_err(|e| ConfigError::Write {
            path: path.into(),
            source: e.error,
        })?;
    Ok(())
}
pub fn generate(directory: &Path, plan: &GenerationPlan) -> Result<Generated, ConfigError> {
    let root = plan
        .repository
        .working_tree_root
        .as_deref()
        .ok_or(ConfigError::NoWorkingTree)?;
    let path = root.join(plan.format.filename());
    // Reject stale plans before staging, then recheck immediately before publishing.
    if discover(directory)? != plan.repository {
        return Err(ConfigError::ContextChanged);
    }
    require_absent(root)?;
    let file = stage(root, &path, render_defaults(plan.format).as_bytes())?;
    if discover(directory)? != plan.repository {
        return Err(ConfigError::ContextChanged);
    }
    require_absent(root)?;
    let local_files = settings::recognized_paths(&plan.repository.common_dir, true)?;
    publish(file, &path)?;
    Ok(Generated { path, local_files })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn staged_bytes_are_invisible_and_abandonment_removes_them() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(Format::Toml.filename());
        let staged = stage(root.path(), &path, b"incomplete").unwrap();
        assert!(!path.exists());
        let temporary = staged.path().to_owned();
        drop(staged);
        assert!(!temporary.exists());
        assert!(!path.exists());
    }

    #[test]
    fn publication_preserves_a_destination_created_after_the_final_check() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(Format::Toml.filename());
        let staged = stage(root.path(), &path, render_defaults(Format::Toml).as_bytes()).unwrap();
        require_absent(root.path()).unwrap();
        fs::write(&path, "concurrent writer").unwrap();
        assert!(publish(staged, &path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "concurrent writer");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn publication_preserves_a_dangling_symlink_created_after_the_final_check() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(Format::Toml.filename());
        let staged = stage(root.path(), &path, render_defaults(Format::Toml).as_bytes()).unwrap();
        require_absent(root.path()).unwrap();
        std::os::unix::fs::symlink("missing-target", &path).unwrap();
        assert!(publish(staged, &path).is_err());
        assert_eq!(fs::read_link(&path).unwrap(), Path::new("missing-target"));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_publication_leaves_no_configuration_or_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root
            .path()
            .join("missing-parent")
            .join(Format::Json.filename());
        let staged = stage(root.path(), &path, b"complete").unwrap();
        assert!(publish(staged, &path).is_err());
        assert!(!path.exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
