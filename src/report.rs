//! Markdown report of a `sync` run, for runs with many repositories and for scheduled runs.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::sync::{Action, Alert, RepoSync, Status, SyncReport};

/// Renders the report as Markdown.
pub fn to_markdown(report: &SyncReport, roots: &[std::path::PathBuf]) -> String {
    let mut out = String::new();
    let title = if report.dry_run {
        "repo-sweep sync (dry run)"
    } else {
        "repo-sweep sync"
    };
    let _ = writeln!(out, "# {title} — {}\n", now_local());
    let folders: Vec<String> = roots.iter().map(|r| format!("`{}`", r.display())).collect();
    let _ = writeln!(out, "Folders: {}\n", folders.join(", "));

    if !report.aborted.is_empty() {
        let servers: Vec<String> = report.aborted.iter().map(|e| format!("`{e}`")).collect();
        let _ = writeln!(
            out,
            "**Required server unreachable: {}. Nothing was done.**",
            servers.join(", ")
        );
        return out;
    }

    let actions = |predicate: fn(&Action) -> bool| {
        report
            .repos
            .iter()
            .flat_map(|repo| repo.actions.iter().map(move |action| (repo, action)))
            .filter(move |(_, action)| predicate(action))
    };
    let up_to_date = report
        .repos
        .iter()
        .filter(|r| r.status == Status::Synced && r.actions.is_empty() && r.alerts.is_empty())
        .count();
    let failed: Vec<&RepoSync> = report
        .repos
        .iter()
        .filter(|r| matches!(r.status, Status::Failed(_)))
        .collect();
    let skipped: Vec<&RepoSync> = report
        .repos
        .iter()
        .filter(|r| matches!(r.status, Status::Skipped(_)))
        .collect();
    let with_alerts: Vec<&RepoSync> = report
        .repos
        .iter()
        .filter(|r| !r.alerts.is_empty())
        .collect();

    let _ = writeln!(out, "## Summary\n\n| | |\n|---|---:|");
    let rows = [
        ("Repositories", report.repos.len()),
        ("Up to date", up_to_date),
        (
            "Branches updated",
            actions(|a| matches!(a, Action::FastForwarded { .. })).count(),
        ),
        (
            "Branches deleted",
            actions(|a| matches!(a, Action::Deleted { .. })).count(),
        ),
        (
            "Switched to the development branch",
            actions(|a| matches!(a, Action::Switched { .. })).count(),
        ),
        ("Repositories needing attention", with_alerts.len()),
        ("Remote errors", failed.len()),
        ("Skipped", skipped.len()),
    ];
    for (label, value) in rows {
        let _ = writeln!(out, "| {label} | {value} |");
    }

    if !with_alerts.is_empty() {
        let _ = writeln!(
            out,
            "\n## Needs attention\n\n| Repository | Alert |\n|---|---|"
        );
        for repo in &with_alerts {
            for alert in &repo.alerts {
                let _ = writeln!(
                    out,
                    "| {} | {} |",
                    cell(&repo.display),
                    cell(&alert_text(alert))
                );
            }
        }
    }

    let switched: Vec<_> =
        actions(|a| matches!(a, Action::Switched { .. } | Action::CreatedBranch { .. })).collect();
    if !switched.is_empty() {
        let _ = writeln!(out, "\n## Switched\n\n| Repository | Change |\n|---|---|");
        for (repo, action) in switched {
            let _ = writeln!(
                out,
                "| {} | {} |",
                cell(&repo.display),
                cell(&action.to_string())
            );
        }
    }

    let deleted: Vec<_> = actions(|a| matches!(a, Action::Deleted { .. })).collect();
    if !deleted.is_empty() {
        let _ = writeln!(
            out,
            "\n## Deleted branches\n\n| Repository | Branch | Reason |\n|---|---|---|"
        );
        for (repo, action) in deleted {
            if let Action::Deleted { branch, reason } = action {
                let _ = writeln!(
                    out,
                    "| {} | `{}` | {} |",
                    cell(&repo.display),
                    cell(branch),
                    cell(reason)
                );
            }
        }
    }

    let updated: Vec<&RepoSync> = report
        .repos
        .iter()
        .filter(|r| {
            r.actions
                .iter()
                .any(|a| matches!(a, Action::FastForwarded { .. }))
        })
        .collect();
    if !updated.is_empty() {
        let _ = writeln!(
            out,
            "\n## Updated branches\n\n| Repository | Branches (new commits) |\n|---|---|"
        );
        for repo in updated {
            let branches: Vec<String> = repo
                .actions
                .iter()
                .filter_map(|a| match a {
                    Action::FastForwarded { branch, commits } => {
                        Some(format!("{branch} (+{commits})"))
                    }
                    _ => None,
                })
                .collect();
            let _ = writeln!(
                out,
                "| {} | {} |",
                cell(&repo.display),
                cell(&branches.join(", "))
            );
        }
    }

    if !failed.is_empty() {
        let _ = writeln!(
            out,
            "\n## Remote errors\n\n| Repository | Kind | Message |\n|---|---|---|"
        );
        for repo in failed {
            if let Status::Failed(error) = &repo.status {
                let _ = writeln!(
                    out,
                    "| {} | {} | {} |",
                    cell(&repo.display),
                    error.kind,
                    cell(&error.message)
                );
            }
        }
    }

    if !skipped.is_empty() {
        let _ = writeln!(out, "\n## Skipped\n\n| Repository | Reason |\n|---|---|");
        for repo in skipped {
            if let Status::Skipped(reason) = &repo.status {
                let _ = writeln!(out, "| {} | {} |", cell(&repo.display), cell(reason));
            }
        }
    }

    if report.dry_run {
        let _ = writeln!(out, "\n_Dry run: no branch was changed._");
    }
    out
}

/// Writes the Markdown report, creating the folder when needed.
pub fn write(path: &Path, report: &SyncReport, roots: &[std::path::PathBuf]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    fs::write(path, to_markdown(report, roots))
        .with_context(|| format!("cannot write {}", path.display()))
}

fn alert_text(alert: &Alert) -> String {
    match alert {
        // In a table every file would make the row unreadable: list them all, separated.
        Alert::UncommittedChanges { files } => {
            let quoted: Vec<String> = files.iter().map(|f| format!("`{f}`")).collect();
            format!(
                "uncommitted changes ({} files): {}",
                files.len(),
                quoted.join(", ")
            )
        }
        other => other.to_string(),
    }
}

/// Escapes the characters that break a Markdown table cell.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

fn now_local() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

/// File name prefix of the reports written by `write_to_dir`.
const REPORT_PREFIX: &str = "sync-";

/// Writes the report into `dir` with a time-stamped name and keeps only the `keep` most recent
/// reports. Returns the path of the new report.
pub fn write_to_dir(
    dir: &Path,
    report: &SyncReport,
    roots: &[std::path::PathBuf],
    keep: usize,
) -> Result<std::path::PathBuf> {
    let name = format!(
        "{REPORT_PREFIX}{}.md",
        chrono::Local::now().format("%Y-%m-%d_%H%M%S")
    );
    let path = dir.join(name);
    write(&path, report, roots)?;
    prune_reports(dir, keep.max(1))?;
    Ok(path)
}

/// Deletes the oldest reports, keeping the `keep` most recent ones. Only files written by
/// `write_to_dir` are considered.
pub fn prune_reports(dir: &Path, keep: usize) -> Result<()> {
    let mut reports: Vec<std::path::PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("cannot read {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(REPORT_PREFIX) && name.ends_with(".md"))
        })
        .collect();
    // The names sort chronologically.
    reports.sort();
    let excess = reports.len().saturating_sub(keep);
    for old in &reports[..excess] {
        fs::remove_file(old).with_context(|| format!("cannot delete {}", old.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_error::RemoteError;

    #[test]
    fn keeps_only_the_most_recent_reports() {
        let dir = tempfile::TempDir::new().unwrap();
        for day in 1..=5 {
            fs::write(
                dir.path().join(format!("sync-2026-11-0{day}_020000.md")),
                "x",
            )
            .unwrap();
        }
        fs::write(dir.path().join("notes.md"), "not a report").unwrap();

        prune_reports(dir.path(), 2).unwrap();

        let mut names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "notes.md",
                "sync-2026-11-04_020000.md",
                "sync-2026-11-05_020000.md"
            ]
        );
    }

    #[test]
    fn escapes_table_cells() {
        assert_eq!(cell("a | b\nc"), "a \\| b c");
    }

    #[test]
    fn lists_every_uncommitted_file() {
        let files: Vec<String> = (1..=7).map(|n| format!("f{n}.txt")).collect();
        let text = alert_text(&Alert::UncommittedChanges { files });
        assert!(text.contains("7 files"));
        assert!(text.contains("`f7.txt`"));
    }

    #[test]
    fn renders_sections() {
        let repo =
            |display: &str, status: Status, actions: Vec<Action>, alerts: Vec<Alert>| RepoSync {
                display: display.to_string(),
                status,
                actions,
                alerts,
            };
        let report = SyncReport {
            check: crate::check::CheckReport {
                git_version: String::new(),
                config_path: None,
                repos: Vec::new(),
                hosts: Vec::new(),
                unreadable: Vec::new(),
            },
            aborted: Vec::new(),
            dry_run: true,
            repos: vec![
                repo(
                    "work/a",
                    Status::Synced,
                    vec![
                        Action::FastForwarded {
                            branch: "develop".into(),
                            commits: 3,
                        },
                        Action::Deleted {
                            branch: "CREW-1".into(),
                            reason: "pushed".into(),
                        },
                    ],
                    vec![Alert::Stashes(2)],
                ),
                repo(
                    "work/b",
                    Status::Failed(RemoteError::classify("ERROR: Repository not found.")),
                    vec![],
                    vec![],
                ),
                repo(
                    "work/c",
                    Status::Skipped("no remote".into()),
                    vec![],
                    vec![],
                ),
            ],
        };

        let markdown = to_markdown(&report, &[std::path::PathBuf::from("/work")]);

        assert!(markdown.contains("| Branches updated | 1 |"));
        assert!(markdown.contains("| work/a | develop (+3) |"));
        assert!(markdown.contains("| work/a | `CREW-1` | pushed |"));
        assert!(markdown.contains("| work/a | 2 stash entries |"));
        assert!(markdown.contains("## Remote errors"));
        assert!(markdown.contains("| work/c | no remote |"));
        assert!(markdown.contains("Dry run"));
    }
}
