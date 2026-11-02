//! Discovery of the git repositories under the given folders.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

/// Folder names never descended into: build outputs and dependency caches.
pub const DEFAULT_EXCLUDE: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    ".angular",
    ".gradle",
    ".idea",
    ".venv",
    "venv",
    "__pycache__",
];

/// Result of a discovery: the repositories found and the folders that could not be read.
#[derive(Debug, Default)]
pub struct Discovery {
    pub repositories: Vec<PathBuf>,
    pub unreadable: Vec<(PathBuf, String)>,
}

/// Returns whether the folder is the working tree of a git repository (`.git` is a folder,
/// or a file for worktrees and submodules).
pub fn is_repository(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// Finds the repositories under each root. A root may itself be a repository; nested
/// repositories are found too. Symbolic links are not followed.
pub fn find_repositories(
    roots: &[PathBuf],
    max_depth: usize,
    exclude: &[String],
) -> Result<Discovery> {
    let mut discovery = Discovery::default();
    let mut found = BTreeSet::new();
    for root in roots {
        if !root.exists() {
            bail!("{} does not exist", root.display());
        }
        if !root.is_dir() {
            bail!("{} is not a folder", root.display());
        }
        walk(root, 0, max_depth, exclude, &mut found, &mut discovery);
    }
    discovery.repositories = found.into_iter().collect();
    Ok(discovery)
}

fn walk(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    exclude: &[String],
    found: &mut BTreeSet<PathBuf>,
    discovery: &mut Discovery,
) {
    if is_repository(dir) {
        found.insert(dir.to_path_buf());
    }
    if depth >= max_depth {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            discovery
                .unreadable
                .push((dir.to_path_buf(), error.to_string()));
            return;
        }
    };
    let mut children: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name != ".git" && !exclude.iter().any(|excluded| excluded == &name)
        })
        .map(|entry| entry.path())
        .collect();
    children.sort();
    for child in children {
        walk(&child, depth + 1, max_depth, exclude, found, discovery);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn default_exclude() -> Vec<String> {
        DEFAULT_EXCLUDE.iter().map(|s| s.to_string()).collect()
    }

    fn make_repo(path: &Path) {
        fs::create_dir_all(path.join(".git")).unwrap();
    }

    #[test]
    fn finds_repositories_in_subfolders() {
        let root = TempDir::new().unwrap();
        make_repo(&root.path().join("group-a/repo-1"));
        make_repo(&root.path().join("group-b/repo-2"));
        fs::create_dir_all(root.path().join("not-a-repo/docs")).unwrap();

        let discovery =
            find_repositories(&[root.path().to_path_buf()], 4, &default_exclude()).unwrap();

        assert_eq!(
            discovery.repositories,
            vec![
                root.path().join("group-a/repo-1"),
                root.path().join("group-b/repo-2")
            ]
        );
    }

    #[test]
    fn accepts_a_repository_as_root() {
        let root = TempDir::new().unwrap();
        make_repo(root.path());

        let discovery =
            find_repositories(&[root.path().to_path_buf()], 4, &default_exclude()).unwrap();

        assert_eq!(discovery.repositories, vec![root.path().to_path_buf()]);
    }

    #[test]
    fn finds_nested_repositories() {
        let root = TempDir::new().unwrap();
        make_repo(&root.path().join("parent"));
        make_repo(&root.path().join("parent/child"));

        let discovery =
            find_repositories(&[root.path().to_path_buf()], 4, &default_exclude()).unwrap();

        assert_eq!(discovery.repositories.len(), 2);
    }

    #[test]
    fn treats_git_file_as_repository() {
        let root = TempDir::new().unwrap();
        let worktree = root.path().join("worktree");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(worktree.join(".git"), "gitdir: ../main/.git/worktrees/x").unwrap();

        let discovery =
            find_repositories(&[root.path().to_path_buf()], 4, &default_exclude()).unwrap();

        assert_eq!(discovery.repositories, vec![worktree]);
    }

    #[test]
    fn skips_excluded_folders() {
        let root = TempDir::new().unwrap();
        make_repo(&root.path().join("app/node_modules/some-package"));
        make_repo(&root.path().join("app/target/copy"));

        let discovery =
            find_repositories(&[root.path().to_path_buf()], 6, &default_exclude()).unwrap();

        assert!(discovery.repositories.is_empty());
    }

    #[test]
    fn respects_max_depth() {
        let root = TempDir::new().unwrap();
        make_repo(&root.path().join("a/b/c/deep"));

        let shallow =
            find_repositories(&[root.path().to_path_buf()], 2, &default_exclude()).unwrap();
        let deep = find_repositories(&[root.path().to_path_buf()], 4, &default_exclude()).unwrap();

        assert!(shallow.repositories.is_empty());
        assert_eq!(deep.repositories.len(), 1);
    }

    #[test]
    fn deduplicates_overlapping_roots() {
        let root = TempDir::new().unwrap();
        let repo = root.path().join("repo");
        make_repo(&repo);

        let discovery = find_repositories(
            &[root.path().to_path_buf(), repo.clone()],
            4,
            &default_exclude(),
        )
        .unwrap();

        assert_eq!(discovery.repositories, vec![repo]);
    }

    #[test]
    fn fails_on_missing_root() {
        let root = TempDir::new().unwrap();
        let missing = root.path().join("missing");

        let error = find_repositories(&[missing], 4, &default_exclude()).unwrap_err();

        assert!(error.to_string().contains("does not exist"));
    }

    #[test]
    fn fails_on_file_root() {
        let root = TempDir::new().unwrap();
        let file = root.path().join("file.txt");
        fs::write(&file, "x").unwrap();

        let error = find_repositories(&[file], 4, &default_exclude()).unwrap_err();

        assert!(error.to_string().contains("is not a folder"));
    }
}
