use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use repo_sweep::check;
use repo_sweep::config::{Config, OnUnreachable};
use repo_sweep::detect::{self, DetectOptions};
use repo_sweep::notify;
use repo_sweep::report;
use repo_sweep::schedule::{self, Task, When};
use repo_sweep::sync::{self, Action, Status, SyncOptions, SyncReport};

/// Version and target triple, e.g. `0.1.3 (aarch64-apple-darwin)`: bug reports then say which build
/// and operating system they come from.
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("REPO_SWEEP_TARGET"),
    ")"
);

/// Keep many local git repositories in sync with their remotes and clean up the branches
/// deleted after a merge.
#[derive(Parser)]
#[command(name = "repo-sweep", version = VERSION)]
struct Cli {
    /// Configuration file (default: repo-sweep.toml in the first folder, then the user
    /// configuration folder)
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check that the servers are reachable and list the repositories found, without
    /// changing anything
    Check {
        /// Root folders or single repositories (default: the current folder)
        paths: Vec<PathBuf>,
    },
    /// Find the development branch of each repository and save it in the configuration
    Detect {
        /// Root folders or single repositories (default: the current folder)
        paths: Vec<PathBuf>,
        /// Detect again the repositories that already have a development branch
        #[arg(long)]
        force: bool,
        /// Show the result without saving it
        #[arg(long)]
        dry_run: bool,
    },
    /// Bring every repository up to date with its remote and delete the branches whose remote
    /// branch was deleted
    Sync {
        /// Root folders or single repositories (default: the current folder)
        paths: Vec<PathBuf>,
        /// Show what would be done without changing any branch
        #[arg(long)]
        dry_run: bool,
        /// What to do when a required server is unreachable (default: from the configuration)
        #[arg(long, value_enum)]
        on_unreachable: Option<UnreachableArg>,
        /// Write the full report to this Markdown file and print only the summary
        #[arg(long, value_name = "FILE", conflicts_with = "report_dir")]
        report: Option<PathBuf>,
        /// Write the full report to a time-stamped file in this folder, keeping the most recent
        /// ones (`keep_reports` in the configuration)
        #[arg(long, value_name = "DIR")]
        report_dir: Option<PathBuf>,
        /// Show a desktop notification with the summary at the end
        #[arg(long)]
        notify: bool,
    },
    /// Run `sync` periodically through the scheduler of the operating system
    Schedule {
        #[command(subcommand)]
        action: ScheduleAction,
    },
}

#[derive(Subcommand)]
enum ScheduleAction {
    /// Register (or replace) the scheduled sync
    Install {
        /// Root folders or single repositories (default: the current folder)
        paths: Vec<PathBuf>,
        /// Local time of the run, HH:MM
        #[arg(long, value_name = "HH:MM")]
        at: String,
        /// Run only on this day of the week (mon, tue, ... sun); every day when omitted
        #[arg(long)]
        day: Option<String>,
    },
    /// Show the scheduled sync
    Status,
    /// Remove the scheduled sync
    Remove,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum UnreachableArg {
    /// Stop before touching any repository
    Abort,
    /// Skip only the repositories of the unreachable server
    Skip,
}

impl From<UnreachableArg> for OnUnreachable {
    fn from(value: UnreachableArg) -> Self {
        match value {
            UnreachableArg::Abort => OnUnreachable::Abort,
            UnreachableArg::Skip => OnUnreachable::Skip,
        }
    }
}

/// Exit code when a required server is unreachable.
const EXIT_UNREACHABLE: u8 = 2;

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Commands::Check { paths } => {
            let roots = roots_or_current(paths)?;
            let (config, config_path) = load_config(cli.config.as_deref(), &roots)?;
            let report = check::run(&roots, &config, config_path)?;
            print!("{report}");
            Ok(exit_code(report.unreachable_required().is_empty()))
        }
        Commands::Detect {
            paths,
            force,
            dry_run,
        } => {
            let roots = roots_or_current(paths)?;
            let (config, config_path) = load_config(cli.config.as_deref(), &roots)?;
            let report = detect::run(
                &roots,
                &config,
                config_path,
                DetectOptions { force, dry_run },
            )?;
            print!("{report}");
            Ok(exit_code(report.aborted.is_empty()))
        }
        Commands::Sync {
            paths,
            dry_run,
            on_unreachable,
            report: report_file,
            report_dir,
            notify: show_notification,
        } => {
            let roots = roots_or_current(paths)?;
            let (config, config_path) = load_config(cli.config.as_deref(), &roots)?;
            let options = SyncOptions {
                dry_run,
                on_unreachable: on_unreachable.map(Into::into),
            };
            let sync_report = sync::run(&roots, &config, config_path, options)?;
            let written = match (report_file, report_dir) {
                (Some(file), _) => {
                    report::write(&file, &sync_report, &roots)?;
                    Some(file)
                }
                (None, Some(dir)) => Some(report::write_to_dir(
                    &dir,
                    &sync_report,
                    &roots,
                    config.keep_reports,
                )?),
                (None, None) => None,
            };
            match &written {
                Some(file) => {
                    println!("{}", summary(&sync_report));
                    println!("Report: {}", file.display());
                }
                None => print!("{sync_report}"),
            }
            if show_notification {
                let mut message = summary(&sync_report);
                if let Some(file) = &written {
                    message.push_str(&format!("\nReport: {}", file.display()));
                }
                notify::send("repo-sweep", &message);
            }
            Ok(exit_code(sync_report.aborted.is_empty()))
        }
        Commands::Schedule { action } => schedule_command(action, cli.config),
    }
}

fn schedule_command(action: ScheduleAction, config: Option<PathBuf>) -> Result<ExitCode> {
    match action {
        ScheduleAction::Install { paths, at, day } => {
            let when = When::parse(&at, day.as_deref())?;
            let roots = roots_or_current(paths)?
                .into_iter()
                .map(|root| std::path::absolute(&root).unwrap_or(root))
                .collect::<Vec<_>>();
            for root in &roots {
                if !root.is_dir() {
                    anyhow::bail!("{} is not a folder", root.display());
                }
            }
            let data = dirs::data_local_dir()
                .context("cannot find the local data folder")?
                .join("repo-sweep");
            let executable =
                std::env::current_exe().context("cannot find the repo-sweep executable")?;
            let mut command = vec![executable.display().to_string()];
            if let Some(config) = config {
                command.push("--config".to_string());
                command.push(
                    std::path::absolute(&config)
                        .unwrap_or(config)
                        .display()
                        .to_string(),
                );
            }
            command.extend([
                "sync".to_string(),
                // Unattended: a server that is down must not stop the other repositories.
                "--on-unreachable".to_string(),
                "skip".to_string(),
                "--report-dir".to_string(),
                data.join("reports").display().to_string(),
                "--notify".to_string(),
            ]);
            command.extend(roots.iter().map(|root| root.display().to_string()));
            let task = Task {
                when,
                command,
                path_env: std::env::var("PATH").ok(),
                log_dir: data.join("logs"),
            };
            let installed = schedule::install(&task)?;
            println!("Scheduled sync {when}: {installed}");
            println!("Reports: {}", data.join("reports").display());
            Ok(ExitCode::SUCCESS)
        }
        ScheduleAction::Status => {
            println!("{}", schedule::status()?);
            Ok(ExitCode::SUCCESS)
        }
        ScheduleAction::Remove => {
            println!("{}", schedule::remove()?);
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// One-line summary of a sync, for the terminal and the notification.
fn summary(report: &SyncReport) -> String {
    if !report.aborted.is_empty() {
        let servers: Vec<String> = report.aborted.iter().map(|e| e.to_string()).collect();
        return format!(
            "Required server unreachable ({}): nothing done",
            servers.join(", ")
        );
    }
    let count = |predicate: fn(&Action) -> bool| {
        report
            .repos
            .iter()
            .flat_map(|r| &r.actions)
            .filter(|a| predicate(a))
            .count()
    };
    let alerts: usize = report.repos.iter().map(|r| r.alerts.len()).sum();
    let errors = report
        .repos
        .iter()
        .filter(|r| matches!(r.status, Status::Failed(_)))
        .count();
    format!(
        "{}{} repositories: {} branches updated, {} deleted, {} alerts, {} errors",
        if report.dry_run { "Dry run, " } else { "" },
        report.repos.len(),
        count(|a| matches!(a, Action::FastForwarded { .. })),
        count(|a| matches!(a, Action::Deleted { .. })),
        alerts,
        errors
    )
}

fn roots_or_current(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    if paths.is_empty() {
        Ok(vec![std::env::current_dir()?])
    } else {
        Ok(paths)
    }
}

fn load_config(explicit: Option<&Path>, roots: &[PathBuf]) -> Result<(Config, Option<PathBuf>)> {
    Config::load(explicit, roots.first().map(PathBuf::as_path))
}

fn exit_code(servers_ok: bool) -> ExitCode {
    if servers_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_UNREACHABLE)
    }
}
