# 🧹 repo-sweep

> Keep dozens of local git repositories in sync with their remotes, and clean up the branches
> deleted after a merge — without ever touching uncommitted work.

[![Version](https://img.shields.io/github/v/release/narra-antonio/repo-sweep?label=version&style=flat-square)](https://github.com/narra-antonio/repo-sweep/releases)
[![License](https://img.shields.io/github/license/narra-antonio/repo-sweep?style=flat-square)](./LICENSE)
[![CI](https://img.shields.io/github/actions/workflow/status/narra-antonio/repo-sweep/ci.yml?branch=main&label=ci&style=flat-square)](https://github.com/narra-antonio/repo-sweep/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/actions/workflow/status/narra-antonio/repo-sweep/release.yml?label=release&style=flat-square)](https://github.com/narra-antonio/repo-sweep/actions/workflows/release.yml)
[![Rust](https://img.shields.io/badge/rust-1.79%2B-B7410E?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Windows](https://img.shields.io/badge/windows-x64%20%7C%20arm64-0078D6?style=flat-square&logo=windows&logoColor=white)](https://github.com/narra-antonio/repo-sweep/releases/latest)
[![macOS](https://img.shields.io/badge/macos-intel%20%7C%20apple%20silicon-000000?style=flat-square&logo=apple&logoColor=white)](https://github.com/narra-antonio/repo-sweep/releases/latest)
[![Linux](https://img.shields.io/badge/linux-x64%20%7C%20arm64-FCC624?style=flat-square&logo=linux&logoColor=black)](https://github.com/narra-antonio/repo-sweep/releases/latest)
[![Changelog](https://img.shields.io/badge/changelog-latest-green?style=flat-square)](./CHANGELOG.md)

---

## ✨ Features

- 🔄 **One command for every repository** — walks a folder tree, finds every repository (nested
  ones included) and brings each one up to date.
- 🧹 **Safe branch cleanup** — deletes the local branches whose remote branch is gone only when
  their work is on the remote or already merged, squash merges included.
- 🛡️ **Never loses work** — no merge, rebase or reset: uncommitted changes, stashes, unpushed and
  diverged branches are reported, not touched.
- 🌐 **Knows why a remote fails** — server unreachable (VPN off?), repository deleted or no
  access, authentication failed: each one reported apart.
- 🔐 **Your git, your keys** — uses the installed `git`, so credentials, SSH keys and
  `~/.ssh/config` aliases work as on the command line.
- ⏰ **Scheduled runs** — daily or weekly through launchd, Task Scheduler or systemd, with a
  Markdown report and a desktop notification.
- 📦 **Single executable** — Windows, macOS and Linux, x64 and ARM64.

---

## 🚀 Quick Start

```bash
# 1. What would be found, and are the servers reachable? (changes nothing)
repo-sweep check ~/work

# 2. Find and save the development branch of each repository (develop, feature, main...)
repo-sweep detect ~/work

# 3. See what sync would do, then do it
repo-sweep sync --dry-run --report sync-plan.md ~/work
repo-sweep sync ~/work

# 4. Every night at 02:00
repo-sweep schedule install --at 02:00 ~/work
```

`~/work` can be a root folder (searched recursively) or a single repository; several paths can
be given. The default is the current folder.

---

## 📦 Available builds

Download the archive for your system from the
[latest release](https://github.com/narra-antonio/repo-sweep/releases/latest), extract it and put
`repo-sweep` (`repo-sweep.exe` on Windows) in a folder on your `PATH`. `git` must be installed.

| System  | Archive                                             | Arch                 |
| ------- | --------------------------------------------------- | -------------------- |
| Windows | `repo-sweep-<version>-x86_64-pc-windows-msvc.zip`   | x64                  |
| Windows | `repo-sweep-<version>-aarch64-pc-windows-msvc.zip`  | ARM64                |
| macOS   | `repo-sweep-<version>-x86_64-apple-darwin.tar.gz`   | Intel                |
| macOS   | `repo-sweep-<version>-aarch64-apple-darwin.tar.gz`  | Apple silicon        |
| Linux   | `repo-sweep-<version>-x86_64-unknown-linux-musl.tar.gz`  | x64 (static)    |
| Linux   | `repo-sweep-<version>-aarch64-unknown-linux-musl.tar.gz` | ARM64 (static)  |

`SHA256SUMS.txt` lists the checksums of the archives. On macOS an executable downloaded with a
browser is quarantined: allow it once with `xattr -d com.apple.quarantine repo-sweep`.

With a Rust toolchain: `cargo install --git https://github.com/narra-antonio/repo-sweep`.

---

## 🧭 Commands

| Command | Description |
|---|---|
| `check [PATHS...]` | List the repositories found and check that their servers are reachable, without changing anything |
| `detect [PATHS...] [--force] [--dry-run]` | Find the development branch of each repository and save it in the configuration |
| `sync [PATHS...] [--dry-run] [--on-unreachable abort\|skip] [--report FILE \| --report-dir DIR] [--notify]` | Bring every repository up to date and delete the branches whose remote branch is gone |
| `schedule install --at HH:MM [--day DAY] [PATHS...]` | Run `sync` every day, or every week on `DAY`, through the scheduler of the operating system |
| `schedule status` / `schedule remove` | Show or remove the scheduled run |

Every command accepts `--config FILE`. Exit codes: `0` success, `1` error, `2` a required server
is unreachable.

### 🔎 detect — development branch

`detect` reads the branches published by each remote (`git ls-remote`) and picks:

1. the only `dev_candidates` branch present (`develop`, `feature` by default);
2. when several are present, the one with the most recent commit;
3. otherwise the first `fallback_branches` branch present (`main`, `master`);
4. otherwise the default branch of the remote.

The result is saved in the `[repos]` table of the configuration file (comments and the rest of
the file are kept), so later runs do not work it out again. A saved branch is kept until it
disappears from the remote; `--force` detects every repository again.

### 🔄 sync — what it does

For each repository:

1. fetches the remote with `--prune --force`;
2. when the working tree is clean, switches to the development branch (creating it from the
   remote when missing);
3. fast-forwards every local branch that follows a remote branch — the checked-out one with
   `git merge --ff-only`, the others without checking them out;
4. deletes the local branches whose remote branch is gone, only when their work cannot be lost:
   - the branch was pushed as it is, then deleted on the remote;
   - its commits are on another branch of the remote;
   - a commit of the development branch has exactly the same content (squash merge of an
     up-to-date branch, even if others changed the same files later);
   - merging it into the development branch changes nothing (squash merge, checked with
     `git merge-tree`, git 2.38 or later).

It never merges, rebases or discards anything. It reports instead: uncommitted changes (with
the files), operations in progress, stashes, branches with commits not pushed, diverged
branches, deleted remote branches whose commits are not on the remote, branches checked out in
other worktrees. Local branches without an upstream are never touched.

- `--dry-run` shows the plan without changing any branch. It fetches without pruning, so that the
  real run can still prove which deleted branches were pushed.
- `--report FILE` writes the full report as Markdown and prints only the summary — handy with
  many repositories. `--report-dir DIR` writes a time-stamped report and keeps the most recent
  `keep_reports` ones.
- `--notify` shows a desktop notification with the summary.
- `--on-unreachable` overrides the configuration: `abort` stops before touching anything when a
  required server is unreachable, `skip` skips only its repositories.

### ⏰ schedule — periodic runs

`schedule install` registers `sync` in the scheduler of the operating system; no process stays
in the background.

| System  | Scheduler                        | Files                                                     |
| ------- | -------------------------------- | --------------------------------------------------------- |
| macOS   | launchd agent                    | `~/Library/LaunchAgents/io.github.repo-sweep.sync.plist`  |
| Windows | Task Scheduler task `repo-sweep` | `%APPDATA%\repo-sweep\scheduled-sync.cmd`                 |
| Linux   | systemd user timer               | `~/.config/systemd/user/repo-sweep.{service,timer}`       |

```bash
repo-sweep schedule install --at 02:00 ~/work            # every day at 02:00
repo-sweep schedule install --at 08:30 --day mon ~/work  # every Monday at 08:30
```

The scheduled run uses `--on-unreachable skip`, writes its reports to the local data folder
(`repo-sweep/reports`) and shows a notification. It runs the executable that performed the
installation, with the `PATH` of the installing shell: install `repo-sweep` in a stable place
first.

---

## ⚙️ Configuration

`repo-sweep` reads, in order: the file given with `--config`, `repo-sweep.toml` in the first
folder passed on the command line, the user configuration file
(`~/.config/repo-sweep/config.toml` on Linux, `~/Library/Application Support/repo-sweep/config.toml`
on macOS, `%APPDATA%\repo-sweep\config.toml` on Windows). Without a file the defaults below apply.

```toml
# Servers that must be reachable before starting, as host:port
# (for example a server reachable only through a VPN).
required_hosts = ["git.example.com:4242"]

# "abort" stops before touching any repository; "skip" skips only the repositories
# of the unreachable server.
on_unreachable = "abort"

max_depth = 4             # folder depth searched for repositories
connect_timeout_secs = 5  # reachability check timeout
parallel = 8              # repositories processed at the same time
keep_reports = 30         # reports kept by --report-dir
dev_candidates = ["develop", "feature"]
fallback_branches = ["main", "master"]
exclude = ["node_modules", "target", "dist", ".angular", ".gradle", ".idea", ".venv", "venv", "__pycache__"]

# Written by `detect`; can be edited by hand.
[repos."/home/me/work/backend"]
dev_branch = "develop"
```

---

## 🛠️ Building and releasing

```bash
cargo build --release
cargo test
```

To release, move the `[Unreleased]` notes of `CHANGELOG.md` under the new version, bump
`version` in `Cargo.toml`, commit and push a matching tag (`git tag v0.2.0 && git push origin
v0.2.0`). The `Release` workflow builds the executables and publishes them with the checksums
and the notes of that version.

---

## 📋 Changelog

See [CHANGELOG.md](./CHANGELOG.md) for the changes of every version.

---

## ☕ Support

If this project is useful to you, consider supporting its development:

<a href="https://www.buymeacoffee.com/narra.antonio"><img src="https://img.buymeacoffee.com/button-api/?text=Buy me a coffee&emoji=&slug=narra.antonio&button_colour=5F7FFF&font_colour=ffffff&font_family=Cookie&outline_colour=000000&coffee_colour=FFDD00" /></a>
[![ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/G2G41WWYAM)
[![GitHub Sponsors](https://img.shields.io/badge/GitHub%20Sponsors-EA4AAA?style=flat-square&logo=github-sponsors&logoColor=white)](https://github.com/sponsors/narra-antonio)

---

## 🤝 Contributors

<a href="https://github.com/narra-antonio/repo-sweep/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=narra-antonio/repo-sweep" />
</a>

---

## 📄 License

MIT © [Antonio Narra](https://antonionarra.io)
