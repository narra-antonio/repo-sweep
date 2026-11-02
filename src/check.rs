//! The `check` command: finds the repositories and checks that their servers are reachable,
//! without changing anything.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::Config;
use crate::connectivity::{self, Endpoint, Reachability};
use crate::discovery;
use crate::git;
use crate::remote::{RemoteUrl, Transport};

/// What was found in one repository.
#[derive(Debug, Clone)]
pub struct RepoInfo {
    pub path: PathBuf,
    /// Path shown in the report, relative to the folder it was found under.
    pub display: String,
    /// Checked-out branch, `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Remote name and URL.
    pub remote: Option<(String, String)>,
    /// Server of the remote, `None` for local remotes or when there is no remote.
    pub endpoint: Option<Endpoint>,
    pub error: Option<String>,
}

/// Reachability of one server.
#[derive(Debug, Clone)]
pub struct HostStatus {
    pub endpoint: Endpoint,
    pub required: bool,
    pub reachability: Reachability,
    pub repositories: usize,
    /// Host names written in the remote URLs (SSH aliases included).
    pub names: BTreeSet<String>,
}

#[derive(Debug)]
pub struct CheckReport {
    pub git_version: String,
    pub config_path: Option<PathBuf>,
    pub repos: Vec<RepoInfo>,
    pub hosts: Vec<HostStatus>,
    pub unreadable: Vec<(PathBuf, String)>,
}

impl CheckReport {
    /// Required servers that cannot be reached.
    pub fn unreachable_required(&self) -> Vec<&HostStatus> {
        self.hosts
            .iter()
            .filter(|host| host.required && !host.reachability.is_reachable())
            .collect()
    }
}

/// Runs the check on the given folders.
pub fn run(
    roots: &[PathBuf],
    config: &Config,
    config_path: Option<PathBuf>,
) -> Result<CheckReport> {
    let git_version = git::version()?;
    let found = discovery::find_repositories(roots, config.max_depth, &config.exclude)?;
    if found.repositories.is_empty() {
        anyhow::bail!(
            "no git repository found in {} (max depth {})",
            roots
                .iter()
                .map(|r| r.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            config.max_depth
        );
    }

    let mut ssh_cache: HashMap<(String, Option<u16>), Endpoint> = HashMap::new();
    let mut names: BTreeMap<Endpoint, BTreeSet<String>> = BTreeMap::new();
    let mut counts: BTreeMap<Endpoint, usize> = BTreeMap::new();
    let mut repos = Vec::with_capacity(found.repositories.len());

    for path in found.repositories {
        let display = display_path(&path, roots);
        let mut info = RepoInfo {
            path: path.clone(),
            display,
            branch: None,
            remote: None,
            endpoint: None,
            error: None,
        };
        match inspect(&path) {
            Ok((branch, remote)) => {
                info.branch = branch;
                if let Some((_, url)) = &remote {
                    match resolve_endpoint(url, &mut ssh_cache) {
                        Ok(Some((endpoint, name))) => {
                            names.entry(endpoint.clone()).or_default().insert(name);
                            *counts.entry(endpoint.clone()).or_default() += 1;
                            info.endpoint = Some(endpoint);
                        }
                        Ok(None) => {}
                        Err(error) => info.error = Some(error),
                    }
                }
                info.remote = remote;
            }
            Err(error) => info.error = Some(format!("{error:#}")),
        }
        repos.push(info);
    }

    let required = config.required_endpoints()?;
    for endpoint in &required {
        names.entry(endpoint.clone()).or_default();
    }
    let endpoints: Vec<Endpoint> = names.keys().cloned().collect();
    let results = connectivity::check_all(&endpoints, config.connect_timeout());
    let hosts = endpoints
        .into_iter()
        .zip(results)
        .map(|(endpoint, reachability)| HostStatus {
            required: required.contains(&endpoint),
            repositories: counts.get(&endpoint).copied().unwrap_or(0),
            names: names.remove(&endpoint).unwrap_or_default(),
            endpoint,
            reachability,
        })
        .collect();

    Ok(CheckReport {
        git_version,
        config_path,
        repos,
        hosts,
        unreadable: found.unreadable,
    })
}

/// Checked-out branch and main remote (name, URL) of a repository.
type Inspection = (Option<String>, Option<(String, String)>);

fn inspect(path: &Path) -> Result<Inspection> {
    Ok((git::current_branch(path)?, git::main_remote(path)?))
}

/// Server of a remote URL and the host name written in it. SSH aliases are resolved
/// once per alias through the SSH configuration.
fn resolve_endpoint(
    url: &str,
    ssh_cache: &mut HashMap<(String, Option<u16>), Endpoint>,
) -> std::result::Result<Option<(Endpoint, String)>, String> {
    let remote = RemoteUrl::parse(url).ok_or_else(|| format!("unsupported remote URL: {url}"))?;
    let endpoint = match remote.transport {
        Transport::Local => return Ok(None),
        Transport::Ssh => ssh_cache
            .entry((remote.host.clone(), remote.port))
            .or_insert_with(|| connectivity::resolve_ssh(&remote.host, remote.port))
            .clone(),
        _ => Endpoint::new(
            remote.host.clone(),
            remote.port.or(remote.default_port()).unwrap_or_default(),
        ),
    };
    Ok(Some((endpoint, remote.host)))
}

fn display_path(path: &Path, roots: &[PathBuf]) -> String {
    for root in roots {
        if let Ok(relative) = path.strip_prefix(root) {
            if relative.as_os_str().is_empty() {
                return path.display().to_string();
            }
            return relative.display().to_string();
        }
    }
    path.display().to_string()
}

fn describe(reachability: &Reachability) -> String {
    match reachability {
        Reachability::Reachable => "reachable".to_string(),
        Reachability::Unresolved(error) => format!("cannot resolve the name ({error})"),
        Reachability::Unreachable(error) => format!("no connection ({error})"),
    }
}

impl fmt::Display for CheckReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.git_version)?;
        match &self.config_path {
            Some(path) => writeln!(f, "Configuration: {}", path.display())?,
            None => writeln!(f, "Configuration: defaults (no configuration file found)")?,
        }

        writeln!(f, "\nServers ({})", self.hosts.len())?;
        let host_width = self
            .hosts
            .iter()
            .map(|h| h.endpoint.to_string().len())
            .max()
            .unwrap_or(0);
        for host in &self.hosts {
            let status = if host.reachability.is_reachable() {
                "OK  "
            } else {
                "FAIL"
            };
            let required = if host.required { " required" } else { "" };
            let aliases: Vec<&str> = host
                .names
                .iter()
                .map(String::as_str)
                .filter(|name| *name != host.endpoint.host)
                .collect();
            let via = if aliases.is_empty() {
                String::new()
            } else {
                format!(" via {}", aliases.join(", "))
            };
            writeln!(
                f,
                "  {status} {:<host_width$}  {:>3} repositories{required}{via}",
                host.endpoint.to_string(),
                host.repositories,
            )?;
            if !host.reachability.is_reachable() {
                writeln!(f, "       {}", describe(&host.reachability))?;
            }
        }

        writeln!(f, "\nRepositories ({})", self.repos.len())?;
        let path_width = self
            .repos
            .iter()
            .map(|r| r.display.len())
            .max()
            .unwrap_or(0);
        let branch_width = self
            .repos
            .iter()
            .map(|r| r.branch.as_deref().unwrap_or("(detached)").len())
            .max()
            .unwrap_or(0);
        for repo in &self.repos {
            let branch = repo.branch.as_deref().unwrap_or("(detached)");
            let server = match (&repo.endpoint, &repo.remote) {
                (Some(endpoint), _) => endpoint.to_string(),
                (None, Some(_)) => "local remote".to_string(),
                (None, None) => "no remote".to_string(),
            };
            writeln!(
                f,
                "  {:<path_width$}  {:<branch_width$}  {server}",
                repo.display, branch
            )?;
        }

        let errors: Vec<&RepoInfo> = self.repos.iter().filter(|r| r.error.is_some()).collect();
        if !errors.is_empty() || !self.unreadable.is_empty() {
            writeln!(f, "\nWarnings")?;
            for repo in errors {
                writeln!(
                    f,
                    "  {}: {}",
                    repo.display,
                    repo.error.as_deref().unwrap_or_default()
                )?;
            }
            for (path, error) in &self.unreadable {
                writeln!(f, "  cannot read {}: {error}", path.display())?;
            }
        }

        let unreachable = self
            .hosts
            .iter()
            .filter(|h| !h.reachability.is_reachable())
            .count();
        writeln!(
            f,
            "\nSummary: {} repositories, {} servers, {} unreachable ({} required)",
            self.repos.len(),
            self.hosts.len(),
            unreachable,
            self.unreachable_required().len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_paths_relative_to_their_root() {
        let roots = vec![PathBuf::from("/work"), PathBuf::from("/other")];
        assert_eq!(
            display_path(Path::new("/work/group/repo"), &roots),
            Path::new("group/repo").display().to_string()
        );
        assert_eq!(display_path(Path::new("/other"), &roots), "/other");
        assert_eq!(
            display_path(Path::new("/elsewhere/repo"), &roots),
            "/elsewhere/repo"
        );
    }

    #[test]
    fn resolves_https_endpoint_with_default_port() {
        let mut cache = HashMap::new();
        let (endpoint, name) = resolve_endpoint("https://example.com/owner/repo.git", &mut cache)
            .unwrap()
            .unwrap();
        assert_eq!(endpoint, Endpoint::new("example.com", 443));
        assert_eq!(name, "example.com");
    }

    #[test]
    fn ignores_local_remotes() {
        let mut cache = HashMap::new();
        assert!(resolve_endpoint("/srv/git/repo.git", &mut cache)
            .unwrap()
            .is_none());
    }

    #[test]
    fn reports_unsupported_urls() {
        let mut cache = HashMap::new();
        assert!(resolve_endpoint("ftp://example.com/repo", &mut cache).is_err());
    }
}
