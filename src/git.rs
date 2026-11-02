//! Thin wrapper around the git command line.
//!
//! The installed `git` is used on purpose: it applies the user's credentials, SSH keys and
//! configuration exactly as on the command line.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

/// Builds a non-interactive git command: no credential prompts, English messages so that
/// errors can be classified.
pub fn command(repo: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    if let Some(repo) = repo {
        command.arg("-C").arg(repo);
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env("LANGUAGE", "C")
        .stdin(Stdio::null());
    command
}

/// Output of a git command.
#[derive(Debug)]
pub struct GitOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Runs git with the given arguments and captures its output.
pub fn run(repo: Option<&Path>, args: &[&str]) -> Result<GitOutput> {
    let output = command(repo)
        .args(args)
        .output()
        .context("cannot run git: is it installed and on the PATH?")?;
    Ok(GitOutput {
        success: output.status.success(),
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string(),
        stderr: String::from_utf8_lossy(&output.stderr)
            .trim_end()
            .to_string(),
    })
}

/// Runs git and returns its standard output, failing with the error message of git.
pub fn run_ok(repo: Option<&Path>, args: &[&str]) -> Result<String> {
    let output = run(repo, args)?;
    if !output.success {
        bail!("git {} failed: {}", args.join(" "), output.stderr);
    }
    Ok(output.stdout)
}

/// Version string of the installed git, for example `git version 2.47.0`.
pub fn version() -> Result<String> {
    run_ok(None, &["--version"])
}

/// Name of the checked-out branch, `None` when HEAD is detached.
pub fn current_branch(repo: &Path) -> Result<Option<String>> {
    let output = run(Some(repo), &["symbolic-ref", "--short", "-q", "HEAD"])?;
    match output.code {
        Some(0) => Ok(Some(output.stdout)),
        Some(1) => Ok(None),
        _ => bail!("cannot read the current branch: {}", output.stderr),
    }
}

/// Names of the configured remotes.
pub fn remotes(repo: &Path) -> Result<Vec<String>> {
    Ok(run_ok(Some(repo), &["remote"])?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// The remote to synchronise with: `origin` when present, otherwise the first one.
pub fn main_remote(repo: &Path) -> Result<Option<(String, String)>> {
    let remotes = remotes(repo)?;
    let name = match remotes
        .iter()
        .find(|name| *name == "origin")
        .or_else(|| remotes.first())
    {
        Some(name) => name.clone(),
        None => return Ok(None),
    };
    let url = run_ok(Some(repo), &["remote", "get-url", &name])?;
    Ok(Some((name, url)))
}

/// Branch names published by the remote (`git ls-remote --heads`), or the error of git.
pub fn remote_heads(repo: &Path, remote: &str) -> Result<std::result::Result<Vec<String>, String>> {
    let output = run(Some(repo), &["ls-remote", "--heads", remote])?;
    if !output.success {
        return Ok(Err(output.stderr));
    }
    Ok(Ok(parse_heads(&output.stdout)))
}

/// Branch names from the output of `git ls-remote --heads`.
pub fn parse_heads(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter_map(|(_, reference)| reference.trim().strip_prefix("refs/heads/"))
        .map(str::to_string)
        .collect()
}

/// Default branch of the remote (the branch its `HEAD` points to), when it publishes one.
pub fn remote_default_branch(repo: &Path, remote: &str) -> Result<Option<String>> {
    let output = run(Some(repo), &["ls-remote", "--symref", remote, "HEAD"])?;
    if !output.success {
        return Ok(None);
    }
    Ok(parse_symref_head(&output.stdout))
}

/// Branch name from the output of `git ls-remote --symref <remote> HEAD`.
pub fn parse_symref_head(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (reference, name) = line.strip_prefix("ref:")?.trim().split_once('\t')?;
        (name.trim() == "HEAD")
            .then(|| reference.trim().strip_prefix("refs/heads/"))
            .flatten()
            .map(str::to_string)
    })
}

/// Updates the remote-tracking refs of the given branches without pruning anything.
pub fn fetch_branches(repo: &Path, remote: &str, branches: &[&str]) -> Result<GitOutput> {
    let mut args = vec!["fetch", "--quiet", "--no-prune", remote];
    args.extend_from_slice(branches);
    run(Some(repo), &args)
}

/// Committer date (Unix seconds) of the commit a reference points to.
pub fn commit_time(repo: &Path, reference: &str) -> Result<Option<i64>> {
    let output = run(Some(repo), &["log", "-1", "--format=%ct", reference, "--"])?;
    if !output.success {
        return Ok(None);
    }
    Ok(output.stdout.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ls_remote_heads() {
        let output = "a1b2\trefs/heads/develop\nc3d4\trefs/heads/feature/x\ne5f6\trefs/tags/v1\n";
        assert_eq!(parse_heads(output), vec!["develop", "feature/x"]);
        assert!(parse_heads("").is_empty());
    }

    #[test]
    fn parses_remote_default_branch() {
        let output = "ref: refs/heads/trunk\tHEAD\na1b2\tHEAD\n";
        assert_eq!(parse_symref_head(output).as_deref(), Some("trunk"));
        assert_eq!(parse_symref_head("a1b2\tHEAD\n"), None);
        assert_eq!(parse_symref_head(""), None);
    }
}

/// A local branch and the remote-tracking ref it follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalBranch {
    pub name: String,
    pub sha: String,
    /// Full name of the upstream ref (`refs/remotes/<remote>/<branch>`), even when that ref no
    /// longer exists.
    pub upstream: Option<String>,
}

/// Local branches with their upstream.
pub fn local_branches(repo: &Path) -> Result<Vec<LocalBranch>> {
    let output = run_ok(
        Some(repo),
        &[
            "for-each-ref",
            "--format=%(refname:short)%09%(objectname)%09%(upstream)",
            "refs/heads/",
        ],
    )?;
    Ok(parse_local_branches(&output))
}

/// Parses the output of `local_branches`.
pub fn parse_local_branches(output: &str) -> Vec<LocalBranch> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?.trim();
            let sha = fields.next()?.trim();
            let upstream = fields.next().map(str::trim).filter(|u| !u.is_empty());
            (!name.is_empty() && !sha.is_empty()).then(|| LocalBranch {
                name: name.to_string(),
                sha: sha.to_string(),
                upstream: upstream.map(str::to_string),
            })
        })
        .collect()
}

/// Remote-tracking refs of a remote, as branch name -> commit.
pub fn remote_tracking_refs(
    repo: &Path,
    remote: &str,
) -> Result<std::collections::BTreeMap<String, String>> {
    let prefix = format!("refs/remotes/{remote}/");
    let output = run_ok(
        Some(repo),
        &[
            "for-each-ref",
            "--format=%(refname)%09%(objectname)",
            &prefix,
        ],
    )?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let (reference, sha) = line.split_once('\t')?;
            let name = reference.strip_prefix(&prefix)?;
            (name != "HEAD").then(|| (name.to_string(), sha.trim().to_string()))
        })
        .collect())
}

/// Commit a revision points to, `None` when it does not exist.
pub fn resolve(repo: &Path, revision: &str) -> Result<Option<String>> {
    let output = run(
        Some(repo),
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{revision}^{{commit}}"),
        ],
    )?;
    Ok(output.success.then_some(output.stdout))
}

/// Whether `ancestor` is reachable from `descendant`.
pub fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let output = run(
        Some(repo),
        &["merge-base", "--is-ancestor", ancestor, descendant],
    )?;
    match output.code {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!(
            "cannot compare {ancestor} and {descendant}: {}",
            output.stderr
        ),
    }
}

/// Number of commits selected by the arguments of `git rev-list`.
pub fn count_commits(repo: &Path, args: &[&str]) -> Result<usize> {
    let mut all = vec!["rev-list", "--count"];
    all.extend_from_slice(args);
    Ok(run_ok(Some(repo), &all)?.trim().parse().unwrap_or(0))
}

/// Whether the commit is contained in at least one branch of the remote.
pub fn contained_in_remote(repo: &Path, remote: &str, sha: &str) -> Result<bool> {
    let output = run_ok(
        Some(repo),
        &[
            "for-each-ref",
            "--count=1",
            "--contains",
            sha,
            "--format=%(refname)",
            &format!("refs/remotes/{remote}/"),
        ],
    )?;
    Ok(!output.trim().is_empty())
}

/// Whether merging `sha` into `base` would leave the tree of `base` unchanged: the changes of
/// the branch are already in `base`, as after a squash merge. Needs git 2.38 or later; returns
/// `false` when the check is not possible.
pub fn changes_already_in(repo: &Path, sha: &str, base: &str) -> Result<bool> {
    let merged = run(Some(repo), &["merge-tree", "--write-tree", base, sha])?;
    if !merged.success {
        return Ok(false);
    }
    let Some(merged_tree) = merged.stdout.lines().next().map(str::trim) else {
        return Ok(false);
    };
    let base_tree = run(Some(repo), &["rev-parse", &format!("{base}^{{tree}}")])?;
    Ok(base_tree.success && base_tree.stdout.trim() == merged_tree)
}

/// Uncommitted changes: modified tracked files and number of untracked files.
pub fn working_tree_changes(repo: &Path) -> Result<(Vec<String>, usize)> {
    let output = run_ok(
        Some(repo),
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )?;
    Ok(parse_status(&output))
}

/// Parses `git status --porcelain=v1`.
pub fn parse_status(output: &str) -> (Vec<String>, usize) {
    let mut modified = Vec::new();
    let mut untracked = 0;
    for line in output.lines().filter(|line| line.len() > 3) {
        if line.starts_with("??") {
            untracked += 1;
        } else if !line.starts_with("!!") {
            modified.push(line[3..].to_string());
        }
    }
    (modified, untracked)
}

/// Name of the operation left in progress (rebase, merge, ...), if any.
pub fn operation_in_progress(repo: &Path) -> Result<Option<&'static str>> {
    const MARKERS: &[(&str, &str)] = &[
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase or am"),
        ("MERGE_HEAD", "merge"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
        ("BISECT_LOG", "bisect"),
    ];
    for (marker, name) in MARKERS {
        let path = run_ok(Some(repo), &["rev-parse", "--git-path", marker])?;
        let path = Path::new(path.trim());
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            repo.join(path)
        };
        if path.exists() {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

/// Branches checked out in the other worktrees of the repository.
pub fn branches_in_other_worktrees(repo: &Path) -> Result<std::collections::BTreeSet<String>> {
    let output = run_ok(Some(repo), &["worktree", "list", "--porcelain"])?;
    let this = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let mut branches = std::collections::BTreeSet::new();
    for block in output.split("\n\n") {
        let mut path = None;
        let mut branch = None;
        for line in block.lines() {
            if let Some(value) = line.strip_prefix("worktree ") {
                path = Some(std::fs::canonicalize(value).unwrap_or_else(|_| value.into()));
            } else if let Some(value) = line.strip_prefix("branch refs/heads/") {
                branch = Some(value.to_string());
            }
        }
        if let (Some(path), Some(branch)) = (path, branch) {
            if path != this {
                branches.insert(branch);
            }
        }
    }
    Ok(branches)
}

/// Number of stash entries.
pub fn stash_count(repo: &Path) -> Result<usize> {
    Ok(run_ok(Some(repo), &["stash", "list"])?.lines().count())
}

#[cfg(test)]
mod sync_parsing_tests {
    use super::*;

    #[test]
    fn parses_local_branches() {
        let output = "develop\ta1\trefs/remotes/origin/develop\nlocal-only\tb2\t\n";
        let branches = parse_local_branches(output);
        assert_eq!(branches.len(), 2);
        assert_eq!(
            branches[0].upstream.as_deref(),
            Some("refs/remotes/origin/develop")
        );
        assert_eq!(branches[1].upstream, None);
    }

    #[test]
    fn parses_status() {
        let output = " M src/a.rs\nM  src/b.rs\n?? notes.txt\nR  old.rs -> new.rs\n";
        let (modified, untracked) = parse_status(output);
        assert_eq!(modified, vec!["src/a.rs", "src/b.rs", "old.rs -> new.rs"]);
        assert_eq!(untracked, 1);
    }
}
