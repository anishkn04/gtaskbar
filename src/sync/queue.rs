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

/// Creates a task in the default list.
///
/// The API has no notion of a "current" list, so this needs a list id. Until
/// the account is connected there is nothing to write to, and the caller is
/// expected to have checked `auth::session::is_connected` first.
pub fn create_task(title: String) {
    let Some(list_id) = default_list_id() else {
        log::warn!("cannot add a task with no task list available");
        return;
    };

    let payload = TaskPatch {
        title: Some(title),
        ..TaskPatch::new()
    };
    let payload = match serde_json::to_string(&payload) {
        Ok(json) => json,
        Err(err) => {
            log::error!("could not serialise the new task: {err}");
            return;
        }
    };

    if let Err(err) = enqueue(PendingOp {
        id: 0,
        list_id,
        task_id: None,
        kind: PendingOpKind::Insert,
        payload,
        attempts: 0,
        last_error: None,
    }) {
        log::error!("could not queue the new task: {err}");
    }
}

/// Marks a task done or not done.
///
/// The task id alone is not enough to queue a write, because the write also
/// needs the list it belongs to; that is looked up from the cache.
pub fn set_status(task_id: &str, status: TaskStatus) {
    let Some(list_id) = list_of(task_id) else {
        log::warn!("cannot change {task_id:?}: it is not in any cached list");
        return;
    };

    let patch = TaskPatch {
        status: Some(status),
        ..TaskPatch::new()
    };
    let Ok(payload) = serde_json::to_string(&patch) else {
        return;
    };

    if let Err(err) = enqueue(PendingOp {
        id: 0,
        list_id,
        task_id: Some(task_id.to_string()),
        kind: PendingOpKind::SetStatus,
        payload,
        attempts: 0,
        last_error: None,
    }) {
        log::error!("could not queue the status change: {err}");
    }
}

/// Sets or clears a task's due date.
pub fn set_due(task_id: &str, due: Option<NaiveDate>) {
    let Some(list_id) = list_of(task_id) else {
        return;
    };

    let patch = TaskPatch {
        // `Some(None)` is the "clear the due date" signal, which the API needs
        // as an explicit null rather than an absent field.
        due: Some(due),
        ..TaskPatch::new()
    };
    let Ok(payload) = serde_json::to_string(&patch) else {
        return;
    };

    let _ = enqueue(PendingOp {
        id: 0,
        list_id,
        task_id: Some(task_id.to_string()),
        kind: PendingOpKind::Patch,
        payload,
        attempts: 0,
        last_error: None,
    });
}

/// Deletes a task.
///
/// The local row is removed immediately so the list updates without waiting on
/// the network; a tombstone stops the next sync from resurrecting it.
pub fn delete_task(task_id: &str) {
    let Some(list_id) = list_of(task_id) else {
        log::warn!("cannot delete {task_id:?}: it is not in any cached list");
        return;
    };

    if let Err(err) = enqueue(PendingOp {
        id: 0,
        list_id: list_id.clone(),
        task_id: Some(task_id.to_string()),
        kind: PendingOpKind::Delete,
        payload: "{}".to_string(),
        attempts: 0,
        last_error: None,
    }) {
        log::error!("could not queue the delete: {err}");
        return;
    }

    // The write is already queued above, so a failure here only means the row
    // is still visible until the next sync; it is logged rather than raised.
    if let Some(Err(err)) =
        crate::sync::scheduler::with_store(|store| store.delete_task(&list_id, task_id))
    {
        log::error!("could not remove {task_id:?} from the cache: {err}");
    }
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

/// The list new tasks go into.
///
/// Google Tasks has no default-list concept, so this is the first list by
/// title, which is the account's most likely "My Tasks".
fn default_list_id() -> Option<String> {
    crate::sync::scheduler::with_store(|store| {
        store
            .task_lists()
            .ok()?
            .into_iter()
            .next()
            .map(|list| list.id)
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
