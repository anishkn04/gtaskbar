use adw::prelude::*;
use gtk::glib;

use super::icons;
use crate::model::view::{self, DueBucket, View};
use crate::store::models::Task;

/// One selectable entry in the sidebar, with its badge count.
pub struct Entry {
    pub view: View,
    pub title: String,
    pub icon: &'static str,
    /// Tasks that still need attention. Drives the badge.
    pub count: usize,
}

impl Entry {
    fn smart(view: View, icon: &'static str) -> Self {
        Self {
            title: view.title(),
            view,
            icon,
            count: 0,
        }
    }
}

/// Builds the sidebar entries, with counts taken from the cached tasks.
///
/// Smart views come first because they are the ones people reach for daily;
/// the account's own lists follow under a heading.
///
/// Each list arrives with its own outstanding count, computed by the caller
/// from that list's tasks. Counting the whole slice once per row used to show
/// every list badged with the account total, so an empty list wore another
/// list's number.
pub fn entries(tasks: &[Task], lists: &[(String, String, usize)]) -> Vec<Entry> {
    let today = view::today();

    let mut smart = vec![
        Entry::smart(View::Today, icons::TODAY),
        Entry::smart(View::Upcoming, icons::UPCOMING),
        Entry::smart(View::Overdue, icons::OVERDUE),
        Entry::smart(View::All, icons::ALL),
        Entry::smart(View::Completed, icons::COMPLETED),
    ];

    for entry in &mut smart {
        entry.count = tasks
            .iter()
            .filter(|task| entry.view.matches(task, today))
            .count();
    }

    let mut out = smart;

    for (id, name, count) in lists {
        out.push(Entry {
            view: View::List(id.clone()),
            title: name.clone(),
            icon: icons::LIST,
            count: *count,
        });
    }

    out
}

/// Builds the sidebar's list box.
/// Builds the sidebar's list box.
///
/// Each row's widget name is its view's key, which is how the selection handler
/// in `window.rs` finds the matching content page without having to rediscover
/// the mapping by walking the widget tree.
pub fn build(tasks: &[Task], lists: &[(String, String, usize)]) -> gtk::ListBox {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(["navigation-sidebar"])
        .build();
    list.add_css_class("gtaskbar-sidebar");

    let entries = entries(tasks, lists);
    let smart_count = entries
        .iter()
        .filter(|e| !matches!(e.view, View::List(_)))
        .count();

    for (index, entry) in entries.iter().enumerate() {
        // A heading between the smart views and the account's own lists, since
        // they are different kinds of thing.
        if index == smart_count && smart_count > 0 {
            list.append(&heading_row("Task lists"));
        }

        let row = row_for(entry);
        row.set_widget_name(&entry.view.key());
        list.append(&row);
    }

    list
}

fn heading_row(label: &str) -> gtk::ListBoxRow {
    let text = gtk::Label::builder().label(label).xalign(0.0).build();
    text.add_css_class("gtaskbar-group-header");
    gtk::ListBoxRow::builder()
        .activatable(false)
        .selectable(false)
        .child(&text)
        .build()
}

fn row_for(entry: &Entry) -> gtk::ListBoxRow {
    // Built from a plain box rather than an AdwActionRow. AdwActionRow's suffix
    // area fills its child vertically regardless of the child's own valign, so
    // the count badge stretched to the full row height however it was styled or
    // sized. A box honours valign, and the row needs no row chrome of its own:
    // it sits inside a navigation-sidebar list, which already provides the
    // hover and selection styling.
    let icon = icons::image(entry.icon);
    icon.set_pixel_size(16);

    let title = gtk::Label::builder()
        .label(&entry.title)
        .xalign(0.0)
        .build();
    title.set_hexpand(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    content.set_margin_top(6);
    content.set_margin_bottom(6);
    content.set_margin_start(6);
    content.set_margin_end(6);
    content.append(&icon);
    content.append(&title);

    if entry.count > 0 {
        let badge = gtk::Label::builder().label(entry.count.to_string()).build();
        badge.add_css_class("gtaskbar-badge");
        badge.set_valign(gtk::Align::Center);
        content.append(&badge);
    }

    gtk::ListBoxRow::builder()
        .activatable(true)
        .child(&content)
        .build()
}

pub fn due_chip(task: &Task) -> Option<gtk::Label> {
    let today = view::today();
    let due = task.due?;

    let (label, class) = if due < today {
        (format!("Overdue · {}", due.format("%-d %b")), "overdue")
    } else if due == today {
        ("Today".to_string(), "today")
    } else if due == today + chrono::Duration::days(1) {
        ("Tomorrow".to_string(), "soon")
    } else {
        (due.format("%-d %b").to_string(), "later")
    };

    let chip = gtk::Label::builder().label(label).build();
    chip.add_css_class("gtaskbar-due-chip");
    chip.add_css_class(class);
    // Same reason as the sidebar badge: a label in a box fills it by default.
    chip.set_valign(gtk::Align::Center);
    chip.set_size_request(-1, 17);
    Some(chip)
}

/// The same grouping over `(list id, task)` pairs, preserving membership so
/// callers that render per-list rows never have to re-derive it.
pub fn group_due_pairs(items: &[(String, Task)]) -> Vec<(&'static str, Vec<(String, Task)>)> {
    let today = view::today();
    let mut sections: Vec<(&'static str, Vec<(String, Task)>)> = DueBucket::ORDER
        .iter()
        .map(|b| (b.label(), Vec::new()))
        .collect();

    for (list_id, task) in items {
        let index = DueBucket::ORDER
            .iter()
            .position(|b| *b == DueBucket::of(task, today))
            .unwrap_or(DueBucket::ORDER.len() - 1);
        sections[index].1.push((list_id.clone(), task.clone()));
    }

    sections.retain(|(_, items)| !items.is_empty());
    sections
}

/// Registers the window-level actions the task context menu triggers.
///
/// They live on the window rather than on each row so the menu can be plain
/// Gio data. Each menu carries its task as a `(list id, task id)` target, so
/// the handlers never have to rediscover which row was opened.
pub fn register_actions(window: &adw::ApplicationWindow) {
    use gtk::gio;

    // Every menu write repaints, syncs now, and says so on failure. A queued
    // change that stays invisible until the next poll reads as a dead menu.
    let due_today = gio::ActionEntry::builder("due-today")
        .parameter_type(Some(glib::VariantTy::STRING))
        .activate(|window: &adw::ApplicationWindow, _, id| {
            let Some(id) = id.and_then(|value| value.get::<String>()) else {
                return;
            };
            apply_menu_write(
                window,
                crate::sync::queue::set_due(&id, Some(view::today())),
            );
        })
        .build();

    let due_tomorrow = gio::ActionEntry::builder("due-tomorrow")
        .parameter_type(Some(glib::VariantTy::STRING))
        .activate(|window: &adw::ApplicationWindow, _, id| {
            let Some(id) = id.and_then(|value| value.get::<String>()) else {
                return;
            };
            let tomorrow = view::today() + chrono::Duration::days(1);
            apply_menu_write(window, crate::sync::queue::set_due(&id, Some(tomorrow)));
        })
        .build();

    let clear_due = gio::ActionEntry::builder("clear-due")
        .parameter_type(Some(glib::VariantTy::STRING))
        .activate(|window: &adw::ApplicationWindow, _, id| {
            let Some(id) = id.and_then(|value| value.get::<String>()) else {
                return;
            };
            // `None` means "clear", which the API needs as an explicit null.
            apply_menu_write(window, crate::sync::queue::set_due(&id, None));
        })
        .build();

    let delete_task = gio::ActionEntry::builder("delete-task")
        .parameter_type(Some(glib::VariantTy::STRING))
        .activate(|window: &adw::ApplicationWindow, _, id| {
            let Some(id) = id.and_then(|value| value.get::<String>()) else {
                return;
            };
            apply_menu_write(window, crate::sync::queue::delete_task(&id));
        })
        .build();

    let reopen_task = gio::ActionEntry::builder("reopen-task")
        .parameter_type(Some(glib::VariantTy::STRING))
        .activate(|window: &adw::ApplicationWindow, _, id| {
            let Some(id) = id.and_then(|value| value.get::<String>()) else {
                return;
            };
            apply_menu_write(
                window,
                crate::sync::queue::set_status(&id, crate::store::models::TaskStatus::NeedsAction),
            );
        })
        .build();

    let edit_task = gio::ActionEntry::builder("edit-task")
        .parameter_type(Some(glib::VariantTy::STRING))
        .activate(|window: &adw::ApplicationWindow, _, id| {
            let Some(id) = id.and_then(|value| value.get::<String>()) else {
                return;
            };
            crate::ui::editor::present(window.upcast_ref(), &id);
        })
        .build();

    window.add_action_entries([
        due_today,
        due_tomorrow,
        clear_due,
        delete_task,
        reopen_task,
        edit_task,
    ]);
}

/// Settles a menu-initiated write: repaint and sync on success, say so on
/// failure. The queue functions already applied the change to the cache, so
/// the repaint shows it at once instead of waiting for the next sync.
fn apply_menu_write(window: &adw::ApplicationWindow, outcome: Result<(), String>) {
    let ok = outcome.is_ok();
    if let Err(reason) = outcome {
        log::warn!("{reason}");
        if let Some(app) = crate::running_app() {
            crate::ui::window::set_status(&app, Some(&reason));
        }
    }
    refresh(window);
    if ok {
        if let Some(app) = crate::running_app() {
            crate::sync::scheduler::request_sync(&app);
        }
    }
}

/// Re-reads the cache and repaints, so a queued change appears immediately.
fn refresh(window: &adw::ApplicationWindow) {
    let app = window
        .application()
        .and_downcast::<adw::Application>()
        .expect("the window belongs to an adw::Application");
    crate::ui::window::rebuild(&app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::models::TaskStatus;
    use chrono::NaiveDate;

    fn paired(tasks: &[Task]) -> Vec<(String, Task)> {
        tasks
            .iter()
            .cloned()
            .map(|task| (String::new(), task))
            .collect()
    }

    fn task(id: &str, due: Option<NaiveDate>, status: TaskStatus) -> Task {
        Task {
            id: id.into(),
            title: id.into(),
            notes: String::new(),
            status,
            due,
            completed: None,
            updated: None,
            parent: None,
            previous: None,
            position: None,
            etag: None,
            hidden: false,
            deleted: false,
        }
    }

    #[test]
    fn smart_views_come_before_lists() {
        let names = vec![("@a".to_string(), "My Tasks".to_string(), 0)];
        let built = entries(&[], &names);

        let list_positions: Vec<usize> = built
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e.view, View::List(_)))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(list_positions, vec![5], "the list should come last");
    }

    #[test]
    fn smart_view_counts_reflect_the_tasks() {
        let today = view::today();
        let tasks = vec![
            task("a", Some(today), TaskStatus::NeedsAction),
            task(
                "b",
                Some(today - chrono::Duration::days(3)),
                TaskStatus::NeedsAction,
            ),
            task(
                "c",
                Some(today + chrono::Duration::days(2)),
                TaskStatus::NeedsAction,
            ),
            task("d", None, TaskStatus::Completed),
        ];

        let built = entries(&tasks, &[]);
        let count = |v: &View| built.iter().find(|e| &e.view == v).unwrap().count;

        assert_eq!(count(&View::Today), 1);
        assert_eq!(count(&View::Overdue), 1);
        assert_eq!(count(&View::Upcoming), 1);
        assert_eq!(count(&View::Completed), 1);
    }

    #[test]
    fn a_list_entry_carries_its_own_count() {
        // The count arrives scoped to the list by the caller. Counting the
        // whole slice once per row used to badge every list with the account
        // total, so an empty list wore another list's number while showing
        // "Nothing here".
        let tasks = vec![
            task("a", None, TaskStatus::NeedsAction),
            task("b", None, TaskStatus::NeedsAction),
            task("c", None, TaskStatus::NeedsAction),
        ];
        let built = entries(&tasks, &[("@a".into(), "My Tasks".into(), 0)]);
        let list_entry = built
            .iter()
            .find(|e| matches!(e.view, View::List(_)))
            .expect("list entry");
        assert_eq!(
            list_entry.count, 0,
            "an empty list must show no badge even when other lists have open tasks"
        );
    }

    #[test]
    fn a_nonempty_list_entry_shows_its_count() {
        let tasks = vec![task("a", None, TaskStatus::NeedsAction)];
        let built = entries(&tasks, &[("@a".into(), "My Tasks".into(), 1)]);
        let list_entry = built
            .iter()
            .find(|e| matches!(e.view, View::List(_)))
            .expect("list entry");
        assert_eq!(list_entry.count, 1);
    }

    #[test]
    fn views_without_tasks_have_no_badge() {
        let built = entries(&[], &[]);
        assert!(built.iter().all(|e| e.count == 0));
    }

    #[test]
    fn grouping_omits_empty_sections() {
        let today = view::today();
        let tasks = vec![
            task(
                "late",
                Some(today - chrono::Duration::days(1)),
                TaskStatus::NeedsAction,
            ),
            task("now", Some(today), TaskStatus::NeedsAction),
        ];
        let sections = group_due_pairs(&paired(&tasks));
        let labels: Vec<_> = sections.iter().map(|(l, _)| *l).collect();
        assert_eq!(
            labels,
            vec!["Overdue", "Today"],
            "empty buckets are dropped"
        );
    }

    #[test]
    fn grouping_orders_sections_by_urgency() {
        let today = view::today();
        let tasks = vec![
            task("none", None, TaskStatus::NeedsAction),
            task(
                "later",
                Some(today + chrono::Duration::days(60)),
                TaskStatus::NeedsAction,
            ),
            task("now", Some(today), TaskStatus::NeedsAction),
        ];
        let labels: Vec<_> = group_due_pairs(&paired(&tasks))
            .iter()
            .map(|(l, _)| *l)
            .collect();
        assert_eq!(labels, vec!["Today", "Later", "No date"]);
    }

    #[test]
    fn an_undated_task_still_lands_in_a_group() {
        // A task that fits no section would silently vanish from a grouped list.
        let tasks = vec![task("none", None, TaskStatus::NeedsAction)];
        let sections = group_due_pairs(&paired(&tasks));
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, "No date");
    }
}
