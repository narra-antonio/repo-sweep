//! Integration tests of the `sync` command on real git repositories with a local bare remote.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use repo_sweep::config::{self, Config, RepoConfig};
use repo_sweep::sync::{self, Action, Alert, RepoSync, SyncOptions};
use tempfile::TempDir;

fn git_output(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "init.defaultBranch=master",
            "-c",
            "advice.detachedHead=false",
        ])
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn git(dir: &Path, args: &[&str]) {
    git_output(dir, args);
}

fn commit(dir: &Path, file: &str, message: &str) {
    fs::write(dir.join(file), message).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", message]);
}

fn sha(dir: &Path, revision: &str) -> String {
    git_output(dir, &["rev-parse", revision])
}

fn ref_exists(dir: &Path, reference: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--verify", "--quiet", reference])
        .output()
        .unwrap()
        .status
        .success()
}

/// A bare "server", the clone under test (`work/repo`) and a second clone (`other`) used to
/// change the server as a colleague would.
struct Fixture {
    _root: TempDir,
    work: PathBuf,
    clone: PathBuf,
    other: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let seed = root.path().join("seed");
        fs::create_dir_all(&seed).unwrap();
        git(&seed, &["init", "-q"]);
        commit(&seed, "README.md", "initial");
        git(&seed, &["branch", "develop"]);

        let bare = root.path().join("server.git");
        git(
            root.path(),
            &[
                "clone",
                "-q",
                "--bare",
                seed.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        let work = root.path().join("work");
        let clone = work.join("repo");
        let other = root.path().join("other");
        for target in [&clone, &other] {
            git(
                root.path(),
                &[
                    "clone",
                    "-q",
                    bare.to_str().unwrap(),
                    target.to_str().unwrap(),
                ],
            );
        }
        git(&clone, &["switch", "-q", "develop"]);
        git(&other, &["switch", "-q", "develop"]);
        Self {
            _root: root,
            work,
            clone,
            other,
        }
    }

    fn config(&self) -> Config {
        let mut config = Config::default();
        config.repos.insert(
            config::repo_key(&self.clone),
            RepoConfig {
                dev_branch: Some("develop".to_string()),
            },
        );
        config
    }

    /// Creates a branch in the clone from develop, commits on it and pushes it.
    fn pushed_branch(&self, name: &str) {
        git(&self.clone, &["switch", "-q", "-c", name, "develop"]);
        commit(&self.clone, &format!("{name}.txt"), name);
        git(&self.clone, &["push", "-q", "-u", "origin", name]);
    }

    /// A colleague adds a commit to a branch on the server.
    fn remote_commit(&self, branch: &str, file: &str) {
        git(&self.other, &["fetch", "-q"]);
        git(&self.other, &["switch", "-q", branch]);
        git(&self.other, &["pull", "-q", "--ff-only"]);
        commit(&self.other, file, file);
        git(&self.other, &["push", "-q", "origin", branch]);
    }

    fn delete_remote_branch(&self, branch: &str) {
        git(&self.other, &["push", "-q", "origin", "--delete", branch]);
    }

    /// Deletes a branch directly in the bare repository, even the one its HEAD points to.
    fn delete_remote_branch_forced(&self, branch: &str) {
        let bare = self._root.path().join("server.git");
        git(
            &bare,
            &["update-ref", "-d", &format!("refs/heads/{branch}")],
        );
    }

    fn sync(&self, dry_run: bool) -> RepoSync {
        let report = sync::run(
            std::slice::from_ref(&self.work),
            &self.config(),
            None,
            SyncOptions {
                dry_run,
                on_unreachable: None,
            },
        )
        .unwrap();
        assert_eq!(report.repos.len(), 1);
        report.repos.into_iter().next().unwrap()
    }
}

fn deleted(result: &RepoSync, branch: &str) -> Option<String> {
    result.actions.iter().find_map(|action| match action {
        Action::Deleted {
            branch: b, reason, ..
        } if b == branch => Some(reason.clone()),
        _ => None,
    })
}

#[test]
fn updates_branches_switches_back_and_deletes_merged_branches() {
    let fx = Fixture::new();
    fx.pushed_branch("CREW-1");
    fx.remote_commit("develop", "dev.txt");
    fx.remote_commit("master", "master.txt");
    fx.delete_remote_branch("CREW-1");

    let result = fx.sync(false);

    assert!(result.alerts.is_empty(), "{:?}", result.alerts);
    assert!(result.actions.contains(&Action::Switched {
        from: Some("CREW-1".to_string()),
        to: "develop".to_string()
    }));
    assert!(result.actions.contains(&Action::FastForwarded {
        branch: "develop".to_string(),
        commits: 1
    }));
    assert!(result.actions.contains(&Action::FastForwarded {
        branch: "master".to_string(),
        commits: 1
    }));
    assert!(deleted(&result, "CREW-1").unwrap().contains("pushed"));
    assert_eq!(
        git_output(&fx.clone, &["branch", "--show-current"]),
        "develop"
    );
    assert_eq!(sha(&fx.clone, "develop"), sha(&fx.clone, "origin/develop"));
    assert_eq!(sha(&fx.clone, "master"), sha(&fx.clone, "origin/master"));
    assert!(!ref_exists(&fx.clone, "refs/heads/CREW-1"));
}

#[test]
fn deletes_squash_merged_branches_after_an_earlier_prune() {
    let fx = Fixture::new();
    fx.pushed_branch("CREW-2");
    git(&fx.clone, &["switch", "-q", "develop"]);
    // A colleague squash-merges the branch into develop and deletes it.
    git(&fx.other, &["fetch", "-q"]);
    git(&fx.other, &["merge", "-q", "--squash", "origin/CREW-2"]);
    git(&fx.other, &["commit", "-q", "-m", "CREW-2 squashed"]);
    git(&fx.other, &["push", "-q", "origin", "develop"]);
    fx.delete_remote_branch("CREW-2");
    // The remote-tracking ref was already pruned by a plain `git fetch --prune`.
    git(&fx.clone, &["fetch", "-q", "--prune"]);

    let result = fx.sync(false);

    assert!(
        deleted(&result, "CREW-2").unwrap().contains("squash"),
        "{:?}",
        result.actions
    );
    assert!(!ref_exists(&fx.clone, "refs/heads/CREW-2"));
}

#[test]
fn keeps_deleted_branches_with_unpushed_commits() {
    let fx = Fixture::new();
    fx.pushed_branch("CREW-3");
    commit(&fx.clone, "local.txt", "not pushed");
    git(&fx.clone, &["switch", "-q", "develop"]);
    fx.delete_remote_branch("CREW-3");

    let result = fx.sync(false);

    assert!(
        result.alerts.contains(&Alert::GoneWithLocalCommits {
            branch: "CREW-3".to_string(),
            // The pushed commit is gone from the remote too, together with the branch.
            commits: 2
        }),
        "{result:?}"
    );
    assert!(ref_exists(&fx.clone, "refs/heads/CREW-3"));
}

#[test]
fn never_touches_a_dirty_working_tree() {
    let fx = Fixture::new();
    fx.pushed_branch("CREW-4");
    fx.delete_remote_branch("CREW-4");
    fx.remote_commit("master", "master.txt");
    fs::write(fx.clone.join("CREW-4.txt"), "work in progress").unwrap();
    let head_before = sha(&fx.clone, "HEAD");

    let result = fx.sync(false);

    assert!(result
        .alerts
        .iter()
        .any(|alert| matches!(alert, Alert::UncommittedChanges { files } if files == &vec!["CREW-4.txt".to_string()])));
    assert!(result.alerts.contains(&Alert::GoneButCheckedOut {
        branch: "CREW-4".to_string()
    }));
    assert!(!result
        .actions
        .iter()
        .any(|a| matches!(a, Action::Switched { .. })));
    // The checked-out branch and its files are untouched, other branches are still updated.
    assert_eq!(
        git_output(&fx.clone, &["branch", "--show-current"]),
        "CREW-4"
    );
    assert_eq!(sha(&fx.clone, "HEAD"), head_before);
    assert_eq!(
        fs::read_to_string(fx.clone.join("CREW-4.txt")).unwrap(),
        "work in progress"
    );
    assert_eq!(sha(&fx.clone, "master"), sha(&fx.clone, "origin/master"));
}

#[test]
fn does_not_update_the_checked_out_branch_with_uncommitted_changes() {
    let fx = Fixture::new();
    fx.remote_commit("develop", "dev.txt");
    fs::write(fx.clone.join("README.md"), "edited").unwrap();
    let before = sha(&fx.clone, "develop");

    let result = fx.sync(false);

    assert!(result.alerts.contains(&Alert::NotUpdated {
        branch: "develop".to_string(),
        commits: 1
    }));
    assert_eq!(sha(&fx.clone, "develop"), before);
}

#[test]
fn reports_unpushed_and_diverged_branches() {
    let fx = Fixture::new();
    fx.pushed_branch("ahead");
    commit(&fx.clone, "ahead-2.txt", "local only");
    fx.pushed_branch("diverged");
    commit(&fx.clone, "mine.txt", "mine");
    git(&fx.clone, &["switch", "-q", "develop"]);
    fx.remote_commit("diverged", "theirs.txt");

    let result = fx.sync(false);

    assert!(result.alerts.contains(&Alert::Unpushed {
        branch: "ahead".to_string(),
        commits: 1
    }));
    assert!(result.alerts.contains(&Alert::Diverged {
        branch: "diverged".to_string(),
        ahead: 1,
        behind: 1
    }));
}

#[test]
fn creates_the_development_branch_when_missing() {
    let fx = Fixture::new();
    git(&fx.clone, &["switch", "-q", "master"]);
    git(&fx.clone, &["branch", "-q", "-D", "develop"]);

    let result = fx.sync(false);

    assert!(result.actions.contains(&Action::CreatedBranch {
        branch: "develop".to_string()
    }));
    assert_eq!(
        git_output(&fx.clone, &["branch", "--show-current"]),
        "develop"
    );
}

#[test]
fn dry_run_changes_no_branch() {
    let fx = Fixture::new();
    fx.pushed_branch("CREW-5");
    fx.remote_commit("develop", "dev.txt");
    fx.delete_remote_branch("CREW-5");
    let develop_before = sha(&fx.clone, "develop");

    let result = fx.sync(true);

    assert!(result.actions.contains(&Action::Switched {
        from: Some("CREW-5".to_string()),
        to: "develop".to_string()
    }));
    assert!(result.actions.contains(&Action::FastForwarded {
        branch: "develop".to_string(),
        commits: 1
    }));
    assert!(deleted(&result, "CREW-5").is_some());
    assert_eq!(
        git_output(&fx.clone, &["branch", "--show-current"]),
        "CREW-5"
    );
    assert_eq!(sha(&fx.clone, "develop"), develop_before);
    assert!(ref_exists(&fx.clone, "refs/heads/CREW-5"));
    // No prune in a dry run: the real run can still prove the branch was pushed.
    assert!(ref_exists(&fx.clone, "refs/remotes/origin/CREW-5"));
}

#[test]
fn reports_a_deleted_remote_repository() {
    let fx = Fixture::new();
    fs::remove_dir_all(fx._root.path().join("server.git")).unwrap();

    let result = fx.sync(false);

    assert!(
        matches!(result.status, sync::Status::Failed(_)),
        "{:?}",
        result.status
    );
}

#[test]
fn skips_repositories_whose_remote_has_no_branches() {
    let fx = Fixture::new();
    for branch in ["master", "develop"] {
        fx.delete_remote_branch_forced(branch);
    }

    let result = fx.sync(false);

    assert_eq!(
        result.status,
        sync::Status::Skipped("the remote has no branches".to_string())
    );
    assert!(ref_exists(&fx.clone, "refs/heads/develop"));
}

#[test]
fn deletes_squash_merged_branches_changed_again_by_others() {
    let fx = Fixture::new();
    fx.pushed_branch("CREW-6");
    git(&fx.clone, &["switch", "-q", "develop"]);
    // The MR is squashed after the branch was brought up to date: the squash commit has the same
    // content as the branch. Then a colleague changes the same file again.
    git(&fx.other, &["fetch", "-q"]);
    git(&fx.other, &["merge", "-q", "--squash", "origin/CREW-6"]);
    git(&fx.other, &["commit", "-q", "-m", "CREW-6 squashed"]);
    fs::write(fx.other.join("CREW-6.txt"), "changed again by a colleague").unwrap();
    git(&fx.other, &["commit", "-q", "-am", "follow-up"]);
    git(&fx.other, &["push", "-q", "origin", "develop"]);
    fx.delete_remote_branch("CREW-6");
    git(&fx.clone, &["fetch", "-q", "--prune"]);

    let result = fx.sync(false);

    let reason = deleted(&result, "CREW-6").unwrap_or_else(|| panic!("{result:?}"));
    assert!(reason.contains("same content"), "{reason}");
}

#[test]
fn stays_on_a_branch_with_commits_not_on_the_remote() {
    let fx = Fixture::new();
    fx.pushed_branch("CREW-7");
    commit(&fx.clone, "wip.txt", "work in progress");

    let result = fx.sync(false);

    assert!(!result
        .actions
        .iter()
        .any(|a| matches!(a, Action::Switched { .. })));
    assert!(result.alerts.contains(&Alert::StayedOnBranch {
        branch: "CREW-7".to_string(),
        dev: "develop".to_string(),
        commits: 1
    }));
    assert_eq!(
        git_output(&fx.clone, &["branch", "--show-current"]),
        "CREW-7"
    );
}

#[test]
fn stays_on_a_local_branch_never_pushed() {
    let fx = Fixture::new();
    git(&fx.clone, &["switch", "-q", "-c", "spike", "develop"]);
    commit(&fx.clone, "spike.txt", "spike");

    let result = fx.sync(false);

    assert!(result.alerts.iter().any(|a| matches!(
        a,
        Alert::StayedOnBranch { branch, .. } if branch == "spike"
    )));
    assert_eq!(
        git_output(&fx.clone, &["branch", "--show-current"]),
        "spike"
    );
}

#[test]
fn deleted_branches_report_the_commit_they_pointed_to() {
    let fx = Fixture::new();
    fx.pushed_branch("CREW-8");
    let tip = sha(&fx.clone, "CREW-8");
    fx.delete_remote_branch("CREW-8");

    let result = fx.sync(false);

    let sha = result.actions.iter().find_map(|action| match action {
        Action::Deleted { branch, sha, .. } if branch == "CREW-8" => Some(sha.clone()),
        _ => None,
    });
    assert_eq!(sha.as_deref(), Some(&tip[..10]));
    assert!(!ref_exists(&fx.clone, "refs/heads/CREW-8"));
}
