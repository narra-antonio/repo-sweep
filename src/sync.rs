//! The `sync` command: brings every repository up to date with its remote.
//!
//! For each repository: fetch with prune; when the working tree is clean, switch to the
//! development branch (creating it from the remote if needed) and fast-forward it; fast-forward
//! every other local branch that follows a remote branch without checking it out; delete the
//! local branches whose remote branch is gone, but only when their commits are known to be on the
//! remote or already in the development branch. Uncommitted work is never touched: it is
//! reported instead.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use anyhow::Result;

use crate::check::{self, CheckReport, RepoInfo};
use crate::config::{Config, OnUnreachable};
use crate::connectivity::Endpoint;
use crate::git;
use crate::parallel;
use crate::remote_error::RemoteError;

/// Options of the `sync` command.
#[derive(Debug, Clone, Copy, Default)]
pub struct SyncOptions {
    /// Show what would be done; only the remote-tracking refs are updated (without pruning).
    pub dry_run: bool,
    /// What to do when a required server is unreachable.
    pub on_unreachable: Option<OnUnreachable>,
}

/// A change made (or planned, in a dry run) to a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    CreatedBranch { branch: String },
    Switched { from: Option<String>, to: String },
    FastForwarded { branch: String, commits: usize },
    Deleted { branch: String, reason: String },
}

/// Something the user should look at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Alert {
    UncommittedChanges {
        files: Vec<String>,
    },
    OperationInProgress(String),
    Stashes(usize),
    /// The development branch is not configured: run `detect`.
    NoDevBranch,
    /// The configured development branch no longer exists on the remote.
    DevBranchGone(String),
    SwitchFailed {
        to: String,
        error: String,
    },
    /// The branch is behind its remote but has uncommitted changes.
    NotUpdated {
        branch: String,
        commits: usize,
    },
    Unpushed {
        branch: String,
        commits: usize,
    },
    Diverged {
        branch: String,
        ahead: usize,
        behind: usize,
    },
    /// The remote branch is gone but the local commits cannot be found on the remote.
    GoneWithLocalCommits {
        branch: String,
        commits: usize,
    },
    /// The remote branch is gone but the branch is checked out and cannot be left.
    GoneButCheckedOut {
        branch: String,
    },
    CheckedOutElsewhere {
        branch: String,
    },
    GitError(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Synced,
    Skipped(String),
    Failed(RemoteError),
}

#[derive(Debug, Clone)]
pub struct RepoSync {
    pub display: String,
    pub status: Status,
    pub actions: Vec<Action>,
    pub alerts: Vec<Alert>,
}

#[derive(Debug)]
pub struct SyncReport {
    pub check: CheckReport,
    /// Required servers that are unreachable when the run must abort.
    pub aborted: Vec<Endpoint>,
    pub repos: Vec<RepoSync>,
    pub dry_run: bool,
}

/// Runs the synchronisation on the given folders.
pub fn run(
    roots: &[std::path::PathBuf],
    config: &Config,
    config_path: Option<std::path::PathBuf>,
    options: SyncOptions,
) -> Result<SyncReport> {
    let check = check::run(roots, config, config_path)?;
    let mut report = SyncReport {
        aborted: Vec::new(),
        repos: Vec::new(),
        dry_run: options.dry_run,
        check,
    };
    let policy = options.on_unreachable.unwrap_or(config.on_unreachable);
    if policy == OnUnreachable::Abort {
        report.aborted = report
            .check
            .unreachable_required()
            .into_iter()
            .map(|host| host.endpoint.clone())
            .collect();
        if !report.aborted.is_empty() {
            return Ok(report);
        }
    }
    let unreachable: BTreeSet<Endpoint> = report
        .check
        .hosts
        .iter()
        .filter(|host| !host.reachability.is_reachable())
        .map(|host| host.endpoint.clone())
        .collect();
    report.repos = parallel::map(&report.check.repos, config.parallel, |repo| {
        sync_one(repo, config, &unreachable, options.dry_run)
    });
    Ok(report)
}

fn sync_one(
    repo: &RepoInfo,
    config: &Config,
    unreachable: &BTreeSet<Endpoint>,
    dry_run: bool,
) -> RepoSync {
    let mut result = RepoSync {
        display: repo.display.clone(),
        status: Status::Synced,
        actions: Vec::new(),
        alerts: Vec::new(),
    };
    if let Some(error) = &repo.error {
        result.status = Status::Skipped(error.clone());
        return result;
    }
    let Some((remote, _)) = &repo.remote else {
        result.status = Status::Skipped("no remote".to_string());
        return result;
    };
    if let Some(endpoint) = &repo.endpoint {
        if unreachable.contains(endpoint) {
            result.status = Status::Skipped(format!("server {endpoint} unreachable"));
            return result;
        }
    }
    let mut sync = RepoSyncer {
        path: &repo.path,
        remote,
        dev_branch: config.dev_branch(&repo.path).map(str::to_string),
        dry_run,
        result,
    };
    if let Err(error) = sync.run() {
        sync.result
            .alerts
            .push(Alert::GitError(format!("{error:#}")));
    }
    sync.result
}

struct RepoSyncer<'a> {
    path: &'a Path,
    remote: &'a str,
    dev_branch: Option<String>,
    dry_run: bool,
    result: RepoSync,
}

impl RepoSyncer<'_> {
    fn run(&mut self) -> Result<()> {
        // 1. What the remote publishes, and where the remote branches were before the prune:
        //    a local branch equal to its remote branch before the deletion was fully pushed.
        let heads: BTreeSet<String> = match git::remote_heads(self.path, self.remote)? {
            Ok(heads) => heads.into_iter().collect(),
            Err(stderr) => {
                self.result.status = Status::Failed(RemoteError::classify(&stderr));
                return Ok(());
            }
        };
        if heads.is_empty() {
            // Nothing was ever pushed: the local branches are not "deleted on the remote".
            self.result.status = Status::Skipped("the remote has no branches".to_string());
            return Ok(());
        }
        let before_prune = git::remote_tracking_refs(self.path, self.remote)?;
        let prune = if self.dry_run {
            "--no-prune"
        } else {
            "--prune"
        };
        let fetch = git::run(
            Some(self.path),
            &["fetch", "--quiet", "--force", prune, self.remote],
        )?;
        if !fetch.success {
            self.result.status = Status::Failed(RemoteError::classify(&fetch.stderr));
            return Ok(());
        }

        // 2. State of the working tree.
        let (modified, _untracked) = git::working_tree_changes(self.path)?;
        let operation = git::operation_in_progress(self.path)?;
        if !modified.is_empty() {
            self.result.alerts.push(Alert::UncommittedChanges {
                files: modified.clone(),
            });
        }
        if let Some(operation) = operation {
            self.result
                .alerts
                .push(Alert::OperationInProgress(operation.to_string()));
        }
        let stashes = git::stash_count(self.path)?;
        if stashes > 0 {
            self.result.alerts.push(Alert::Stashes(stashes));
        }
        let clean = modified.is_empty() && operation.is_none();
        let mut current = git::current_branch(self.path)?;
        let branches = git::local_branches(self.path)?;
        let elsewhere = git::branches_in_other_worktrees(self.path)?;

        // 3. Development branch: switch to it when the working tree allows it.
        let dev = match &self.dev_branch {
            None => {
                self.result.alerts.push(Alert::NoDevBranch);
                None
            }
            Some(dev) if !heads.contains(dev) => {
                self.result.alerts.push(Alert::DevBranchGone(dev.clone()));
                None
            }
            Some(dev) => Some(dev.clone()),
        };
        if let Some(dev) = &dev {
            if clean
                && current.as_deref() != Some(dev.as_str())
                && !elsewhere.contains(dev)
                && self.switch_to(dev, &branches, current.clone())?
            {
                current = Some(dev.clone());
            }
        }
        let dev_ref = dev
            .as_ref()
            .map(|dev| format!("refs/remotes/{}/{dev}", self.remote));

        // 4. Every local branch that follows a branch of this remote.
        let prefix = format!("refs/remotes/{}/", self.remote);
        for branch in &branches {
            let Some(remote_branch) = branch
                .upstream
                .as_deref()
                .and_then(|upstream| upstream.strip_prefix(&prefix))
            else {
                continue;
            };
            if elsewhere.contains(&branch.name) {
                self.result.alerts.push(Alert::CheckedOutElsewhere {
                    branch: branch.name.clone(),
                });
                continue;
            }
            let is_current = current.as_deref() == Some(branch.name.as_str());
            if heads.contains(remote_branch) {
                self.update(branch, remote_branch, is_current, clean)?;
            } else if is_current {
                self.result.alerts.push(Alert::GoneButCheckedOut {
                    branch: branch.name.clone(),
                });
            } else {
                self.delete_if_safe(branch, before_prune.get(remote_branch), dev_ref.as_deref())?;
            }
        }
        Ok(())
    }

    /// Switches to the development branch, creating it from the remote when missing.
    fn switch_to(
        &mut self,
        dev: &str,
        branches: &[git::LocalBranch],
        from: Option<String>,
    ) -> Result<bool> {
        let exists = branches.iter().any(|b| b.name == dev);
        if !exists {
            let start = format!("{}/{dev}", self.remote);
            if !self.dry_run {
                let output = git::run(Some(self.path), &["branch", "--track", dev, &start])?;
                if !output.success {
                    self.result.alerts.push(Alert::SwitchFailed {
                        to: dev.to_string(),
                        error: output.stderr,
                    });
                    return Ok(false);
                }
            }
            self.result.actions.push(Action::CreatedBranch {
                branch: dev.to_string(),
            });
        }
        if !self.dry_run {
            let output = git::run(Some(self.path), &["switch", "--quiet", dev])?;
            if !output.success {
                self.result.alerts.push(Alert::SwitchFailed {
                    to: dev.to_string(),
                    error: output.stderr,
                });
                return Ok(false);
            }
        }
        self.result.actions.push(Action::Switched {
            from,
            to: dev.to_string(),
        });
        Ok(true)
    }

    /// Fast-forwards a branch to its remote branch, or reports why it cannot.
    fn update(
        &mut self,
        branch: &git::LocalBranch,
        remote_branch: &str,
        is_current: bool,
        clean: bool,
    ) -> Result<()> {
        let remote_ref = format!("refs/remotes/{}/{remote_branch}", self.remote);
        let Some(remote_sha) = git::resolve(self.path, &remote_ref)? else {
            return Ok(());
        };
        if remote_sha == branch.sha {
            return Ok(());
        }
        if git::is_ancestor(self.path, &branch.sha, &remote_sha)? {
            let commits =
                git::count_commits(self.path, &[&format!("{}..{remote_sha}", branch.sha)])?;
            if is_current && !clean {
                self.result.alerts.push(Alert::NotUpdated {
                    branch: branch.name.clone(),
                    commits,
                });
                return Ok(());
            }
            if !self.dry_run {
                let output = if is_current {
                    git::run(
                        Some(self.path),
                        &["merge", "--ff-only", "--quiet", &remote_ref],
                    )?
                } else {
                    // The old value guards against a concurrent change of the branch.
                    git::run(
                        Some(self.path),
                        &[
                            "update-ref",
                            &format!("refs/heads/{}", branch.name),
                            &remote_sha,
                            &branch.sha,
                        ],
                    )?
                };
                if !output.success {
                    self.result.alerts.push(Alert::GitError(format!(
                        "cannot update {}: {}",
                        branch.name, output.stderr
                    )));
                    return Ok(());
                }
            }
            self.result.actions.push(Action::FastForwarded {
                branch: branch.name.clone(),
                commits,
            });
        } else if git::is_ancestor(self.path, &remote_sha, &branch.sha)? {
            let commits =
                git::count_commits(self.path, &[&format!("{remote_sha}..{}", branch.sha)])?;
            self.result.alerts.push(Alert::Unpushed {
                branch: branch.name.clone(),
                commits,
            });
        } else {
            let ahead = git::count_commits(self.path, &[&format!("{remote_sha}..{}", branch.sha)])?;
            let behind =
                git::count_commits(self.path, &[&format!("{}..{remote_sha}", branch.sha)])?;
            self.result.alerts.push(Alert::Diverged {
                branch: branch.name.clone(),
                ahead,
                behind,
            });
        }
        Ok(())
    }

    /// Deletes a branch whose remote branch is gone, when its work cannot be lost.
    fn delete_if_safe(
        &mut self,
        branch: &git::LocalBranch,
        sha_before_prune: Option<&String>,
        dev_ref: Option<&str>,
    ) -> Result<()> {
        let reason = if sha_before_prune == Some(&branch.sha) {
            Some("pushed, then deleted on the remote".to_string())
        } else if git::contained_in_remote(self.path, self.remote, &branch.sha)? {
            Some(format!("its commits are on {}", self.remote))
        } else {
            dev_ref
                .filter(|dev_ref| {
                    git::changes_already_in(self.path, &branch.sha, dev_ref).unwrap_or(false)
                })
                .map(|dev_ref| {
                    format!(
                        "its changes are already in {} (squash merge)",
                        dev_ref.trim_start_matches("refs/remotes/")
                    )
                })
        };
        let Some(reason) = reason else {
            let commits = git::count_commits(
                self.path,
                &[&branch.sha, "--not", &format!("--remotes={}", self.remote)],
            )?;
            self.result.alerts.push(Alert::GoneWithLocalCommits {
                branch: branch.name.clone(),
                commits,
            });
            return Ok(());
        };
        if !self.dry_run {
            let output = git::run(Some(self.path), &["branch", "-D", "--quiet", &branch.name])?;
            if !output.success {
                self.result.alerts.push(Alert::GitError(format!(
                    "cannot delete {}: {}",
                    branch.name, output.stderr
                )));
                return Ok(());
            }
        }
        self.result.actions.push(Action::Deleted {
            branch: branch.name.clone(),
            reason,
        });
        Ok(())
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::CreatedBranch { branch } => write!(f, "created {branch} from the remote"),
            Action::Switched { from, to } => match from {
                Some(from) => write!(f, "switched {from} -> {to}"),
                None => write!(f, "switched to {to}"),
            },
            Action::FastForwarded { branch, commits } => {
                write!(f, "updated {branch} (+{commits} commits)")
            }
            Action::Deleted { branch, reason } => write!(f, "deleted {branch}: {reason}"),
        }
    }
}

impl fmt::Display for Alert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Alert::UncommittedChanges { files } => {
                let shown: Vec<&str> = files.iter().take(5).map(String::as_str).collect();
                let more = files.len().saturating_sub(shown.len());
                write!(f, "uncommitted changes ({} files): {}", files.len(), shown.join(", "))?;
                if more > 0 {
                    write!(f, " and {more} more")?;
                }
                Ok(())
            }
            Alert::OperationInProgress(operation) => write!(f, "{operation} in progress"),
            Alert::Stashes(count) => write!(f, "{count} stash entries"),
            Alert::NoDevBranch => write!(f, "no development branch configured: run detect"),
            Alert::DevBranchGone(branch) => write!(
                f,
                "development branch {branch} is no longer on the remote: run detect --force"
            ),
            Alert::SwitchFailed { to, error } => write!(f, "cannot switch to {to}: {error}"),
            Alert::NotUpdated { branch, commits } => write!(
                f,
                "{branch} not updated (+{commits} commits on the remote) because of uncommitted changes"
            ),
            Alert::Unpushed { branch, commits } => {
                write!(f, "{branch} has {commits} commits not pushed")
            }
            Alert::Diverged {
                branch,
                ahead,
                behind,
            } => write!(
                f,
                "{branch} diverged from the remote ({ahead} local, {behind} remote commits)"
            ),
            Alert::GoneWithLocalCommits { branch, commits } => write!(
                f,
                "{branch}: remote branch deleted, kept because {commits} commits are not on the remote"
            ),
            Alert::GoneButCheckedOut { branch } => write!(
                f,
                "{branch}: remote branch deleted, kept because it is checked out"
            ),
            Alert::CheckedOutElsewhere { branch } => {
                write!(f, "{branch} is checked out in another worktree: not touched")
            }
            Alert::GitError(error) => write!(f, "git error: {error}"),
        }
    }
}

impl fmt::Display for SyncReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.aborted.is_empty() {
            let servers: Vec<String> = self.aborted.iter().map(Endpoint::to_string).collect();
            return writeln!(
                f,
                "Required server unreachable: {}. Nothing was done (is the VPN connected?).",
                servers.join(", ")
            );
        }
        let title = if self.dry_run {
            "Dry run: changes that would be made"
        } else {
            "Changes"
        };

        let mut by_status: BTreeMap<&str, Vec<&RepoSync>> = BTreeMap::new();
        for repo in &self.repos {
            let key = match &repo.status {
                Status::Synced if repo.actions.is_empty() && repo.alerts.is_empty() => "up to date",
                Status::Synced => "changed",
                Status::Skipped(_) => "skipped",
                Status::Failed(_) => "failed",
            };
            by_status.entry(key).or_default().push(repo);
        }

        let changed = by_status.get("changed").cloned().unwrap_or_default();
        writeln!(f, "{title} ({} repositories)", changed.len())?;
        for repo in &changed {
            writeln!(f, "  {}", repo.display)?;
            for action in &repo.actions {
                writeln!(f, "      {action}")?;
            }
            for alert in &repo.alerts {
                writeln!(f, "    ! {alert}")?;
            }
        }
        if let Some(failed) = by_status.get("failed") {
            writeln!(f, "\nRemote errors ({})", failed.len())?;
            for repo in failed {
                if let Status::Failed(error) = &repo.status {
                    writeln!(f, "  {}: {error}", repo.display)?;
                }
            }
        }
        if let Some(skipped) = by_status.get("skipped") {
            writeln!(f, "\nSkipped ({})", skipped.len())?;
            for repo in skipped {
                if let Status::Skipped(reason) = &repo.status {
                    writeln!(f, "  {}: {reason}", repo.display)?;
                }
            }
        }

        let count = |predicate: fn(&Action) -> bool| -> usize {
            self.repos
                .iter()
                .flat_map(|r| &r.actions)
                .filter(|a| predicate(a))
                .count()
        };
        let alerts: usize = self.repos.iter().map(|r| r.alerts.len()).sum();
        writeln!(
            f,
            "\nSummary: {} repositories ({} up to date), {} branches updated, {} deleted, {} switched, {} alerts, {} errors, {} skipped",
            self.repos.len(),
            by_status.get("up to date").map_or(0, Vec::len),
            count(|a| matches!(a, Action::FastForwarded { .. })),
            count(|a| matches!(a, Action::Deleted { .. })),
            count(|a| matches!(a, Action::Switched { .. })),
            alerts,
            by_status.get("failed").map_or(0, Vec::len),
            by_status.get("skipped").map_or(0, Vec::len),
        )?;
        if self.dry_run {
            writeln!(f, "Dry run: no branch was changed.")?;
        }
        Ok(())
    }
}
