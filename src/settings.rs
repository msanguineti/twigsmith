//! Strict, local, read-only configuration shared by workflows and inspection.
use crate::{
    git,
    repository::{InspectionError, RepositoryContext},
};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use std::{
    collections::BTreeMap,
    fmt, fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveAction {
    Ask,
    PublishOnly,
    Staged,
    Wip,
    Stash,
}

/// Update policy is independent of permission to replace published history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishUpdate {
    Ask,
    Rebase,
    Merge,
    None,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishRewrite {
    Ask,
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishMode {
    Ask,
    Review,
    Integrate,
}
impl FinishMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Review => "review",
            Self::Integrate => "integrate",
        }
    }
}
pub(crate) fn parse_mode(value: &str) -> Result<FinishMode, String> {
    match value {
        "ask" => Ok(FinishMode::Ask),
        "review" => Ok(FinishMode::Review),
        "integrate" => Ok(FinishMode::Integrate),
        _ => Err("finish-mode must be ask, review, or integrate".into()),
    }
}
/// Final target integration is separate from updating the working branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishIntegration {
    Ask,
    FfOnly,
    /// Create a merge commit, including when fast-forward is possible.
    Merge,
    Squash,
}
impl FinishIntegration {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::FfOnly => "ff-only",
            Self::Merge => "merge",
            Self::Squash => "squash",
        }
    }
}
pub(crate) fn parse_integration(value: &str) -> Result<FinishIntegration, String> {
    match value {
        "ask" => Ok(FinishIntegration::Ask),
        "ff-only" => Ok(FinishIntegration::FfOnly),
        "merge" => Ok(FinishIntegration::Merge),
        "squash" => Ok(FinishIntegration::Squash),
        _ => Err("finish-integration must be ask, ff-only, merge, or squash".into()),
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishMessage {
    Ask,
    Auto,
}
impl FinishMessage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Auto => "auto",
        }
    }
}
pub(crate) fn parse_message(value: &str) -> Result<FinishMessage, String> {
    match value {
        "ask" => Ok(FinishMessage::Ask),
        "auto" => Ok(FinishMessage::Auto),
        _ => Err("finish-message must be ask or auto".into()),
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishCheckout {
    Target,
    Work,
}
impl FinishCheckout {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Target => "target",
            Self::Work => "work",
        }
    }
}
pub(crate) fn parse_checkout(value: &str) -> Result<FinishCheckout, String> {
    match value {
        "target" => Ok(FinishCheckout::Target),
        "work" => Ok(FinishCheckout::Work),
        _ => Err("finish-checkout must be target or work".into()),
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishCleanup {
    Ask,
    Keep,
    Local,
    Both,
}
impl FinishCleanup {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Keep => "keep",
            Self::Local => "local",
            Self::Both => "both",
        }
    }
}
pub(crate) fn parse_cleanup(value: &str) -> Result<FinishCleanup, String> {
    match value {
        "ask" => Ok(FinishCleanup::Ask),
        "keep" => Ok(FinishCleanup::Keep),
        "local" => Ok(FinishCleanup::Local),
        "both" => Ok(FinishCleanup::Both),
        _ => Err("finish-cleanup must be ask, keep, local, or both".into()),
    }
}

/// An exact process argument vector; shell syntax requires an explicit shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FinishCheck {
    pub name: String,
    pub command: Vec<String>,
}
impl<'de> Deserialize<'de> for FinishCheck {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            name: Text,
            command: List<Text>,
        }
        struct CheckVisitor;
        impl<'de> Visitor<'de> for CheckVisitor {
            type Value = FinishCheck;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a finish check object containing name and command")
            }
            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<FinishCheck, M::Error> {
                let fields = Fields::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(FinishCheck {
                    name: fields.name.0,
                    command: fields.command.0.into_iter().map(|text| text.0).collect(),
                })
            }
        }
        // Accept only objects, including in formats that allow struct tuples.
        d.deserialize_any(CheckVisitor)
    }
}

pub(crate) fn validate_finish_checks(checks: &[FinishCheck]) -> Result<(), String> {
    for (index, check) in checks.iter().enumerate() {
        let invalid = |reason| format!("finish-checks[{}]: {reason}", index + 1);
        if check.name.trim().is_empty() {
            return Err(invalid("name must be a nonblank display label"));
        }
        if check.command.first().is_none_or(String::is_empty) {
            return Err(invalid(
                "command must contain a nonempty executable name or path",
            ));
        }
        if check.command.iter().any(|argument| argument.contains('\0')) {
            return Err(invalid(
                "command strings must not contain NUL (process arguments cannot contain NUL)",
            ));
        }
    }
    Ok(())
}
impl FinishUpdate {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Rebase => "rebase",
            Self::Merge => "merge",
            Self::None => "none",
        }
    }
}
impl FinishRewrite {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}
pub(crate) fn parse_update(value: &str) -> Result<FinishUpdate, String> {
    match value {
        "ask" => Ok(FinishUpdate::Ask),
        "rebase" => Ok(FinishUpdate::Rebase),
        "merge" => Ok(FinishUpdate::Merge),
        "none" => Ok(FinishUpdate::None),
        _ => Err("finish-update must be ask, rebase, merge, or none".into()),
    }
}
pub(crate) fn parse_rewrite(value: &str) -> Result<FinishRewrite, String> {
    match value {
        "ask" => Ok(FinishRewrite::Ask),
        "allow" => Ok(FinishRewrite::Allow),
        "deny" => Ok(FinishRewrite::Deny),
        _ => Err("finish-rewrite must be ask, allow, or deny".into()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkstreamKind {
    Ordinary,
    Release,
    Hotfix,
}
impl WorkstreamKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ordinary => "ordinary",
            Self::Release => "release",
            Self::Hotfix => "hotfix",
        }
    }
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ordinary" => Ok(Self::Ordinary),
            "release" => Ok(Self::Release),
            "hotfix" => Ok(Self::Hotfix),
            _ => Err("policy must be ordinary, release, or hotfix".into()),
        }
    }
}
fn optional_text<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Text::deserialize(d).map(|v| Some(v.0))
}
/// Sparse routes and behavior, merged independently between shared/private layers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct WorkstreamConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_tagging: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_marker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_release: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_update: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_integration: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_cleanup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_checkout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_rewrite: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_message: Option<String>,
}
impl<'de> Deserialize<'de> for WorkstreamConfig {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "kebab-case")]
        struct Fields {
            #[serde(default, deserialize_with = "optional_text")]
            creation_source: Option<String>,
            #[serde(default, deserialize_with = "optional_text")]
            preparation_source: Option<String>,
            #[serde(default, deserialize_with = "optional_text")]
            integration_target: Option<String>,
            #[serde(default, deserialize_with = "optional_text")]
            publish_remote: Option<String>,
            #[serde(default, deserialize_with = "optional_text")]
            base_remote: Option<String>,
            #[serde(default, deserialize_with = "optional_text")]
            version_tagging: Option<String>,
            #[serde(default, deserialize_with = "optional_text")]
            release_marker: Option<String>,
            #[serde(default, deserialize_with = "optional_text")]
            active_release: Option<String>,
            #[serde(default, deserialize_with = "optional_text")]
            finish_mode: Option<String>,
            #[serde(default, deserialize_with = "optional_text")]
            finish_update: Option<String>,
            #[serde(default, deserialize_with = "optional_text")]
            finish_integration: Option<String>,
            #[serde(default, deserialize_with = "optional_text")]
            finish_cleanup: Option<String>,
            #[serde(default, deserialize_with = "optional_text")]
            finish_checkout: Option<String>,
            #[serde(default, deserialize_with = "optional_text")]
            finish_rewrite: Option<String>,
            #[serde(default, deserialize_with = "optional_text")]
            finish_message: Option<String>,
        }
        struct ConfigVisitor;
        impl<'de> Visitor<'de> for ConfigVisitor {
            type Value = WorkstreamConfig;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a workstream policy object")
            }
            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                let fields = Fields::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(WorkstreamConfig {
                    creation_source: fields.creation_source,
                    preparation_source: fields.preparation_source,
                    integration_target: fields.integration_target,
                    publish_remote: fields.publish_remote,
                    base_remote: fields.base_remote,
                    version_tagging: fields.version_tagging,
                    release_marker: fields.release_marker,
                    active_release: fields.active_release,
                    finish_mode: fields.finish_mode,
                    finish_update: fields.finish_update,
                    finish_integration: fields.finish_integration,
                    finish_cleanup: fields.finish_cleanup,
                    finish_checkout: fields.finish_checkout,
                    finish_rewrite: fields.finish_rewrite,
                    finish_message: fields.finish_message,
                })
            }
        }
        d.deserialize_any(ConfigVisitor)
    }
}
fn deserialize_workstreams<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<WorkstreamKind, WorkstreamConfig>, D::Error> {
    struct Policies;
    impl<'de> Visitor<'de> for Policies {
        type Value = BTreeMap<WorkstreamKind, WorkstreamConfig>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a mapping of explicit workstream policies")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut result = BTreeMap::new();
            while let Some(Text(key)) = map.next_key()? {
                let kind = WorkstreamKind::parse(&key).map_err(de::Error::custom)?;
                if result.contains_key(&kind) {
                    return Err(de::Error::custom(format!("duplicate workstream {key:?}")));
                }
                result.insert(kind, map.next_value()?);
            }
            Ok(result)
        }
    }
    d.deserialize_any(Policies)
}
impl WorkstreamConfig {
    pub(crate) fn overlay(&mut self, other: Self) {
        if other.creation_source.is_some() {
            self.creation_source = other.creation_source;
        }
        if other.preparation_source.is_some() {
            self.preparation_source = other.preparation_source;
        }
        if other.integration_target.is_some() {
            self.integration_target = other.integration_target;
        }
        if other.publish_remote.is_some() {
            self.publish_remote = other.publish_remote;
        }
        if other.base_remote.is_some() {
            self.base_remote = other.base_remote;
        }
        if other.version_tagging.is_some() {
            self.version_tagging = other.version_tagging;
        }
        if other.release_marker.is_some() {
            self.release_marker = other.release_marker;
        }
        if other.active_release.is_some() {
            self.active_release = other.active_release;
        }
        if other.finish_mode.is_some() {
            self.finish_mode = other.finish_mode;
        }
        if other.finish_update.is_some() {
            self.finish_update = other.finish_update;
        }
        if other.finish_integration.is_some() {
            self.finish_integration = other.finish_integration;
        }
        if other.finish_cleanup.is_some() {
            self.finish_cleanup = other.finish_cleanup;
        }
        if other.finish_checkout.is_some() {
            self.finish_checkout = other.finish_checkout;
        }
        if other.finish_rewrite.is_some() {
            self.finish_rewrite = other.finish_rewrite;
        }
        if other.finish_message.is_some() {
            self.finish_message = other.finish_message;
        }
    }
    pub(crate) fn validate_for(
        &self,
        directory: &Path,
        kind: WorkstreamKind,
    ) -> Result<(), SettingsError> {
        self.validate(directory)?;
        if kind == WorkstreamKind::Hotfix
            && let Some(source) = &self.creation_source
            && (!source.starts_with("refs/tags/") || !valid_ref(directory, source)?)
        {
            return Err(SettingsError::InvalidSettings(
                "hotfix creation-source must be an exact full version tag reference refs/tags/TAG"
                    .into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn validate(&self, directory: &Path) -> Result<(), SettingsError> {
        for value in [&self.publish_remote, &self.base_remote]
            .into_iter()
            .flatten()
        {
            if !valid_remote(directory, value)? {
                return Err(SettingsError::InvalidSettings(format!(
                    "invalid workstream remote {value:?}"
                )));
            }
        }
        for value in [&self.preparation_source, &self.integration_target]
            .into_iter()
            .flatten()
        {
            if !valid_branch(directory, value)? {
                return Err(SettingsError::InvalidSettings(format!(
                    "invalid preparation/integration branch {value:?}"
                )));
            }
        }
        for value in [&self.release_marker, &self.active_release]
            .into_iter()
            .flatten()
        {
            if value != "none" && !valid_branch(directory, value)? {
                return Err(SettingsError::InvalidSettings(format!(
                    "invalid workstream branch {value:?}"
                )));
            }
        }
        if self
            .creation_source
            .as_ref()
            .is_some_and(|v| v.is_empty() || v.starts_with('-') || v.contains('\0'))
        {
            return Err(SettingsError::InvalidSettings(
                "invalid creation-source".into(),
            ));
        }
        if self.version_tagging.as_deref() == Some("required")
            && (self.finish_update.as_ref().is_some_and(|v| v != "none")
                || self.finish_integration.as_deref() == Some("squash")
                || self.finish_mode.as_ref().is_some_and(|v| v != "integrate"))
        {
            return Err(SettingsError::InvalidSettings("tagged work requires integrate mode, no source preparation, and history-preserving integration".into()));
        }
        if self
            .version_tagging
            .as_ref()
            .is_some_and(|v| v != "required" && v != "disabled")
        {
            return Err(SettingsError::InvalidSettings(
                "version-tagging must be required or disabled".into(),
            ));
        }
        if let Some(v) = &self.finish_mode {
            parse_mode(v).map_err(SettingsError::InvalidSettings)?;
        }
        if let Some(v) = &self.finish_update {
            parse_update(v).map_err(SettingsError::InvalidSettings)?;
        }
        if let Some(v) = &self.finish_integration {
            parse_integration(v).map_err(SettingsError::InvalidSettings)?;
        }
        if let Some(v) = &self.finish_cleanup {
            parse_cleanup(v).map_err(SettingsError::InvalidSettings)?;
        }
        if let Some(v) = &self.finish_checkout {
            parse_checkout(v).map_err(SettingsError::InvalidSettings)?;
        }
        if let Some(v) = &self.finish_rewrite {
            parse_rewrite(v).map_err(SettingsError::InvalidSettings)?;
        }
        if let Some(v) = &self.finish_message {
            parse_message(v).map_err(SettingsError::InvalidSettings)?;
        }
        Ok(())
    }
}
/// Explicit branch policy. Names and prefixes never select this value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkstreamPolicy {
    pub kind: WorkstreamKind,
    pub creation_source: Option<String>,
    pub preparation_source: String,
    pub integration_target: String,
    pub publish_remote: String,
    pub base_remote: String,
    pub version_tagging: bool,
    pub release_marker: Option<String>,
    pub active_release: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BranchPolicy {
    pub policy: WorkstreamPolicy,
    pub target_override: Option<String>,
    pub creation_commit: String,
    pub creation_input: Option<String>,
    pub anchor: Vec<u8>,
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

pub const CONFIG_FILES: [&str; 4] = [
    "git-oneflow.config.toml",
    "git-oneflow.config.yaml",
    "git-oneflow.config.yml",
    "git-oneflow.config.json",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettingSource {
    BuiltIn,
    Shared(PathBuf),
    Local(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingSources {
    pub publish_remote: SettingSource,
    /// None means the effective base remote follows publication.
    pub base_remote: Option<SettingSource>,
    pub base: SettingSource,
    pub save_action: SettingSource,
    pub finish_mode: SettingSource,
    pub finish_integration: SettingSource,
    pub finish_message: SettingSource,
    pub finish_checkout: SettingSource,
    pub finish_cleanup: SettingSource,
    pub finish_update: SettingSource,
    pub finish_rewrite: SettingSource,
    pub finish_checks: SettingSource,
    #[serde(default)]
    pub workstreams: BTreeMap<String, SettingSource>,
}
impl Default for SettingSources {
    fn default() -> Self {
        Self {
            publish_remote: SettingSource::BuiltIn,
            base_remote: None,
            base: SettingSource::BuiltIn,
            save_action: SettingSource::BuiltIn,
            finish_mode: SettingSource::BuiltIn,
            finish_integration: SettingSource::BuiltIn,
            finish_message: SettingSource::BuiltIn,
            finish_checkout: SettingSource::BuiltIn,
            finish_cleanup: SettingSource::BuiltIn,
            finish_update: SettingSource::BuiltIn,
            finish_rewrite: SettingSource::BuiltIn,
            finish_checks: SettingSource::BuiltIn,
            workstreams: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub publish_remote: String,
    pub base_remote: String,
    pub base: String,
    pub save_action: SaveAction,
    pub finish_mode: FinishMode,
    pub finish_integration: FinishIntegration,
    pub finish_message: FinishMessage,
    pub finish_checkout: FinishCheckout,
    pub finish_cleanup: FinishCleanup,
    pub finish_update: FinishUpdate,
    pub finish_rewrite: FinishRewrite,
    pub finish_checks: Vec<FinishCheck>,
    pub sources: SettingSources,
    #[serde(default)]
    pub workstreams: BTreeMap<WorkstreamKind, WorkstreamConfig>,
    #[serde(default)]
    pub workstream: Option<WorkstreamPolicy>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            publish_remote: "origin".into(),
            base_remote: "origin".into(),
            base: "main".into(),
            save_action: SaveAction::Ask,
            finish_mode: FinishMode::Ask,
            finish_integration: FinishIntegration::Ask,
            finish_message: FinishMessage::Ask,
            finish_checkout: FinishCheckout::Target,
            finish_cleanup: FinishCleanup::Ask,
            finish_update: FinishUpdate::Ask,
            finish_rewrite: FinishRewrite::Ask,
            finish_checks: Vec::new(),
            sources: SettingSources::default(),
            workstreams: BTreeMap::new(),
            workstream: None,
        }
    }
}

/// One sparse schema for all supported formats.
#[derive(Debug, Default)]
struct Layer {
    publish_remote: Option<String>,
    base_remote: Option<String>,
    base: Option<String>,
    save_action: Option<String>,
    finish_mode: Option<String>,
    finish_integration: Option<String>,
    finish_message: Option<String>,
    finish_checkout: Option<String>,
    finish_cleanup: Option<String>,
    finish_update: Option<String>,
    finish_rewrite: Option<String>,
    finish_checks: Option<Vec<FinishCheck>>,
    redact_credentials: Option<bool>,
    workstreams: Option<BTreeMap<WorkstreamKind, WorkstreamConfig>>,
}
impl Layer {
    fn slot(&mut self, key: &str) -> Result<&mut Option<String>, String> {
        match key {
            "publish-remote" => Ok(&mut self.publish_remote),
            "base-remote" => Ok(&mut self.base_remote),
            "base" => Ok(&mut self.base),
            "save-action" => Ok(&mut self.save_action),
            "finish-mode" => Ok(&mut self.finish_mode),
            "finish-integration" => Ok(&mut self.finish_integration),
            "finish-message" => Ok(&mut self.finish_message),
            "finish-checkout" => Ok(&mut self.finish_checkout),
            "finish-cleanup" => Ok(&mut self.finish_cleanup),
            "finish-update" => Ok(&mut self.finish_update),
            "finish-rewrite" => Ok(&mut self.finish_rewrite),
            _ => Err(format!(
                "unknown key {key:?} (supported keys: publish-remote, base-remote, base, save-action, finish-mode, finish-integration, finish-message, finish-checkout, finish-cleanup, finish-update, finish-rewrite, finish-checks; local-only: redact-credentials)"
            )),
        }
    }
    fn validate(&self, directory: &Path, path: &Path, local: bool) -> Result<(), SettingsError> {
        if let Some(policies) = &self.workstreams {
            for (kind, config) in policies {
                config.validate_for(directory, *kind)?;
            }
        }
        if !local && self.redact_credentials.is_some() {
            return Err(invalid(
                path,
                "redact-credentials is a private local setting",
            ));
        }
        for (key, value) in [
            ("publish-remote", &self.publish_remote),
            ("base-remote", &self.base_remote),
        ] {
            if let Some(remote) = value
                && !valid_remote(directory, remote)?
            {
                return Err(invalid(path, format!("invalid {key} name: {remote:?}")));
            }
        }
        if let Some(base) = &self.base
            && !valid_branch(directory, base)?
        {
            return Err(invalid(path, format!("invalid base branch name: {base:?}")));
        }
        if let Some(action) = &self.save_action {
            parse_action(action).map_err(|e| invalid(path, e))?;
        }
        if let Some(value) = &self.finish_mode {
            parse_mode(value).map_err(|e| invalid(path, e))?;
        }
        if let Some(value) = &self.finish_integration {
            parse_integration(value).map_err(|e| invalid(path, e))?;
        }
        if let Some(value) = &self.finish_message {
            parse_message(value).map_err(|e| invalid(path, e))?;
        }
        if let Some(value) = &self.finish_checkout {
            parse_checkout(value).map_err(|e| invalid(path, e))?;
        }
        if let Some(value) = &self.finish_cleanup {
            parse_cleanup(value).map_err(|e| invalid(path, e))?;
        }
        if let Some(update) = &self.finish_update {
            parse_update(update).map_err(|e| invalid(path, e))?;
        }
        if let Some(rewrite) = &self.finish_rewrite {
            parse_rewrite(rewrite).map_err(|e| invalid(path, e))?;
        }
        if let Some(checks) = &self.finish_checks {
            validate_finish_checks(checks).map_err(|e| invalid(path, e))?;
        }
        Ok(())
    }
    fn apply(
        self,
        settings: &mut Settings,
        redact_credentials: &mut bool,
        redact_credentials_source: &mut SettingSource,
        source: SettingSource,
    ) {
        if let Some(policies) = self.workstreams {
            for (kind, config) in policies {
                let fields = serde_json::to_value(&config).expect("config serialization");
                for key in fields.as_object().expect("config object").keys() {
                    settings
                        .sources
                        .workstreams
                        .insert(format!("{}.{}", kind.as_str(), key), source.clone());
                }
                settings
                    .workstreams
                    .entry(kind)
                    .or_default()
                    .overlay(config);
            }
        }
        if let Some(remote) = self.publish_remote {
            settings.publish_remote = remote;
            settings.sources.publish_remote = source.clone();
        }
        if let Some(remote) = self.base_remote {
            settings.base_remote = remote;
            settings.sources.base_remote = Some(source.clone());
        }
        if let Some(base) = self.base {
            settings.base = base;
            settings.sources.base = source.clone();
        }
        if let Some(action) = self.save_action {
            settings.save_action = parse_action(&action).expect("validated layer");
            settings.sources.save_action = source.clone();
        }
        if let Some(value) = self.finish_mode {
            settings.finish_mode = parse_mode(&value).expect("validated layer");
            settings.sources.finish_mode = source.clone();
        }
        if let Some(value) = self.finish_integration {
            settings.finish_integration = parse_integration(&value).expect("validated layer");
            settings.sources.finish_integration = source.clone();
        }
        if let Some(value) = self.finish_message {
            settings.finish_message = parse_message(&value).expect("validated layer");
            settings.sources.finish_message = source.clone();
        }
        if let Some(value) = self.finish_checkout {
            settings.finish_checkout = parse_checkout(&value).expect("validated layer");
            settings.sources.finish_checkout = source.clone();
        }
        if let Some(value) = self.finish_cleanup {
            settings.finish_cleanup = parse_cleanup(&value).expect("validated layer");
            settings.sources.finish_cleanup = source.clone();
        }
        if let Some(update) = self.finish_update {
            settings.finish_update = parse_update(&update).expect("validated layer");
            settings.sources.finish_update = source.clone();
        }
        if let Some(rewrite) = self.finish_rewrite {
            settings.finish_rewrite = parse_rewrite(&rewrite).expect("validated layer");
            settings.sources.finish_rewrite = source.clone();
        }
        if let Some(checks) = self.finish_checks {
            settings.finish_checks = checks;
            settings.sources.finish_checks = source.clone();
        }
        if let Some(redact) = self.redact_credentials {
            *redact_credentials = redact;
            *redact_credentials_source = source;
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
// Some YAML deserializers accept null as an empty Vec. Only a supplied sequence
// represents a list here; null must not disable inherited checks.
struct List<T>(Vec<T>);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for List<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ListVisitor<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for ListVisitor<T> {
            type Value = List<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a list")
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<List<T>, S::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element()? {
                    values.push(value);
                }
                Ok(List(values))
            }
        }
        d.deserialize_any(ListVisitor(std::marker::PhantomData))
    }
}
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
                f.write_str("a mapping of recognized Oneflow settings")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Layer, M::Error> {
                let mut layer = Layer::default();
                while let Some(Text(key)) = map.next_key::<Text>()? {
                    if key == "workstreams" {
                        if layer.workstreams.is_some() {
                            return Err(de::Error::custom("duplicate workstreams"));
                        }
                        struct Policies(BTreeMap<WorkstreamKind, WorkstreamConfig>);
                        impl<'de> Deserialize<'de> for Policies {
                            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                                deserialize_workstreams(d).map(Self)
                            }
                        }
                        layer.workstreams = Some(map.next_value::<Policies>()?.0);
                        continue;
                    }
                    if key == "finish-checks" {
                        if layer.finish_checks.is_some() {
                            return Err(de::Error::custom(format!("duplicate key {key:?}")));
                        }
                        layer.finish_checks = Some(map.next_value::<List<FinishCheck>>()?.0);
                        continue;
                    }
                    if key == "redact-credentials" {
                        if layer.redact_credentials.is_some() {
                            return Err(de::Error::custom(format!("duplicate key {key:?}")));
                        }
                        layer.redact_credentials = Some(map.next_value::<bool>()?);
                        continue;
                    }
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
pub(crate) fn recognized_paths(root: &Path) -> Result<Vec<PathBuf>, SettingsError> {
    let mut paths = Vec::new();
    for name in CONFIG_FILES {
        let path = root.join(name);
        if exists(&path)? {
            paths.push(path);
        }
    }
    Ok(paths)
}
fn discover_layer(root: &Path, local: bool) -> Result<Option<PathBuf>, SettingsError> {
    let mut paths = recognized_paths(root)?;
    if paths.len() > 1 {
        let files = paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(SettingsError::InvalidSettings(format!(
            "multiple configuration files in {} layer: {files}. Keep exactly one recognized file in this layer; extensions have no priority.",
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
        _ => unreachable!("discovered configuration format"),
    };
    result.map_err(|e| invalid(path, e))
}

/// Validate every present file before sparse merging. Remote availability is
/// deliberately left to the workflow. Git-discovered paths avoid inheritance.
pub fn load(directory: &Path, repository: &RepositoryContext) -> Result<Settings, SettingsError> {
    let mut settings = load_with_files(directory, repository)?.settings;
    let output = git::inspect(directory, ["symbolic-ref", "--quiet", "--short", "HEAD"])
        .map_err(InspectionError::Io)?;
    if output.status.success() {
        let branch = output
            .stdout
            .strip_suffix(b"\n")
            .ok_or(InspectionError::InvalidOutput("HEAD branch"))?;
        apply_branch_policy(directory, branch, &mut settings)?;
    }
    Ok(settings)
}

/// Validated settings and the files inspected, including empty/sparse layers.
#[derive(Debug)]
pub struct LoadedSettings {
    pub settings: Settings,
    pub shared: Option<PathBuf>,
    pub local: Option<PathBuf>,
    pub redact_credentials: bool,
    pub redact_credentials_source: SettingSource,
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
    let mut redact_credentials = true;
    let mut redact_credentials_source = SettingSource::BuiltIn;
    for (path, is_local) in [(shared.clone(), false), (local.clone(), true)] {
        if let Some(path) = path {
            let layer = read_layer(&path)?;
            layer.validate(directory, &path, is_local)?;
            let source = if is_local {
                SettingSource::Local(path)
            } else {
                SettingSource::Shared(path)
            };
            layer.apply(
                &mut settings,
                &mut redact_credentials,
                &mut redact_credentials_source,
                source,
            );
        }
    }
    if settings.sources.base_remote.is_none() {
        settings.base_remote = settings.publish_remote.clone();
    }
    for kind in settings.workstreams.keys() {
        let mut effective = settings.clone();
        resolve_workstream(&mut effective, *kind, None)?;
    }
    Ok(LoadedSettings {
        settings,
        shared,
        local,
        redact_credentials,
        redact_credentials_source,
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

/// Resolve explicit policy routes before start; each dimension remains independently configurable.
pub fn resolve_workstream(
    settings: &mut Settings,
    kind: WorkstreamKind,
    target_override: Option<&str>,
) -> Result<(), SettingsError> {
    let config = settings.workstreams.get(&kind).cloned().unwrap_or_default();
    if kind != WorkstreamKind::Ordinary && !settings.workstreams.contains_key(&kind) {
        return Err(SettingsError::InvalidSettings(format!(
            "configure workstreams.{} before selecting this policy (or use config --oneflow)",
            kind.as_str()
        )));
    }
    let integration_target = target_override
        .map(str::to_owned)
        .or(config.integration_target.clone())
        .unwrap_or_else(|| settings.base.clone());
    let preparation_source = target_override
        .map(str::to_owned)
        .or(config.preparation_source.clone())
        .unwrap_or_else(|| integration_target.clone());
    let active_release = match config.active_release.as_deref() {
        Some("none") => None,
        Some(branch) => Some(branch.to_owned()),
        None if kind == WorkstreamKind::Hotfix => {
            return Err(SettingsError::InvalidSettings(
                "hotfix requires explicit active-release = none or an exact branch".into(),
            ));
        }
        None => None,
    };
    let integration_target = if kind == WorkstreamKind::Hotfix {
        active_release.clone().unwrap_or(integration_target)
    } else {
        integration_target
    };
    let version_tagging = config.version_tagging.as_deref() == Some("required");
    if kind == WorkstreamKind::Ordinary && version_tagging {
        return Err(SettingsError::InvalidSettings(
            "ordinary work cannot require version tagging".into(),
        ));
    }
    if let Some(v) = &config.publish_remote {
        settings.publish_remote = v.clone();
        settings.sources.publish_remote = settings
            .sources
            .workstreams
            .get(&format!("{}.publish-remote", kind.as_str()))
            .cloned()
            .unwrap_or(SettingSource::BuiltIn);
    }
    if let Some(v) = &config.base_remote {
        settings.base_remote = v.clone();
        settings.sources.base_remote = Some(
            settings
                .sources
                .workstreams
                .get(&format!("{}.base-remote", kind.as_str()))
                .cloned()
                .unwrap_or(SettingSource::BuiltIn),
        );
    } else if settings.sources.base_remote.is_none() {
        settings.base_remote = settings.publish_remote.clone();
    }
    if let Some(v) = &config.finish_mode {
        settings.finish_mode = parse_mode(v).map_err(SettingsError::InvalidSettings)?;
        settings.sources.finish_mode = settings
            .sources
            .workstreams
            .get(&format!("{}.finish-mode", kind.as_str()))
            .cloned()
            .unwrap_or(SettingSource::BuiltIn);
    }
    if let Some(v) = &config.finish_update {
        settings.finish_update = parse_update(v).map_err(SettingsError::InvalidSettings)?;
        settings.sources.finish_update = settings
            .sources
            .workstreams
            .get(&format!("{}.finish-update", kind.as_str()))
            .cloned()
            .unwrap_or(SettingSource::BuiltIn);
    }
    if let Some(v) = &config.finish_integration {
        settings.finish_integration =
            parse_integration(v).map_err(SettingsError::InvalidSettings)?;
        settings.sources.finish_integration = settings
            .sources
            .workstreams
            .get(&format!("{}.finish-integration", kind.as_str()))
            .cloned()
            .unwrap_or(SettingSource::BuiltIn);
    }
    if let Some(v) = &config.finish_cleanup {
        settings.finish_cleanup = parse_cleanup(v).map_err(SettingsError::InvalidSettings)?;
        settings.sources.finish_cleanup = settings
            .sources
            .workstreams
            .get(&format!("{}.finish-cleanup", kind.as_str()))
            .cloned()
            .unwrap_or(SettingSource::BuiltIn);
    }
    if let Some(v) = &config.finish_checkout {
        settings.finish_checkout = parse_checkout(v).map_err(SettingsError::InvalidSettings)?;
        settings.sources.finish_checkout = settings
            .sources
            .workstreams
            .get(&format!("{}.finish-checkout", kind.as_str()))
            .cloned()
            .unwrap_or(SettingSource::BuiltIn);
    }
    if let Some(v) = &config.finish_rewrite {
        settings.finish_rewrite = parse_rewrite(v).map_err(SettingsError::InvalidSettings)?;
        settings.sources.finish_rewrite = settings
            .sources
            .workstreams
            .get(&format!("{}.finish-rewrite", kind.as_str()))
            .cloned()
            .unwrap_or(SettingSource::BuiltIn);
    }
    if let Some(v) = &config.finish_message {
        settings.finish_message = parse_message(v).map_err(SettingsError::InvalidSettings)?;
        settings.sources.finish_message = settings
            .sources
            .workstreams
            .get(&format!("{}.finish-message", kind.as_str()))
            .cloned()
            .unwrap_or(SettingSource::BuiltIn);
    }

    if version_tagging
        && (settings.finish_mode != FinishMode::Integrate
            || settings.finish_update != FinishUpdate::None
            || settings.finish_integration == FinishIntegration::Squash)
    {
        return Err(SettingsError::InvalidSettings("version-tagging requires integrate mode, finish-update = none, and history-preserving integration".into()));
    }
    let release_marker = config.release_marker.filter(|v| v != "none");
    if release_marker.as_deref() == Some(&integration_target) {
        return Err(SettingsError::InvalidSettings(
            "release marker must differ from integration target".into(),
        ));
    }
    if kind == WorkstreamKind::Ordinary && release_marker.is_some() {
        return Err(SettingsError::InvalidSettings(
            "ordinary work cannot advance a release marker".into(),
        ));
    }
    settings.base = integration_target.clone();
    if let Some(source) = settings
        .sources
        .workstreams
        .get(&format!("{}.integration-target", kind.as_str()))
    {
        settings.sources.base = source.clone();
    }
    settings.workstream = Some(WorkstreamPolicy {
        kind,
        creation_source: if target_override.is_some() {
            None
        } else {
            config.creation_source
        },
        preparation_source,
        integration_target,
        publish_remote: settings.publish_remote.clone(),
        base_remote: settings.base_remote.clone(),
        version_tagging,
        release_marker,
        active_release,
    });
    Ok(())
}

pub fn load_for_branch(
    directory: &Path,
    repository: &RepositoryContext,
    branch: &[u8],
) -> Result<Settings, SettingsError> {
    let mut settings = load_with_files(directory, repository)?.settings;
    apply_branch_policy(directory, branch, &mut settings)?;
    Ok(settings)
}
pub(crate) fn branch_policy(
    directory: &Path,
    branch: &[u8],
) -> Result<Option<BranchPolicy>, SettingsError> {
    let values =
        crate::tracking::config(directory, &crate::tracking::key(branch, b".oneflow-policy"))?;
    match values.as_slice() {
        [] => Ok(None),
        [value] => serde_json::from_slice(value)
            .map(Some)
            .map_err(|e| SettingsError::InvalidSettings(format!("invalid branch policy: {e}"))),
        _ => Err(SettingsError::InvalidSettings(
            "ambiguous branch policy: multiple associations".into(),
        )),
    }
}
pub(crate) fn branch_anchor(
    directory: &Path,
    branch: &[u8],
) -> Result<Option<Vec<u8>>, SettingsError> {
    let path_ref = [b"logs/refs/heads/".as_slice(), branch].concat();
    let args = vec![
        "rev-parse".into(),
        "--git-path".into(),
        crate::repository::os(&path_ref)?,
    ];
    let output = git::inspect(directory, &args).map_err(InspectionError::Io)?;
    if !output.status.success() {
        return Err(InspectionError::Git {
            arguments: args,
            output,
        }
        .into());
    }
    let path_bytes = output
        .stdout
        .strip_suffix(b"\n")
        .ok_or(InspectionError::InvalidOutput("branch reflog path"))?;
    let path = PathBuf::from(crate::repository::os(path_bytes)?);
    let path = if path.is_absolute() {
        path
    } else {
        directory.join(path)
    };
    match fs::read(&path) {
        Ok(bytes) => Ok(bytes
            .split(|b| *b == b'\n')
            .next()
            .filter(|v| !v.is_empty())
            .map(<[u8]>::to_vec)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(SettingsError::SettingsIo { path, source }),
    }
}
fn apply_branch_policy(
    directory: &Path,
    branch: &[u8],
    settings: &mut Settings,
) -> Result<(), SettingsError> {
    if let Some(record) = branch_policy(directory, branch)? {
        // Missing source is allowed for retained completion. A surviving or recreated
        // source must still carry the exact original branch-creation reflog entry.
        let args = vec![
            "show-ref".into(),
            "--verify".into(),
            "--quiet".into(),
            crate::repository::os(&[b"refs/heads/".as_slice(), branch].concat())?,
        ];
        let output = git::inspect(directory, &args).map_err(InspectionError::Io)?;
        if output.status.success()
            && branch_anchor(directory, branch)?.as_ref() != Some(&record.anchor)
        {
            return Err(SettingsError::InvalidSettings("branch policy belongs to an earlier branch instance; resolve the association before using recreated work".into()));
        }
        if !output.status.success() && output.status.code() != Some(1) {
            return Err(InspectionError::Git {
                arguments: args,
                output,
            }
            .into());
        }
        resolve_workstream(
            settings,
            record.policy.kind,
            record.target_override.as_deref(),
        )?;
        if settings.workstream.as_ref() != Some(&record.policy) {
            return Err(SettingsError::InvalidSettings("established workstream routes changed; restore the policy configuration before resuming".into()));
        }
    }
    Ok(())
}
/// Explicit editable OneFlow data; selecting this bundle never changes Git refs.
pub fn oneflow_bundle(settings: &mut Settings, marker: Option<String>) {
    let development = settings.base.clone();
    let project_remote = settings.base_remote.clone();
    settings.workstreams.insert(
        WorkstreamKind::Ordinary,
        WorkstreamConfig {
            creation_source: Some(development.clone()),
            preparation_source: Some(development.clone()),
            integration_target: Some(development.clone()),
            version_tagging: Some("disabled".into()),
            release_marker: Some("none".into()),
            finish_mode: Some("integrate".into()),
            finish_update: Some("merge".into()),
            finish_integration: Some("merge".into()),
            finish_cleanup: Some("both".into()),
            finish_checkout: Some("target".into()),
            finish_message: Some("auto".into()),
            ..WorkstreamConfig::default()
        },
    );
    for kind in [WorkstreamKind::Release, WorkstreamKind::Hotfix] {
        settings.workstreams.insert(
            kind,
            WorkstreamConfig {
                creation_source: (kind == WorkstreamKind::Release).then(|| development.clone()),
                preparation_source: Some(development.clone()),
                integration_target: Some(development.clone()),
                publish_remote: Some(project_remote.clone()),
                base_remote: Some(project_remote.clone()),
                version_tagging: Some("required".into()),
                release_marker: Some(marker.clone().unwrap_or_else(|| "none".into())),
                active_release: (kind == WorkstreamKind::Hotfix).then(|| "none".into()),
                finish_mode: Some("integrate".into()),
                finish_update: Some("none".into()),
                finish_integration: Some("merge".into()),
                finish_cleanup: Some("both".into()),
                finish_checkout: Some("target".into()),
                finish_rewrite: Some("deny".into()),
                finish_message: Some("auto".into()),
            },
        );
    }
}
