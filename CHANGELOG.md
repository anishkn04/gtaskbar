# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Commits follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/).

## [Unreleased]

### Added

- GTK4 and libadwaita application shell with a split view layout
- Sort modes: manual, due date, alphabetical, created, updated, priority
- Group modes: none, by list, by due date, by status
- Configuration file at `~/.config/gtaskbar/config.toml`
- Preferences dialog for sort, group, notification and tray settings
- Sync scheduler with a rate-limited manual sync path
- GResource pipeline embedding icons, stylesheet and menus into the binary
- `install.sh` for building and installing into `~/.local`, with an
  `--uninstall` counterpart
- Desktop entry, autostart entry and scalable/symbolic icons

### Notes

- Google Tasks has no CalDAV or VTODO endpoint, which is why this app exists
  and why it talks to the Google Tasks API directly.
- The API discards the time portion of a task's due date and exposes no
  recurrence, priority, tag or reminder fields. See
  [docs/architecture.md](docs/architecture.md) for how gtaskbar works around
  each of these.
