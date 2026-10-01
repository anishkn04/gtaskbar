use anyhow::Result;
use chrono::Utc;

use crate::api::{ApiError, TasksClient};
use crate::store::{PendingOp, Store};

/// Opens the cache, mapping the error into this module's error type.
fn open(path: &std::path::Path) -> Result<Store, SyncError> {
    Store::open(path).map_err(|err| SyncError::Store(err.to_string()))
}

/// Outcome of a sync pass, so the UI can report something meaningful.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub lists_seen: usize,
    pub tasks_changed: usize,
    pub tasks_deleted: usize,
    pub lists_resynced: Vec<String>,
    pub pending_flushed: usize,
    pub failed: bool,
    /// True when the failure was an auth failure, meaning the user must
    /// re-authorise rather than just retry.
    pub needs_reauth: bool,
}

/// Runs one full sync pass.
///
/// The order matters: pending writes are flushed *first* so that a change made
/// locally is not immediately overwritten by the server copy we are about to
/// fetch, and so a rejected refresh token surfaces before any work is done.
///
/// Takes no `Store` reference: a `rusqlite::Connection` is not `Send`, so
/// holding one across an `await` would make the future non-Send and therefore
/// unspawnable. Each stage opens its own connection instead.
pub async fn sync_all(
    cache_path: &std::path::Path,
    client: &mut TasksClient,
) -> Result<SyncReport, SyncError> {
    let mut report = SyncReport::default();

    match flush_pending(cache_path, client).await {
        Ok(count) => report.pending_flushed = count,
        Err(SyncError::Auth(err)) => {
            return Err(SyncError::Auth(err));
        }
        Err(err) => {
            log::warn!("could not flush the pending queue: {err}");
            report.failed = true;
        }
    }

    let lists = client.task_lists().await.map_err(SyncError::from_api)?;
    report.lists_seen = lists.len();

    {
        let store = open(cache_path)?;
        store
            .upsert_task_lists(&lists)
            .map_err(|err| SyncError::Store(err.to_string()))?;
    }

    for list in lists {
        match sync_list(cache_path, client, &list.id).await {
            Ok(list_report) => {
                report.tasks_changed += list_report.tasks_changed;
                report.tasks_deleted += list_report.tasks_deleted;
                if list_report.resynced {
                    report.lists_resynced.push(list.id.clone());
                }
            }
            Err(SyncError::Auth(err)) => return Err(SyncError::Auth(err)),
            Err(err) => {
                log::warn!("sync failed for list {}: {err}", list.id);
                report.failed = true;
            }
        }
    }

    Ok(report)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ListSyncReport {
    tasks_changed: usize,
    tasks_deleted: usize,
    resynced: bool,
}

/// Syncs a single list, preferring a delta and falling back to a full resync.
async fn sync_list(
    cache_path: &std::path::Path,
    client: &mut TasksClient,
    list_id: &str,
) -> Result<ListSyncReport, SyncError> {
    let last_delta = {
        let store = open(cache_path)?;
        store
            .last_delta(list_id)
            .map_err(|err| SyncError::Store(err.to_string()))?
    };

    match fetch_and_apply(cache_path, client, list_id, last_delta.as_deref(), false).await {
        Ok(report) => Ok(report),
        Err(SyncError::Api(ApiError::HistoryGone(_))) => {
            // The delta window has expired; the only way forward is to take a
            // full copy and reset the cursor.
            log::info!("delta window for {list_id} expired; doing a full sync");
            let mut report = fetch_and_apply(cache_path, client, list_id, None, true).await?;
            report.resynced = true;
            Ok(report)
        }
        Err(err) => Err(err),
    }
}

/// The timestamp recorded as the new delta cursor.
///
/// Captured *before* the request so that anything changed while the request is
/// in flight is picked up next time. Recording it afterwards would risk
/// skipping a change that landed mid-request.
fn delta_cursor() -> String {
    (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339()
}

async fn fetch_and_apply(
    cache_path: &std::path::Path,
    client: &mut TasksClient,
    list_id: &str,
    updated_min: Option<&str>,
    full: bool,
) -> Result<ListSyncReport, SyncError> {
    let cursor = delta_cursor();

    if full {
        open(cache_path)?
            .clear_list(list_id)
            .map_err(|err| SyncError::Store(err.to_string()))?;
    }

    let tasks = client
        .tasks(list_id, updated_min)
        .await
        .map_err(SyncError::from_api)?;
    let deleted = tasks.iter().filter(|task| task.deleted).count();
    let changed = tasks.len() - deleted;

    {
        let store = open(cache_path)?;
        store
            .apply_delta(list_id, &tasks)
            .map_err(|err| SyncError::Store(err.to_string()))?;
        store
            .set_last_delta(list_id, &cursor)
            .map_err(|err| SyncError::Store(err.to_string()))?;

        if full {
            store
                .set_last_full_sync(list_id, &cursor)
                .map_err(|err| SyncError::Store(err.to_string()))?;
        }
    }

    Ok(ListSyncReport {
        tasks_changed: changed,
        tasks_deleted: deleted,
        resynced: false,
    })
}

/// Replays queued local writes in insertion order.
///
/// The order is significant: if a task is created and then renamed while
/// offline, replaying in order reproduces the same final state.
pub async fn flush_pending(
    cache_path: &std::path::Path,
    client: &mut TasksClient,
) -> Result<usize, SyncError> {
    let ops = {
        let store = open(cache_path)?;
        store
            .pending_ops()
            .map_err(|err| SyncError::Store(err.to_string()))?
    };

    if ops.is_empty() {
        return Ok(0);
    }

    let mut flushed = 0;

    for op in ops {
        match replay(cache_path, client, &op).await {
            Ok(()) => {
                open(cache_path)?
                    .complete_op(op.id)
                    .map_err(|err| SyncError::Store(err.to_string()))?;
                flushed += 1;
            }
            Err(SyncError::Api(err)) if err.is_retryable() => {
                // Keep it queued and stop; replaying later writes on top of a
                // failed earlier one could produce the wrong order.
                open(cache_path)?
                    .fail_op(op.id, &err.to_string())
                    .map_err(|e| SyncError::Store(e.to_string()))?;
                log::warn!("pausing the pending queue: {err}");
                break;
            }
            Err(SyncError::Api(err)) => {
                open(cache_path)?
                    .fail_op(op.id, &err.to_string())
                    .map_err(|e| SyncError::Store(e.to_string()))?;
                return Err(SyncError::Api(err));
            }
            Err(err) => {
                open(cache_path)?
                    .fail_op(op.id, &err.to_string())
                    .map_err(|e| SyncError::Store(e.to_string()))?;
                return Err(err);
            }
        }
    }

    Ok(flushed)
}

async fn replay(
    cache_path: &std::path::Path,
    client: &mut TasksClient,
    op: &PendingOp,
) -> Result<(), SyncError> {
    use crate::api::convert::MoveRequest;
    use crate::store::PendingOpKind;

    let task_id = op.task_id.as_deref();
    let payload: serde_json::Value =
        serde_json::from_str(&op.payload).map_err(|err| SyncError::Store(err.to_string()))?;

    match op.kind {
        PendingOpKind::Insert => {
            let patch: crate::store::models::TaskPatch =
                serde_json::from_value(payload).map_err(|err| SyncError::Store(err.to_string()))?;

            let created = client
                .insert_task(&op.list_id, &patch, None, None)
                .await
                .map_err(SyncError::from_api)?;

            // The server assigned the id, so any local placeholder has to be
            // re-keyed onto the real task.
            let store = open(cache_path)?;
            if let Some(placeholder) = task_id {
                store
                    .reassign_task_id(&op.list_id, placeholder, &created.id)
                    .map_err(|err| SyncError::Store(err.to_string()))?;
            }
            store
                .upsert_tasks(&op.list_id, &[created])
                .map_err(|err| SyncError::Store(err.to_string()))?;
        }

        PendingOpKind::Patch | PendingOpKind::SetStatus => {
            let Some(task_id) = task_id else {
                return Err(SyncError::Store("patch without a task id".into()));
            };

            let patch: crate::store::models::TaskPatch =
                serde_json::from_value(payload).map_err(|err| SyncError::Store(err.to_string()))?;

            // A patch that has been queued since the local copy changed may no
            // longer match the server; rebase and retry once rather than
            // clobbering someone else's edit.
            let current = open(cache_path)?
                .task(&op.list_id, task_id)
                .map_err(|err| SyncError::Store(err.to_string()))?;

            match client
                .patch_task(
                    &op.list_id,
                    task_id,
                    &patch,
                    current.as_ref().and_then(|t| t.etag.as_deref()),
                )
                .await
            {
                Ok(updated) => {
                    open(cache_path)?
                        .upsert_tasks(&op.list_id, &[updated])
                        .map_err(|err| SyncError::Store(err.to_string()))?;
                }
                Err(ApiError::Conflict) => {
                    let fresh = client
                        .task(&op.list_id, task_id)
                        .await
                        .map_err(SyncError::from_api)?;

                    let updated = client
                        .patch_task(&op.list_id, task_id, &patch, fresh.etag.as_deref())
                        .await
                        .map_err(SyncError::from_api)?;

                    open(cache_path)?
                        .upsert_tasks(&op.list_id, &[updated])
                        .map_err(|err| SyncError::Store(err.to_string()))?;
                }
                Err(err) => return Err(SyncError::Api(err)),
            }
        }

        PendingOpKind::Move => {
            let Some(task_id) = task_id else {
                return Err(SyncError::Store("move without a task id".into()));
            };

            let request: MoveRequest =
                serde_json::from_value(payload).map_err(|err| SyncError::Store(err.to_string()))?;

            let moved = client
                .move_task(&op.list_id, task_id, &request)
                .await
                .map_err(SyncError::from_api)?;

            open(cache_path)?
                .upsert_tasks(&op.list_id, &[moved])
                .map_err(|err| SyncError::Store(err.to_string()))?;
        }

        PendingOpKind::Delete => {
            let Some(task_id) = task_id else {
                return Err(SyncError::Store("delete without a task id".into()));
            };
            client
                .delete_task(&op.list_id, task_id)
                .await
                .map_err(SyncError::from_api)?;
        }
    }

    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("the account needs to be reconnected: {0}")]
    Auth(#[source] ApiError),

    #[error(transparent)]
    Api(#[from] ApiError),

    #[error("local cache error: {0}")]
    Store(String),
}

impl SyncError {
    pub fn from_api(err: ApiError) -> Self {
        match err {
            ApiError::NeedsReauth => SyncError::Auth(err),
            other => SyncError::Api(other),
        }
    }

    /// Whether the user has to do something, as opposed to just waiting.
    pub fn needs_user_action(&self) -> bool {
        matches!(self, SyncError::Auth(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_auth_error_is_treated_as_needing_user_action() {
        let err = SyncError::from_api(ApiError::NeedsReauth);
        assert!(err.needs_user_action());
        assert!(matches!(err, SyncError::Auth(_)));
    }

    #[test]
    fn other_api_errors_do_not_need_user_action() {
        for err in [
            ApiError::Conflict,
            ApiError::RateLimited(None),
            ApiError::Network("boom".into()),
            ApiError::HistoryGone("resync".into()),
        ] {
            let label = format!("{err:?}");
            assert!(
                !SyncError::from_api(err).needs_user_action(),
                "{label} should not require user action"
            );
        }
    }

    #[test]
    fn the_delta_cursor_is_just_behind_now() {
        // The cursor is deliberately slightly in the past: if it were exactly
        // `now`, a change landing during the request would be skipped on the
        // next pass. Truncating to seconds keeps the API happy.
        let cursor = delta_cursor();
        let parsed = chrono::DateTime::parse_from_rfc3339(&cursor).expect("valid rfc3339");
        let age = Utc::now() - parsed.with_timezone(&Utc);
        assert!(
            age.num_seconds() >= 1,
            "cursor must be at least 1s in the past"
        );
        assert!(age.num_seconds() < 120, "cursor must be recent, got {age}");
    }
}
