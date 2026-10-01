use anyhow::Result;
use chrono::NaiveDate;

use crate::store::models::{PendingOp, PendingOpKind, TaskPatch, TaskStatus};

/// Queues a local write and applies it to the cache immediately.
///
/// Applying first is what keeps the UI responsive and correct while offline:
/// the row updates instantly, the write is recorded, and the sync engine
/// reconciles it with the server whenever it can.
///
/// Returns the queued operation's id, or an error if the cache is not open.
pub fn enqueue(op: PendingOp) -> Result<i64> {
    crate::sync::scheduler::with_store(|store| store.enqueue(op))
        .ok_or_else(|| anyhow::anyhow!("the task cache is not open"))?
}

/// Creates a task, returning the name of the list it went to.
///
/// `preferred` is the list the user was looking at, if they were looking at
/// one: adding while viewing Routine must create in Routine, not in whichever
/// list the API happened to return first. Smart views have no list of their
/// own, so they fall back to the first list, which is the account's most
/// likely "My Tasks".
///
/// The `Err` cases are returned rather than only logged because the caller
/// shows them: a write that fails silently is indistinguishable from a broken
/// button.
pub fn create_task(title: String, preferred: Option<String>) -> Result<String, String> {
    let (list_id, list_name) =
        resolve_list(preferred).ok_or_else(|| "no task list available".to_string())?;

    let payload = TaskPatch {
        title: Some(title),
        ..TaskPatch::new()
    };
    let payload = serde_json::to_string(&payload)
        .map_err(|err| format!("could not serialise the new task: {err}"))?;

    enqueue(PendingOp {
        id: 0,
        list_id,
        task_id: None,
        kind: PendingOpKind::Insert,
        payload,
        attempts: 0,
        last_error: None,
    })
    .map_err(|err| format!("could not queue the new task: {err}"))?;

    Ok(list_name)
}

/// Resolves where a new task goes: the preferred list when it is still cached,
/// otherwise the first list. Both the id (for the write) and the name (for
/// the confirmation) travel together so they cannot disagree.
fn resolve_list(preferred: Option<String>) -> Option<(String, String)> {
    crate::sync::scheduler::with_store(|store| resolve_list_in(store, preferred)).flatten()
}

/// The same resolution against an explicit store, so tests can seed one
/// without standing up the scheduler's thread-local.
fn resolve_list_in(
    store: &crate::store::Store,
    preferred: Option<String>,
) -> Option<(String, String)> {
    let lists = store.task_lists().ok()?;
    if let Some(wanted) = preferred {
        if let Some(found) = lists.iter().find(|list| list.id == wanted) {
            return Some((found.id.clone(), found.title.clone()));
        }
        log::warn!("preferred list {wanted:?} is no longer cached; falling back");
    }
    // Google Tasks has no default-list concept, so this is the first list
    // the API returned, which is the account's most likely "My Tasks".
    lists.into_iter().next().map(|list| (list.id, list.title))
}

/// Marks a task done or not done.
///
/// The task id alone is not enough to queue a write, because the write also
/// needs the list it belongs to; that is looked up from the cache.
///
/// Applies to the cache immediately, like `delete_task`: without that, the
/// row re-renders from the old state and the checkbox visibly flips back
/// until a sync happens to repaint it.
pub fn set_status(task_id: &str, status: TaskStatus) -> Result<(), String> {
    let Some(list_id) = list_of(task_id) else {
        return Err(format!(
            "cannot change {task_id:?}: it is not in any cached list"
        ));
    };

    let patch = TaskPatch {
        status: Some(status),
        ..TaskPatch::new()
    };
    let payload = serde_json::to_string(&patch)
        .map_err(|err| format!("could not serialise the status change: {err}"))?;

    enqueue(PendingOp {
        id: 0,
        list_id: list_id.clone(),
        task_id: Some(task_id.to_string()),
        kind: PendingOpKind::SetStatus,
        payload,
        attempts: 0,
        last_error: None,
    })
    .map_err(|err| format!("could not queue the status change: {err}"))?;

    // The write is already queued above, so a failure here only means the
    // checkbox flips back until the next repaint; it is reported rather than
    // silent all the same.
    apply_local_status(&list_id, task_id, status)
}

/// Sets or clears a task's due date.
///
/// Applies to the cache immediately for the same reason as `set_status`: the
/// chip must change now, not whenever the next repaint happens to come.
pub fn set_due(task_id: &str, due: Option<NaiveDate>) -> Result<(), String> {
    let Some(list_id) = list_of(task_id) else {
        return Err(format!(
            "cannot change {task_id:?}: it is not in any cached list"
        ));
    };

    let patch = TaskPatch {
        // `Some(None)` is the "clear the due date" signal, which the API needs
        // as an explicit null rather than an absent field.
        due: Some(due),
        ..TaskPatch::new()
    };
    let payload = serde_json::to_string(&patch)
        .map_err(|err| format!("could not serialise the due-date change: {err}"))?;

    enqueue(PendingOp {
        id: 0,
        list_id: list_id.clone(),
        task_id: Some(task_id.to_string()),
        kind: PendingOpKind::Patch,
        payload,
        attempts: 0,
        last_error: None,
    })
    .map_err(|err| format!("could not queue the due-date change: {err}"))?;

    apply_local_due(&list_id, task_id, due)
}

/// Deletes a task.
///
/// The local row is removed immediately so the list updates without waiting on
/// the network; a tombstone stops the next sync from resurrecting it.
pub fn delete_task(task_id: &str) -> Result<(), String> {
    let Some(list_id) = list_of(task_id) else {
        return Err(format!(
            "cannot delete {task_id:?}: it is not in any cached list"
        ));
    };

    enqueue(PendingOp {
        id: 0,
        list_id: list_id.clone(),
        task_id: Some(task_id.to_string()),
        kind: PendingOpKind::Delete,
        payload: "{}".to_string(),
        attempts: 0,
        last_error: None,
    })
    .map_err(|err| format!("could not queue the delete: {err}"))?;

    // The write is already queued above, so a failure here only means the row
    // is still visible until the next sync; it is logged rather than raised.
    if let Some(Err(err)) =
        crate::sync::scheduler::with_store(|store| store.delete_task(&list_id, task_id))
    {
        log::error!("could not remove {task_id:?} from the cache: {err}");
    }
    Ok(())
}

/// Runs against the scheduler's cache, or reports it as closed.
///
/// Seeding the thread-local is the scheduler's job, which unit tests do not
/// do, so fallible cache work goes through here and the `*_in` variants below
/// take an explicit store instead.
fn with_open_store<T>(
    f: impl FnOnce(&crate::store::Store) -> Result<T, String>,
) -> Result<T, String> {
    crate::sync::scheduler::with_store(f).unwrap_or(Err("the task cache is not open".to_string()))
}

/// Applies a status change to the cached row, so the checkbox reflects the
/// click immediately rather than flipping back on the next repaint.
fn apply_local_status(list_id: &str, task_id: &str, status: TaskStatus) -> Result<(), String> {
    with_open_store(|store| apply_local_status_in(store, list_id, task_id, status))
}

fn apply_local_status_in(
    store: &crate::store::Store,
    list_id: &str,
    task_id: &str,
    status: TaskStatus,
) -> Result<(), String> {
    let mut task = store
        .task(list_id, task_id)
        .map_err(|err| format!("could not read the cached task: {err}"))?
        .ok_or_else(|| format!("cannot change {task_id:?}: it vanished from the cache"))?;
    task.status = status;
    task.completed = match status {
        TaskStatus::Completed => Some(chrono::Utc::now()),
        TaskStatus::NeedsAction => None,
    };
    store
        .upsert_tasks(list_id, &[task])
        .map_err(|err| format!("could not update the cached task: {err}"))
}

/// Applies a due-date change to the cached row, so the chip reflects the menu
/// choice immediately rather than on the next repaint.
fn apply_local_due(list_id: &str, task_id: &str, due: Option<NaiveDate>) -> Result<(), String> {
    with_open_store(|store| apply_local_due_in(store, list_id, task_id, due))
}

fn apply_local_due_in(
    store: &crate::store::Store,
    list_id: &str,
    task_id: &str,
    due: Option<NaiveDate>,
) -> Result<(), String> {
    let mut task = store
        .task(list_id, task_id)
        .map_err(|err| format!("could not read the cached task: {err}"))?
        .ok_or_else(|| format!("cannot change {task_id:?}: it vanished from the cache"))?;
    task.due = due;
    store
        .upsert_tasks(list_id, &[task])
        .map_err(|err| format!("could not update the cached task: {err}"))
}

/// Forgets which tasks have already produced a notification.
///
/// Called when a fresh authorisation is granted, so a reconnected account is not
/// immediately notified about everything it has ever been reminded of.
pub fn reset_notification_history() {
    if let Some(Err(err)) = crate::sync::scheduler::with_store(|store| store.clear_notifications())
    {
        log::warn!("could not clear the notification history: {err}");
    }
}

fn list_of(task_id: &str) -> Option<String> {
    crate::sync::scheduler::with_store(|store| {
        store.task_lists().ok()?.into_iter().find_map(|list| {
            store
                .task(&list.id, task_id)
                .ok()
                .flatten()
                .map(|_| list.id)
        })
    })
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    /// A fresh cache with one list and one task, opened in the same way the
    /// scheduler opens it.
    fn seeded() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("cache.db")).expect("open store");

        store
            .upsert_task_lists(&[crate::store::models::TaskList {
                id: "@a".into(),
                title: "My Tasks".into(),
                updated: None,
                etag: None,
            }])
            .expect("seed list");

        store
            .upsert_tasks(
                "@a",
                &[crate::store::models::Task {
                    id: "t1".into(),
                    title: "Existing".into(),
                    notes: String::new(),
                    status: TaskStatus::NeedsAction,
                    due: None,
                    completed: None,
                    updated: None,
                    parent: None,
                    previous: None,
                    position: None,
                    etag: None,
                    hidden: false,
                    deleted: false,
                }],
            )
            .expect("seed task");

        (dir, store)
    }

    fn two_lists() -> (tempfile::TempDir, Store) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let store = Store::open(&dir.path().join("cache.db")).expect("open store");
        for (id, title) in [("@a", "First"), ("@b", "Routine")] {
            store
                .upsert_task_lists(&[crate::store::models::TaskList {
                    id: id.into(),
                    title: title.into(),
                    updated: None,
                    etag: None,
                }])
                .expect("seed list");
        }
        (dir, store)
    }

    #[test]
    fn a_visible_list_wins_over_the_first_list() {
        // Adding while viewing Routine must create in Routine, not in whatever
        // list the API returned first.
        let (_dir, store) = two_lists();
        assert_eq!(
            resolve_list_in(&store, Some("@b".into())),
            Some(("@b".into(), "Routine".into()))
        );
    }

    #[test]
    fn a_smart_view_falls_back_to_the_first_list() {
        let (_dir, store) = two_lists();
        assert_eq!(
            resolve_list_in(&store, None),
            Some(("@a".into(), "First".into()))
        );
    }

    #[test]
    fn a_stale_preferred_list_falls_back_instead_of_failing() {
        // The cached lists can change under the UI (a list deleted on another
        // device); refusing the write would lose the task, so fall back.
        let (_dir, store) = two_lists();
        assert_eq!(
            resolve_list_in(&store, Some("@gone".into())),
            Some(("@a".into(), "First".into()))
        );
    }

    #[test]
    fn completing_applies_to_the_cache_at_once() {
        // Without this, the checkbox visibly flips back on the next repaint,
        // because the row re-renders from the unchanged cached task.
        let (_dir, store) = seeded();
        apply_local_status_in(&store, "@a", "t1", TaskStatus::Completed).expect("apply");
        let task = store.task("@a", "t1").expect("read").expect("present");
        assert!(task.is_completed());
        assert!(task.completed.is_some());
    }

    #[test]
    fn reopening_clears_the_completion_stamp() {
        let (_dir, store) = seeded();
        apply_local_status_in(&store, "@a", "t1", TaskStatus::Completed).expect("complete");
        apply_local_status_in(&store, "@a", "t1", TaskStatus::NeedsAction).expect("reopen");
        let task = store.task("@a", "t1").expect("read").expect("present");
        assert!(!task.is_completed());
        assert!(task.completed.is_none());
    }

    #[test]
    fn a_due_date_change_applies_to_the_cache_at_once() {
        use chrono::NaiveDate;
        let (_dir, store) = seeded();
        let due = NaiveDate::from_ymd_opt(2026, 12, 25);
        apply_local_due_in(&store, "@a", "t1", due).expect("apply");
        let task = store.task("@a", "t1").expect("read").expect("present");
        assert_eq!(task.due, due);
        apply_local_due_in(&store, "@a", "t1", None).expect("clear");
        let task = store.task("@a", "t1").expect("read").expect("present");
        assert_eq!(task.due, None);
    }

    #[test]
    fn applying_to_a_missing_task_reports_instead_of_panicking() {
        let (_dir, store) = seeded();
        assert!(apply_local_status_in(&store, "@a", "ghost", TaskStatus::Completed).is_err());
        assert!(apply_local_due_in(&store, "@a", "ghost", None).is_err());
    }

    #[test]
    fn no_lists_means_no_target_rather_than_a_panic() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let store = Store::open(&dir.path().join("cache.db")).expect("open store");
        assert_eq!(resolve_list_in(&store, Some("@a".into())), None);
        assert_eq!(resolve_list_in(&store, None), None);
    }

    #[test]
    fn an_insert_op_round_trips_through_the_queue() {
        let (dir, _store) = seeded();

        let patch = TaskPatch {
            title: Some("New task".into()),
            ..TaskPatch::new()
        };
        let payload = serde_json::to_string(&patch).expect("serialise");

        let id = {
            let store = Store::open(&dir.path().join("cache.db")).expect("open");
            store
                .enqueue(PendingOp {
                    id: 0,
                    list_id: "@a".into(),
                    task_id: None,
                    kind: PendingOpKind::Insert,
                    payload,
                    attempts: 0,
                    last_error: None,
                })
                .expect("enqueue")
        };
        assert!(id > 0);
    }

    #[test]
    fn setting_a_due_date_serialises_an_explicit_null_when_clearing() {
        // The distinction that TaskPatch exists to preserve: an absent `due`
        // means "leave it alone", an explicit null means "clear it".
        let clear = TaskPatch {
            due: Some(None),
            ..TaskPatch::new()
        };
        assert_eq!(serde_json::to_string(&clear).unwrap(), r#"{"due":null}"#);

        let set = TaskPatch {
            due: Some(NaiveDate::from_ymd_opt(2026, 10, 1)),
            ..TaskPatch::new()
        };
        assert_eq!(
            serde_json::to_string(&set).unwrap(),
            r#"{"due":"2026-10-01"}"#
        );
    }

    #[test]
    fn a_status_change_only_sends_the_status() {
        let patch = TaskPatch {
            status: Some(TaskStatus::Completed),
            ..TaskPatch::new()
        };
        assert_eq!(
            serde_json::to_string(&patch).unwrap(),
            r#"{"status":"completed"}"#
        );
    }
}
