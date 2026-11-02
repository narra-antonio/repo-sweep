//! Configuration file (TOML).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::connectivity::Endpoint;
use crate::discovery::DEFAULT_EXCLUDE;

/// File name looked up in the first folder passed on the command line.
pub const LOCAL_FILE_NAME: &str = "repo-sweep.toml";

/// What to do when a required host cannot be reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OnUnreachable {
    /// Stop before touching any repository.
    #[default]
    Abort,
    /// Skip only the repositories of the unreachable host.
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Hosts that must be reachable, as `host:port` (for example a server reachable only
    /// through a VPN).
    pub required_hosts: Vec<String>,
    /// Behaviour when a required host is unreachable.
    pub on_unreachable: OnUnreachable,
    /// Maximum folder depth searched for repositories.
    pub max_depth: usize,
    /// Folder names never descended into.
    pub exclude: Vec<String>,
    /// Timeout of each reachability check, in seconds.
    pub connect_timeout_secs: u64,
    /// Number of repositories processed at the same time.
    pub parallel: usize,
    /// Number of reports kept by `sync --report-dir`.
    pub keep_reports: usize,
    /// Development branch names, in order of preference when their age is the same.
    pub dev_candidates: Vec<String>,
    /// Branches used as development branch when none of `dev_candidates` exists.
    pub fallback_branches: Vec<String>,
    /// Per-repository settings, keyed by the absolute repository path.
    pub repos: BTreeMap<String, RepoConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct RepoConfig {
    /// Development branch the repository returns to after a sync.
    pub dev_branch: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            required_hosts: Vec::new(),
            on_unreachable: OnUnreachable::Abort,
            max_depth: 4,
            exclude: DEFAULT_EXCLUDE.iter().map(|s| s.to_string()).collect(),
            connect_timeout_secs: 5,
            parallel: 8,
            keep_reports: 30,
            dev_candidates: vec!["develop".to_string(), "feature".to_string()],
            fallback_branches: vec!["main".to_string(), "master".to_string()],
            repos: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Parses and validates a configuration.
    pub fn parse(text: &str) -> Result<Self> {
        let config: Config = toml::from_str(text)?;
        config.required_endpoints()?;
        Ok(config)
    }

    /// Loads the configuration from, in order: the explicit file, `repo-sweep.toml` in the
    /// first folder, the user configuration folder. Returns the defaults when none exists,
    /// together with the path of the file actually read.
    pub fn load(
        explicit: Option<&Path>,
        first_root: Option<&Path>,
    ) -> Result<(Self, Option<PathBuf>)> {
        if let Some(path) = explicit {
            if !path.is_file() {
                bail!("configuration file {} not found", path.display());
            }
            return Ok((Self::read(path)?, Some(path.to_path_buf())));
        }
        let candidates = [
            first_root.map(|root| root.join(LOCAL_FILE_NAME)),
            user_config_path(),
        ];
        for path in candidates.into_iter().flatten() {
            if path.is_file() {
                return Ok((Self::read(&path)?, Some(path)));
            }
        }
        Ok((Self::default(), None))
    }

    fn read(path: &Path) -> Result<Self> {
        let text =
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid configuration in {}", path.display()))
    }

    /// The required hosts as endpoints.
    pub fn required_endpoints(&self) -> Result<Vec<Endpoint>> {
        self.required_hosts
            .iter()
            .map(|value| {
                Endpoint::parse(value).with_context(|| {
                    format!("required host '{value}' must be written as host:port")
                })
            })
            .collect()
    }

    /// Development branch saved for a repository.
    pub fn dev_branch(&self, repo: &Path) -> Option<&str> {
        self.repos.get(&repo_key(repo))?.dev_branch.as_deref()
    }

    pub fn connect_timeout(&self) -> Duration {
        Duration::from_secs(self.connect_timeout_secs.max(1))
    }
}

/// Key of a repository in the `repos` table: its absolute path.
pub fn repo_key(repo: &Path) -> String {
    std::path::absolute(repo)
        .unwrap_or_else(|_| repo.to_path_buf())
        .display()
        .to_string()
}

/// Writes the development branch of each repository into the configuration file, keeping
/// the rest of the file (comments included) unchanged. Creates the file when missing.
pub fn save_dev_branches(path: &Path, branches: &BTreeMap<String, String>) -> Result<()> {
    let text = if path.is_file() {
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?
    } else {
        String::new()
    };
    let mut document: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("invalid configuration in {}", path.display()))?;
    let repos = document
        .entry("repos")
        .or_insert_with(|| {
            let mut table = toml_edit::Table::new();
            table.set_implicit(true);
            toml_edit::Item::Table(table)
        })
        .as_table_like_mut()
        .context("'repos' must be a table")?;
    for (key, branch) in branches {
        let entry = repos
            .entry(key)
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_like_mut()
            .with_context(|| format!("repos.\"{key}\" must be a table"))?;
        entry.insert("dev_branch", toml_edit::value(branch.as_str()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    fs::write(path, document.to_string())
        .with_context(|| format!("cannot write {}", path.display()))
}

/// Path of the user-level configuration file, for example
/// `~/.config/repo-sweep/config.toml` on Linux.
pub fn user_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("repo-sweep").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn uses_defaults_for_empty_file() {
        let config = Config::parse("").unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.max_depth, 4);
        assert!(config.exclude.contains(&"node_modules".to_string()));
    }

    #[test]
    fn parses_full_file() {
        let config = Config::parse(
            r#"
            required_hosts = ["git.example.com:4242"]
            on_unreachable = "skip"
            max_depth = 6
            connect_timeout_secs = 3

            [repos."/work/backend"]
            dev_branch = "develop"
            "#,
        )
        .unwrap();
        assert_eq!(config.on_unreachable, OnUnreachable::Skip);
        assert_eq!(config.max_depth, 6);
        assert_eq!(
            config.required_endpoints().unwrap(),
            vec![Endpoint::new("git.example.com", 4242)]
        );
        assert_eq!(
            config.repos["/work/backend"].dev_branch.as_deref(),
            Some("develop")
        );
    }

    #[test]
    fn saves_dev_branches_keeping_comments() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(LOCAL_FILE_NAME);
        fs::write(
            &path,
            "# my servers\nrequired_hosts = [\"git.example.com:4242\"]\n\n[repos.\"/work/a\"]\ndev_branch = \"feature\"\n",
        )
        .unwrap();
        let branches = BTreeMap::from([
            ("/work/a".to_string(), "develop".to_string()),
            ("/work/b".to_string(), "master".to_string()),
        ]);

        save_dev_branches(&path, &branches).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my servers"));
        let config = Config::parse(&text).unwrap();
        assert_eq!(config.required_hosts, vec!["git.example.com:4242"]);
        assert_eq!(
            config.repos["/work/a"].dev_branch.as_deref(),
            Some("develop")
        );
        assert_eq!(
            config.repos["/work/b"].dev_branch.as_deref(),
            Some("master")
        );
    }

    #[test]
    fn creates_missing_configuration_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/config.toml");
        let branches = BTreeMap::from([("/work/a".to_string(), "main".to_string())]);

        save_dev_branches(&path, &branches).unwrap();

        let config = Config::parse(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config.repos["/work/a"].dev_branch.as_deref(), Some("main"));
    }

    #[test]
    fn finds_dev_branch_by_absolute_path() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.repos.insert(
            repo_key(dir.path()),
            RepoConfig {
                dev_branch: Some("develop".to_string()),
            },
        );
        assert_eq!(config.dev_branch(dir.path()), Some("develop"));
        assert_eq!(config.dev_branch(&dir.path().join("other")), None);
    }

    #[test]
    fn rejects_required_host_without_port() {
        let error = Config::parse(r#"required_hosts = ["git.example.com"]"#).unwrap_err();
        assert!(format!("{error:#}").contains("host:port"));
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(Config::parse("unknown = 1").is_err());
    }

    #[test]
    fn loads_local_file_from_first_root() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join(LOCAL_FILE_NAME), "max_depth = 2").unwrap();

        let (config, path) = Config::load(None, Some(root.path())).unwrap();

        assert_eq!(config.max_depth, 2);
        assert_eq!(path, Some(root.path().join(LOCAL_FILE_NAME)));
    }

    #[test]
    fn explicit_file_wins_and_must_exist() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join(LOCAL_FILE_NAME), "max_depth = 2").unwrap();
        let explicit = root.path().join("other.toml");
        fs::write(&explicit, "max_depth = 7").unwrap();

        let (config, _) = Config::load(Some(&explicit), Some(root.path())).unwrap();
        assert_eq!(config.max_depth, 7);

        assert!(Config::load(Some(&root.path().join("missing.toml")), None).is_err());
    }
}
