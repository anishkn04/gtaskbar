use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// A task list as returned by `tasklists.list` / `tasklists.get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskList {
    pub id: String,
    pub title: String,
    #[serde(rename = "updated")]
    pub updated: Option<DateTime<Utc>>,
    #[serde(rename = "etag")]
    pub etag: Option<String>,
}

impl TaskList {
    /// Stable identity for UI keys and database lookups.
    pub fn key(&self) -> &str {
        &self.id
    }
}

/// A task as returned by `tasks.list` and friends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(rename = "notes", default)]
    pub notes: String,
    /// Defaults to `needsAction` when the API omits it.
    #[serde(default)]
    pub status: TaskStatus,
    /// Date-only. The API accepts an RFC3339 timestamp but discards the time
    /// portion, so nothing beyond the date is meaningful here.
    pub due: Option<NaiveDate>,
    #[serde(rename = "completed")]
    pub completed: Option<DateTime<Utc>>,
    #[serde(rename = "updated")]
    pub updated: Option<DateTime<Utc>>,
    pub parent: Option<String>,
    pub previous: Option<String>,
    /// Output only. An opaque lexicographic key describing the task's position
    /// among its siblings. Reordering goes through the `tasks.move` endpoint.
    pub position: Option<String>,
    #[serde(rename = "etag")]
    pub etag: Option<String>,
    #[serde(rename = "hidden", default)]
    pub hidden: bool,
    #[serde(rename = "deleted", default)]
    pub deleted: bool,
}

/// The subset of task fields accepted by `tasks.insert` and `tasks.patch`.
///
/// `id`, `position`, `parent` and `completed` are deliberately absent: the
/// first two are output-only, the third is read-only, and completion is set
/// implicitly from `status`.
///
/// `due` is modelled as `Option<Option<NaiveDate>>` on purpose. `None` means
/// "leave the due date alone" and the field is omitted from the request;
/// `Some(None)` means "clear the due date" and serialises to an explicit
/// `null`. Collapsing this to a plain `Option` would make clearing impossible.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TaskPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<Option<NaiveDate>>,
}

impl TaskPatch {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Task {
    pub fn is_completed(&self) -> bool {
        self.status == TaskStatus::Completed
    }

    pub fn is_subtask(&self) -> bool {
        self.parent.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    #[default]
    NeedsAction,
    Completed,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::NeedsAction => "needsAction",
            TaskStatus::Completed => "completed",
        }
    }
}

/// A locally requested write awaiting acknowledgement from the API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingOp {
    pub id: i64,
    pub list_id: String,
    pub task_id: Option<String>,
    pub kind: PendingOpKind,
    /// Serialised JSON body, interpreted per `kind`.
    pub payload: String,
    pub attempts: u32,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingOpKind {
    /// Create a task; the API assigns the id.
    Insert,
    /// Partially update a task.
    Patch,
    /// Mark a task completed or incomplete.
    SetStatus,
    /// Move a task to a new position, parent, or list.
    Move,
    /// Delete a task.
    Delete,
}

impl PendingOpKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PendingOpKind::Insert => "insert",
            PendingOpKind::Patch => "patch",
            PendingOpKind::SetStatus => "set_status",
            PendingOpKind::Move => "move",
            PendingOpKind::Delete => "delete",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "insert" => Some(Self::Insert),
            "patch" => Some(Self::Patch),
            "set_status" => Some(Self::SetStatus),
            "move" => Some(Self::Move),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_status_serialises_to_api_wire_values() {
        assert_eq!(
            serde_json::to_string(&TaskStatus::NeedsAction).unwrap(),
            "\"needsAction\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Completed).unwrap(),
            "\"completed\""
        );
    }

    #[test]
    fn task_status_round_trips() {
        for status in [TaskStatus::NeedsAction, TaskStatus::Completed] {
            let json = serde_json::to_string(&status).unwrap();
            let decoded: TaskStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, status);
        }
    }

    #[test]
    fn task_uses_api_field_names() {
        // The API's JSON keys do not match Rust's snake_case conventions in
        // several places, so the renames above are load-bearing.
        let json = r#"{
            "id": "abc",
            "title": "Test",
            "status": "needsAction",
            "updated": "2026-01-02T03:04:05.000Z",
            "etag": "\"xyz\"",
            "hidden": false,
            "deleted": false
        }"#;

        let task: Task = serde_json::from_str(json).expect("parse task");
        assert_eq!(task.id, "abc");
        assert_eq!(
            task.updated.unwrap().to_rfc3339(),
            "2026-01-02T03:04:05+00:00"
        );
        assert_eq!(task.etag.as_deref(), Some("\"xyz\""));
        assert!(!task.hidden);
    }

    #[test]
    fn task_tolerates_a_minimal_payload() {
        // Optional fields genuinely are optional; a task with only an id and
        // title must still parse.
        let task: Task = serde_json::from_str(r#"{"id":"a","title":"b"}"#).expect("parse");
        assert_eq!(task.notes, "");
        assert_eq!(task.status, TaskStatus::NeedsAction);
        assert!(task.due.is_none());
        assert!(task.parent.is_none());
    }

    #[test]
    fn patch_omits_absent_fields() {
        let mut patch = TaskPatch::new();
        patch.title = Some("Renamed".into());
        let json = serde_json::to_string(&patch).unwrap();
        assert_eq!(json, r#"{"title":"Renamed"}"#);
    }

    #[test]
    fn patch_serialises_a_cleared_due_date_distinctly() {
        // `due: null` means "clear the due date" and must be sent, whereas an
        // absent `due` means "leave it alone". Modelling it as
        // Option<Option<NaiveDate>> is what keeps those two cases apart.
        let patch = TaskPatch {
            due: Some(None),
            ..TaskPatch::new()
        };
        let json = serde_json::to_string(&patch).unwrap();
        assert_eq!(json, r#"{"due":null}"#);
    }

    #[test]
    fn pending_op_kind_round_trips_through_its_string_form() {
        for kind in [
            PendingOpKind::Insert,
            PendingOpKind::Patch,
            PendingOpKind::SetStatus,
            PendingOpKind::Move,
            PendingOpKind::Delete,
        ] {
            assert_eq!(PendingOpKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(PendingOpKind::parse("nonsense"), None);
    }

    #[test]
    fn task_list_key_is_the_id() {
        let list = TaskList {
            id: "@abc123".into(),
            title: "My Tasks".into(),
            updated: None,
            etag: None,
        };
        assert_eq!(list.key(), "@abc123");
    }
}
