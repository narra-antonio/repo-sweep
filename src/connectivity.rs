//! Reachability checks of the servers that host the remotes.

use std::fmt;
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

/// A server address to check.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }

    /// Parses `host:port` (`[ipv6]:port` for IPv6 addresses). The port is mandatory.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        let (host, port) = match value.strip_prefix('[') {
            Some(rest) => {
                let (host, after) = rest.split_once(']')?;
                (host, after.strip_prefix(':')?)
            }
            None => value.rsplit_once(':')?,
        };
        if host.is_empty() {
            return None;
        }
        Some(Self::new(host, port.parse().ok()?))
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// Outcome of a reachability check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reachability {
    Reachable,
    /// The host name could not be resolved (no DNS, or a VPN-only name without the VPN).
    Unresolved(String),
    /// The host was resolved but no connection could be opened within the timeout.
    Unreachable(String),
}

impl Reachability {
    pub fn is_reachable(&self) -> bool {
        matches!(self, Reachability::Reachable)
    }
}

/// Opens a TCP connection to the endpoint, trying every resolved address.
pub fn check(endpoint: &Endpoint, timeout: Duration) -> Reachability {
    let addresses: Vec<_> = match (endpoint.host.as_str(), endpoint.port).to_socket_addrs() {
        Ok(addresses) => addresses.collect(),
        Err(error) => return Reachability::Unresolved(error.to_string()),
    };
    if addresses.is_empty() {
        return Reachability::Unresolved("no address found".to_string());
    }
    let mut last_error = String::new();
    for address in addresses {
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(_) => return Reachability::Reachable,
            Err(error) => last_error = error.to_string(),
        }
    }
    Reachability::Unreachable(last_error)
}

/// Checks all endpoints in parallel, preserving the input order.
pub fn check_all(endpoints: &[Endpoint], timeout: Duration) -> Vec<Reachability> {
    thread::scope(|scope| {
        let handles: Vec<_> = endpoints
            .iter()
            .map(|endpoint| scope.spawn(move || check(endpoint, timeout)))
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Reachability::Unreachable("check failed".to_string()))
            })
            .collect()
    })
}

/// Resolves the real host and port of an SSH destination with `ssh -G`, which applies the
/// user's SSH configuration (host aliases, `HostName`, `Port`). Falls back to the values
/// written in the URL when `ssh` is not available.
pub fn resolve_ssh(host: &str, port: Option<u16>) -> Endpoint {
    let fallback = Endpoint::new(host, port.unwrap_or(22));
    let mut command = Command::new("ssh");
    command.arg("-G");
    if let Some(port) = port {
        command.arg("-p").arg(port.to_string());
    }
    command.arg(host).stdin(Stdio::null()).stderr(Stdio::null());
    match command.output() {
        Ok(output) if output.status.success() => {
            let (resolved_host, resolved_port) =
                parse_ssh_config_output(&String::from_utf8_lossy(&output.stdout));
            Endpoint::new(
                resolved_host.unwrap_or(fallback.host),
                resolved_port.unwrap_or(fallback.port),
            )
        }
        _ => fallback,
    }
}

/// Extracts `hostname` and `port` from the output of `ssh -G`.
pub fn parse_ssh_config_output(output: &str) -> (Option<String>, Option<u16>) {
    let mut host = None;
    let mut port = None;
    for line in output.lines() {
        let Some((key, value)) = line.trim().split_once(' ') else {
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "hostname" if host.is_none() => host = Some(value.trim().to_string()),
            "port" if port.is_none() => port = value.trim().parse().ok(),
            _ => {}
        }
    }
    (host, port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn parses_endpoint() {
        assert_eq!(
            Endpoint::parse("git.example.com:4242"),
            Some(Endpoint::new("git.example.com", 4242))
        );
        assert_eq!(Endpoint::parse("[::1]:22"), Some(Endpoint::new("::1", 22)));
    }

    #[test]
    fn rejects_endpoint_without_port() {
        assert_eq!(Endpoint::parse("git.example.com"), None);
        assert_eq!(Endpoint::parse(":22"), None);
        assert_eq!(Endpoint::parse("host:port"), None);
    }

    #[test]
    fn displays_endpoint() {
        assert_eq!(Endpoint::new("host", 22).to_string(), "host:22");
        assert_eq!(Endpoint::new("::1", 22).to_string(), "[::1]:22");
    }

    #[test]
    fn parses_ssh_config_output() {
        let output = "user git\nhostname github.com\nport 22\nidentityfile ~/.ssh/id_work\n";
        assert_eq!(
            parse_ssh_config_output(output),
            (Some("github.com".to_string()), Some(22))
        );
    }

    #[test]
    fn ignores_incomplete_ssh_config_output() {
        assert_eq!(parse_ssh_config_output(""), (None, None));
        assert_eq!(parse_ssh_config_output("port abc\n"), (None, None));
    }

    #[test]
    fn reports_open_port_as_reachable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let result = check(&Endpoint::new("127.0.0.1", port), Duration::from_secs(2));
        assert!(result.is_reachable());
    }

    #[test]
    fn reports_closed_port_as_unreachable() {
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let result = check(&Endpoint::new("127.0.0.1", port), Duration::from_secs(2));
        assert!(matches!(result, Reachability::Unreachable(_)));
    }

    #[test]
    fn reports_unknown_host_as_unresolved() {
        let result = check(&Endpoint::new("host.invalid", 22), Duration::from_secs(2));
        assert!(matches!(result, Reachability::Unresolved(_)));
    }

    #[test]
    fn checks_all_in_input_order() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let open = listener.local_addr().unwrap().port();
        let endpoints = vec![
            Endpoint::new("host.invalid", 22),
            Endpoint::new("127.0.0.1", open),
        ];
        let results = check_all(&endpoints, Duration::from_secs(2));
        assert!(matches!(results[0], Reachability::Unresolved(_)));
        assert!(results[1].is_reachable());
    }
}
