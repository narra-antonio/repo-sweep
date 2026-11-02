use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

use repo_sweep::check;
use repo_sweep::config::Config;
use repo_sweep::detect::{self, DetectOptions};

/// Keep many local git repositories in sync with their remotes and clean up the branches
/// deleted after a merge.
#[derive(Parser)]
#[command(name = "repo-sweep", version)]
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
    }
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
