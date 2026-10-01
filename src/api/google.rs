use anyhow::{bail, Result};
use reqwest::header::{HeaderMap, HeaderValue, IF_MATCH};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use std::time::Duration;

use super::convert::{into_task, into_task_list};
use super::dto::{Endpoint, RequestPath, TaskListsResponse, TasksResponse, BASE_URL};
use crate::store::models::{Task, TaskList};

/// Google's courtesy limit is 50,000 queries per day. Polling every five
/// minutes with a handful of calls per list stays far below this, but the
/// counter exists so a pathological retry loop cannot run away.
const DAILY_QUERY_BUDGET: u32 = 50_000;

/// An error from the API that the sync engine needs to react to differently.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The refresh token was rejected. The user must re-authorise; retrying is
    /// pointless and, left alone, burns the daily budget.
    #[error("the refresh token was rejected; reconnection is required")]
    NeedsReauth,

    /// The delta window was too old. The caller must fall back to a full sync
    /// of that list.
    #[error("the sync window has expired ({0}); a full sync is required")]
    HistoryGone(String),

    /// Another client changed the resource. Refetch and replay.
    #[error("precondition failed: the task changed on the server")]
    Conflict,

    /// Rate limited or temporarily broken. Retry after the indicated delay.
    #[error("rate limited; retry after {0:?}")]
    RateLimited(Option<Duration>),

    #[error("network error: {0}")]
    Network(String),

    /// Any other non-success response.
    #[error("API returned {status}: {body}")]
    Status { status: StatusCode, body: String },
}

impl ApiError {
    /// Whether retrying could plausibly succeed without user intervention.
    pub fn is_retryable(&self) -> bool {
        matches!(self, ApiError::RateLimited(_) | ApiError::Network(_))
    }
}

pub struct TasksClient {
    http: reqwest::Client,
    token: String,
    queries_today: u32,
    day_started: chrono::NaiveDate,
}

impl TasksClient {
    pub fn new(token: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("gtaskbar/", env!("CARGO_PKG_VERSION")))
            .build()?;

        Ok(Self {
            http,
            token,
            queries_today: 0,
            day_started: chrono::Local::now().date_naive(),
        })
    }

    /// Replaces the access token, as happens after a refresh.
    pub fn set_token(&mut self, token: String) {
        self.token = token;
    }

    fn spend_query(&mut self) -> Result<()> {
        let today = chrono::Local::now().date_naive();
        if today != self.day_started {
            self.day_started = today;
            self.queries_today = 0;
        }
        if self.queries_today >= DAILY_QUERY_BUDGET {
            bail!(
                "refusing to exceed the {DAILY_QUERY_BUDGET}-query daily budget for the Tasks API"
            );
        }
        self.queries_today += 1;
        Ok(())
    }

    /// Performs a request, translating HTTP status codes into `ApiError` so
    /// the sync engine can decide whether to retry, resync or re-authenticate.
    async fn send(
        &mut self,
        method: Method,
        endpoint: Endpoint,
        path_args: &RequestPath,
        query: &[(&str, String)],
        if_match: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> Result<reqwest::Response, ApiError> {
        self.spend_query()
            .map_err(|err| ApiError::Network(err.to_string()))?;

        let url = format!("{BASE_URL}{}", endpoint.path(path_args));

        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.token))
                .map_err(|_| ApiError::NeedsReauth)?,
        );
        if let Some(etag) = if_match {
            let value = HeaderValue::from_str(etag).map_err(|_| ApiError::Status {
                status: StatusCode::BAD_REQUEST,
                body: format!("malformed etag: {etag}"),
            })?;
            headers.insert(IF_MATCH, value);
        }

        let mut request = self
            .http
            .request(method, &url)
            .headers(headers)
            .query(query);

        if let Some(body) = body {
            request = request.json(&body);
        }

        let response = request.send().await.map_err(|err| {
            // A 401 here is a token problem, not a transport problem.
            if err.status() == Some(StatusCode::UNAUTHORIZED) {
                ApiError::NeedsReauth
            } else {
                ApiError::Network(err.to_string())
            }
        })?;

        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_secs);

        let body = response.text().await.unwrap_or_default();

        Err(match status {
            StatusCode::UNAUTHORIZED => ApiError::NeedsReauth,
            StatusCode::GONE => ApiError::HistoryGone(body),
            StatusCode::PRECONDITION_FAILED | StatusCode::CONFLICT => ApiError::Conflict,
            StatusCode::TOO_MANY_REQUESTS => ApiError::RateLimited(retry_after),
            status if status.is_server_error() => ApiError::RateLimited(retry_after),
            status => ApiError::Status { status, body },
        })
    }

    async fn get_json<T: DeserializeOwned>(
        &mut self,
        endpoint: Endpoint,
        path_args: &RequestPath,
        query: &[(&str, String)],
    ) -> Result<T, ApiError> {
        let response = self
            .send(Method::GET, endpoint, path_args, query, None, None)
            .await?;
        response
            .json()
            .await
            .map_err(|err| ApiError::Network(err.to_string()))
    }

    // ---- task lists -------------------------------------------------------

    /// All task lists, following pagination. Lists are few, so this is a single
    /// call in practice.
    pub async fn task_lists(&mut self) -> Result<Vec<TaskList>, ApiError> {
        let mut lists = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let mut query: Vec<(&str, String)> = vec![("maxResults", "100".into())];
            if let Some(token) = &page_token {
                query.push(("pageToken", token.clone()));
            }

            let response: TaskListsResponse = self
                .get_json(Endpoint::TaskLists, &RequestPath::default(), &query)
                .await?;

            lists.extend(response.items.into_iter().map(into_task_list));

            match response.next_page_token {
                Some(token) if !token.is_empty() => page_token = Some(token),
                _ => break,
            }
        }

        Ok(lists)
    }

    // ---- tasks ------------------------------------------------------------

    /// Fetches every task in a list, following `nextPageToken`.
    ///
    /// `show_completed` is only honoured alongside `show_hidden`, so the latter
    /// is always sent. The API's default `maxResults` is 20, so omitting
    /// pagination here would silently truncate large lists.
    pub async fn tasks(
        &mut self,
        list_id: &str,
        updated_min: Option<&str>,
    ) -> Result<Vec<Task>, ApiError> {
        let mut tasks = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let mut query: Vec<(&str, String)> = vec![
                ("maxResults", "100".into()),
                ("showCompleted", "true".into()),
                ("showHidden", "true".into()),
            ];
            if let Some(updated_min) = updated_min {
                query.push(("updatedMin", updated_min.to_string()));
            }
            if let Some(token) = &page_token {
                query.push(("pageToken", token.clone()));
            }

            let response: TasksResponse = self
                .get_json(
                    Endpoint::TaskList_,
                    &RequestPath {
                        tasklist: Some(list_id.to_string()),
                        ..Default::default()
                    },
                    &query,
                )
                .await?;

            tasks.extend(response.items.into_iter().map(into_task));

            match response.next_page_token {
                Some(token) if !token.is_empty() => page_token = Some(token),
                _ => break,
            }
        }

        Ok(tasks)
    }

    /// Fetches a single task, used to rebase a change after a 412.
    pub async fn task(&mut self, list_id: &str, task_id: &str) -> Result<Task, ApiError> {
        let wire: super::dto::TaskWire = self
            .get_json(
                Endpoint::TaskGet,
                &RequestPath {
                    tasklist: Some(list_id.to_string()),
                    task: Some(task_id.to_string()),
                    ..Default::default()
                },
                &[],
            )
            .await?;
        Ok(into_task(wire))
    }

    /// Creates a task, optionally positioned under a parent.
    pub async fn insert_task(
        &mut self,
        list_id: &str,
        patch: &crate::store::models::TaskPatch,
        parent: Option<&str>,
        previous: Option<&str>,
    ) -> Result<Task, ApiError> {
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(parent) = parent {
            query.push(("parent", parent.to_string()));
        }
        if let Some(previous) = previous {
            query.push(("previous", previous.to_string()));
        }

        let response = self
            .send(
                Method::POST,
                Endpoint::TaskInsert,
                &RequestPath {
                    tasklist: Some(list_id.to_string()),
                    ..Default::default()
                },
                &query,
                None,
                Some(serde_json::to_value(patch).map_err(|err| {
                    ApiError::Network(format!("could not serialise task: {err}"))
                })?),
            )
            .await?;

        let wire: super::dto::TaskWire = response
            .json()
            .await
            .map_err(|err| ApiError::Network(err.to_string()))?;
        Ok(into_task(wire))
    }

    /// Partially updates a task, guarded by its etag.
    pub async fn patch_task(
        &mut self,
        list_id: &str,
        task_id: &str,
        patch: &crate::store::models::TaskPatch,
        etag: Option<&str>,
    ) -> Result<Task, ApiError> {
        let response = self
            .send(
                Method::PATCH,
                Endpoint::TaskPatch,
                &RequestPath {
                    tasklist: Some(list_id.to_string()),
                    task: Some(task_id.to_string()),
                    ..Default::default()
                },
                &[],
                etag,
                Some(serde_json::to_value(patch).map_err(|err| {
                    ApiError::Network(format!("could not serialise patch: {err}"))
                })?),
            )
            .await?;

        let wire: super::dto::TaskWire = response
            .json()
            .await
            .map_err(|err| ApiError::Network(err.to_string()))?;
        Ok(into_task(wire))
    }

    /// Moves a task to a new position, parent, or list.
    pub async fn move_task(
        &mut self,
        list_id: &str,
        task_id: &str,
        request: &super::convert::MoveRequest,
    ) -> Result<Task, ApiError> {
        let response = self
            .send(
                Method::POST,
                Endpoint::TaskMove,
                &RequestPath {
                    tasklist: Some(list_id.to_string()),
                    task: Some(task_id.to_string()),
                    ..Default::default()
                },
                &[],
                None,
                Some(serde_json::to_value(request).map_err(|err| {
                    ApiError::Network(format!("could not serialise move: {err}"))
                })?),
            )
            .await?;

        let wire: super::dto::TaskWire = response
            .json()
            .await
            .map_err(|err| ApiError::Network(err.to_string()))?;
        Ok(into_task(wire))
    }

    /// Deletes a task.
    pub async fn delete_task(&mut self, list_id: &str, task_id: &str) -> Result<(), ApiError> {
        self.send(
            Method::DELETE,
            Endpoint::TaskDelete,
            &RequestPath {
                tasklist: Some(list_id.to_string()),
                task: Some(task_id.to_string()),
                ..Default::default()
            },
            &[],
            None,
            None,
        )
        .await
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_reauth_is_not_retryable() {
        // Retrying a rejected refresh token would burn the daily budget for
        // nothing, so this must not be treated as transient.
        assert!(!ApiError::NeedsReauth.is_retryable());
    }

    #[test]
    fn conflicts_and_history_gone_are_not_retryable() {
        // Both need a different strategy, not the same request again.
        assert!(!ApiError::Conflict.is_retryable());
        assert!(!ApiError::HistoryGone("resyncToken".into()).is_retryable());
    }

    #[test]
    fn rate_limits_and_network_errors_are_retryable() {
        assert!(ApiError::RateLimited(None).is_retryable());
        assert!(ApiError::RateLimited(Some(Duration::from_secs(30))).is_retryable());
        assert!(ApiError::Network("connection reset".into()).is_retryable());
    }

    #[test]
    fn a_client_can_be_built_with_a_token() {
        let client = TasksClient::new("token".into()).expect("build client");
        assert_eq!(client.token, "token");
    }

    #[test]
    fn replacing_the_token_takes_effect() {
        let mut client = TasksClient::new("old".into()).expect("build client");
        client.set_token("new".into());
        assert_eq!(client.token, "new");
    }

    #[test]
    fn the_daily_budget_stops_runaway_requests() {
        let mut client = TasksClient::new("token".into()).expect("build client");
        // Exhaust the budget.
        for _ in 0..DAILY_QUERY_BUDGET {
            client.spend_query().expect("within budget");
        }
        let err = client.spend_query().expect_err("budget must be enforced");
        assert!(err.to_string().contains("daily budget"), "got {err}");
    }
}
