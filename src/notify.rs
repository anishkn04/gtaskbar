use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use adw::prelude::*;
use chrono::Timelike;
use gtk::gio;

use crate::config::Config;
use crate::model::view;
use crate::store::models::{Task, TaskStatus};
use crate::store::Store;

/// How often the due-date check runs.
///
/// Once a minute: cheap, and fine enough that a task becoming due is noticed
/// within a minute of the hour it is scheduled for.
const TICK: Duration = Duration::from_secs(60);

/// The action behind each notification's Complete button.
///
/// Must carry the `app.` prefix: GIO routes notification actions by
/// `app.`- and `win.`-prefixed names, and logs a warning for anything else.
/// An unprefixed name looks like it works and silently does not, so the
/// registration has to match this string exactly.
///
/// A single action serves every task, with the task's identity passed as the
/// target value.
const ACTION_COMPLETE: &str = "app.complete-task";

/// Notification id for the batch of tasks that became due today.
const ID_DUE_TODAY: &str = "gtaskbar-due-today";

/// Notification id for the batch of tasks that went overdue.
const ID_OVERDUE: &str = "gtaskbar-overdue";

/// Registers the action the notification buttons dispatch to.
pub fn register_actions(app: &adw::Application) {
    let complete = gio::ActionEntry::builder("complete-task")
        .activate(|app: &adw::Application, _, parameter| {
            let Some(task_id) = parameter.and_then(|value| value.str()) else {
                return;
            };
            let task_id = task_id.to_string();

            crate::sync::queue::set_status(&task_id, TaskStatus::Completed);
            crate::sync::scheduler::request_sync(app);

            // Re-sending under the same id replaces the notification rather than
            // stacking another, which is how a completed task leaves the list.
            // There is no way to dismiss by id from here, so replacement is the
            // mechanism. Both batches are refreshed because a task that is
            // overdue is also a candidate for the due-today batch, and the
            // action does not say which one it came from.
            for batch in [Batch::DueToday, Batch::Overdue] {
                refresh_batch(app, batch);
            }
        })
        .build();

    app.add_action_entries([complete]);
}

/// What a batch of notifications is about, so it can be re-sent after a
/// completion changes the membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Batch {
    DueToday,
    Overdue,
}

impl Batch {
    fn id(self) -> &'static str {
        match self {
            Batch::DueToday => ID_DUE_TODAY,
            Batch::Overdue => ID_OVERDUE,
        }
    }

    fn notification_kind(self) -> &'static str {
        match self {
            Batch::DueToday => "due_today",
            Batch::Overdue => "overdue",
        }
    }

    fn phrase(self) -> &'static str {
        match self {
            Batch::DueToday => "due today",
            Batch::Overdue => "overdue",
        }
    }
}

thread_local! {
    // The tasks last sent for each batch, so a completion can re-render the
    // rest. Not const-initialised because HashMap::new is not const.
    static LAST_SENT: RefCell<HashMap<&'static str, Vec<Task>>> =
        RefCell::new(HashMap::new());
}

/// Re-sends a batch without the task that was just completed.
fn refresh_batch(app: &adw::Application, batch: Batch) {
    let remaining: Vec<Task> = LAST_SENT.with(|sent| {
        let mut map = sent.borrow_mut();
        let entry = map.entry(batch.id()).or_default();
        entry.retain(|task| !task.is_completed());
        entry.clone()
    });

    send(app, batch, &remaining);
}

/// Starts the notification timer.
pub fn init(app: &adw::Application) {
    glib::timeout_add_local(TICK, {
        let app = app.clone();
        move || {
            check_due_dates(&app);
            glib::ControlFlow::Continue
        }
    });
}

/// Notifies about tasks that became due, and about tasks that went overdue.
///
/// Both are deduplicated through the `notified` table, so a task that stays due
/// does not re-notify on every tick.
pub fn check_due_dates(app: &adw::Application) {
    let config = Config::load();
    let today = view::today();
    let now_hour = chrono::Local::now().hour();

    // The list id travels with each task because the dedupe is keyed on
    // (list, task), and the task itself carries no list.
    let tasks = crate::sync::scheduler::with_store(|cache| cache.all_tasks_with_lists())
        .and_then(|result| result.ok())
        .unwrap_or_default();

    if tasks.is_empty() {
        return;
    }

    let (became_due, newly_overdue) = crate::sync::scheduler::with_store(|cache| {
        let mut due = Vec::new();
        let mut overdue = Vec::new();

        for (list_id, task) in &tasks {
            if task.is_completed() {
                continue;
            }
            let Some(due_date) = task.due else {
                continue;
            };

            // The notify hour gates the "due today" batch only: an overdue task
            // is already late, so waiting until a configured hour would only
            // delay bad news.
            let batch = if due_date < today {
                Batch::Overdue
            } else if due_date == today && now_hour >= config.notify_hour {
                Batch::DueToday
            } else {
                continue;
            };

            if first_time(cache, list_id, task, batch.notification_kind()) {
                match batch {
                    Batch::Overdue => overdue.push(task.clone()),
                    Batch::DueToday => due.push(task.clone()),
                }
            }
        }

        (due, overdue)
    })
    .unwrap_or_default();

    if !became_due.is_empty() {
        send(app, Batch::DueToday, &became_due);
    }

    if config.notify_overdue && !newly_overdue.is_empty() {
        send(app, Batch::Overdue, &newly_overdue);
    }
}

/// Whether this task has not already produced this kind of notification.
///
/// Recording on the first sighting is what makes the check idempotent across
/// ticks.
fn first_time(store: &Store, list_id: &str, task: &Task, kind: &str) -> bool {
    match store.mark_notified(list_id, &task.id, kind) {
        Ok(is_first) => is_first,
        Err(err) => {
            log::warn!("could not record the notification for {}: {err}", task.id);
            // Failing to record means a notification may repeat, which is
            // better than silently never notifying.
            true
        }
    }
}

/// Sends one notification for a batch, with a Complete button per task.
fn send(app: &adw::Application, batch: Batch, tasks: &[Task]) {
    LAST_SENT.with(|sent| {
        sent.borrow_mut().insert(batch.id(), tasks.to_vec());
    });

    // An empty batch still sends, deliberately: re-sending under the same id is
    // what replaces the previous notification, so sending nothing would leave
    // the stale one on screen.
    let notification = gio::Notification::new(&summarise(tasks.len(), batch.phrase()));
    notification.set_icon(&gio::ThemedIcon::new(crate::ui::icons::APP_SYMBOLIC));

    if tasks.len() == 1 {
        let task = &tasks[0];
        if let Some(due) = task.due {
            notification.set_body(Some(&format!("Due {}", due.format("%-d %b"))));
        }
    }

    // A button per task, carrying that task's id as the target value.
    for task in tasks.iter().take(5) {
        notification.add_button_with_target_value(
            &format!("Complete \u{201c}{}\u{201d}", truncate(&task.title, 40)),
            ACTION_COMPLETE,
            Some(&glib::Variant::from(task.id.as_str())),
        );
    }

    notification.set_default_action("app.show-window");
    app.send_notification(Some(batch.id()), &notification);
}

fn summarise(count: usize, phrase: &str) -> String {
    match count {
        1 => format!("1 task {phrase}"),
        n => format!("{n} tasks {phrase}"),
    }
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_keeps_short_titles_whole() {
        assert_eq!(truncate("Buy milk", 40), "Buy milk");
    }

    #[test]
    fn truncation_marks_ellipsis_and_respects_the_limit() {
        let short = truncate(&"a".repeat(100), 10);
        assert!(short.chars().count() <= 10, "got {short:?}");
        assert!(short.ends_with('…'), "got {short:?}");
    }

    #[test]
    fn truncation_does_not_split_a_multibyte_character() {
        // Counting bytes would panic on a char boundary; counting chars is the
        // whole point.
        let short = truncate("日本語のタスク名", 4);
        assert!(short.chars().count() <= 4, "got {short:?}");
    }

    #[test]
    fn summaries_count_the_tasks() {
        assert_eq!(summarise(1, "due today"), "1 task due today");
        assert_eq!(summarise(3, "overdue"), "3 tasks overdue");
    }

    #[test]
    fn batch_ids_are_distinct() {
        assert_ne!(Batch::DueToday.id(), Batch::Overdue.id());
    }

    #[test]
    fn batch_ids_are_namespaced() {
        for batch in [Batch::DueToday, Batch::Overdue] {
            assert!(
                batch.id().starts_with("gtaskbar-"),
                "{} is not namespaced",
                batch.id()
            );
        }
    }
}
