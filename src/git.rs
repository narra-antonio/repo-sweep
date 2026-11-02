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
