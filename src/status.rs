//! Human-readable presentation of repository observations at the CLI boundary.

use git_oneflow::repository::{
    Change, ChangeKind, Head, RepositoryKind, RepositoryState, Upstream, UpstreamReference,
};

// Escape controls and backslashes; replace invalid UTF-8 for terminal display.
fn display(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .flat_map(|ch| {
            if ch.is_control() || ch == '\\' {
                ch.escape_default().collect::<Vec<_>>()
            } else {
                vec![ch]
            }
        })
        .collect()
}

pub fn print(state: &RepositoryState) {
    let kind = match state.kind {
        RepositoryKind::Working => "working tree",
        RepositoryKind::LinkedWorktree => "linked worktree",
        RepositoryKind::Bare => "bare (no working tree)",
    };
    println!("Repository: {kind}");
    match &state.head {
        Head::Attached { branch, .. } => println!("Branch: {}", display(branch)),
        Head::Unborn { branch } => println!("Branch: {} (unborn; no commits)", display(branch)),
        Head::Detached { commit } => println!("HEAD: detached at {}", display(commit)),
    }
    if let Some(tree) = &state.working_tree {
        if !tree.is_dirty() {
            println!("Working tree: clean");
        } else {
            let (mut staged, mut unstaged, mut conflicts, mut untracked, mut submodules) =
                (0, 0, 0, 0, 0);
            for change in &tree.changes {
                let submodule = match &change.kind {
                    ChangeKind::Tracked {
                        staged: index,
                        unstaged: worktree,
                        submodule,
                        ..
                    } => {
                        staged += usize::from(*index != Change::Unchanged);
                        unstaged += usize::from(*worktree != Change::Unchanged);
                        submodule
                    }
                    ChangeKind::Conflicted { submodule, .. } => {
                        conflicts += 1;
                        submodule
                    }
                    ChangeKind::Untracked => {
                        untracked += 1;
                        &None
                    }
                };
                if submodule
                    .is_some_and(|s| s.commit_changed || s.tracked_changes || s.untracked_changes)
                {
                    submodules += 1;
                }
            }
            println!(
                "Working tree: {staged} staged, {unstaged} unstaged, {conflicts} conflicted, {untracked} untracked paths"
            );
            if submodules > 0 {
                println!("Submodules: {submodules} with commit or content changes");
            }
        }
    }
    let operations: Vec<_> = [
        (state.operations.merge, "merge"),
        (state.operations.rebase, "rebase"),
        (state.operations.cherry_pick, "cherry-pick"),
        (state.operations.revert, "revert"),
        (state.operations.am, "am"),
        (state.operations.bisect, "bisect"),
    ]
    .into_iter()
    .filter_map(|(present, name)| present.then_some(name))
    .collect();
    println!(
        "Operations: {}",
        if operations.is_empty() {
            "none detected".to_owned()
        } else {
            operations.join(", ")
        }
    );
    match &state.upstream {
        Upstream::NotApplicable => println!("Upstream: not applicable (detached HEAD)"),
        Upstream::Absent => println!("Upstream: not configured"),
        Upstream::Configured {
            remote,
            merge_refs,
            local_reference,
            reference,
        } => {
            println!(
                "Upstream: remote {}; merge {}",
                remote
                    .as_deref()
                    .map(display)
                    .unwrap_or_else(|| "unspecified".into()),
                if merge_refs.is_empty() {
                    "unspecified".into()
                } else {
                    merge_refs
                        .iter()
                        .map(|r| display(r))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            );
            if let Some(reference) = local_reference {
                println!("Local upstream reference: {}", display(reference));
            }
            match reference {
                UpstreamReference::Unavailable => {
                    println!("Upstream comparison: unavailable (local reference unavailable)");
                }
                UpstreamReference::Available {
                    divergence: None, ..
                } => {
                    println!("Upstream comparison: unavailable (HEAD has no commits)");
                }
                UpstreamReference::Available {
                    divergence: Some(counts),
                    ..
                } => {
                    println!(
                        "Upstream comparison: {} ahead, {} behind (locally available references/history only)",
                        counts.ahead, counts.behind
                    );
                }
            }
        }
    }
}
