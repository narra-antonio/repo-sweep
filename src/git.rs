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

/// Branch names published by the remote (`git ls-remote --heads`), or the error of git.
pub fn remote_heads(repo: &Path, remote: &str) -> Result<std::result::Result<Vec<String>, String>> {
    let output = run(Some(repo), &["ls-remote", "--heads", remote])?;
    if !output.success {
        return Ok(Err(output.stderr));
    }
    Ok(Ok(parse_heads(&output.stdout)))
}

/// Branch names from the output of `git ls-remote --heads`.
pub fn parse_heads(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter_map(|(_, reference)| reference.trim().strip_prefix("refs/heads/"))
        .map(str::to_string)
        .collect()
}

/// Default branch of the remote (the branch its `HEAD` points to), when it publishes one.
pub fn remote_default_branch(repo: &Path, remote: &str) -> Result<Option<String>> {
    let output = run(Some(repo), &["ls-remote", "--symref", remote, "HEAD"])?;
    if !output.success {
        return Ok(None);
    }
    Ok(parse_symref_head(&output.stdout))
}

/// Branch name from the output of `git ls-remote --symref <remote> HEAD`.
pub fn parse_symref_head(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (reference, name) = line.strip_prefix("ref:")?.trim().split_once('\t')?;
        (name.trim() == "HEAD")
            .then(|| reference.trim().strip_prefix("refs/heads/"))
            .flatten()
            .map(str::to_string)
    })
}

/// Updates the remote-tracking refs of the given branches without pruning anything.
pub fn fetch_branches(repo: &Path, remote: &str, branches: &[&str]) -> Result<GitOutput> {
    let mut args = vec!["fetch", "--quiet", "--no-prune", remote];
    args.extend_from_slice(branches);
    run(Some(repo), &args)
}

/// Committer date (Unix seconds) of the commit a reference points to.
pub fn commit_time(repo: &Path, reference: &str) -> Result<Option<i64>> {
    let output = run(Some(repo), &["log", "-1", "--format=%ct", reference, "--"])?;
    if !output.success {
        return Ok(None);
    }
    Ok(output.stdout.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ls_remote_heads() {
        let output = "a1b2\trefs/heads/develop\nc3d4\trefs/heads/feature/x\ne5f6\trefs/tags/v1\n";
        assert_eq!(parse_heads(output), vec!["develop", "feature/x"]);
        assert!(parse_heads("").is_empty());
    }

    #[test]
    fn parses_remote_default_branch() {
        let output = "ref: refs/heads/trunk\tHEAD\na1b2\tHEAD\n";
        assert_eq!(parse_symref_head(output).as_deref(), Some("trunk"));
        assert_eq!(parse_symref_head("a1b2\tHEAD\n"), None);
        assert_eq!(parse_symref_head(""), None);
    }
}
