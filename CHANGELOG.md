# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Commits follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/).

## [Unreleased]

### Added

- **Google account connection.** OAuth 2.0 with PKCE over a loopback
  `127.0.0.1` redirect, because Google offers no device-code grant for the Tasks
  API. The CSRF state is validated before the authorisation code is used, and
  the listener never binds anything but loopback.
- **Keyring-backed credentials.** The client id, client secret and refresh token
  live in the desktop keyring, never in a config file and never in this
  repository. `GTASKBAR_CLIENT_ID` and `GTASKBAR_CLIENT_SECRET` take precedence
  so a developer, and CI, need no keyring.
- **Local cache.** SQLite in WAL mode at `~/.local/share/gtaskbar/cache.db`:
  `task_lists`, `tasks`, `tombstones`, `pending_ops`, `sync_state` and
  `notified`, with `user_version`-based migrations. The UI reads from it and
  never performs network I/O, so it renders instantly and works offline.
- **Sync engine.** Delta sync per list via `updatedMin`, full resync on
  `410 Gone`, pagination followed to exhaustion, `If-Match` writes with one
  rebase-and-retry on `412`, exponential backoff honouring `Retry-After`, and a
  query counter against the API's 50,000/day courtesy limit.
- **Offline write queue.** Local changes apply to the cache immediately and are
  replayed in order, so a create-then-rename while offline does not orphan the
  rename.
- **Smart views.** Today, next 7 days, overdue, all tasks and completed, each
  with a live count, plus one entry per account task list.
- **Sorting.** Manual (Google's own ordering), due date, alphabetical, created,
  updated, and a locally derived priority. Undated tasks always sort last, and
  a task with no deadline never outranks one that is due.
- **Grouping.** By task list, by due bucket, by status, or none, with empty
  sections omitted and section headers hidden when their tasks are filtered out.
- **Task rows.** Checkbox, notes preview, a due chip coloured for overdue and
  today, subtask indentation, and a context menu for due dates and delete.
- **Search.** A real search entry filtering titles and notes as you type.
- **Quick add.** An inline entry above the list rather than a dialog, because
  adding a task is the most frequent action in the app.
- **Notifications.** One per batch for tasks that become due and for tasks that
  go overdue, each with a **Complete** button per task. Deduplicated through the
  `notified` table so a task that stays due does not re-notify.
- **Official GNOME interface icons.** Every icon in the interface is an
  `adwaita-icon-theme` icon referenced by name and resolved from the system icon
  theme, so the app follows the user's chosen theme, high-contrast variants and
  custom icon sets. See [docs/icons.md](docs/icons.md).
- CI: fmt, clippy with `-D warnings`, tests, a release build, an Arch container
  job, and a job pinned to the documented minimum GTK and libadwaita versions.
- `install.sh` with `--no-build` and `--uninstall`, installing into `~/.local`
  along with the desktop entry, the full icon set and an autostart entry.

### Fixed

- A task could be shown in the Completed view while still being open.
- Sidebar membership was inferred from parent links rather than letting the
  store scope the query, which put every task in every list.
- A hand-authored symbolic icon set rendered as solid blobs, and its thin-line
  icons were invisible on a dark background, because GTK neither preserves
  `fill="none"` nor reads `currentColor`. Replaced with upstream GNOME icons,
  which is both the idiomatic choice for a libadwaita app and one that cannot
  hit those constraints at all.

### Notes

- Google Tasks has no CalDAV or VTODO endpoint, which is why this app exists
  and why it talks to the Google Tasks API directly.
- The API discards the time portion of a task's due date and exposes no
  recurrence, priority, tag or reminder fields. Priority is therefore derived
  locally, and the notify hour is a single global setting rather than a
  per-task field. See [docs/architecture.md](docs/architecture.md).
