//! Parsing of git remote URLs into the parts needed to reach their server.

/// How git reaches a remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    Ssh,
    Https,
    Http,
    Git,
    Local,
}

/// A remote URL split into transport, host, port and path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteUrl {
    pub transport: Transport,
    /// Host as written in the URL: for SSH it may be an alias of the SSH configuration.
    pub host: String,
    /// Port written in the URL, if any.
    pub port: Option<u16>,
    /// Repository path on the server, or the local path for local remotes.
    pub path: String,
}

impl RemoteUrl {
    /// Parses the URL forms accepted by git: `scheme://[user@]host[:port]/path`, the scp-like
    /// `[user@]host:path` and local paths. Returns `None` for empty or unsupported URLs.
    pub fn parse(url: &str) -> Option<Self> {
        let url = url.trim();
        if url.is_empty() {
            return None;
        }
        if let Some((scheme, rest)) = url.split_once("://") {
            return Self::parse_with_scheme(scheme, rest);
        }
        if let Some(parsed) = Self::parse_scp_like(url) {
            return Some(parsed);
        }
        Some(Self::local(url))
    }

    /// Default port of the transport, `None` for local remotes.
    pub fn default_port(&self) -> Option<u16> {
        match self.transport {
            Transport::Ssh => Some(22),
            Transport::Https => Some(443),
            Transport::Http => Some(80),
            Transport::Git => Some(9418),
            Transport::Local => None,
        }
    }

    fn local(path: &str) -> Self {
        Self {
            transport: Transport::Local,
            host: String::new(),
            port: None,
            path: path.to_string(),
        }
    }

    fn parse_with_scheme(scheme: &str, rest: &str) -> Option<Self> {
        let transport = match scheme.to_ascii_lowercase().as_str() {
            "ssh" | "git+ssh" | "ssh+git" => Transport::Ssh,
            "https" => Transport::Https,
            "http" => Transport::Http,
            "git" => Transport::Git,
            "file" => return Some(Self::local(rest)),
            _ => return None,
        };
        let (authority, path) = match rest.find('/') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, ""),
        };
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
        let (host, port) = split_host_port(host_port)?;
        if host.is_empty() {
            return None;
        }
        Some(Self {
            transport,
            host,
            port,
            path: path.trim_start_matches('/').to_string(),
        })
    }

    fn parse_scp_like(url: &str) -> Option<Self> {
        let (before, after) = url.split_once(':')?;
        // Like git, a slash before the first colon means a local path.
        if before.contains('/') || before.contains('\\') {
            return None;
        }
        let host = before.rsplit_once('@').map_or(before, |(_, h)| h);
        // "C:\repo" or "C:/repo" is a Windows drive, not a host.
        if host.len() == 1 && host.chars().all(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        if host.is_empty() || after.is_empty() {
            return None;
        }
        Some(Self {
            transport: Transport::Ssh,
            host: host.to_string(),
            port: None,
            path: after.to_string(),
        })
    }
}

fn split_host_port(value: &str) -> Option<(String, Option<u16>)> {
    if let Some(rest) = value.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(port) if !port.is_empty() => Some(port.parse().ok()?),
            _ => None,
        };
        return Some((host.to_string(), port));
    }
    match value.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() => {
            Some((host.to_string(), Some(port.parse().ok()?)))
        }
        Some((host, _)) => Some((host.to_string(), None)),
        None => Some((value.to_string(), None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(url: &str) -> RemoteUrl {
        RemoteUrl::parse(url).expect("url should parse")
    }

    #[test]
    fn parses_scp_like_url() {
        let url = parse("git@github.com:owner/repo.git");
        assert_eq!(url.transport, Transport::Ssh);
        assert_eq!(url.host, "github.com");
        assert_eq!(url.port, None);
        assert_eq!(url.path, "owner/repo.git");
    }

    #[test]
    fn parses_scp_like_url_with_ssh_alias() {
        let url = parse("git@github-work:owner/repo.git");
        assert_eq!(url.host, "github-work");
        assert_eq!(url.default_port(), Some(22));
    }

    #[test]
    fn parses_ssh_url_with_port() {
        let url = parse("ssh://git@git.example.com:4242/group/project.git");
        assert_eq!(url.transport, Transport::Ssh);
        assert_eq!(url.host, "git.example.com");
        assert_eq!(url.port, Some(4242));
        assert_eq!(url.path, "group/project.git");
    }

    #[test]
    fn parses_https_url_with_user_info() {
        let url = parse("https://org@dev.azure.com/org/project/_git/repo");
        assert_eq!(url.transport, Transport::Https);
        assert_eq!(url.host, "dev.azure.com");
        assert_eq!(url.port, None);
        assert_eq!(url.default_port(), Some(443));
    }

    #[test]
    fn parses_azure_devops_ssh_url() {
        let url = parse("git@ssh.dev.azure.com:v3/org/project/repo");
        assert_eq!(url.transport, Transport::Ssh);
        assert_eq!(url.host, "ssh.dev.azure.com");
    }

    #[test]
    fn parses_ipv6_host() {
        let url = parse("ssh://git@[::1]:2222/repo.git");
        assert_eq!(url.host, "::1");
        assert_eq!(url.port, Some(2222));
    }

    #[test]
    fn treats_paths_as_local() {
        assert_eq!(parse("/srv/git/repo.git").transport, Transport::Local);
        assert_eq!(parse("../other/repo").transport, Transport::Local);
        assert_eq!(
            parse("file:///srv/git/repo.git").transport,
            Transport::Local
        );
    }

    #[test]
    fn treats_windows_drive_as_local() {
        assert_eq!(parse("C:\\repos\\repo.git").transport, Transport::Local);
        assert_eq!(parse("D:/repos/repo.git").transport, Transport::Local);
    }

    #[test]
    fn rejects_empty_and_unknown_urls() {
        assert_eq!(RemoteUrl::parse("  "), None);
        assert_eq!(RemoteUrl::parse("ftp://host/repo"), None);
        assert_eq!(RemoteUrl::parse("ssh://git@host:notaport/repo"), None);
    }
}
