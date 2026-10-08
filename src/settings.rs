//! Strict, local, read-only configuration shared by workflows and inspection.
use crate::{
    git,
    repository::{InspectionError, RepositoryContext},
};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveAction {
    Ask,
    PublishOnly,
    Staged,
    Wip,
    Stash,
}

#[derive(Debug)]
pub enum SettingsError {
    Repository(InspectionError),
    SettingsIo { path: PathBuf, source: io::Error },
    InvalidSettings(String),
}
impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(e) => write!(f, "{e}"),
            Self::SettingsIo { path, source } => {
                write!(f, "cannot read settings {}: {source}", path.display())
            }
            Self::InvalidSettings(reason) => {
                write!(f, "invalid settings: {reason}")
            }
        }
    }
}
impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Repository(e) => Some(e),
            Self::SettingsIo { source, .. } => Some(source),
            _ => None,
        }
    }
}
impl From<InspectionError> for SettingsError {
    fn from(e: InspectionError) -> Self {
        Self::Repository(e)
    }
}

/// The legacy local filename; new configuration uses CONFIG_FILES.
pub const SETTINGS_FILE: &str = "oneflow.conf";
pub const CONFIG_FILES: [&str; 4] = [
    "git-oneflow.config.toml",
    "git-oneflow.config.yaml",
    "git-oneflow.config.yml",
    "git-oneflow.config.json",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingSource {
    BuiltIn,
    Shared(PathBuf),
    Local(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingSources {
    pub remote: SettingSource,
    pub base: SettingSource,
    pub save_action: SettingSource,
}
impl Default for SettingSources {
    fn default() -> Self {
        Self {
            remote: SettingSource::BuiltIn,
            base: SettingSource::BuiltIn,
            save_action: SettingSource::BuiltIn,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub remote: String,
    pub base: String,
    pub save_action: SaveAction,
    pub sources: SettingSources,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            remote: "origin".into(),
            base: "main".into(),
            save_action: SaveAction::Ask,
            sources: SettingSources::default(),
        }
    }
}

/// One sparse schema for all formats, including the legacy compatibility boundary.
#[derive(Debug, Default)]
struct Layer {
    remote: Option<String>,
    base: Option<String>,
    save_action: Option<String>,
}
impl Layer {
    fn slot(&mut self, key: &str) -> Result<&mut Option<String>, String> {
        match key {
            "remote" => Ok(&mut self.remote),
            "base" => Ok(&mut self.base),
            "save-action" => Ok(&mut self.save_action),
            _ => Err(format!(
                "unknown key {key:?} (supported keys: remote, base, save-action)"
            )),
        }
    }
    fn validate(&self, directory: &Path, path: &Path) -> Result<(), SettingsError> {
        if let Some(remote) = &self.remote
            && !valid_remote(directory, remote)?
        {
            return Err(invalid(path, format!("invalid remote name: {remote:?}")));
        }
        if let Some(base) = &self.base
            && !valid_branch(directory, base)?
        {
            return Err(invalid(path, format!("invalid base branch name: {base:?}")));
        }
        if let Some(action) = &self.save_action {
            parse_action(action).map_err(|e| invalid(path, e))?;
        }
        Ok(())
    }
    fn apply(self, settings: &mut Settings, source: SettingSource) {
        if let Some(remote) = self.remote {
            settings.remote = remote;
            settings.sources.remote = source.clone();
        }
        if let Some(base) = self.base {
            settings.base = base;
            settings.sources.base = source.clone();
        }
        if let Some(action) = self.save_action {
            settings.save_action = parse_action(&action).expect("validated layer");
            settings.sources.save_action = source;
        }
    }
}
impl SaveAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::PublishOnly => "publish-only",
            Self::Staged => "staged",
            Self::Wip => "wip",
            Self::Stash => "stash",
        }
    }
}

pub(crate) fn parse_action(value: &str) -> Result<SaveAction, String> {
    match value {
        "ask" => Ok(SaveAction::Ask),
        "publish-only" => Ok(SaveAction::PublishOnly),
        "staged" => Ok(SaveAction::Staged),
        "wip" => Ok(SaveAction::Wip),
        "stash" => Ok(SaveAction::Stash),
        _ => Err("save-action must be ask, publish-only, staged, wip, or stash".into()),
    }
}

// deserialize_any preserves scalar types, including YAML's inferred types.
// String deserialization alone permits scalar coercion in some YAML parsers.
struct Text(String);
impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct TextVisitor;
        impl<'de> Visitor<'de> for TextVisitor {
            type Value = Text;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Text, E> {
                Ok(Text(value.into()))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Text, E> {
                Ok(Text(value))
            }
        }
        d.deserialize_any(TextVisitor)
    }
}
impl<'de> Deserialize<'de> for Layer {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct LayerVisitor;
        impl<'de> Visitor<'de> for LayerVisitor {
            type Value = Layer;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a flat mapping of remote, base, and save-action strings")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Layer, M::Error> {
                let mut layer = Layer::default();
                while let Some(Text(key)) = map.next_key::<Text>()? {
                    let slot = layer.slot(&key).map_err(de::Error::custom)?;
                    if slot.is_some() {
                        return Err(de::Error::custom(format!("duplicate key {key:?}")));
                    }
                    *slot = Some(map.next_value::<Text>()?.0);
                }
                Ok(layer)
            }
        }
        d.deserialize_any(LayerVisitor)
    }
}

fn invalid(path: &Path, reason: impl fmt::Display) -> SettingsError {
    SettingsError::InvalidSettings(format!("{}: {reason}", path.display()))
}
fn exists(path: &Path) -> Result<bool, SettingsError> {
    // Only absence defaults: dangling symlinks and unreadable entries fail.
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(SettingsError::SettingsIo {
            path: path.into(),
            source,
        }),
    }
}
pub(crate) fn recognized_paths(root: &Path, local: bool) -> Result<Vec<PathBuf>, SettingsError> {
    let mut paths = Vec::new();
    for name in CONFIG_FILES {
        let path = root.join(name);
        if exists(&path)? {
            paths.push(path);
        }
    }
    if local {
        let legacy = root.join(SETTINGS_FILE);
        if exists(&legacy)? {
            paths.push(legacy);
        }
    }
    Ok(paths)
}
fn discover_layer(root: &Path, local: bool) -> Result<Option<PathBuf>, SettingsError> {
    let mut paths = recognized_paths(root, local)?;
    if paths.len() > 1 {
        let files = paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let migration = if paths
            .iter()
            .any(|p| p.file_name().is_some_and(|n| n == SETTINGS_FILE))
        {
            " Migrate oneflow.conf values into one new-format local file, then remove or rename oneflow.conf."
        } else {
            " Keep exactly one recognized file in this layer; extensions have no priority."
        };
        return Err(SettingsError::InvalidSettings(format!(
            "multiple configuration files in {} layer: {files}.{migration}",
            if local { "local" } else { "shared" }
        )));
    }
    Ok(paths.pop())
}
fn read_layer(path: &Path) -> Result<Layer, SettingsError> {
    let contents = fs::read_to_string(path).map_err(|source| SettingsError::SettingsIo {
        path: path.into(),
        source,
    })?;
    let result = match path.extension().and_then(|s| s.to_str()) {
        Some("toml") => toml::from_str::<Layer>(&contents).map_err(|e| e.to_string()),
        Some("json") => serde_json::from_str::<Layer>(&contents).map_err(|e| e.to_string()),
        Some("yaml" | "yml") => {
            let mut options = serde_saphyr::Options::default();
            options.merge_keys = serde_saphyr::options::MergeKeyPolicy::Error;
            options.reject_unsupported_tags = true;
            options.strict_booleans = true;
            serde_saphyr::from_str_with_options::<Layer>(&contents, options)
                .map_err(|e| e.to_string())
        }
        _ => parse_legacy(&contents),
    };
    result.map_err(|e| invalid(path, e))
}
fn parse_legacy(contents: &str) -> Result<Layer, String> {
    let mut layer = Layer::default();
    for (index, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let invalid = |reason: String| format!("line {}: {reason}", index + 1);
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| invalid("expected key = value".into()))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(invalid("empty value".into()));
        }
        let slot = layer.slot(key.trim()).map_err(invalid)?;
        if slot.is_some() {
            return Err(invalid("duplicate key".into()));
        }
        *slot = Some(value.into());
    }
    Ok(layer)
}

/// Validate every present file before sparse merging. Remote availability is
/// deliberately left to the workflow. Git-discovered paths avoid inheritance.
pub fn load(directory: &Path, repository: &RepositoryContext) -> Result<Settings, SettingsError> {
    Ok(load_with_files(directory, repository)?.settings)
}

/// Validated settings and the files inspected, including empty/sparse layers.
#[derive(Debug)]
pub struct LoadedSettings {
    pub settings: Settings,
    pub shared: Option<PathBuf>,
    pub local: Option<PathBuf>,
}

pub fn load_with_files(
    directory: &Path,
    repository: &RepositoryContext,
) -> Result<LoadedSettings, SettingsError> {
    let shared = repository
        .working_tree_root
        .as_deref()
        .map(|root| discover_layer(root, false))
        .transpose()?
        .flatten();
    let local = discover_layer(&repository.common_dir, true)?;
    let mut settings = Settings::default();
    for (path, is_local) in [(shared.clone(), false), (local.clone(), true)] {
        if let Some(path) = path {
            let layer = read_layer(&path)?;
            layer.validate(directory, &path)?;
            let source = if is_local {
                SettingSource::Local(path)
            } else {
                SettingSource::Shared(path)
            };
            layer.apply(&mut settings, source);
        }
    }
    Ok(LoadedSettings {
        settings,
        shared,
        local,
    })
}

pub(crate) fn valid_branch(directory: &Path, name: &str) -> Result<bool, InspectionError> {
    // --branch expands @{-n}; validating a full ref never interprets revisions.
    if name.starts_with('-') || name == "HEAD" {
        return Ok(false);
    }
    valid_ref(directory, &format!("refs/heads/{name}"))
}

pub(crate) fn valid_remote(directory: &Path, name: &str) -> Result<bool, InspectionError> {
    // A named remote, not a URL, path, local-repository '.', or command option.
    if name.is_empty() || name.starts_with('-') || name == "." {
        return Ok(false);
    }
    valid_ref(directory, &format!("refs/remotes/{name}/base"))
}

fn valid_ref(directory: &Path, reference: &str) -> Result<bool, InspectionError> {
    if reference.contains('\0') {
        return Ok(false);
    }
    let args = ["check-ref-format", reference];
    let output = git::inspect(directory, args).map_err(InspectionError::Io)?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) if output.stderr.is_empty() => Ok(false),
        _ => Err(InspectionError::Git {
            arguments: args.iter().map(std::ffi::OsString::from).collect(),
            output,
        }),
    }
}
