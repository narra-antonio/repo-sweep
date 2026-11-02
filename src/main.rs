use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

use repo_sweep::check;
use repo_sweep::config::Config;

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
            let roots = if paths.is_empty() {
                vec![std::env::current_dir()?]
            } else {
                paths
            };
            let (config, config_path) =
                Config::load(cli.config.as_deref(), roots.first().map(PathBuf::as_path))?;
            let report = check::run(&roots, &config, config_path)?;
            print!("{report}");
            if report.unreachable_required().is_empty() {
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::from(EXIT_UNREACHABLE))
            }
        }
    }
}
