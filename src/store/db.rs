use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use super::models::{PendingOp, PendingOpKind, Task, TaskList, TaskStatus};
use chrono::NaiveDate;

/// Current schema version. Bump this and add a matching step to `migrate` when
/// the schema changes; never edit an existing migration in place.
const SCHEMA_VERSION: i64 = 1;

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating if necessary) the cache database.
    ///
    /// WAL mode plus `synchronous = NORMAL` is the right trade-off here: the
    /// database is a cache of server state, so losing the last few
    /// milliseconds of writes on a hard power loss is acceptable, while the
    /// concurrency is not.
    pub fn open(path: &std::path::Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("opening cache database at {}", path.display()))?;

        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        conn.pragma_update(None, "foreign_keys", "ON").ok();

        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Opens an in-memory database, for tests.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let current: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap_or(0);

        if current == SCHEMA_VERSION {
            return Ok(());
        }

        if current > SCHEMA_VERSION {
            anyhow::bail!(
                "cache database schema is version {current}, but this build only understands {SCHEMA_VERSION}. \
                 Remove {} to rebuild it from scratch.",
                crate::config::data_dir().join("cache.db").display()
            );
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS task_lists (
                id       TEXT PRIMARY KEY,
                title    TEXT NOT NULL,
                updated  TEXT,
                etag     TEXT
            );

            CREATE TABLE IF NOT EXISTS tasks (
                list_id   TEXT NOT NULL,
                id        TEXT NOT NULL,
                title     TEXT NOT NULL,
                notes     TEXT NOT NULL DEFAULT '',
                status    TEXT NOT NULL DEFAULT 'needsAction',
                due       TEXT,
                completed TEXT,
                updated   TEXT,
                parent    TEXT,
                previous  TEXT,
                position  TEXT,
                etag      TEXT,
                hidden    INTEGER NOT NULL DEFAULT 0,
                deleted   INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (list_id, id)
            );

            CREATE INDEX IF NOT EXISTS tasks_by_due
                ON tasks (list_id, due);
            CREATE INDEX IF NOT EXISTS tasks_by_parent
                ON tasks (list_id, parent);

            CREATE TABLE IF NOT EXISTS tombstones (
                list_id    TEXT NOT NULL,
                task_id    TEXT NOT NULL,
                deleted_at TEXT NOT NULL,
                PRIMARY KEY (list_id, task_id)
            );

            CREATE TABLE IF NOT EXISTS pending_ops (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                list_id    TEXT NOT NULL,
                task_id    TEXT,
                kind       TEXT NOT NULL,
                payload    TEXT NOT NULL,
                attempts   INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS sync_state (
                list_id       TEXT PRIMARY KEY,
                last_delta_ts TEXT,
                last_full_sync TEXT,
                last_error    TEXT
            );

            CREATE TABLE IF NOT EXISTS notified (
                list_id   TEXT NOT NULL,
                task_id   TEXT NOT NULL,
                kind      TEXT NOT NULL,
                notified_at TEXT NOT NULL,
                PRIMARY KEY (list_id, task_id, kind)
            );
            "#,
        )?;

        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
    }

    // ---- task lists -------------------------------------------------------

    pub fn upsert_task_lists(&self, lists: &[TaskList]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO task_lists (id, title, updated, etag) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET
                     title = excluded.title,
                     updated = excluded.updated,
                     etag = excluded.etag",
            )?;
            for list in lists {
                stmt.execute(params![
                    list.id,
                    list.title,
                    list.updated.map(|t| t.to_rfc3339()),
                    list.etag,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn task_lists(&self) -> Result<Vec<TaskList>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, updated, etag FROM task_lists ORDER BY title COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(TaskList {
                id: row.get(0)?,
                title: row.get(1)?,
                updated: row
                    .get::<_, Option<String>>(2)?
                    .and_then(|s| s.parse().ok()),
                etag: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete_task_list(&self, list_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM task_lists WHERE id = ?1", params![list_id])?;
        self.conn
            .execute("DELETE FROM tasks WHERE list_id = ?1", params![list_id])?;
        self.conn.execute(
            "DELETE FROM sync_state WHERE list_id = ?1",
            params![list_id],
        )?;
        Ok(())
    }

    // ---- tasks ------------------------------------------------------------

    /// Upserts tasks into a list, replacing any existing row for the same
    /// `(list_id, id)`. The list id is a parameter because `Task` deliberately
    /// does not carry one: the API's task objects have no list field, the list
    /// comes from the request path.
    pub fn upsert_tasks(&self, list_id: &str, tasks: &[Task]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO tasks
                    (list_id, id, title, notes, status, due, completed, updated,
                     parent, previous, position, etag, hidden, deleted)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(list_id, id) DO UPDATE SET
                     title = excluded.title,
                     notes = excluded.notes,
                     status = excluded.status,
                     due = excluded.due,
                     completed = excluded.completed,
                     updated = excluded.updated,
                     parent = excluded.parent,
                     previous = excluded.previous,
                     position = excluded.position,
                     etag = excluded.etag,
                     hidden = excluded.hidden,
                     deleted = excluded.deleted",
            )?;
            for task in tasks {
                stmt.execute(params![
                    list_id,
                    task.id,
                    task.title,
                    task.notes,
                    task.status.as_str(),
                    task.due.map(|d| d.to_string()),
                    task.completed.map(|t| t.to_rfc3339()),
                    task.updated.map(|t| t.to_rfc3339()),
                    task.parent,
                    task.previous,
                    task.position,
                    task.etag,
                    task.hidden as i64,
                    task.deleted as i64,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Reads a single task, or `None` if it is not cached.
    pub fn task(&self, list_id: &str, task_id: &str) -> Result<Option<Task>> {
        let mut stmt = self
            .conn
            .prepare(&format!("{TASK_COLUMNS} WHERE list_id = ?1 AND id = ?2"))?;
        let task = stmt
            .query_row(params![list_id, task_id], row_to_task)
            .optional()?;
        Ok(task)
    }

    /// All tasks in a list, excluding those the API has marked deleted.
    pub fn tasks_in_list(&self, list_id: &str) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(&format!(
            "{TASK_COLUMNS} WHERE list_id = ?1 AND deleted = 0"
        ))?;
        let rows = stmt.query_map(params![list_id], row_to_task)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every non-deleted task, for smart views that span lists.
    pub fn all_tasks(&self) -> Result<Vec<Task>> {
        let mut stmt = self
            .conn
            .prepare(&format!("{TASK_COLUMNS} WHERE deleted = 0"))?;
        let rows = stmt.query_map([], row_to_task)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Removes a task and records a tombstone, so a later full sync does not
    /// resurrect it from the server copy we are about to keep.
    pub fn delete_task(&self, list_id: &str, task_id: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM tasks WHERE list_id = ?1 AND id = ?2",
            params![list_id, task_id],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO tombstones (list_id, task_id, deleted_at)
             VALUES (?1, ?2, datetime('now'))",
            params![list_id, task_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Applies a delta sync result: upserts, converts deletions into tombstones,
    /// and returns the ids the caller should remove from the UI.
    pub fn apply_delta(&self, list_id: &str, tasks: &[Task]) -> Result<()> {
        let mut visible = Vec::with_capacity(tasks.len());
        let mut deleted = Vec::new();

        for task in tasks {
            if task.deleted {
                deleted.push(task.id.clone());
            } else {
                visible.push(task.clone());
            }
        }

        let tx = self.conn.unchecked_transaction()?;

        if !visible.is_empty() {
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO tasks
                        (list_id, id, title, notes, status, due, completed, updated,
                         parent, previous, position, etag, hidden, deleted)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0)
                     ON CONFLICT(list_id, id) DO UPDATE SET
                         title = excluded.title,
                         notes = excluded.notes,
                         status = excluded.status,
                         due = excluded.due,
                         completed = excluded.completed,
                         updated = excluded.updated,
                         parent = excluded.parent,
                         previous = excluded.previous,
                         position = excluded.position,
                         etag = excluded.etag,
                         hidden = excluded.hidden",
                )?;
                for task in &visible {
                    stmt.execute(params![
                        list_id,
                        task.id,
                        task.title,
                        task.notes,
                        task.status.as_str(),
                        task.due.map(|d| d.to_string()),
                        task.completed.map(|t| t.to_rfc3339()),
                        task.updated.map(|t| t.to_rfc3339()),
                        task.parent,
                        task.previous,
                        task.position,
                        task.etag,
                        task.hidden as i64,
                    ])?;
                }
            }
        }

        for task_id in &deleted {
            tx.execute(
                "DELETE FROM tasks WHERE list_id = ?1 AND id = ?2",
                params![list_id, task_id],
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO tombstones (list_id, task_id, deleted_at)
                 VALUES (?1, ?2, datetime('now'))",
                params![list_id, task_id],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Discards a list's local copy, used before a full resync.
    pub fn clear_list(&self, list_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM tasks WHERE list_id = ?1", params![list_id])?;
        Ok(())
    }

    // ---- pending operations ----------------------------------------------

    pub fn enqueue(&self, op: PendingOp) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO pending_ops (list_id, task_id, kind, payload, attempts, last_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                op.list_id,
                op.task_id,
                op.kind.as_str(),
                op.payload,
                op.attempts,
                op.last_error,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Queued operations in insertion order; the write path must be ordered.
    pub fn pending_ops(&self) -> Result<Vec<PendingOp>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, list_id, task_id, kind, payload, attempts, last_error
             FROM pending_ops ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            let kind: String = row.get(3)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                kind,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?;

        let mut ops = Vec::new();
        for row in rows {
            let (id, list_id, task_id, kind, payload, attempts, last_error) = row?;
            let kind = PendingOpKind::parse(&kind)
                .ok_or_else(|| anyhow::anyhow!("pending op {id} has an unknown kind: {kind}"))?;
            ops.push(PendingOp {
                id,
                list_id,
                task_id,
                kind,
                payload,
                attempts: attempts as u32,
                last_error,
            });
        }
        Ok(ops)
    }

    pub fn pending_op_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM pending_ops", [], |row| row.get(0))?)
    }

    pub fn complete_op(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM pending_ops WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn fail_op(&self, id: i64, error: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE pending_ops
             SET attempts = attempts + 1, last_error = ?2
             WHERE id = ?1",
            params![id, error],
        )?;
        Ok(())
    }

    // ---- sync state -------------------------------------------------------

    pub fn last_delta(&self, list_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT last_delta_ts FROM sync_state WHERE list_id = ?1",
                params![list_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten())
    }

    pub fn set_last_delta(&self, list_id: &str, timestamp: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sync_state (list_id, last_delta_ts) VALUES (?1, ?2)
             ON CONFLICT(list_id) DO UPDATE SET last_delta_ts = excluded.last_delta_ts",
            params![list_id, timestamp],
        )?;
        Ok(())
    }

    pub fn set_last_full_sync(&self, list_id: &str, timestamp: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sync_state (list_id, last_full_sync) VALUES (?1, ?2)
             ON CONFLICT(list_id) DO UPDATE SET last_full_sync = excluded.last_full_sync",
            params![list_id, timestamp],
        )?;
        Ok(())
    }

    pub fn clear_sync_state(&self, list_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM sync_state WHERE list_id = ?1",
            params![list_id],
        )?;
        Ok(())
    }

    // ---- notification dedupe ---------------------------------------------

    /// Records that a notification fired. Returns `false` if one already had,
    /// which is the caller's signal to stay quiet.
    pub fn mark_notified(&self, list_id: &str, task_id: &str, kind: &str) -> Result<bool> {
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO notified (list_id, task_id, kind, notified_at)
             VALUES (?1, ?2, ?3, datetime('now'))",
            params![list_id, task_id, kind],
        )?;
        Ok(inserted > 0)
    }

    pub fn clear_notifications(&self) -> Result<()> {
        self.conn.execute("DELETE FROM notified", [])?;
        Ok(())
    }

    /// Wipes everything, used by "Disconnect account".
    pub fn clear_all(&self) -> Result<()> {
        self.conn.execute_batch(
            "DELETE FROM tasks;
             DELETE FROM task_lists;
             DELETE FROM tombstones;
             DELETE FROM pending_ops;
             DELETE FROM sync_state;
             DELETE FROM notified;",
        )?;
        Ok(())
    }

    pub fn cache_path() -> std::path::PathBuf {
        crate::config::data_dir().join("cache.db")
    }
}

const TASK_COLUMNS: &str = "SELECT list_id, id, title, notes, status, due, completed, updated, \
     parent, previous, position, etag, hidden, deleted FROM tasks";

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let status: String = row.get(4)?;
    let due: Option<String> = row.get(5)?;

    Ok(Task {
        id: row.get(1)?,
        title: row.get(2)?,
        notes: row.get(3)?,
        status: if status == "completed" {
            TaskStatus::Completed
        } else {
            TaskStatus::NeedsAction
        },
        due: due
            .as_deref()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()),
        completed: row
            .get::<_, Option<String>>(6)?
            .and_then(|s| s.parse().ok()),
        updated: row
            .get::<_, Option<String>>(7)?
            .and_then(|s| s.parse().ok()),
        parent: row.get(8)?,
        previous: row.get(9)?,
        position: row.get(10)?,
        etag: row.get(11)?,
        hidden: row.get::<_, i64>(12)? != 0,
        deleted: row.get::<_, i64>(13)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn sample_task(id: &str, title: &str) -> Task {
        Task {
            id: id.into(),
            title: title.into(),
            notes: String::new(),
            status: TaskStatus::NeedsAction,
            due: Some(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()),
            completed: None,
            updated: Some(Utc.with_ymd_and_hms(2026, 10, 1, 9, 0, 0).unwrap()),
            parent: None,
            previous: None,
            position: Some("000001".into()),
            etag: Some("\"e1\"".into()),
            hidden: false,
            deleted: false,
        }
    }

    fn store() -> Store {
        Store::open_in_memory().expect("open in-memory store")
    }

    #[test]
    fn task_lists_round_trip() {
        let store = store();
        let lists = vec![TaskList {
            id: "@a".into(),
            title: "My Tasks".into(),
            updated: Some(Utc.with_ymd_and_hms(2026, 10, 1, 9, 0, 0).unwrap()),
            etag: Some("\"l1\"".into()),
        }];

        store.upsert_task_lists(&lists).unwrap();
        let loaded = store.task_lists().unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "@a");
        assert_eq!(loaded[0].title, "My Tasks");
        assert_eq!(loaded[0].etag.as_deref(), Some("\"l1\""));
        assert!(loaded[0].updated.is_some());
    }

    #[test]
    fn upserting_a_list_twice_updates_rather_than_duplicates() {
        let store = store();
        store
            .upsert_task_lists(&[TaskList {
                id: "@a".into(),
                title: "First".into(),
                updated: None,
                etag: None,
            }])
            .unwrap();
        store
            .upsert_task_lists(&[TaskList {
                id: "@a".into(),
                title: "Second".into(),
                updated: None,
                etag: None,
            }])
            .unwrap();

        let loaded = store.task_lists().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].title, "Second");
    }

    #[test]
    fn tasks_round_trip_including_due_and_status() {
        let store = store();
        let mut task = sample_task("t1", "Buy milk");
        task.notes = "Semi-skimmed".into();
        task.status = TaskStatus::Completed;

        store.upsert_tasks("@a", &[task]).unwrap();
        let loaded = store.task("@a", "t1").unwrap().unwrap();

        assert_eq!(loaded.title, "Buy milk");
        assert_eq!(loaded.notes, "Semi-skimmed");
        assert_eq!(loaded.status, TaskStatus::Completed);
        assert_eq!(loaded.due, NaiveDate::from_ymd_opt(2026, 10, 1));
        assert_eq!(loaded.position.as_deref(), Some("000001"));
        assert!(loaded.updated.is_some());
    }

    #[test]
    fn apply_delta_turns_deletions_into_tombstones() {
        let store = store();
        store
            .upsert_tasks(
                "@a",
                &[sample_task("keep", "Keep"), sample_task("gone", "Gone")],
            )
            .unwrap();
        assert_eq!(store.all_tasks().unwrap().len(), 2);

        let mut deleted = sample_task("gone", "Gone");
        deleted.deleted = true;
        let mut kept = sample_task("keep", "Keep renamed");
        kept.title = "Keep renamed".into();

        store.apply_delta("@a", &[deleted, kept]).unwrap();

        let remaining = store.all_tasks().unwrap();
        assert_eq!(remaining.len(), 1, "the deleted task must be gone");
        assert_eq!(remaining[0].title, "Keep renamed");
    }

    #[test]
    fn delete_task_records_a_tombstone() {
        let store = store();
        store
            .upsert_tasks("@a", &[sample_task("t1", "One")])
            .unwrap();
        store.delete_task("@a", "t1").unwrap();

        assert!(store.task("@a", "t1").unwrap().is_none());
        let tombstones: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM tombstones", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tombstones, 1);
    }

    #[test]
    fn pending_ops_are_returned_in_insertion_order() {
        let store = store();
        for (id, kind) in [("t1", PendingOpKind::Patch), ("t2", PendingOpKind::Delete)] {
            store
                .enqueue(PendingOp {
                    id: 0,
                    list_id: "@a".into(),
                    task_id: Some(id.into()),
                    kind,
                    payload: "{}".into(),
                    attempts: 0,
                    last_error: None,
                })
                .unwrap();
        }

        let ops = store.pending_ops().unwrap();
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].task_id.as_deref(), Some("t1"));
        assert_eq!(ops[1].task_id.as_deref(), Some("t2"));
    }

    #[test]
    fn failing_an_op_increments_attempts_and_records_the_error() {
        let store = store();
        let id = store
            .enqueue(PendingOp {
                id: 0,
                list_id: "@a".into(),
                task_id: Some("t1".into()),
                kind: PendingOpKind::Patch,
                payload: "{}".into(),
                attempts: 0,
                last_error: None,
            })
            .unwrap();

        store.fail_op(id, "429 rate limited").unwrap();
        let ops = store.pending_ops().unwrap();

        assert_eq!(ops.len(), 1, "a failed op must stay queued for retry");
        assert_eq!(ops[0].attempts, 1);
        assert_eq!(ops[0].last_error.as_deref(), Some("429 rate limited"));
    }

    #[test]
    fn completing_an_op_removes_it() {
        let store = store();
        let id = store
            .enqueue(PendingOp {
                id: 0,
                list_id: "@a".into(),
                task_id: Some("t1".into()),
                kind: PendingOpKind::Patch,
                payload: "{}".into(),
                attempts: 0,
                last_error: None,
            })
            .unwrap();

        store.complete_op(id).unwrap();
        assert_eq!(store.pending_op_count().unwrap(), 0);
    }

    #[test]
    fn mark_notified_only_fires_once_per_task_and_kind() {
        let store = store();
        assert!(store.mark_notified("@a", "t1", "due_today").unwrap());
        assert!(
            !store.mark_notified("@a", "t1", "due_today").unwrap(),
            "a repeat notification for the same task must be suppressed"
        );
        assert!(
            store.mark_notified("@a", "t1", "overdue").unwrap(),
            "a different notification kind is a separate notification"
        );
    }

    #[test]
    fn sync_state_survives_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.db");

        {
            let store = Store::open(&path).unwrap();
            store.set_last_delta("@a", "2026-10-01T00:00:00Z").unwrap();
        }

        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.last_delta("@a").unwrap().as_deref(),
            Some("2026-10-01T00:00:00Z")
        );
    }

    #[test]
    fn clear_all_empties_every_table() {
        let store = store();
        store
            .upsert_task_lists(&[TaskList {
                id: "@a".into(),
                title: "L".into(),
                updated: None,
                etag: None,
            }])
            .unwrap();
        store
            .upsert_tasks("@a", &[sample_task("t1", "One")])
            .unwrap();
        store.set_last_delta("@a", "ts").unwrap();

        store.clear_all().unwrap();

        assert!(store.task_lists().unwrap().is_empty());
        assert!(store.all_tasks().unwrap().is_empty());
        assert!(store.last_delta("@a").unwrap().is_none());
    }
}
