# repo-sweep

Keep many local git repositories in sync with their remotes, and clean up the local branches
whose remote branch was deleted after a merge.

If you work on dozens of repositories where merge requests delete the source branch, your
clones slowly fill up with stale branches and outdated development branches. `repo-sweep`
walks a folder tree, finds every repository and brings each one up to date — without ever
touching uncommitted work.

> **Status:** early development: all the commands below are available.

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
| `repo-sweep detect [PATHS...] [--force] [--dry-run]` | Find the development branch of each repository and save it in the configuration |
| `repo-sweep sync [PATHS...] [--dry-run] [--on-unreachable abort\|skip] [--report FILE \| --report-dir DIR] [--notify]` | Bring every repository up to date and delete the branches whose remote branch is gone |
| `repo-sweep schedule install --at HH:MM [--day DAY] [PATHS...]` | Run `sync` every day (or every week) through the scheduler of the operating system |
| `repo-sweep schedule status` / `schedule remove` | Show or remove the scheduled run |

`PATHS` are root folders (searched recursively) or single repositories; the default is the
current folder.

Exit codes: `0` success, `1` error, `2` a required server is unreachable.

### Development branch detection

`detect` reads the branches published by each remote (`git ls-remote`) and picks:

1. the only `dev_candidates` branch present (`develop`, `feature` by default);
2. when several are present, the one with the most recent commit;
3. otherwise the first `fallback_branches` branch present (`main`, `master`);
4. otherwise the default branch of the remote.

The result is saved in the `[repos]` table of the configuration file (comments and the rest
of the file are kept), so later runs do not work it out again. Saved branches are kept until
they disappear from the remote; `--force` detects every repository again.

Remote failures are reported by kind: server unreachable, remote repository not found (or no
access with the credentials in use), authentication failed.

### Synchronisation

For each repository, `sync`:

1. fetches the remote with `--prune --force`;
2. when the working tree is clean, switches to the development branch (creating it from the
   remote when missing);
3. fast-forwards every local branch that follows a remote branch — the checked-out one with
   `git merge --ff-only`, the others without checking them out;
4. deletes the local branches whose remote branch is gone, only when their work cannot be
   lost: the branch was pushed as it is, its commits are on another remote branch, or its
   changes are already in the development branch (squash merge, checked with
   `git merge-tree`, git 2.38 or later).

It never merges, rebases or discards anything. It reports instead: uncommitted changes (with
the files), operations in progress, stashes, branches with commits not pushed, diverged
branches, deleted remote branches with local commits, branches checked out in other worktrees.
Local branches without an upstream are never touched.

`--dry-run` shows the plan without changing any branch: it fetches without pruning, so that the
real run can still prove which deleted branches were pushed.

With many repositories, `--report FILE` writes the full report as Markdown (tables of the
updated, deleted and switched branches, the alerts, the remote errors) and prints only the
summary; `--report-dir DIR` writes a time-stamped report and keeps the most recent
`keep_reports` ones. `--notify` shows a desktop notification with the summary.

### Scheduling

`schedule install` registers `sync` in the scheduler of the operating system — no process stays
in the background:

| System | Scheduler | Files |
|---|---|---|
| macOS | launchd agent | `~/Library/LaunchAgents/io.github.repo-sweep.sync.plist` |
| Windows | Task Scheduler task `repo-sweep` | `%APPDATA%\repo-sweep\scheduled-sync.cmd` |
| Linux | systemd user timer | `~/.config/systemd/user/repo-sweep.{service,timer}` |

The scheduled run uses `--on-unreachable skip` (an unreachable server only skips its
repositories), writes its reports to the local data folder (`repo-sweep/reports`) and shows a
notification. It runs the executable that performed the installation, with the `PATH` of the
installing shell: install `repo-sweep` in a stable place first (for example `cargo install
--path .`).

```bash
repo-sweep schedule install --at 02:00 ~/work           # every day at 02:00
repo-sweep schedule install --at 08:30 --day mon ~/work # every Monday at 08:30
```

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
parallel = 8
keep_reports = 30
dev_candidates = ["develop", "feature"]
fallback_branches = ["main", "master"]
exclude = ["node_modules", "target", "dist", ".angular", ".gradle", ".idea", ".venv", "venv", "__pycache__"]

[repos."/home/me/work/backend"]
dev_branch = "develop"
```

## Installation

Download the archive for your system from the
[releases](https://github.com/narra-antonio/repo-sweep/releases) page, extract it and put
`repo-sweep` (`repo-sweep.exe` on Windows) in a folder on your `PATH`. `git` must be installed.

| System | Archive |
|---|---|
| Windows x64 / ARM64 | `repo-sweep-<version>-x86_64-pc-windows-msvc.zip` / `aarch64-pc-windows-msvc.zip` |
| macOS Intel / Apple silicon | `repo-sweep-<version>-x86_64-apple-darwin.tar.gz` / `aarch64-apple-darwin.tar.gz` |
| Linux x64 (static) | `repo-sweep-<version>-x86_64-unknown-linux-musl.tar.gz` |

`SHA256SUMS.txt` lists the checksums of the archives. On macOS, an executable downloaded with a
browser is quarantined: allow it once with `xattr -d com.apple.quarantine repo-sweep`.

With a Rust toolchain: `cargo install --git https://github.com/narra-antonio/repo-sweep`.

## Releasing

Bump `version` in `Cargo.toml`, commit, then push a matching tag:

```bash
git tag v0.1.0
git push origin v0.1.0
```

The `Release` workflow builds the executables and publishes them, with the checksums, in a
GitHub release; GitHub adds the source code archives.

## Building

```bash
cargo build --release
cargo test
```

## License

[MIT](LICENSE)
