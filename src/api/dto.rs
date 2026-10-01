use serde::{Deserialize, Serialize};

/// Every endpoint the app calls, so the sync engine never constructs a URL or
/// knows a method name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Endpoint {
    /// `GET /tasks/v1/users/@me/lists`
    TaskLists,
    /// `GET /tasks/v1/users/@me/lists/{tasklist}`
    TaskListGet,
    /// `POST /tasks/v1/users/@me/lists`
    TaskListInsert,
    /// `PATCH /tasks/v1/users/@me/lists/{tasklist}`
    TaskListPatch,

    /// `GET /tasks/v1/lists/{tasklist}/tasks`
    TaskList_,
    /// `GET /tasks/v1/lists/{tasklist}/tasks/{task}`
    TaskGet,
    /// `POST /tasks/v1/lists/{tasklist}/tasks`
    TaskInsert,
    /// `PATCH /tasks/v1/lists/{tasklist}/tasks/{task}`
    TaskPatch,
    /// `PUT /tasks/v1/lists/{tasklist}/tasks/{task}`
    TaskUpdate,
    /// `DELETE /tasks/v1/lists/{tasklist}/tasks/{task}`
    TaskDelete,
    /// `POST /tasks/v1/lists/{tasklist}/tasks/{task}/move`
    TaskMove,
}

pub const BASE_URL: &str = "https://tasks.googleapis.com/tasks/v1";

/// Path segments for a request. A leading `@` in a list id is valid in the
/// Tasks API and must be sent verbatim rather than percent-encoded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestPath {
    pub tasklist: Option<String>,
    pub task: Option<String>,
    pub suffix: Option<&'static str>,
}

impl Endpoint {
    /// The path for this endpoint, e.g. `/users/@me/lists/@abc/tasks/t1/move`.
    pub fn path(&self, args: &RequestPath) -> String {
        let list = args.tasklist.as_deref().unwrap_or("@me");
        let task = args.task.as_deref().unwrap_or_default();

        let path = match self {
            Endpoint::TaskLists => "/users/@me/lists".to_string(),
            Endpoint::TaskListGet => format!("/users/@me/lists/{list}"),
            Endpoint::TaskListInsert => "/users/@me/lists".to_string(),
            Endpoint::TaskListPatch => format!("/users/@me/lists/{list}"),
            Endpoint::TaskList_ => format!("/lists/{list}/tasks"),
            Endpoint::TaskGet => format!("/lists/{list}/tasks/{task}"),
            Endpoint::TaskInsert => format!("/lists/{list}/tasks"),
            Endpoint::TaskPatch => format!("/lists/{list}/tasks/{task}"),
            Endpoint::TaskUpdate => format!("/lists/{list}/tasks/{task}"),
            Endpoint::TaskDelete => format!("/lists/{list}/tasks/{task}"),
            Endpoint::TaskMove => format!("/lists/{list}/tasks/{task}/move"),
        };

        match args.suffix {
            Some(suffix) => format!("{path}/{suffix}"),
            None => path,
        }
    }
}

/// Body of `tasklists.list`.
#[derive(Debug, Clone, Deserialize)]
pub struct TaskListsResponse {
    #[serde(rename = "nextPageToken", default)]
    pub next_page_token: Option<String>,
    #[serde(rename = "items", default)]
    pub items: Vec<TaskListWire>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskListWire {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub updated: Option<String>,
    #[serde(default)]
    pub etag: Option<String>,
}

/// Body of `tasks.list`.
#[derive(Debug, Clone, Deserialize)]
pub struct TasksResponse {
    #[serde(rename = "nextPageToken", default)]
    pub next_page_token: Option<String>,
    #[serde(rename = "items", default)]
    pub items: Vec<TaskWire>,
}

/// The raw wire shape of a task.
///
/// This is deliberately separate from `store::models::Task`: this type keeps the
/// API's strings exactly as sent so nothing is lost in translation, while the
/// model type is what the rest of the app uses. Conversion happens in
/// `api::dto::into_task`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TaskWire {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub due: Option<String>,
    #[serde(default)]
    pub completed: Option<String>,
    #[serde(default)]
    pub updated: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub previous: Option<String>,
    #[serde(default)]
    pub position: Option<String>,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub hidden: Option<bool>,
    #[serde(default)]
    pub deleted: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::models::TaskStatus;

    fn path_args(list: Option<&str>, task: Option<&str>) -> RequestPath {
        RequestPath {
            tasklist: list.map(str::to_string),
            task: task.map(str::to_string),
            suffix: None,
        }
    }

    #[test]
    fn paths_match_the_api_reference() {
        assert_eq!(
            Endpoint::TaskLists.path(&RequestPath::default()),
            "/users/@me/lists"
        );
        assert_eq!(
            Endpoint::TaskLists.path(&path_args(Some("@abc"), None)),
            "/users/@me/lists"
        );
        assert_eq!(
            Endpoint::TaskListGet.path(&path_args(Some("@abc"), None)),
            "/users/@me/lists/@abc"
        );
        assert_eq!(
            Endpoint::TaskList_.path(&path_args(Some("@abc"), None)),
            "/lists/@abc/tasks"
        );
        assert_eq!(
            Endpoint::TaskGet.path(&path_args(Some("@abc"), Some("t1"))),
            "/lists/@abc/tasks/t1"
        );
        assert_eq!(
            Endpoint::TaskMove.path(&path_args(Some("@abc"), Some("t1"))),
            "/lists/@abc/tasks/t1/move"
        );
    }

    #[test]
    fn list_ids_keeping_their_at_sign_are_not_encoded() {
        // Google task list ids look like "@NQl..." and must be sent verbatim.
        let path = Endpoint::TaskList_.path(&path_args(Some("@MDQ6V2Zpc2hlZEdyb3Vw"), None));
        assert!(path.contains("@MDQ6V2Zpc2hlZEdyb3Vw"), "got {path}");
        assert!(!path.contains("%40"), "the @ must not be percent-encoded");
    }

    #[test]
    fn task_wire_tolerates_a_sparse_payload() {
        let wire: TaskWire = serde_json::from_str(r#"{"id":"t1"}"#).expect("parse");
        assert_eq!(wire.id, "t1");
        assert_eq!(wire.title, "");
        assert!(wire.status.is_none());
        assert!(wire.due.is_none());
    }

    #[test]
    fn task_wire_reads_every_documented_field() {
        let json = r#"{
            "id": "t1",
            "title": "Buy milk",
            "notes": "Semi-skimmed",
            "status": "needsAction",
            "due": "2026-10-01T00:00:00.000Z",
            "updated": "2026-10-01T09:00:00.000Z",
            "parent": "p1",
            "previous": "p2",
            "position": "000003",
            "etag": "\"e\"",
            "hidden": true,
            "deleted": false
        }"#;
        let wire: TaskWire = serde_json::from_str(json).expect("parse");
        assert_eq!(wire.title, "Buy milk");
        assert_eq!(wire.notes.as_deref(), Some("Semi-skimmed"));
        assert_eq!(wire.position.as_deref(), Some("000003"));
        assert_eq!(wire.hidden, Some(true));
        assert_eq!(wire.deleted, Some(false));
    }

    #[test]
    fn tasklists_response_parses() {
        let json = r#"{"items":[{"id":"@a","title":"My Tasks","etag":"\"l\""}]}"#;
        let parsed: TaskListsResponse = serde_json::from_str(json).expect("parse");
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0].id, "@a");
    }

    #[test]
    fn an_unknown_status_falls_back_rather_than_failing_the_parse() {
        use crate::api::convert::into_task;
        let wire: TaskWire =
            serde_json::from_str(r#"{"id":"t1","title":"x","status":"somethingNewIn2027"}"#)
                .expect("parse");
        let task = into_task(wire);
        assert_eq!(
            task.status,
            TaskStatus::NeedsAction,
            "an unrecognised status must not break the sync"
        );
    }
}
