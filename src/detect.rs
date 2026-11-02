//! The `detect` command: finds the development branch of each repository and saves it in the
//! configuration, so that later runs do not have to work it out again.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

use anyhow::Result;

use crate::check::{self, CheckReport, RepoInfo};
use crate::config::{self, Config, OnUnreachable};
use crate::connectivity::Endpoint;
use crate::git;
use crate::parallel;
use crate::remote_error::RemoteError;

/// Options of the `detect` command.
#[derive(Debug, Clone, Copy, Default)]
pub struct DetectOptions {
    /// Detect again the repositories that already have a development branch.
    pub force: bool,
    /// Do not write the configuration file.
    pub dry_run: bool,
}

/// Result of the detection for one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Detected {
        branch: String,
        reason: String,
        /// Whether the branch differs from the one saved in the configuration.
        changed: bool,
    },
    Skipped(String),
    Failed(RemoteError),
}

#[derive(Debug, Clone)]
pub struct Detection {
    pub display: String,
    pub key: String,
    pub outcome: Outcome,
}

#[derive(Debug)]
pub struct DetectReport {
    pub check: CheckReport,
    /// Required servers that are unreachable when the configuration says to abort.
    pub aborted: Vec<Endpoint>,
    pub detections: Vec<Detection>,
    /// File the changes were written to, `None` in a dry run or without changes.
    pub saved_to: Option<PathBuf>,
    pub dry_run: bool,
}

/// Picks the development branch among the branches published by the remote.
///
/// The `dev` candidates win over the `fallback` ones; when several `dev` candidates exist the
/// one with the most recent commit wins (ties and unknown dates follow the candidate order).
/// Returns the branch and a human-readable reason.
pub fn choose(
    heads: &BTreeSet<String>,
    dev: &[String],
    fallback: &[String],
    commit_time: impl Fn(&str) -> Option<i64>,
) -> Option<(String, String)> {
    let present: Vec<&String> = dev.iter().filter(|b| heads.contains(*b)).collect();
    match present.as_slice() {
        [] => fallback
            .iter()
            .find(|b| heads.contains(*b))
            .map(|b| (b.clone(), format!("no development branch, using {b}"))),
        [only] => Some(((*only).clone(), format!("only {only}"))),
        several => {
            let dated: Vec<(&String, Option<i64>)> =
                several.iter().map(|b| (*b, commit_time(b))).collect();
            let mut best = dated[0];
            for candidate in &dated[1..] {
                if candidate.1.unwrap_or(i64::MIN) > best.1.unwrap_or(i64::MIN) {
                    best = *candidate;
                }
            }
            let dates: Vec<String> = dated
                .iter()
                .map(|(b, t)| format!("{b} {}", t.map(format_date).unwrap_or_else(|| "?".into())))
                .collect();
            Some((best.0.clone(), format!("most recent: {}", dates.join(", "))))
        }
    }
}

/// Runs the detection on the given folders.
pub fn run(
    roots: &[PathBuf],
    config: &Config,
    config_path: Option<PathBuf>,
    options: DetectOptions,
) -> Result<DetectReport> {
    let check = check::run(roots, config, config_path.clone())?;
    let mut report = DetectReport {
        aborted: Vec::new(),
        detections: Vec::new(),
        saved_to: None,
        dry_run: options.dry_run,
        check,
    };
    if config.on_unreachable == OnUnreachable::Abort {
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
    report.detections = parallel::map(&report.check.repos, config.parallel, |repo| {
        detect_one(repo, config, &unreachable, options.force)
    });

    let changes: BTreeMap<String, String> = report
        .detections
        .iter()
        .filter_map(|detection| match &detection.outcome {
            Outcome::Detected {
                branch,
                changed: true,
                ..
            } => Some((detection.key.clone(), branch.clone())),
            _ => None,
        })
        .collect();
    if !changes.is_empty() && !options.dry_run {
        let target = config_path
            .or_else(config::user_config_path)
            .ok_or_else(|| anyhow::anyhow!("cannot determine where to save the configuration"))?;
        config::save_dev_branches(&target, &changes)?;
        report.saved_to = Some(target);
    }
    Ok(report)
}

fn detect_one(
    repo: &RepoInfo,
    config: &Config,
    unreachable: &BTreeSet<Endpoint>,
    force: bool,
) -> Detection {
    let key = config::repo_key(&repo.path);
    let outcome = detect_outcome(repo, config, unreachable, force);
    Detection {
        display: repo.display.clone(),
        key,
        outcome,
    }
}

fn detect_outcome(
    repo: &RepoInfo,
    config: &Config,
    unreachable: &BTreeSet<Endpoint>,
    force: bool,
) -> Outcome {
    if let Some(error) = &repo.error {
        return Outcome::Skipped(error.clone());
    }
    let Some((remote, _)) = &repo.remote else {
        return Outcome::Skipped("no remote".to_string());
    };
    if let Some(endpoint) = &repo.endpoint {
        if unreachable.contains(endpoint) {
            return Outcome::Skipped(format!("server {endpoint} unreachable"));
        }
    }
    let heads: BTreeSet<String> = match git::remote_heads(&repo.path, remote) {
        Ok(Ok(heads)) => heads.into_iter().collect(),
        Ok(Err(stderr)) => return Outcome::Failed(RemoteError::classify(&stderr)),
        Err(error) => return Outcome::Skipped(format!("{error:#}")),
    };

    if heads.is_empty() {
        return Outcome::Skipped("the remote has no branches".to_string());
    }

    let saved = config.dev_branch(&repo.path);
    if let Some(saved) = saved {
        if !force && heads.contains(saved) {
            return Outcome::Detected {
                branch: saved.to_string(),
                reason: "saved in the configuration".to_string(),
                changed: false,
            };
        }
    }

    let present: Vec<&str> = config
        .dev_candidates
        .iter()
        .filter(|b| heads.contains(*b))
        .map(String::as_str)
        .collect();
    if present.len() > 1 {
        // Refresh the candidates without pruning: other remote-tracking refs stay untouched.
        let _ = git::fetch_branches(&repo.path, remote, &present);
    }
    let commit_time = |branch: &str| {
        git::commit_time(&repo.path, &format!("refs/remotes/{remote}/{branch}"))
            .ok()
            .flatten()
    };
    match choose(
        &heads,
        &config.dev_candidates,
        &config.fallback_branches,
        commit_time,
    ) {
        Some((branch, mut reason)) => {
            if let Some(saved) = saved {
                if saved != branch {
                    reason = format!("{reason} (was {saved}, no longer on the remote)");
                }
            }
            Outcome::Detected {
                changed: saved != Some(branch.as_str()),
                branch,
                reason,
            }
        }
        None => match git::remote_default_branch(&repo.path, remote)
            .ok()
            .flatten()
        {
            Some(branch) if heads.contains(&branch) => Outcome::Detected {
                changed: saved != Some(branch.as_str()),
                reason: "default branch of the remote".to_string(),
                branch,
            },
            _ => Outcome::Skipped(format!(
                "none of {} on the remote",
                config
                    .dev_candidates
                    .iter()
                    .chain(&config.fallback_branches)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        },
    }
}

/// Formats Unix seconds as `YYYY-MM-DD` (UTC).
pub fn format_date(seconds: i64) -> String {
    // Civil-from-days algorithm (Howard Hinnant), valid for the proleptic Gregorian calendar.
    let days = seconds.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

impl fmt::Display for DetectReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.aborted.is_empty() {
            let servers: Vec<String> = self.aborted.iter().map(Endpoint::to_string).collect();
            return writeln!(
                f,
                "Required server unreachable: {}. Nothing was done (is the VPN connected?).",
                servers.join(", ")
            );
        }

        let detected: Vec<(&Detection, &String, &String, bool)> = self
            .detections
            .iter()
            .filter_map(|d| match &d.outcome {
                Outcome::Detected {
                    branch,
                    reason,
                    changed,
                } => Some((d, branch, reason, *changed)),
                _ => None,
            })
            .collect();
        writeln!(f, "Development branches ({})", detected.len())?;
        let path_width = detected
            .iter()
            .map(|d| d.0.display.len())
            .max()
            .unwrap_or(0);
        let branch_width = detected.iter().map(|d| d.1.len()).max().unwrap_or(0);
        for (detection, branch, reason, changed) in &detected {
            let marker = if *changed { "*" } else { " " };
            writeln!(
                f,
                "  {marker} {:<path_width$}  {:<branch_width$}  {reason}",
                detection.display, branch
            )?;
        }

        let failed: Vec<(&Detection, &RemoteError)> = self
            .detections
            .iter()
            .filter_map(|d| match &d.outcome {
                Outcome::Failed(error) => Some((d, error)),
                _ => None,
            })
            .collect();
        if !failed.is_empty() {
            writeln!(f, "\nRemote errors ({})", failed.len())?;
            for (detection, error) in failed {
                writeln!(f, "  {}: {error}", detection.display)?;
            }
        }

        let skipped: Vec<(&Detection, &String)> = self
            .detections
            .iter()
            .filter_map(|d| match &d.outcome {
                Outcome::Skipped(reason) => Some((d, reason)),
                _ => None,
            })
            .collect();
        if !skipped.is_empty() {
            writeln!(f, "\nSkipped ({})", skipped.len())?;
            for (detection, reason) in skipped {
                writeln!(f, "  {}: {reason}", detection.display)?;
            }
        }

        let changes = detected.iter().filter(|d| d.3).count();
        writeln!(f)?;
        match (&self.saved_to, self.dry_run, changes) {
            (_, _, 0) => writeln!(f, "No change to save."),
            (_, true, n) => writeln!(f, "Dry run: {n} changes (*) not saved."),
            (Some(path), false, n) => writeln!(f, "Saved {n} changes (*) to {}", path.display()),
            (None, false, n) => writeln!(f, "{n} changes (*) not saved."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heads(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    fn dev() -> Vec<String> {
        names(&["develop", "feature"])
    }

    fn fallback() -> Vec<String> {
        names(&["main", "master"])
    }

    #[test]
    fn picks_the_only_development_branch() {
        let chosen = choose(
            &heads(&["master", "feature", "CREW-1"]),
            &dev(),
            &fallback(),
            |_| None,
        );
        assert_eq!(chosen.unwrap().0, "feature");
    }

    #[test]
    fn picks_the_most_recent_of_several_development_branches() {
        let times = |b: &str| match b {
            "develop" => Some(1_700_000_000),
            "feature" => Some(1_790_000_000),
            _ => None,
        };
        let (branch, reason) = choose(
            &heads(&["develop", "feature", "master"]),
            &dev(),
            &fallback(),
            times,
        )
        .unwrap();
        assert_eq!(branch, "feature");
        assert!(reason.contains("develop 2023-11-14"), "{reason}");
    }

    #[test]
    fn keeps_candidate_order_when_dates_are_unknown() {
        let chosen = choose(&heads(&["develop", "feature"]), &dev(), &fallback(), |_| {
            None
        });
        assert_eq!(chosen.unwrap().0, "develop");
    }

    #[test]
    fn falls_back_to_master_only_repositories() {
        let (branch, reason) = choose(&heads(&["master"]), &dev(), &fallback(), |_| None).unwrap();
        assert_eq!(branch, "master");
        assert!(reason.contains("no development branch"));
        assert_eq!(
            choose(&heads(&["main", "master"]), &dev(), &fallback(), |_| None)
                .unwrap()
                .0,
            "main"
        );
    }

    #[test]
    fn returns_none_without_known_branches() {
        assert!(
            choose(&heads(&["CREW-1", "release"]), &dev(), &fallback(), |_| {
                None
            })
            .is_none()
        );
    }

    #[test]
    fn formats_dates() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(951_782_400), "2000-02-29");
        assert_eq!(format_date(1_793_318_400), "2026-10-30");
    }
}
