//! Shell-neutral grammar and offline values. Shell adapters own tokenization and
//! quoting; this boundary receives and returns literal arguments only.
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::{OsStr, OsString},
    io::{self, Write},
    path::Path,
};

use clap::{Arg, ArgAction, Command, ValueHint};
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate};

use crate::{git, repository, settings};

#[derive(Clone, Copy)]
enum Values {
    WorkBranches,
    TargetBranches,
    CurrentTargets,
    Sources,
    Tags,
}

fn flag(name: &'static str) -> Arg {
    Arg::new(name).long(name).action(ArgAction::SetTrue)
}
fn value(name: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .num_args(1)
        .allow_hyphen_values(true)
        .value_hint(ValueHint::Other)
}
fn fixed(name: &'static str, choices: impl Into<clap::builder::ValueParser>) -> Arg {
    value(name).value_parser(choices)
}
fn dynamic(
    arg: Arg,
    kind: Values,
    directory: &Path,
    policy: Option<settings::WorkstreamKind>,
) -> Arg {
    let directory = directory.to_owned();
    arg.add(ArgValueCompleter::new(move |current: &OsStr| {
        repository_values(&directory, kind, policy, current)
            .into_iter()
            .map(CompletionCandidate::new)
            .collect::<Vec<_>>()
    }))
}

/// The accepted command vocabulary shared by all shell adapters. Workflow
/// parsers retain their validation and execution responsibilities.
pub fn command() -> Command {
    command_in(&env::current_dir().unwrap_or_default(), None)
}

fn command_in(directory: &Path, policy: Option<settings::WorkstreamKind>) -> Command {
    let branches = |arg, kind| dynamic(arg, kind, directory, policy);
    let start = Command::new("start")
        .visible_alias("s")
        .arg(branches(
            Arg::new("branch-name")
                .required(true)
                .value_hint(ValueHint::Other),
            Values::WorkBranches,
        ))
        .arg(fixed("policy", ["ordinary", "release", "hotfix"]))
        .arg(branches(
            value("from").conflicts_with("from-tag"),
            Values::Sources,
        ))
        .arg(branches(
            value("from-tag").conflicts_with("from"),
            Values::Tags,
        ))
        .arg(branches(value("target"), Values::TargetBranches));
    let save = Command::new("save")
        .visible_alias("v")
        .arg(flag("publish-only").conflicts_with_all(["commit", "stash"]))
        .arg(
            fixed("commit", ["all", "staged", "unstaged"])
                .action(ArgAction::Append)
                .conflicts_with("publish-only"),
        )
        .arg(
            fixed("stash", ["all", "staged", "unstaged"])
                .action(ArgAction::Append)
                .conflicts_with("publish-only"),
        )
        .arg(value("message"))
        .arg(value("stash-message"));
    let finish = Command::new("finish")
        .visible_alias("f")
        .arg(fixed("mode", ["review", "integrate"]))
        .arg(fixed("integration", ["ff-only", "merge", "squash"]))
        .arg(fixed("checkout", ["target", "work"]))
        .arg(fixed("cleanup", ["keep", "local", "both"]))
        .arg(value("message").short('m'))
        .arg(branches(value("merge-back"), Values::CurrentTargets))
        // --tag creates a version identity; existing tags are source hints only.
        .arg(value("tag"))
        .arg(value("tag-message"))
        .arg(flag("sign-tag"))
        .arg(fixed("update", ["rebase", "merge", "none"]))
        .arg(fixed("history-edit", ["none", "interactive"]))
        .args([
            flag("allow-rewrite").conflicts_with("no-rewrite"),
            flag("no-rewrite").conflicts_with("allow-rewrite"),
            flag("skip-checks"),
            flag("show-credentials"),
        ]);
    let config = Command::new("config")
        .subcommand(Command::new("show").disable_help_flag(true))
        .arg(flag("yes").short('y'))
        .args([
            flag("oneflow").conflicts_with("oneflow-hosted"),
            flag("oneflow-hosted").conflicts_with("oneflow"),
        ])
        .arg(branches(value("marker"), Values::TargetBranches))
        .arg(fixed("format", ["toml", "yaml", "json"]));
    let shells = ["bash", "zsh", "fish", "powershell"];
    let registration = |name| {
        Command::new(name)
            .arg(Arg::new("shell").required(true).value_parser(shells))
            .arg(value("file").value_hint(ValueHint::AnyPath))
            .arg(flag("yes"))
    };
    let completion = Command::new("completion")
        .arg(Arg::new("shell").value_parser(shells))
        .subcommands([registration("install"), registration("uninstall")]);
    Command::new("twigsmith")
        .version(env!("CARGO_PKG_VERSION"))
        .disable_help_subcommand(true)
        .subcommands([
            start,
            save,
            finish,
            Command::new("status"),
            config,
            completion,
        ])
}

fn safe(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_control)
}

/// Query with a zero-based cursor index, including the executable as word zero.
/// The current word contains only text before the cursor. No process-wide cwd or
/// environment changes are needed, including when called by library tests.
pub fn candidates(words: &[OsString], index: usize, directory: &Path) -> Vec<String> {
    if index == 0 || index >= words.len() {
        return Vec::new();
    }
    if words.get(1).is_some_and(|value| value == "config")
        && index > 2
        && words.get(2).is_some_and(|value| value == "show")
    {
        return Vec::new();
    }
    let policy = words[..index]
        .windows(2)
        .find(|pair| pair[0] == "--policy")
        .and_then(|pair| pair[1].to_str())
        .and_then(|value| settings::WorkstreamKind::parse(value).ok());
    let mut command = command_in(directory, policy);
    let completed =
        clap_complete::engine::complete(&mut command, words.to_vec(), index, Some(directory))
            .unwrap_or_default();
    let mut current = &command;
    let mut position = 1;
    let mut used = BTreeSet::new();
    let mut pending_value = false;
    while position < index {
        let word = words[position].to_str().unwrap_or_default();
        if let Some(subcommand) = current.find_subcommand(word) {
            current = subcommand;
        } else if let Some(arg) = current.get_arguments().find(|arg| {
            arg.get_long()
                .is_some_and(|name| word.strip_prefix("--") == Some(name))
                || arg
                    .get_short()
                    .is_some_and(|short| word == format!("-{short}"))
        }) {
            if arg.get_id().as_str() == "help" || arg.get_id().as_str() == "version" {
                return Vec::new();
            }
            if !matches!(arg.get_action(), ArgAction::Append) {
                used.insert(arg.get_id().as_str());
            }
            used.extend(
                current
                    .get_arg_conflicts_with(arg)
                    .into_iter()
                    .map(|arg| arg.get_id().as_str()),
            );
            if arg.get_num_args().is_some_and(|count| count.takes_values()) {
                pending_value = position + 1 == index;
                position += 1;
            }
        }
        position += 1;
    }
    if index > 2
        && matches!(
            current.get_name(),
            "start" | "save" | "finish" | "config" | "status"
        )
    {
        used.insert("help");
    }
    let prefix = words[index].to_str().unwrap_or_default();
    // Workflow option syntax uses separated values, while literal pending
    // values may legitimately start with '--' and contain '=' (exact tags do).
    if !pending_value && prefix.starts_with("--") && prefix.contains('=') {
        return Vec::new();
    }
    let mut expanded = BTreeSet::new();
    for candidate in completed {
        let id = candidate.get_id().map(String::as_str).unwrap_or_default();
        if let Some(name) = id.strip_prefix("command::") {
            if let Some(subcommand) = current.find_subcommand(name) {
                expanded.extend(
                    subcommand
                        .get_name_and_visible_aliases()
                        .into_iter()
                        .map(str::to_owned),
                );
            }
        } else if let Some(id) = id.strip_prefix("arg::") {
            if used.contains(id) {
                continue;
            }
            if let Some(arg) = current
                .get_arguments()
                .find(|arg| arg.get_id().as_str() == id)
            {
                if let Some(long) = arg.get_long() {
                    expanded.insert(format!("--{long}"));
                }
                if let Some(short) = arg.get_short() {
                    expanded.insert(format!("-{short}"));
                }
            }
        } else if let Some(value) = candidate.get_value().to_str() {
            expanded.insert(value.to_owned());
        }
    }
    expanded
        .into_iter()
        .filter(|value| safe(value) && value.starts_with(prefix))
        .collect()
}

/// Handle the private adapter protocol before activity or workflow dispatch.
/// Invalid requests and unavailable observations produce no diagnostics.
pub fn try_complete() -> bool {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    if arguments.first().is_none_or(|arg| arg != "__complete") {
        return false;
    }
    if let [
        _,
        shell_flag,
        shell,
        index_flag,
        index,
        separator,
        words @ ..,
    ] = arguments.as_slice()
        && shell_flag == "--shell"
        && matches!(shell.to_str(), Some("bash" | "zsh" | "fish" | "powershell"))
        && index_flag == "--index"
        && separator == "--"
        && let Some(index) = index.to_str().and_then(|value| value.parse().ok())
        && let Ok(directory) = env::current_dir()
    {
        let mut out = io::stdout().lock();
        for value in candidates(words, index, &directory) {
            if writeln!(out, "{value}").is_err() {
                break;
            }
        }
    }
    true
}

fn read(directory: &Path, arguments: &[&str]) -> Option<Vec<u8>> {
    let output = git::inspect(directory, arguments).ok()?;
    output.status.success().then_some(output.stdout)
}

#[derive(Clone)]
struct Ref {
    name: String,
    commit: bool,
}
fn references(directory: &Path, patterns: &[String]) -> Vec<Ref> {
    let mut arguments = vec![
        "for-each-ref",
        "--format=%(refname)%09%(symref)%09%(objecttype)%09%(*objecttype)",
    ];
    arguments.extend(patterns.iter().map(String::as_str));
    read(directory, &arguments)
        .unwrap_or_default()
        .split(|byte| *byte == b'\n')
        .filter_map(|line| {
            let line = std::str::from_utf8(line).ok()?;
            let mut fields = line.split('\t');
            let name = fields.next()?;
            let symbolic = fields.next()?;
            let object = fields.next()?;
            let peeled = fields.next()?;
            (safe(name) && symbolic.is_empty()).then(|| Ref {
                name: name.to_owned(),
                commit: object == "commit" || peeled == "commit",
            })
        })
        .collect()
}

fn resolved_settings(
    directory: &Path,
    kind: Values,
    policy: Option<settings::WorkstreamKind>,
) -> Option<settings::Settings> {
    let context = repository::discover(directory).ok()??;
    let mut resolved = if matches!(kind, Values::CurrentTargets) {
        settings::load(directory, &context).ok()?
    } else {
        settings::load_with_files(directory, &context)
            .ok()?
            .settings
    };
    if let Some(policy) = policy {
        settings::resolve_workstream(&mut resolved, policy, None).ok()?;
    }
    Some(resolved)
}

fn substitute(pattern: &str, replacement: &str, value: &str) -> Option<String> {
    match (pattern.split_once('*'), replacement.split_once('*')) {
        (None, None) => (pattern == value).then(|| replacement.to_owned()),
        (Some((prefix, suffix)), Some((before, after)))
            if !suffix.contains('*') && !after.contains('*') =>
        {
            Some(format!(
                "{before}{}{after}",
                value.strip_prefix(prefix)?.strip_suffix(suffix)?
            ))
        }
        _ => None,
    }
}

fn fetch_mappings(directory: &Path, remote: &str) -> Vec<String> {
    let raw = read(
        directory,
        &["config", "--null", "--get-regexp", "^remote\\..*\\.fetch$"],
    )
    .unwrap_or_default();
    let key = format!("remote.{remote}.fetch");
    raw.split(|byte| *byte == 0)
        .filter_map(|record| std::str::from_utf8(record).ok()?.split_once('\n'))
        .filter(|(name, _)| *name == key)
        .map(|(_, value)| value.strip_prefix('+').unwrap_or(value).to_owned())
        .collect()
}

fn mapped_branches(mappings: &[String], refs: &[Ref]) -> BTreeSet<String> {
    let mut branches = BTreeSet::new();
    for reference in refs.iter().filter(|reference| reference.commit) {
        let mut sources = BTreeSet::new();
        for mapping in mappings {
            let Some((source, target)) = mapping.split_once(':') else {
                continue;
            };
            if !source.starts_with("refs/heads/") || !target.starts_with("refs/") {
                continue;
            }
            let Some(source) = substitute(target, source, &reference.name) else {
                continue;
            };
            if mappings
                .iter()
                .filter_map(|value| value.strip_prefix('^'))
                .any(|negative| substitute(negative, negative, &source).is_some())
            {
                continue;
            }
            sources.insert(source);
        }
        // Overlapping mappings cannot identify which source owns a cache ref.
        if sources.len() == 1
            && let Some(name) = sources
                .first()
                .and_then(|source| source.strip_prefix("refs/heads/"))
            && safe(name)
            && name != "HEAD"
            && !name.starts_with('-')
        {
            branches.insert(name.to_owned());
        }
    }
    branches
}

fn repository_values(
    directory: &Path,
    kind: Values,
    policy: Option<settings::WorkstreamKind>,
    current: &OsStr,
) -> Vec<String> {
    let Some(prefix) = current.to_str() else {
        return Vec::new();
    };
    let mut values = BTreeSet::new();
    match kind {
        Values::WorkBranches | Values::TargetBranches | Values::CurrentTargets => {
            let settings = resolved_settings(directory, kind, policy);
            let mappings = settings
                .as_ref()
                .map(|settings| {
                    let remote = if matches!(kind, Values::WorkBranches) {
                        &settings.publish_remote
                    } else {
                        &settings.base_remote
                    };
                    fetch_mappings(directory, remote)
                })
                .unwrap_or_default();
            // Git's wildcard filtering treats '/' as a path boundary. '**'
            // retains nested branch names; ordinary '*' would omit them.
            let mut patterns = vec![format!("refs/heads/{prefix}**")];
            // Keep every selected cache destination, including nonstandard
            // namespaces, while narrowing potentially large local inventories.
            patterns.extend(mappings.iter().filter_map(|mapping| {
                let (source, target) = mapping.split_once(':')?;
                (source.starts_with("refs/heads/") && target.starts_with("refs/"))
                    .then(|| target.replace('*', "**"))
            }));
            let refs = references(directory, &patterns);
            values.extend(refs.iter().filter_map(|reference| {
                reference
                    .name
                    .strip_prefix("refs/heads/")
                    .map(str::to_owned)
            }));
            if let Some(settings) = settings {
                values.extend(mapped_branches(&mappings, &refs));
                if matches!(kind, Values::WorkBranches) {
                    values.remove(&settings.base);
                    if let Some(marker) = settings
                        .workstream
                        .as_ref()
                        .and_then(|policy| policy.release_marker.as_ref())
                    {
                        values.remove(marker);
                    }
                }
            }
            values.retain(|value| value != "HEAD" && !value.starts_with('-'));
        }
        Values::Tags => {
            let refs = references(directory, &[format!("refs/tags/{prefix}**")]);
            values.extend(refs.iter().filter(|reference| reference.commit).filter_map(
                |reference| reference.name.strip_prefix("refs/tags/").map(str::to_owned),
            ));
        }
        Values::Sources => {
            let patterns = if prefix.is_empty() {
                vec![]
            } else if prefix.starts_with("refs/") {
                vec![format!("{prefix}**")]
            } else {
                vec![
                    format!("refs/heads/{prefix}**"),
                    format!("refs/tags/{prefix}**"),
                    format!("refs/remotes/{prefix}**"),
                    format!("refs/{prefix}**"),
                ]
            };
            let refs = references(directory, &patterns);
            let mut short = BTreeMap::<String, usize>::new();
            for reference in &refs {
                let name = reference
                    .name
                    .strip_prefix("refs/heads/")
                    .or_else(|| reference.name.strip_prefix("refs/tags/"))
                    .or_else(|| reference.name.strip_prefix("refs/remotes/"));
                if let Some(name) = name {
                    *short.entry(name.to_owned()).or_default() += 1;
                }
                if let Some(name) = reference.name.strip_prefix("refs/") {
                    *short.entry(name.to_owned()).or_default() += 1;
                }
                if reference.commit {
                    values.insert(reference.name.clone());
                }
            }
            for reference in refs.iter().filter(|reference| reference.commit) {
                let name = reference
                    .name
                    .strip_prefix("refs/heads/")
                    .or_else(|| reference.name.strip_prefix("refs/tags/"))
                    .or_else(|| reference.name.strip_prefix("refs/remotes/"));
                if let Some(name) = name
                    && short.get(name) == Some(&1)
                    && !name.starts_with('-')
                    && name != "HEAD"
                {
                    values.insert(name.to_owned());
                }
            }
            if "HEAD".starts_with(prefix)
                && read(
                    directory,
                    &[
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        "--end-of-options",
                        "HEAD^{commit}",
                    ],
                )
                .is_some()
            {
                values.insert("HEAD".to_owned());
            }
        }
    }
    values
        .into_iter()
        .filter(|value| safe(value) && value.starts_with(prefix))
        .collect()
}
