//! Integration tests of the `check` command on real git repositories.

use std::fs;
use std::path::Path;
use std::process::Command;

use repo_sweep::check;
use repo_sweep::config::Config;
use repo_sweep::connectivity::Endpoint;
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .status()
        .expect("git should run");
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

fn init_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    fs::write(dir.join("README.md"), "test").unwrap();
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "-q", "-m", "initial"]);
}

#[test]
fn lists_repositories_with_branch_and_remote() {
    let root = TempDir::new().unwrap();
    let bare = root.path().join("server/project.git");
    fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare"]);

    let local = root.path().join("work/project");
    init_repo(&local);
    git(&local, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&local, &["checkout", "-q", "-b", "feature"]);

    let detached = root.path().join("work/detached");
    init_repo(&detached);
    git(&detached, &["checkout", "-q", "--detach"]);

    let work = vec![root.path().join("work")];
    let report = check::run(&work, &Config::default(), None).unwrap();

    assert_eq!(report.repos.len(), 2);
    let project = report.repos.iter().find(|r| r.path == local).unwrap();
    assert_eq!(project.branch.as_deref(), Some("feature"));
    assert_eq!(
        project.remote.as_ref().map(|(name, _)| name.as_str()),
        Some("origin")
    );
    assert!(project.endpoint.is_none(), "a local remote has no server");

    let detached_repo = report.repos.iter().find(|r| r.path == detached).unwrap();
    assert_eq!(detached_repo.branch, None);
    assert!(detached_repo.remote.is_none());

    assert!(report.hosts.is_empty());
    assert!(report.unreachable_required().is_empty());
    assert!(report.to_string().contains("Summary: 2 repositories"));
}

#[test]
fn reports_unreachable_required_host() {
    let root = TempDir::new().unwrap();
    let repo = root.path().join("repo");
    init_repo(&repo);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://git.host.invalid/group/repo.git",
        ],
    );

    let config = Config::parse(
        r#"
        required_hosts = ["git.host.invalid:443"]
        connect_timeout_secs = 1
        "#,
    )
    .unwrap();
    let report = check::run(&[root.path().to_path_buf()], &config, None).unwrap();

    let unreachable = report.unreachable_required();
    assert_eq!(unreachable.len(), 1);
    assert_eq!(
        unreachable[0].endpoint,
        Endpoint::new("git.host.invalid", 443)
    );
    assert_eq!(unreachable[0].repositories, 1);
    assert!(report.to_string().contains("FAIL"));
}

#[test]
fn fails_when_no_repository_is_found() {
    let root = TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("empty/folder")).unwrap();

    let error = check::run(&[root.path().to_path_buf()], &Config::default(), None).unwrap_err();

    assert!(error.to_string().contains("no git repository found"));
}
