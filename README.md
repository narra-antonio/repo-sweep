# repo-sweep

Keep many local git repositories in sync with their remotes, and clean up the local branches
whose remote branch was deleted after a merge.

If you work on dozens of repositories where merge requests delete the source branch, your
clones slowly fill up with stale branches and outdated development branches. `repo-sweep`
walks a folder tree, finds every repository and brings each one up to date — without ever
touching uncommitted work.

> **Status:** early development. Only the read-only `check` command is available.

## Features

- Single executable for Windows, macOS and Linux.
- Uses the installed `git`, so your credentials, SSH keys and `~/.ssh/config` aliases work
  as on the command line.
- Checks that the servers are reachable before starting (for example a server reachable only
  through a VPN), and tells an unreachable server apart from a deleted repository.

## Commands

| Command | Description |
|---|---|
| `repo-sweep check [PATHS...]` | Check the servers and list the repositories found, without changing anything |

`PATHS` are root folders (searched recursively) or single repositories; the default is the
current folder.

Exit codes: `0` success, `1` error, `2` a required server is unreachable.

### Planned

- `sync`: fetch with prune, fast-forward the development branch, `master`/`main` and every
  local branch that still has a remote, delete the local branches whose remote is gone,
  return to the development branch. Repositories with uncommitted changes are reported, never
  modified.
- `detect`: find the development branch of each repository and save it in the configuration.
- `schedule`: register a daily or weekly run in the operating system scheduler (launchd,
  Task Scheduler, systemd timers).

## Configuration

`repo-sweep` reads, in order: the file given with `--config`, `repo-sweep.toml` in the first
folder passed on the command line, the user configuration file
(`~/.config/repo-sweep/config.toml` on Linux, `~/Library/Application Support/repo-sweep/config.toml`
on macOS, `%APPDATA%\repo-sweep\config.toml` on Windows).

```toml
# Servers that must be reachable before starting, as host:port.
required_hosts = ["git.example.com:4242"]

# "abort" stops before touching any repository; "skip" skips only the repositories
# of the unreachable server.
on_unreachable = "abort"

max_depth = 4
connect_timeout_secs = 5
exclude = ["node_modules", "target", "dist", ".angular", ".gradle", ".idea", ".venv", "venv", "__pycache__"]

[repos."/home/me/work/backend"]
dev_branch = "develop"
```

## Building

```bash
cargo build --release
cargo test
```

## License

[MIT](LICENSE)
