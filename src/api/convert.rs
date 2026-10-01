use chrono::{DateTime, NaiveDate, Utc};

use crate::store::models::{Task, TaskList, TaskStatus};

use super::dto::{TaskListWire, TaskWire};

/// Converts a wire task into the model the app uses.
///
/// Unknown or missing enum values degrade to a default rather than failing:
/// a sync must not break because Google added a status we do not know yet.
pub fn into_task(wire: TaskWire) -> Task {
    Task {
        id: wire.id,
        title: wire.title,
        notes: wire.notes.unwrap_or_default(),
        status: match wire.status.as_deref() {
            Some("completed") => TaskStatus::Completed,
            _ => TaskStatus::NeedsAction,
        },
        due: wire.due.as_deref().and_then(parse_due_date),
        completed: parse_timestamp(wire.completed.as_deref()),
        updated: parse_timestamp(wire.updated.as_deref()),
        parent: wire.parent,
        previous: wire.previous,
        position: wire.position,
        etag: wire.etag,
        hidden: wire.hidden.unwrap_or(false),
        deleted: wire.deleted.unwrap_or(false),
    }
}

pub fn into_task_list(wire: TaskListWire) -> TaskList {
    TaskList {
        id: wire.id,
        title: wire.title,
        updated: parse_timestamp(wire.updated.as_deref()),
        etag: wire.etag,
    }
}

/// Extracts the date from a `due` timestamp.
///
/// The API accepts a full RFC3339 timestamp but only ever stores the date, so
/// the time portion is discarded server-side. Parsing the leading date and
/// ignoring the rest is therefore correct, not a shortcut.
fn parse_due_date(raw: &str) -> Option<NaiveDate> {
    raw.get(..10)
        .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
}

fn parse_timestamp(raw: Option<&str>) -> Option<DateTime<Utc>> {
    raw.and_then(|value| value.parse().ok())
}

/// Body for `tasks.move`.
///
/// `parent` and `previous` are what define a task's position, and clearing the
/// parent is how a subtask is promoted back to the top level. Because the API
/// distinguishes "absent" from "explicitly null", these are
/// `Option<Option<&str>>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MoveRequest {
    /// The API spells this `destinationTasklist`, not snake_case.
    #[serde(
        rename = "destinationTasklist",
        skip_serializing_if = "Option::is_none"
    )]
    pub destination_tasklist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<Option<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_date_keeps_only_the_date_part() {
        // The API returns midnight UTC but documents that the time is
        // meaningless, so only the date is extracted.
        let date = parse_due_date("2026-10-01T00:00:00.000Z").expect("parse");
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());
    }

    #[test]
    fn due_date_parses_a_bare_date_too() {
        let date = parse_due_date("2026-10-01").expect("parse");
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());
    }

    #[test]
    fn a_malformed_due_date_is_ignored_rather_than_failing() {
        assert!(parse_due_date("not-a-date").is_none());
        assert!(parse_due_date("").is_none());
    }

    #[test]
    fn wire_conversion_maps_every_field() {
        let wire: TaskWire = serde_json::from_str(
            r#"{
                "id": "t1",
                "title": "Buy milk",
                "notes": "Semi-skimmed",
                "status": "completed",
                "due": "2026-10-01T00:00:00.000Z",
                "completed": "2026-10-02T08:00:00.000Z",
                "updated": "2026-10-02T08:00:00.000Z",
                "parent": "p1",
                "position": "000004",
                "etag": "\"e1\"",
                "hidden": true
            }"#,
        )
        .expect("parse");

        let task = into_task(wire);
        assert_eq!(task.id, "t1");
        assert_eq!(task.title, "Buy milk");
        assert_eq!(task.notes, "Semi-skimmed");
        assert!(task.is_completed());
        assert_eq!(task.due, NaiveDate::from_ymd_opt(2026, 10, 1));
        assert!(task.completed.is_some());
        assert_eq!(task.parent.as_deref(), Some("p1"));
        assert!(task.is_subtask());
        assert_eq!(task.position.as_deref(), Some("000004"));
        assert!(task.hidden);
        assert!(!task.deleted);
    }

    #[test]
    fn deleted_and_hidden_default_to_false() {
        let wire: TaskWire = serde_json::from_str(r#"{"id":"t1","title":"x"}"#).expect("parse");
        let task = into_task(wire);
        assert!(!task.hidden);
        assert!(!task.deleted);
    }

    #[test]
    fn move_request_distinguishes_absent_from_null_parent() {
        // Omitted: leave the parent alone.
        let unchanged = MoveRequest::default();
        assert_eq!(
            serde_json::to_string(&unchanged).unwrap(),
            "{}",
            "an empty move must not clear the parent"
        );

        // Explicit null: promote the subtask to the top level.
        let promote = MoveRequest {
            parent: Some(None),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&promote).unwrap(),
            r#"{"parent":null}"#,
            "clearing a parent requires an explicit null"
        );

        // Explicit value: re-parent.
        let reparent = MoveRequest {
            parent: Some(Some("p9".into())),
            previous: Some(Some("p8".into())),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&reparent).unwrap(),
            r#"{"parent":"p9","previous":"p8"}"#
        );
    }

    #[test]
    fn move_request_can_change_list() {
        let request = MoveRequest {
            destination_tasklist: Some("@other".into()),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"destinationTasklist":"@other"}"#
        );
    }

    #[test]
    fn tasklist_conversion_maps_fields() {
        let wire: TaskListWire = serde_json::from_str(
            r#"{"id":"@a","title":"My Tasks","etag":"\"l\"","updated":"2026-10-01T00:00:00.000Z"}"#,
        )
        .expect("parse");
        let list = into_task_list(wire);
        assert_eq!(list.id, "@a");
        assert_eq!(list.title, "My Tasks");
        assert_eq!(list.etag.as_deref(), Some("\"l\""));
        assert!(list.updated.is_some());
    }
}
