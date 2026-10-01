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

- **Tray icon.** A StatusNotifierItem via `ksni`, which is pure Rust over D-Bus;
  the `tray-icon` crate's `libappindicator` feature needs a package that is not
  installed here. The icon is a themed symbolic name rather than a pixmap, so it
  is recoloured by the shell. It carries an attention state and a tooltip while
  tasks are due today or overdue, and re-registers itself if the shell drops it.
- **Close-to-tray that degrades correctly.** Closing the window hides to the tray
  only when a tray is actually registered, so a session with no
  status-notifier host quits instead of becoming unreachable.
- **Ctrl+W** closes the window through the same `close-request` path as the
  window button, so it also respects the hide-or-quit decision.
- **`--hidden`**, which the autostart entry has always assumed and which the app
  never implemented. A hidden start builds no window at all, so autostart gives
  the tray icon and the background sync without a window appearing at every
  login. A later launch from the app picker hands over to the same process
  through single-instance semantics and opens the window as normal.

### Fixed

- **Quick-add created into the wrong list.** The visible pane was found by
  visibility flag, but `AdwViewStack` unmaps hidden pages without clearing
  their flag, so every pane reported visible and the lookup always returned
  the first one (Today, which has no list), falling back to the first account
  list whatever was viewed. The lookup now matches on mapped pages, verified
  live by selecting the Routine row and asking which pane answers.
- **List badges showed the account total on every list.** Each row counted all
  open tasks instead of its own, so the empty Routine list wore Anis Nep's "3"
  while showing "Nothing here". Counts are now computed per list, and an empty
  list shows no badge.
- **Quick-add did nothing on Enter.** The row's `apply` signal never fires on
  this system's libadwaita even though the key reaches the widget, so the
  handler never ran, the text stayed, and nothing happened. Return and keypad
  Enter are now handled by a key controller on the row itself, kept alongside
  `apply` with a per-press yield so a working `apply` never double-submits.
- **Quick-add created into the wrong list, silently and eventually.** It always
  used the first list rather than the visible one, gave no feedback, and
  triggered no sync, so a task sat invisible for up to five minutes even when
  everything worked, and longer when it did not. It now targets the visible
  list, confirms with a toast, syncs immediately, keeps the text on failure,
  and reports the reason instead of only logging it.
- **Synced changes never repainted.** A sync completion updated the tray and
  cleared the status but never rebuilt the list, so flushed quick-adds, remote
  edits, and checkbox toggles stayed invisible until something else rebuilt.
  Completions now repaint when they changed or pushed anything, skipping only
  while quick-add holds unsubmitted text so a background sync cannot eat what
  is being typed.
- **Checkbox toggles and due-date changes flipped back.** They queued the write
  without applying it locally, so the next repaint re-rendered the old state.
  Like deletes already did, they now apply to the cache at once, repaint, sync,
  and toast on failure.
- **The app logs to a file.** Picker launches have no terminal, which is why
  every one of these failures was invisible. Records now also append to
  `gtaskbar.log` in the data directory, with one rotated generation.

- **Signing in crashed the app, so the browser's callback landed on a dead
  port.** Three defects stacked. First, the token exchange read the OAuth
  credentials out of the keyring from inside the Tokio runtime driving it, and
  secret-service builds and `block_on`s a runtime of its own, which aborts with
  "Cannot start a runtime from within a runtime". The credentials are now read
  once, on the main thread, and travel with the request. Second, the release
  profile set `panic = "abort"`, so that worker-thread panic killed the whole
  process: window, tray icon, and the loopback listener the browser was about
  to call back into, which is what produced `ERR_CONNECTION_REFUSED`. A
  background failure is now confined to its thread and the button reports it.
  Third, the listener was dropped after binding and re-bound later, leaving a
  window with nothing listening; it is now held open for the whole flow.
- **Signing in still crashed the app after the browser said success, and the
  account was lost with it.** The startup token refresh read the keyring from
  inside the sync runtime and died there, which poisoned the backend's
  process-global lock; the persist that followed then panicked on the poisoned
  mutex on the main thread, inside a GTK callback that cannot unwind, so the
  whole app aborted after the tokens had arrived but before they were stored.
  No async code touches the keyring anymore: the refresh takes its values as
  parameters, the restore reads them before spawning, persist runs only on the
  main thread, and any keyring call attempted from inside a runtime is refused
  with an ordinary error instead of poisoning. The restore now also triggers
  the sync it previously left for the next poll interval.
- **The first sync after signing in crashed the app instead.** The throttle
  matched on a live `RefCell` borrow and mutated inside one of its arms, which
  panics with "RefCell already borrowed" on the very first request. It had never
  fired only because nothing had reached it before. The decision is now a small
  tested function.
- **The connect dialog had two close buttons.** A custom one was packed next to
  the header bar's native window controls. It now relies on the native ones,
  which go through the same close path as everywhere else.
- **The "Connect Google account" button did nothing.** `adw::Application::default()`
  looks like the running application and is not: the parent type has both an
  inherent `default()` (the real getter) and a `Default` impl that constructs a
  brand-new unregistered object, and on the subtype the path resolves to the
  latter, so every window lookup on it quietly matched nothing. All lookups go
  through a `running_app()` helper instead, which also fixes the post-sign-in
  refresh: a successful authorisation used to leave the "Not connected" page on
  screen and request no sync.
- **Launching from the app picker did nothing, because there were two identical
  "GTaskbar" entries and the broken one was as easy to click as the working one.**
  The autostart entry was `NoDisplay=false`, so the session's copy of it was
  listed in the launcher next to the real one. Its `Exec` passed `--hidden`,
  which the app never declared, and GApplication parses `argv` before any app
  code runs and exits 1 on an unknown flag, so the process died silently and the
  launcher reported nothing. The autostart entry is now `NoDisplay=true` and the
  flag is declared. A hidden start also needs an explicit `hold`, since
  GApplication ends `run` as soon as there is no window and the process would
  otherwise exit straight after starting.
- **The app could not be launched from the app picker or started at login, at
  all.** Both desktop entries used a bare `Exec=gtaskbar`, which the spec
  resolves through `$PATH`. `~/.local/bin` is on the `PATH` of an interactive
  shell but not of a graphical session, so Noctalia's exec failed with ENOENT and
  reported nothing, and XDG autostart resolved it the same way, so the app never
  started at login and no tray icon appeared. `install.sh` now substitutes an
  absolute path into both entries and refuses to install one that does not end up
  absolute and executable. The release tarball carries `install.sh` and no longer
  documents a hand-written `install` of the desktop file, which could not
  substitute the placeholder and is where the bare name came from.
- **Both entries dropped `%U` from `Exec`.** The app declares no `MimeType` and
  does not set `HANDLES_OPEN`, so GIO treats the substituted URL as a file to
  open and aborts with "This application can not open files".
- **OAuth failed outright with "Missing required parameter: redirect_uri".**
  `redirect_uri` was never set on the authorize request, so Google rejected it
  before showing the consent screen. It is now also held on the pending flow
  rather than re-derived from the listener, so the value sent to the token
  endpoint is byte-for-byte the one sent to the authorize endpoint.
- **Notification action buttons would not have worked.** GIO routes notification
  actions by `app.`-prefixed name and logs a warning for anything else; the
  Complete button used an unprefixed name, which looks correct and silently does
  nothing. The task id is now passed as the action's target value.
- **Sidebar count badges and due chips filled their whole row.** A label in a
  box fills it by default, and `AdwActionRow`'s suffix area fills its child
  vertically regardless of the child's own `valign`, so neither the stylesheet
  nor an explicit height request could shrink them. The sidebar row is now
  composed explicitly instead of using `AdwActionRow`, which the suffix area was
  the only reason for. Both are substantially smaller than before.
- A task could be shown in the Completed view while still being open.
- Sidebar membership was inferred from parent links rather than letting the
  store scope the query, which put every task in every list.
- **OAuth failed outright with "Missing required parameter: redirect_uri".**
  `redirect_uri` was never set on the authorize request, so Google rejected it
  before showing the consent screen. The redirect URI is now also held on the
  pending flow rather than re-derived, so the value sent to the token endpoint
  is byte-for-byte the one sent to the authorize endpoint. Covered by a test
  that asserts every parameter Google requires, and which runs in CI.
- **Notification action buttons would not have worked.** GIO routes notification
  actions by `app.`-prefixed name and logs a warning for anything else; the
  Complete button used an unprefixed name, which looks correct and silently does
  nothing. The task id is now passed as the action's target value.
- **Sidebar count badges and due chips filled their whole row.** A label in a box
  fills it by default, and `AdwActionRow`'s suffix area fills its child
  vertically regardless of the child's own `valign`, so neither styling nor an
  explicit height request could shrink them. The sidebar row is now composed
  explicitly instead of using `AdwActionRow`, which the suffix area was the only
  reason for. Both elements are also substantially smaller than before.
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
