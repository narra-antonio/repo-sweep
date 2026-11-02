//! Classification of the errors git reports when it talks to a remote.

use std::fmt;

/// Why git could not talk to a remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteErrorKind {
    /// The server answered, but the repository does not exist or the credentials in use have
    /// no access to it (several servers give the same answer in both cases).
    NotFound,
    /// The server refused the credentials.
    Auth,
    /// The server could not be reached.
    Network,
    Other,
}

/// An error reported by git, with its classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteError {
    pub kind: RemoteErrorKind,
    /// Most significant line of the git message.
    pub message: String,
}

const NOT_FOUND: &[&str] = &[
    "repository not found",
    "tf401019",
    "project you were looking for could not be found",
    "does not appear to be a git repository",
    "returned error: 404",
    "not found",
];

const AUTH: &[&str] = &[
    "permission denied",
    "authentication failed",
    "could not read username",
    "could not read password",
    "returned error: 401",
    "returned error: 403",
    "access denied",
    "host key verification failed",
    "account has been deactivated",
    "account has been blocked",
    "account is disabled",
];

const NETWORK: &[&str] = &[
    "could not resolve host",
    "could not resolve hostname",
    "connection refused",
    "connection timed out",
    "operation timed out",
    "network is unreachable",
    "no route to host",
    "connection reset",
    "failed to connect",
    "could not connect",
];

impl RemoteError {
    /// Classifies the standard error of a git command run with `LC_ALL=C`.
    pub fn classify(stderr: &str) -> Self {
        let lower = stderr.to_ascii_lowercase();
        // Specific "not found" answers win over the generic "could not read from remote" that
        // git prints after them.
        let kind = if contains_any(&lower, NOT_FOUND) {
            RemoteErrorKind::NotFound
        } else if contains_any(&lower, AUTH) {
            RemoteErrorKind::Auth
        } else if contains_any(&lower, NETWORK) {
            RemoteErrorKind::Network
        } else {
            RemoteErrorKind::Other
        };
        Self {
            kind,
            message: main_line(stderr),
        }
    }
}

impl fmt::Display for RemoteErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RemoteErrorKind::NotFound => "remote repository not found or no access",
            RemoteErrorKind::Auth => "authentication failed",
            RemoteErrorKind::Network => "server unreachable",
            RemoteErrorKind::Other => "git error",
        })
    }
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

fn contains_any(text: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| text.contains(pattern))
}

/// First line that is not the generic trailer git appends to every remote failure.
fn main_line(stderr: &str) -> String {
    // Servers prefix their messages with "remote:" and often frame them with banners.
    let lines: Vec<&str> = stderr
        .lines()
        .map(|line| {
            line.trim()
                .strip_prefix("remote:")
                .unwrap_or(line.trim())
                .trim()
        })
        .filter(|line| !line.is_empty() && !line.chars().all(|c| matches!(c, '=' | '-' | '*')))
        .collect();
    lines
        .iter()
        .find(|line| {
            let lower = line.to_ascii_lowercase();
            !lower.contains("could not read from remote repository")
                && !lower.contains("please make sure you have the correct access rights")
                && !lower.contains("and the repository exists")
        })
        .or(lines.first())
        .map(|line| line.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRAILER: &str = "fatal: Could not read from remote repository.\n\nPlease make sure you have the correct access rights\nand the repository exists.";

    #[test]
    fn classifies_github_missing_repository() {
        let error = RemoteError::classify(&format!("ERROR: Repository not found.\n{TRAILER}"));
        assert_eq!(error.kind, RemoteErrorKind::NotFound);
        assert_eq!(error.message, "ERROR: Repository not found.");
    }

    #[test]
    fn classifies_gitlab_missing_project() {
        let error = RemoteError::classify(
            "remote: The project you were looking for could not be found or you don't have permission to view it.\nfatal: repository 'https://gitlab.example.com/g/p.git/' not found",
        );
        assert_eq!(error.kind, RemoteErrorKind::NotFound);
    }

    #[test]
    fn classifies_azure_devops_missing_repository() {
        let error = RemoteError::classify(
            "remote: TF401019: The Git repository with name or identifier repo does not exist or you do not have permissions for the operation you are attempting.\nfatal: repository 'https://dev.azure.com/o/p/_git/repo/' not found",
        );
        assert_eq!(error.kind, RemoteErrorKind::NotFound);
    }

    #[test]
    fn classifies_missing_ssh_path() {
        let error = RemoteError::classify(&format!(
            "fatal: '/srv/git/gone.git' does not appear to be a git repository\n{TRAILER}"
        ));
        assert_eq!(error.kind, RemoteErrorKind::NotFound);
    }

    #[test]
    fn classifies_rejected_key() {
        let error = RemoteError::classify(&format!(
            "git@github.com: Permission denied (publickey).\n{TRAILER}"
        ));
        assert_eq!(error.kind, RemoteErrorKind::Auth);
        assert_eq!(
            error.message,
            "git@github.com: Permission denied (publickey)."
        );
    }

    #[test]
    fn classifies_https_credentials_failure() {
        let error = RemoteError::classify(
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
        );
        assert_eq!(error.kind, RemoteErrorKind::Auth);
    }

    #[test]
    fn classifies_network_failures() {
        for stderr in [
            "ssh: Could not resolve hostname git.example.com: nodename nor servname provided",
            "ssh: connect to host gitea.local port 2222: Connection refused",
            "fatal: unable to access 'https://x/': Failed to connect to x port 443",
        ] {
            assert_eq!(
                RemoteError::classify(stderr).kind,
                RemoteErrorKind::Network,
                "{stderr}"
            );
        }
    }

    #[test]
    fn classifies_deactivated_account_and_strips_banners() {
        let error = RemoteError::classify(
            "remote: \nremote: ========================================================================\nremote: \nremote: Your account has been deactivated by your administrator.\nremote:\n",
        );
        assert_eq!(error.kind, RemoteErrorKind::Auth);
        assert_eq!(
            error.message,
            "Your account has been deactivated by your administrator."
        );
    }

    #[test]
    fn classifies_unknown_errors() {
        let error = RemoteError::classify("fatal: something unexpected");
        assert_eq!(error.kind, RemoteErrorKind::Other);
        assert_eq!(error.message, "fatal: something unexpected");
    }
}
