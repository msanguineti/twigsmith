//! Configuration inspection and create-only generation, independent of CLI input.
use std::{
    fmt, io,
    io::Write,
    path::{Path, PathBuf},
};

use crate::{
    git,
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
    render(format, &settings::Settings::default())
}

pub fn render(format: Format, proposed: &settings::Settings) -> String {
    render_document(format, proposed, false)
}
fn render_document(format: Format, proposed: &settings::Settings, guided: bool) -> String {
    // Serialize the schema in each format rather than sharing scalar quoting rules.
    #[derive(serde::Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct Document<'a> {
        publish_remote: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        base_remote: Option<&'a str>,
        base: &'a str,
        save_action: &'a str,
        finish_mode: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        finish_integration: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        finish_message: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        finish_checkout: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        finish_cleanup: Option<&'a str>,
        finish_update: &'a str,
        finish_rewrite: &'a str,
        finish_checks: &'a [settings::FinishCheck],
        #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        workstreams:
            &'a std::collections::BTreeMap<settings::WorkstreamKind, settings::WorkstreamConfig>,
    }
    let document = Document {
        publish_remote: &proposed.publish_remote,
        base_remote: (proposed.sources.base_remote.is_some()
            || proposed.base_remote != proposed.publish_remote)
            .then_some(proposed.base_remote.as_str()),
        base: &proposed.base,
        save_action: proposed.save_action.as_str(),
        finish_mode: proposed.finish_mode.as_str(),
        finish_integration: (!guided || proposed.finish_mode != settings::FinishMode::Review)
            .then_some(proposed.finish_integration.as_str()),
        finish_message: (!guided
            || (proposed.finish_mode != settings::FinishMode::Review
                && proposed.finish_integration != settings::FinishIntegration::FfOnly))
            .then_some(proposed.finish_message.as_str()),
        finish_checkout: (!guided || proposed.finish_mode != settings::FinishMode::Review)
            .then_some(proposed.finish_checkout.as_str()),
        finish_cleanup: (!guided || proposed.finish_mode != settings::FinishMode::Review)
            .then_some(proposed.finish_cleanup.as_str()),
        finish_update: proposed.finish_update.as_str(),
        finish_rewrite: proposed.finish_rewrite.as_str(),
        finish_checks: &proposed.finish_checks,
        workstreams: &proposed.workstreams,
    };
    match format {
        Format::Toml => toml::to_string_pretty(&document).expect("settings serialization"),
        Format::Yaml => serde_saphyr::to_string(&document).expect("settings serialization"),
        Format::Json => format!(
            "{}\n",
            serde_json::to_string_pretty(&document).expect("settings serialization")
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
    ReviewChanged,
    Input(io::Error),
    Write { path: PathBuf, source: io::Error },
}
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(e) => write!(f, "{e}"),
            Self::Settings(e) => write!(f, "{e}"),
            Self::OutsideRepository => f.write_str("current directory is outside a Git repository"),
            Self::NoWorkingTree => f.write_str("configuration generation requires a Git working tree; bare repositories have no shared configuration root"),
            Self::ExistingShared(paths) => write!(f, "shared configuration already exists: {}; use gof config show to inspect, or edit the existing file", paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")),
            Self::ContextChanged => f.write_str("repository context changed before configuration publication; rerun setup from the intended working tree"),
            Self::ReviewChanged => f.write_str("reviewed repository hints or private overrides changed; rerun setup before creating configuration"),
            Self::Input(e) => write!(f, "cannot read terminal input: {e}"),
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
            Self::Input(e) => Some(e),
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
    /// Layered settings before the current branch association selects a policy.
    pub templates: settings::Settings,
}
pub fn inspect(directory: &Path) -> Result<Inspection, ConfigError> {
    let repository = discover(directory)?;
    let mut loaded = settings::load_with_files(directory, &repository)?;
    let templates = loaded.settings.clone();
    loaded.settings = settings::load(directory, &repository)?;
    Ok(Inspection {
        repository,
        loaded,
        templates,
    })
}

/// Captured repository selection, not an authorization to replace any file.
#[derive(Debug)]
pub struct GenerationPlan {
    repository: RepositoryContext,
    format: Format,
    pub proposed: settings::Settings,
    review: Option<Review>,
}
#[derive(Debug)]
pub struct Generated {
    pub path: PathBuf,
    /// Recognized private files at publication.
    /// Generation does not need to parse or copy private values.
    pub local_files: Vec<PathBuf>,
}
fn require_absent(root: &Path) -> Result<(), ConfigError> {
    let paths = settings::recognized_paths(root)?;
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
    Ok(GenerationPlan {
        repository,
        format,
        proposed: settings::Settings::default(),
        review: None,
    })
}

/// Locally observed hints and validated override values, never proposed defaults.
#[derive(Debug, PartialEq, Eq)]
pub struct Review {
    pub overrides: settings::Settings,
    pub redact_credentials: bool,
    pub redact_credentials_source: settings::SettingSource,
    pub local: Option<PathBuf>,
    pub remotes: Vec<u8>,
    remote_config: Vec<u8>,
    pub branches: Vec<u8>,
}
fn observation(directory: &Path, args: &[&str], empty_ok: bool) -> Result<Vec<u8>, ConfigError> {
    let output = git::inspect(directory, args).map_err(InspectionError::Io)?;
    if output.status.success()
        || (empty_ok && output.status.code() == Some(1) && output.stderr.is_empty())
    {
        Ok(output.stdout)
    } else {
        Err(InspectionError::Git {
            arguments: args.iter().map(std::ffi::OsString::from).collect(),
            output,
        }
        .into())
    }
}
fn review(directory: &Path, plan: &GenerationPlan) -> Result<Review, ConfigError> {
    if discover(directory)? != plan.repository {
        return Err(ConfigError::ContextChanged);
    }
    require_absent(
        plan.repository
            .working_tree_root
            .as_deref()
            .ok_or(ConfigError::NoWorkingTree)?,
    )?;
    let loaded = settings::load_with_files(directory, &plan.repository)?;
    Ok(Review {
        overrides: loaded.settings,
        redact_credentials: loaded.redact_credentials,
        redact_credentials_source: loaded.redact_credentials_source,
        local: loaded.local,
        remotes: observation(directory, &["remote"], false)?,
        remote_config: observation(directory, &["config", "--get-regexp", "^remote\\."], true)?,
        branches: observation(
            directory,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/heads/",
                "refs/remotes/",
            ],
            false,
        )?,
    })
}
pub fn prepare_guided(directory: &Path, format: Format) -> Result<GenerationPlan, ConfigError> {
    let mut plan = prepare(directory, format)?;
    plan.review = Some(review(directory, &plan)?);
    Ok(plan)
}
impl GenerationPlan {
    pub fn destination(&self) -> PathBuf {
        self.repository
            .working_tree_root
            .as_ref()
            .expect("prepared working tree")
            .join(self.format.filename())
    }
    pub fn contents(&self) -> String {
        render_document(self.format, &self.proposed, self.review.is_some())
    }
    pub fn review(&self) -> &Review {
        self.review.as_ref().expect("guided plan")
    }
}
pub fn revalidate_review(directory: &Path, plan: &GenerationPlan) -> Result<(), ConfigError> {
    if let Some(expected) = &plan.review {
        let current = review(directory, plan).map_err(|e| match e {
            ConfigError::Settings(_) => ConfigError::ReviewChanged,
            other => other,
        })?;
        if &current != expected {
            return Err(ConfigError::ReviewChanged);
        }
    }
    Ok(())
}
fn validate(directory: &Path, proposed: &settings::Settings) -> Result<(), ConfigError> {
    if !settings::valid_remote(directory, &proposed.publish_remote)?
        || !settings::valid_remote(directory, &proposed.base_remote)?
        || !settings::valid_branch(directory, &proposed.base)?
    {
        return Err(SettingsError::InvalidSettings(
            "invalid proposed publish-remote, base-remote, or base name".into(),
        )
        .into());
    }
    for (kind, value) in &proposed.workstreams {
        value.validate_for(directory, *kind)?;
    }
    for kind in proposed.workstreams.keys() {
        let mut effective = proposed.clone();
        settings::resolve_workstream(&mut effective, *kind, None)?;
    }
    settings::validate_finish_checks(&proposed.finish_checks)
        .map_err(SettingsError::InvalidSettings)?;
    Ok(())
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
    revalidate_review(directory, plan)?;
    validate(directory, &plan.proposed)?;
    let file = stage(root, &path, plan.contents().as_bytes())?;
    if discover(directory)? != plan.repository {
        return Err(ConfigError::ContextChanged);
    }
    require_absent(root)?;
    revalidate_review(directory, plan)?;
    let local_files = settings::recognized_paths(&plan.repository.common_dir)?;
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
