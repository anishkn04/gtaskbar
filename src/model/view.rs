use chrono::{Datelike, Local, NaiveDate};

use crate::config::SortMode;
use crate::store::models::Task;

/// A selectable collection of tasks in the sidebar.
///
/// Smart views span every list; a list view shows one list. Keeping both in one
/// enum means the sidebar and the list view only have to handle one selection
/// type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    Today,
    Upcoming,
    Overdue,
    All,
    Completed,
    List(String),
}

impl View {
    /// A stable identifier for this view, used to pair a sidebar row with its
    /// content page. List ids contain characters that are awkward in a widget
    /// name, so they are sanitised.
    pub fn key(&self) -> String {
        match self {
            View::Today => "today".into(),
            View::Upcoming => "upcoming".into(),
            View::Overdue => "overdue".into(),
            View::All => "all".into(),
            View::Completed => "completed".into(),
            View::List(id) => format!("list-{}", id.replace(['@', '/', '.'], "-")),
        }
    }

    pub fn title(&self) -> String {
        match self {
            View::Today => "Today".into(),
            View::Upcoming => "Next 7 days".into(),
            View::Overdue => "Overdue".into(),
            View::All => "All tasks".into(),
            View::Completed => "Completed".into(),
            View::List(id) => id.clone(),
        }
    }

    /// Whether the view hides completed tasks. Only `Completed` shows them,
    /// which matches how people actually use a task list.
    pub fn include_completed(&self) -> bool {
        matches!(self, View::Completed)
    }

    /// Whether a task belongs in this view, given the current date.
    pub fn matches(&self, task: &Task, today: NaiveDate) -> bool {
        if task.is_completed() {
            return self.include_completed();
        }

        match self {
            View::Today => task.due == Some(today),
            View::Upcoming => {
                matches!(task.due, Some(due) if due > today && due <= today + chrono::Duration::days(7))
            }
            // Overdue means a due date that has passed. An undated task is not
            // overdue; it is simply unscheduled, which is the `All` view's job.
            View::Overdue => matches!(task.due, Some(due) if due < today),
            View::All | View::List(_) => true,
            // The completed view holds only completed tasks, which the early
            // return above already let through.
            View::Completed => false,
        }
    }
}

/// The tasks a view should show, already filtered, sorted and grouped.
///
/// This is pure: no database, no widgets, no clock other than the `today` that
/// is passed in. That is what makes the sorting and grouping rules testable
/// without standing up the app.
pub fn assemble(view: &View, tasks: Vec<Task>, today: NaiveDate, sort: SortMode) -> Vec<Task> {
    let mut filtered: Vec<Task> = tasks
        .into_iter()
        .filter(|task| match view {
            View::List(list_id) => {
                // The list id is not on the task, so the caller has already
                // scoped the query; membership is decided upstream.
                let _ = list_id;
                view.matches(task, today)
            }
            _ => view.matches(task, today),
        })
        .collect();

    match sort {
        SortMode::Manual => {
            // Google's own ordering, which the API expresses as an opaque
            // lexicographic `position` string. It is output only, so there is
            // nothing to sort on locally beyond preserving what arrived.
            filtered.sort_by(|a, b| {
                a.position
                    .as_deref()
                    .unwrap_or_default()
                    .cmp(b.position.as_deref().unwrap_or_default())
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
        SortMode::DueDate => {
            // Undated tasks sort last: a task with no deadline should not
            // outrank one that is due today.
            filtered.sort_by(|a, b| match (a.due, b.due) {
                (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.id.cmp(&b.id)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.id.cmp(&b.id),
            });
        }
        SortMode::Alphabetical => {
            filtered.sort_by(|a, b| {
                a.title
                    .to_lowercase()
                    .cmp(&b.title.to_lowercase())
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
        SortMode::Created | SortMode::Updated => {
            // The Tasks API exposes no creation timestamp, only `updated`, so
            // both of these fall back to it. Documented rather than hidden.
            filtered.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
        }
        SortMode::Priority => {
            // Whether a task has outstanding subtasks is a property of the set,
            // not of one task, so it has to be resolved before sorting.
            let with_open_subtasks = parents_with_open_subtasks(&filtered);
            filtered.sort_by(|a, b| {
                let score =
                    |t: &Task| priority(t, today, with_open_subtasks.contains(t.id.as_str()));
                score(a)
                    .cmp(&score(b))
                    .then_with(|| b.updated.cmp(&a.updated))
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
    }

    filtered
}

/// The locally derived priority score. Lower sorts first.
///
/// Google Tasks has no priority field, so this is entirely our own heuristic:
/// an overdue task is more urgent than one due today, which is more urgent than
/// one due next week. Tasks carrying notes or with incomplete subtasks get a
/// small penalty because they represent more outstanding work.
///
/// The table is surfaced in the app's preferences so the behaviour is not
/// mysterious to the user.
pub fn priority(task: &Task, today: NaiveDate, has_open_subtasks: bool) -> u8 {
    let base = match task.due {
        Some(due) if due < today => 0,
        Some(due) if due == today => 1,
        Some(due) if due <= today + chrono::Duration::days(3) => 2,
        // Later or undated.
        _ => 3,
    };

    let mut score = base;

    if !task.notes.trim().is_empty() {
        score += 1;
    }
    if has_open_subtasks {
        score += 1;
    }

    score
}

/// Ids of tasks that still have at least one incomplete subtask.
fn parents_with_open_subtasks(tasks: &[Task]) -> std::collections::HashSet<String> {
    tasks
        .iter()
        .filter(|task| !task.is_completed())
        .filter_map(|task| task.parent.clone())
        .collect()
}

/// A due-date bucket, used by `GroupMode::ByDue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueBucket {
    Overdue,
    Today,
    Tomorrow,
    ThisWeek,
    Later,
    NoDate,
}

impl DueBucket {
    pub const ORDER: [DueBucket; 6] = [
        DueBucket::Overdue,
        DueBucket::Today,
        DueBucket::Tomorrow,
        DueBucket::ThisWeek,
        DueBucket::Later,
        DueBucket::NoDate,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DueBucket::Overdue => "Overdue",
            DueBucket::Today => "Today",
            DueBucket::Tomorrow => "Tomorrow",
            DueBucket::ThisWeek => "This week",
            DueBucket::Later => "Later",
            DueBucket::NoDate => "No date",
        }
    }

    pub fn of(task: &Task, today: NaiveDate) -> Self {
        match task.due {
            None => DueBucket::NoDate,
            Some(due) if due < today => DueBucket::Overdue,
            Some(due) if due == today => DueBucket::Today,
            Some(due) if due == today + chrono::Duration::days(1) => DueBucket::Tomorrow,
            // "This week" means the rest of the current calendar week, which is
            // what a person means by it, rather than a rolling seven days.
            Some(due) if due <= end_of_week(today) => DueBucket::ThisWeek,
            Some(_) => DueBucket::Later,
        }
    }
}

fn end_of_week(today: NaiveDate) -> NaiveDate {
    // ISO weeks end on Sunday: 7 days from Monday.
    let monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    monday + chrono::Duration::days(6)
}

/// Today's date in the local timezone.
pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::models::TaskStatus;
    use chrono::{TimeZone, Utc};

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn task(id: &str, due: Option<NaiveDate>) -> Task {
        Task {
            id: id.into(),
            title: id.to_string(),
            notes: String::new(),
            status: TaskStatus::NeedsAction,
            due,
            completed: None,
            updated: Some(Utc.with_ymd_and_hms(2026, 10, 1, 9, 0, 0).unwrap()),
            parent: None,
            previous: None,
            // Position is the API's opaque ordering key; using the id keeps
            // manual-sort expectations readable in these tests.
            position: Some(id.to_string()),
            etag: None,
            hidden: false,
            deleted: false,
        }
    }

    fn done(id: &str) -> Task {
        let mut t = task(id, None);
        t.status = TaskStatus::Completed;
        t.completed = Some(Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap());
        t
    }

    fn ids(tasks: &[Task]) -> Vec<&str> {
        tasks.iter().map(|t| t.id.as_str()).collect()
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()
    }

    #[test]
    fn today_view_only_holds_tasks_due_today() {
        let tasks = vec![
            task("t", Some(today())),
            task("tm", Some(today() + chrono::Duration::days(1))),
            task("n", None),
        ];
        let result = assemble(&View::Today, tasks, today(), SortMode::Manual);
        assert_eq!(ids(&result), vec!["t"]);
    }

    #[test]
    fn upcoming_covers_the_next_seven_days_and_excludes_today() {
        let tasks = vec![
            task("yest", Some(today() - chrono::Duration::days(1))),
            task("today", Some(today())),
            task("d3", Some(today() + chrono::Duration::days(3))),
            task("d7", Some(today() + chrono::Duration::days(7))),
            task("d8", Some(today() + chrono::Duration::days(8))),
        ];
        let result = assemble(&View::Upcoming, tasks, today(), SortMode::Manual);
        assert_eq!(ids(&result), vec!["d3", "d7"]);
    }

    #[test]
    fn overdue_excludes_undated_tasks() {
        // An unscheduled task is not late; it just has no date, which is a
        // different thing and belongs in the All view.
        let tasks = vec![
            task("late", Some(today() - chrono::Duration::days(2))),
            task("today", Some(today())),
            task("none", None),
        ];
        let result = assemble(&View::Overdue, tasks, today(), SortMode::Manual);
        assert_eq!(ids(&result), vec!["late"]);
    }

    #[test]
    fn view_keys_are_unique() {
        let views = [
            View::Today,
            View::Upcoming,
            View::Overdue,
            View::All,
            View::Completed,
            View::List("@abc123".into()),
            View::List("@def456".into()),
        ];
        let mut keys: Vec<String> = views.iter().map(View::key).collect();
        let count = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), count, "view keys must be unique");
    }

    #[test]
    fn list_view_keys_are_usable_as_widget_names() {
        // Widget names cannot contain characters that would break a CSS
        // selector or a path lookup.
        let key = View::List("@MDQ6V2Zpc2hlZEdyb3Vw".into()).key();
        assert!(
            key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "got {key}"
        );
    }

    #[test]
    fn only_the_completed_view_shows_completed_tasks() {
        let tasks = vec![task("open", None), done("closed")];

        for view in [
            View::Today,
            View::Upcoming,
            View::Overdue,
            View::All,
            View::List("@a".into()),
        ] {
            let result = assemble(&view, tasks.clone(), today(), SortMode::Manual);
            assert!(
                !result.iter().any(|t| t.is_completed()),
                "{view:?} must not show completed tasks"
            );
        }

        let result = assemble(&View::Completed, tasks, today(), SortMode::Manual);
        assert_eq!(ids(&result), vec!["closed"]);
    }

    #[test]
    fn all_view_keeps_every_open_task() {
        let tasks = vec![
            task("a", Some(today() - chrono::Duration::days(1))),
            task("b", Some(today())),
            task("c", None),
            done("d"),
        ];
        let result = assemble(&View::All, tasks, today(), SortMode::Manual);
        assert_eq!(ids(&result), vec!["a", "b", "c"]);
    }

    #[test]
    fn due_date_sort_puts_undated_tasks_last() {
        let tasks = vec![
            task("none", None),
            task("late", Some(today() - chrono::Duration::days(1))),
            task("soon", Some(today())),
        ];
        let result = assemble(&View::All, tasks, today(), SortMode::DueDate);
        assert_eq!(
            ids(&result),
            vec!["late", "soon", "none"],
            "a task with no deadline must not outrank one that is due"
        );
    }

    #[test]
    fn alphabetical_sort_ignores_case() {
        let mut tasks = Vec::new();
        for (id, title) in [("a", "banana"), ("b", "Apple"), ("c", "cherry")] {
            let mut t = task(id, None);
            t.title = title.into();
            tasks.push(t);
        }
        let result = assemble(&View::All, tasks, today(), SortMode::Alphabetical);
        assert_eq!(
            result.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            vec!["Apple", "banana", "cherry"]
        );
    }

    #[test]
    fn manual_sort_uses_google_position_order() {
        let mut a = task("a", None);
        a.position = Some("aaa".into());
        let mut b = task("b", None);
        b.position = Some("bbb".into());
        let result = assemble(&View::All, vec![b, a], today(), SortMode::Manual);
        assert_eq!(ids(&result), vec!["a", "b"]);
    }

    #[test]
    fn priority_ranks_overdue_before_today_before_soon_before_later() {
        let scores = [
            priority(
                &task("late", Some(today() - chrono::Duration::days(1))),
                today(),
                false,
            ),
            priority(&task("now", Some(today())), today(), false),
            priority(
                &task("soon", Some(today() + chrono::Duration::days(2))),
                today(),
                false,
            ),
            priority(
                &task("far", Some(today() + chrono::Duration::days(30))),
                today(),
                false,
            ),
            priority(&task("undated", None), today(), false),
        ];

        assert_eq!(scores[0], 0, "overdue is the most urgent");
        assert_eq!(scores[1], 1);
        assert_eq!(scores[2], 2);
        assert_eq!(scores[3], 3);
        assert_eq!(scores[4], 3, "an undated task is not urgent");
        assert!(
            scores.windows(2).all(|w| w[0] <= w[1]),
            "scores must be non-decreasing across the table"
        );
    }

    #[test]
    fn priority_adds_a_penalty_for_notes_and_open_subtasks() {
        let plain = task("plain", Some(today()));
        assert_eq!(priority(&plain, today(), false), 1);

        let mut noted = task("noted", Some(today()));
        noted.notes = "has detail".into();
        assert_eq!(priority(&noted, today(), false), 2, "notes mean more work");

        assert_eq!(
            priority(&plain, today(), true),
            2,
            "an outstanding subtask is outstanding work"
        );
    }

    #[test]
    fn only_parents_with_open_subtasks_are_penalised() {
        let mut child_open = task("child", None);
        child_open.parent = Some("parent".into());
        let mut child_done = done("child2");
        child_done.parent = Some("parent".into());

        let parents = parents_with_open_subtasks(&[child_open, child_done]);
        assert!(parents.contains("parent"), "an open child counts");

        // Both children complete, so the parent is no longer penalised.
        let parents = parents_with_open_subtasks(&[done("child2")]);
        assert!(!parents.contains("parent"));
    }

    #[test]
    fn priority_sort_uses_the_score() {
        let mut urgent = task("z-late", Some(today() - chrono::Duration::days(5)));
        urgent.title = "zzz".into();
        let mut relaxed = task("a-none", None);
        relaxed.title = "aaa".into();

        let result = assemble(
            &View::All,
            vec![relaxed, urgent],
            today(),
            SortMode::Priority,
        );
        assert_eq!(
            ids(&result),
            vec!["z-late", "a-none"],
            "an overdue task outranks an alphabetically earlier unscheduled one"
        );
    }

    #[test]
    fn due_buckets_partition_tasks_correctly() {
        // 2026-10-01 is a Thursday; the ISO week runs Monday to Sunday, so
        // "this week" ends on the 4th.
        let assert_bucket = |due: Option<NaiveDate>, expected: DueBucket| {
            assert_eq!(
                DueBucket::of(&task("t", due), today()),
                expected,
                "for {due:?}"
            );
        };

        assert_bucket(Some(day(2026, 9, 30)), DueBucket::Overdue);
        assert_bucket(Some(day(2026, 10, 1)), DueBucket::Today);
        assert_bucket(Some(day(2026, 10, 2)), DueBucket::Tomorrow);
        assert_bucket(Some(day(2026, 10, 4)), DueBucket::ThisWeek);
        assert_bucket(Some(day(2026, 10, 5)), DueBucket::Later);
        assert_bucket(None, DueBucket::NoDate);
    }

    #[test]
    fn due_buckets_cover_every_bucket_in_order() {
        // Guards against a due date falling into no bucket at all, which would
        // make it vanish from a grouped list.
        for offset in -400..400 {
            let due = today() + chrono::Duration::days(offset);
            let bucket = DueBucket::of(&task("t", Some(due)), today());
            assert!(
                DueBucket::ORDER.contains(&bucket),
                "{due} landed in no bucket"
            );
        }
    }

    #[test]
    fn bucket_labels_are_unique() {
        let labels: Vec<_> = DueBucket::ORDER.iter().map(|b| b.label()).collect();
        let unique: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len());
    }
}
