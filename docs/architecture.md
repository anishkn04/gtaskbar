# Architecture

Design notes for gtaskbar. Kept in-repo so the reasoning behind the sync
engine, the local schema and the API workarounds is discoverable.

## Layering

```
ui/          GTK4 + libadwaita widgets. Talks to sync/, never to api/ directly.
sync/        Orchestration: when to sync, what to replay, how to back off.
api/         Google Tasks REST client. The only place that knows HTTP details.
store/       SQLite cache. The UI's source of truth; api/ is the authority.
auth/        OAuth loopback flow and keyring-backed token storage.
model/       View models: sort, group, smart views, priority heuristic.
config.rs    User preferences, serialised to TOML.
```

The one rule that keeps this honest: **`ui/` never performs network I/O.** It
reads from `store/` and issues intents (`sync::queue::complete_task`). That
keeps the UI instant and correct while offline, and means the API client can be
tested without a display.

## Threading model

The GTK main loop owns the widgets. `reqwest` runs on a Tokio runtime on a
background thread; results are handed back to the main thread through
`glib::MainContext::spawn_local`, so no widget is ever touched off-thread.

## Local schema

`~/.local/share/gtaskbar/cache.db`, SQLite in WAL mode.

| Table | Purpose |
| --- | --- |
| `task_lists` | Mirror of the server's task lists, including `etag` and `updated`. |
| `tasks` | Mirror of tasks, keyed by `(list_id, id)`. Carries `etag` for conditional writes. |
| `tombstones` | Tasks deleted elsewhere, kept until every list has been delta-synced past the deletion. |
| `pending_ops` | Locally requested writes awaiting acknowledgement, with attempt count and last error. |
| `sync_state` | Per-list `last_delta_ts` and API quota counters. |
| `notified` | Which tasks have already produced a notification, to avoid duplicates. |

Tombstones exist because the API's delta sync reports deletions as ordinary
tasks with `deleted=true` and a `hidden` flag. They must be recorded and
applied, then pruned once a full sync has passed.

## Sync strategy

**Full sync** on first run, on demand, and whenever a list's `updated` field
advances unexpectedly.

**Delta sync** otherwise, per list:

```
tasks.list?tasklist={id}
          &showCompleted=true
          &showHidden=true
          &showDeleted=true
          &updatedMin={last_delta_ts}
          &maxResults=100
```

Note that `showCompleted` has no effect unless `showHidden=true` as well. The
default `maxResults` is 20, so pagination must always be followed; the client
loops on `nextPageToken`.

A `410 Gone` from a delta means the `updatedMin` window is too old and the
caller must fall back to a full sync of that list.

## Write path

1. The UI calls `sync::queue::*`.
2. The write is applied to SQLite **immediately** so the UI updates without waiting on the network.
3. A row is appended to `pending_ops`.
4. The scheduler drains the queue, performing the API call with `If-Match: {etag}`.
5. On success the row is removed and the server's `etag`/`updated` recorded.
6. On `412 Precondition Failed` the task is refetched, the local change is
   replayed on top of the fresh copy, and the write is retried once. Two
   failures in a row mean a genuine conflict, which is surfaced in the UI
   rather than retried forever.
7. On `429` or `5xx` the attempt count increments and the row is retried with
   exponential backoff, honouring `Retry-After` when present.

## ETag concurrency

Every writable resource carries an `etag`. Writes send `If-Match`, so a write
based on stale data is rejected by the server rather than silently clobbering a
change made in another client. This is what makes the app safe to run
alongside the Google Tasks web or mobile clients.

## Refresh token rotation

Google issues a **new** refresh token on most exchanges and expects the client
to store it. A discarded old token invalidates the session. Every successful
exchange therefore persists the new token to the keyring before any further
request is made, and an `invalid_grant` response is surfaced as a "Reconnect"
prompt rather than retried.

## Priority heuristic

Google Tasks has no priority field. The `Priority` sort mode is therefore
computed locally, scoring each incomplete task:

| Signal | Score |
| --- | --- |
| Overdue | 0 |
| Due today | 1 |
| Due within 3 days | 2 |
| Later or no due date | 3 |

with `+1` each for having notes and for having incomplete subtasks. Ties break
on `updated` descending. Lower scores sort first. The table is exposed in the
app's preferences so the behaviour is documented rather than mysterious.

## Notifications

`notify.rs` ticks once a minute. It fires a `GNotification` for tasks newly due
today (at the user's configured hour) and for tasks that have just passed their
due date, and records what it has notified in the `notified` table.

The API offers no per-task reminder times, so the notify hour is a single
global setting rather than a per-task field.

## Theming

GTaskbar is plain libadwaita and honours `AdwStyleManager`, so it follows the
system light/dark preference and the active GTK theme. The stylesheet in
`data/ui/style.css` never hardcodes a palette: every colour resolves to a
libadwaita named colour, which is what lets the app pick up shell-generated
palettes (Noctalia, Matugen and similar) without any app-side configuration.
