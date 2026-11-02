//! Integration tests of the `detect` command on real git repositories with local remotes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use repo_sweep::config::{self, Config, LOCAL_FILE_NAME};
use repo_sweep::detect::{self, DetectOptions, Outcome};
use repo_sweep::remote_error::RemoteErrorKind;
use tempfile::TempDir;

fn git_at(dir: &Path, date: Option<&str>, args: &[&str]) {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "init.defaultBranch=master",
        ])
        .args(args);
    if let Some(date) = date {
        command
            .env("GIT_COMMITTER_DATE", date)
            .env("GIT_AUTHOR_DATE", date);
    }
    let status = command.status().expect("git should run");
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

fn git(dir: &Path, args: &[&str]) {
    git_at(dir, None, args);
}

/// Creates a bare "server" repository with the given branches, each with a commit at the given
/// date, and a clone of it.
fn server_and_clone(root: &Path, name: &str, branches: &[(&str, &str)]) -> PathBuf {
    let seed = root.join(format!("seed-{name}"));
    fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q"]);
    fs::write(seed.join("README.md"), "seed").unwrap();
    git(&seed, &["add", "README.md"]);
    git_at(
        &seed,
        Some("2020-01-01T00:00:00Z"),
        &["commit", "-q", "-m", "initial"],
    );
    for (branch, date) in branches {
        if *branch != "master" {
            git(&seed, &["checkout", "-q", "-b", branch, "master"]);
        } else {
            git(&seed, &["checkout", "-q", "master"]);
        }
        fs::write(seed.join(format!("{branch}.txt")), branch).unwrap();
        git(&seed, &["add", "."]);
        git_at(&seed, Some(date), &["commit", "-q", "-m", branch]);
    }

    let bare = root.join(format!("server/{name}.git"));
    git(
        root,
        &[
            "clone",
            "-q",
            "--bare",
            seed.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let clone = root.join(format!("work/{name}"));
    git(
        root,
        &[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    clone
}

fn outcome<'a>(report: &'a detect::DetectReport, clone: &Path) -> &'a Outcome {
    let key = config::repo_key(clone);
    &report
        .detections
        .iter()
        .find(|d| d.key == key)
        .unwrap_or_else(|| panic!("no detection for {key}"))
        .outcome
}

#[test]
fn detects_and_saves_development_branches() {
    let root = TempDir::new().unwrap();
    let both = server_and_clone(
        root.path(),
        "both",
        &[
            ("develop", "2021-05-01T00:00:00Z"),
            ("feature", "2026-10-30T00:00:00Z"),
        ],
    );
    let master_only = server_and_clone(
        root.path(),
        "master-only",
        &[("master", "2026-01-01T00:00:00Z")],
    );
    let work = root.path().join("work");
    let config_file = work.join(LOCAL_FILE_NAME);

    let report = detect::run(
        std::slice::from_ref(&work),
        &Config::default(),
        Some(config_file.clone()),
        DetectOptions::default(),
    )
    .unwrap();

    match outcome(&report, &both) {
        Outcome::Detected {
            branch,
            reason,
            changed,
        } => {
            assert_eq!(branch, "feature");
            assert!(reason.contains("develop 2021-05-01"), "{reason}");
            assert!(changed);
        }
        other => panic!("unexpected {other:?}"),
    }
    match outcome(&report, &master_only) {
        Outcome::Detected { branch, .. } => assert_eq!(branch, "master"),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(report.saved_to.as_deref(), Some(config_file.as_path()));

    // Second run: the saved branches are kept and nothing changes.
    let (config, _) = Config::load(Some(&config_file), None).unwrap();
    assert_eq!(config.dev_branch(&both), Some("feature"));
    let again = detect::run(
        &[work],
        &config,
        Some(config_file),
        DetectOptions::default(),
    )
    .unwrap();
    assert!(matches!(
        outcome(&again, &both),
        Outcome::Detected { changed: false, .. }
    ));
    assert!(again.saved_to.is_none());
}

#[test]
fn dry_run_does_not_write_the_configuration() {
    let root = TempDir::new().unwrap();
    server_and_clone(root.path(), "repo", &[("develop", "2026-01-01T00:00:00Z")]);
    let work = root.path().join("work");
    let config_file = work.join(LOCAL_FILE_NAME);

    let report = detect::run(
        &[work],
        &Config::default(),
        Some(config_file.clone()),
        DetectOptions {
            force: false,
            dry_run: true,
        },
    )
    .unwrap();

    assert!(report.saved_to.is_none());
    assert!(!config_file.exists());
    assert!(report.to_string().contains("Dry run"));
}

#[test]
fn reports_a_deleted_remote_repository() {
    let root = TempDir::new().unwrap();
    let clone = server_and_clone(root.path(), "gone", &[("develop", "2026-01-01T00:00:00Z")]);
    fs::remove_dir_all(root.path().join("server/gone.git")).unwrap();

    let report = detect::run(
        &[root.path().join("work")],
        &Config::default(),
        None,
        DetectOptions {
            force: false,
            dry_run: true,
        },
    )
    .unwrap();

    match outcome(&report, &clone) {
        Outcome::Failed(error) => assert_eq!(error.kind, RemoteErrorKind::NotFound),
        other => panic!("unexpected {other:?}"),
    }
}
