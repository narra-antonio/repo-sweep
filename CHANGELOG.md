# Changelog

All notable changes to this project are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.1] - 2026-11-02

### Added

- Linux ARM64 executable (`aarch64-unknown-linux-musl`) in the releases.
- Release notes taken from this changelog.

### Changed

- GitHub Actions updated to their Node.js 24 versions.

## [0.1.0] - 2026-11-02

First release.

### Added

- `check`: finds the git repositories under one or more folders (nested repositories and
  worktrees included) and checks that their servers are reachable. SSH host aliases are resolved
  through the SSH configuration; required servers (for example reachable only through a VPN)
  are declared in the configuration.
- `detect`: finds the development branch of each repository (`develop` or `feature`, the most
  recent one when both exist; otherwise `main`/`master` or the remote default branch) and saves
  it in the configuration file, keeping its comments.
- `sync`: fetches with prune, fast-forwards every local branch that follows a remote branch,
  switches back to the development branch and deletes the local branches whose remote branch is
  gone — only when their work is known to be on the remote or already merged (pushed as is,
  contained in a remote branch, or squash-merged). Uncommitted work is never touched: it is
  reported together with stashes, unpushed and diverged branches.
- Remote errors told apart: server unreachable, remote repository not found or no access,
  authentication failed.
- `sync --dry-run`, `--report FILE` (Markdown report), `--report-dir DIR` (time-stamped reports
  with retention), `--notify` (desktop notification).
- `schedule install|status|remove`: daily or weekly `sync` through launchd (macOS), Task
  Scheduler (Windows) or a systemd user timer (Linux).
- Executables for Windows (x64, ARM64), macOS (Intel, Apple silicon) and Linux (x64, static).

[Unreleased]: https://github.com/narra-antonio/repo-sweep/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/narra-antonio/repo-sweep/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/narra-antonio/repo-sweep/releases/tag/v0.1.0
