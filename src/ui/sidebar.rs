use adw::prelude::*;
use gtk::gio;

use super::icons;
use crate::model::view::{self, DueBucket, View};
use crate::store::models::{Task, TaskStatus};

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
pub fn entries(tasks: &[Task], list_names: &[(String, String)]) -> Vec<Entry> {
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

    for (id, name) in list_names {
        let count = tasks.iter().filter(|task| !task.is_completed()).count();
        out.push(Entry {
            view: View::List(id.clone()),
            title: name.clone(),
            icon: icons::LIST,
            count,
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
pub fn build(tasks: &[Task], list_names: &[(String, String)]) -> gtk::ListBox {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(["navigation-sidebar"])
        .build();
    list.add_css_class("gtaskbar-sidebar");

    let entries = entries(tasks, list_names);
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
    let action = adw::ActionRow::builder()
        .title(&entry.title)
        .activatable(true)
        .build();

    action.add_prefix(&icons::image(entry.icon));

    if entry.count > 0 {
        let badge = gtk::Label::builder().label(entry.count.to_string()).build();
        badge.add_css_class("gtaskbar-badge");
        action.add_suffix(&badge);
    }

    gtk::ListBoxRow::builder()
        .activatable(true)
        .child(&action)
        .build()
}

/// A single task row: checkbox, title, notes preview and a due-date chip.
pub fn task_row(task: &Task, subtask_depth: u32) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(&task.title)
        .activatable(false)
        .build();
    row.add_css_class("gtaskbar-task-row");

    if task.is_completed() {
        row.add_css_class("done");
    }

    // Subtasks are indented to mirror Google Tasks' own hierarchy.
    if subtask_depth > 0 {
        row.add_css_class("gtaskbar-subtask");
    }

    let check = gtk::CheckButton::builder()
        .active(task.is_completed())
        .valign(gtk::Align::Center)
        .build();
    check.add_css_class("gtaskbar-check");
    check.add_css_class("flat");
    check.set_tooltip_text(Some(if task.is_completed() {
        "Mark as not done"
    } else {
        "Mark as done"
    }));

    // Clicking the box queues the change; the row re-renders from the cache,
    // so the checkbox is not toggled here directly.
    let task_id = task.id.clone();
    let was_completed = task.is_completed();
    check.connect_toggled(move |button| {
        if button.is_active() == was_completed {
            return;
        }
        let next = if button.is_active() {
            TaskStatus::Completed
        } else {
            TaskStatus::NeedsAction
        };
        crate::sync::queue::set_status(&task_id, next);
    });

    if task.is_completed() {
        // A themed tick reads as "done" more clearly than a dimmed empty box,
        // and reuses the icon set rather than a third visual language.
        let tick = icons::image(icons::CHECK);
        tick.add_css_class("gtaskbar-done-tick");
        row.add_prefix(&tick);
        row.remove(&check);
    } else {
        row.add_prefix(&check);
    }

    // A compact preview of the notes, so a task with detail is distinguishable
    // at a glance without opening it.
    if !task.notes.trim().is_empty() {
        let first_line = task
            .notes
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        if !first_line.is_empty() {
            row.set_subtitle(&first_line);
            row.set_subtitle_lines(1);
        }
    }

    if let Some(bucket) = due_chip(task) {
        row.add_suffix(&bucket);
    }

    attach_menu(&row, task);
    row
}

/// Gives a task row a right-click menu.
///
/// A menu rather than inline controls: a task row already carries a checkbox, a
/// title, a notes preview and a due chip, and adding buttons for every action
/// would crowd out the text at any reasonable width.
fn attach_menu(row: &adw::ActionRow, task: &Task) {
    let task_id = task.id.clone();

    let menu = gio::Menu::new();

    // Gio models rather than widgets: the menu is data, and building it as
    // widgets would be a lot of code for no benefit.
    let due_section = gio::Menu::new();
    if task.due.is_some() {
        due_section.append(Some("Clear due date"), Some("win.clear-due"));
    } else {
        due_section.append(Some("Due today"), Some("win.due-today"));
        due_section.append(Some("Due tomorrow"), Some("win.due-tomorrow"));
    }
    menu.append_section(None, &due_section);

    let edit = gio::Menu::new();
    edit.append(Some("Edit…"), Some("win.edit-task"));
    menu.append_section(None, &edit);

    let danger = gio::Menu::new();
    danger.append(Some("Delete"), Some("win.delete-task"));
    menu.append_section(None, &danger);

    let popup = gtk::PopoverMenu::from_model(Some(&menu));
    popup.set_parent(row);
    popup.set_has_arrow(false);
    // ActionRow has no secondary-click target, so a right-click is handled by
    // listening for the button directly on the row.
    let controller = gtk::GestureClick::new();
    controller.set_button(gdk::BUTTON_SECONDARY);
    let popup_secondary = popup.clone();
    controller.connect_pressed(glib::clone!(
        #[weak]
        popup_secondary,
        move |_gesture, n_press, _x, _y| {
            if n_press != 1 {
                return;
            }
            popup_secondary.popup();
        }
    ));
    row.add_controller(controller);

    row.connect_activate(glib::clone!(
        #[weak]
        popup,
        move |row| {
            // Activating the row opens the same menu as a secondary click.
            popup.set_pointing_to(Some(&gdk::Rectangle::new(0, 0, row.width(), 0)));
            popup.popup();
        }
    ));

    // The actions act on whichever row the menu was opened from, so the id is
    // recorded on the popover's parent row and read back by each action.
    row.set_widget_name(&format!("{TASK_ROW_PREFIX}{task_id}"));
}

fn due_chip(task: &Task) -> Option<gtk::Label> {
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
    Some(chip)
}

/// Splits tasks into the sections a grouped list shows, in bucket order.
pub fn group_due(tasks: &[Task]) -> Vec<(&'static str, Vec<Task>)> {
    let today = view::today();
    let mut sections: Vec<(&'static str, Vec<Task>)> = DueBucket::ORDER
        .iter()
        .map(|b| (b.label(), Vec::new()))
        .collect();

    for task in tasks {
        let index = DueBucket::ORDER
            .iter()
            .position(|b| *b == DueBucket::of(task, today))
            .unwrap_or(DueBucket::ORDER.len() - 1);
        sections[index].1.push(task.clone());
    }

    sections.retain(|(_, tasks)| !tasks.is_empty());
    sections
}

/// Resolves the task the user is acting on, by walking the window for the row
/// whose menu was opened.
fn target_task(window: &adw::ApplicationWindow) -> Option<String> {
    let root = window.content()?;
    let mut found = None;
    let mut stack: Vec<gtk::Widget> = vec![root];

    while let Some(widget) = stack.pop() {
        if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>() {
            if let Some(id) = row.widget_name().strip_prefix(TASK_ROW_PREFIX) {
                found = Some(id.to_string());
            }
        }
        let mut child = widget.first_child();
        while let Some(next) = child {
            stack.push(next.clone());
            child = next.first_child();
        }
    }

    found
}

/// Widget-name prefix marking a task row and carrying its id.
const TASK_ROW_PREFIX: &str = "gtaskbar-task:";

/// Registers the window-level actions the task context menu triggers.
///
/// They live on the window rather than on each row so the menu can be plain
/// Gio data. The task the action applies to is identified by the widget that
/// currently has focus, which is the row whose menu was opened.
pub fn register_actions(window: &adw::ApplicationWindow) {
    use gtk::gio;

    let due_today = gio::ActionEntry::builder("due-today")
        .activate(|window: &adw::ApplicationWindow, _, _| {
            let Some(id) = target_task(window) else {
                return;
            };
            crate::sync::queue::set_due(&id, Some(view::today()));
            refresh(window);
        })
        .build();

    let due_tomorrow = gio::ActionEntry::builder("due-tomorrow")
        .activate(|window: &adw::ApplicationWindow, _, _| {
            let Some(id) = target_task(window) else {
                return;
            };
            let tomorrow = view::today() + chrono::Duration::days(1);
            crate::sync::queue::set_due(&id, Some(tomorrow));
            refresh(window);
        })
        .build();

    let clear_due = gio::ActionEntry::builder("clear-due")
        .activate(|window: &adw::ApplicationWindow, _, _| {
            let Some(id) = target_task(window) else {
                return;
            };
            // `None` means "clear", which the API needs as an explicit null.
            crate::sync::queue::set_due(&id, None);
            refresh(window);
        })
        .build();

    let delete_task = gio::ActionEntry::builder("delete-task")
        .activate(|window: &adw::ApplicationWindow, _, _| {
            let Some(id) = target_task(window) else {
                return;
            };
            crate::sync::queue::delete_task(&id);
            refresh(window);
        })
        .build();

    // Editing notes and titles needs a dialog, which is its own step; until then
    // the item reports that rather than silently doing nothing.
    let edit_task = gio::ActionEntry::builder("edit-task")
        .activate(|window: &adw::ApplicationWindow, _, _| {
            let Some(id) = target_task(window) else {
                return;
            };
            log::info!("editing {id:?} is not wired up yet");
        })
        .build();

    window.add_action_entries([due_today, due_tomorrow, clear_due, delete_task, edit_task]);
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
        let names = vec![("@a".to_string(), "My Tasks".to_string())];
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
    fn a_list_entry_counts_only_its_own_open_tasks() {
        // The caller scopes the tasks to the list, so the count is over the
        // supplied slice.
        let tasks = vec![
            task("a", None, TaskStatus::NeedsAction),
            task("b", None, TaskStatus::Completed),
        ];
        let built = entries(&tasks, &[("@a".into(), "My Tasks".into())]);
        let list_entry = built
            .iter()
            .find(|e| matches!(e.view, View::List(_)))
            .expect("list entry");
        assert_eq!(list_entry.count, 1, "a completed task is not outstanding");
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
        let sections = group_due(&tasks);
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
        let labels: Vec<_> = group_due(&tasks).iter().map(|(l, _)| *l).collect();
        assert_eq!(labels, vec!["Today", "Later", "No date"]);
    }

    #[test]
    fn an_undated_task_still_lands_in_a_group() {
        // A task that fits no section would silently vanish from a grouped list.
        let tasks = vec![task("none", None, TaskStatus::NeedsAction)];
        let sections = group_due(&tasks);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, "No date");
    }
}
