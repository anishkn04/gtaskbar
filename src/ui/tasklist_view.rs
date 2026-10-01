use std::cell::RefCell;

use adw::prelude::*;

use super::icons;
use crate::model::view::{self, View};
use crate::store::models::Task;

/// The main content pane: a toolbar, a quick-add row, and the task list.
#[derive(Clone)]
pub struct TaskListView {
    pub root: adw::NavigationPage,
    search: gtk::SearchEntry,
    stack: adw::ViewStack,
    list: gtk::ListBox,
    quick_add: adw::EntryRow,
    view: View,
    today: chrono::NaiveDate,
}

thread_local! {
    // The panes currently in the window, keyed by view.
    //
    // The split view owns the pages as AdwNavigationPages, which carry no
    // back-reference to the logic that filled them. Keeping the panes here is
    // what lets `app.add-task` and the search field act on whichever view is
    // showing, without the window having to plumb a handle to every callback.
    static PANES: RefCell<Vec<TaskListView>> = const { RefCell::new(Vec::new()) };
}

pub fn register_panes(panes: Vec<TaskListView>) {
    PANES.with(|slot| *slot.borrow_mut() = panes);
}

/// The pane whose page is currently visible in the content stack, if any.
pub fn visible_pane() -> Option<TaskListView> {
    PANES.with(|slot| {
        slot.borrow()
            .iter()
            .find(|pane| pane.root.is_visible())
            .cloned()
    })
}

/// Moves keyboard focus to the quick-add field of the visible pane.
pub fn focus_quick_add() {
    if let Some(pane) = visible_pane() {
        pane.focus_quick_add();
    }
}

/// Moves keyboard focus to the search field of the visible pane.
pub fn focus_search() {
    if let Some(pane) = visible_pane() {
        pane.focus_search();
    }
}

impl TaskListView {
    pub fn new(view: View, title: &str) -> Self {
        let today = view::today();

        // --- toolbar ---------------------------------------------------
        let header = adw::HeaderBar::new();

        // A real search field rather than a toggle: filtering has to be
        // visible while typing, and a hidden entry gives no feedback.
        let search = gtk::SearchEntry::builder()
            .hexpand(false)
            .placeholder_text("Search tasks")
            .build();
        search.add_css_class("gtaskbar-search");
        header.pack_start(&search);

        let sync = gtk::Button::from_icon_name(icons::SYNC);
        sync.set_tooltip_text(Some("Sync now"));
        sync.set_action_name(Some("app.sync-now"));
        header.pack_end(&sync);

        let add = gtk::Button::from_icon_name(icons::ADD);
        add.set_tooltip_text(Some("Add task"));
        add.set_action_name(Some("app.add-task"));
        header.pack_end(&add);

        // --- quick add -------------------------------------------------
        // An inline entry rather than a dialog: adding a task is the most
        // frequent action in the app, so it should cost as few keystrokes as
        // possible.
        let quick_add = adw::EntryRow::builder().title("Add a task").build();
        quick_add.add_css_class("gtaskbar-quick-add");

        // The entry is passed to the handler, so capturing it by weak
        // reference as well would be redundant.
        quick_add.connect_apply(move |entry| {
            let text = entry.text().trim().to_string();
            if text.is_empty() {
                return;
            }
            if crate::auth::session::is_connected() {
                crate::sync::queue::create_task(text);
            } else {
                log::info!("no account connected; not adding {text:?}");
            }
            entry.set_text("");
        });

        // --- list ------------------------------------------------------
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        list.add_css_class("gtaskbar-task-list");

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();

        let empty = adw::StatusPage::builder()
            .icon_name(icons::LIST)
            .title("Nothing here")
            .description("Tasks will appear once they are synced.")
            .build();

        let stack = adw::ViewStack::new();
        stack.add_titled(&scroller, Some("list"), "Tasks");
        stack.add_titled(&empty, Some("empty"), "Empty");

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&quick_add);
        content.append(&stack);
        toolbar.set_content(Some(&content));

        let page = adw::NavigationPage::builder()
            .title(title)
            .child(&toolbar)
            .build();

        let me = Self {
            root: page,
            search: search.clone(),
            stack,
            list,
            quick_add,
            view,
            today,
        };
        me.render(&[]);

        // Wired after construction because the handler needs the pane, which
        // only exists once `me` is built.
        let pane = me.clone();
        search.connect_search_changed(move |entry| {
            pane.apply_filter(&entry.text());
        });

        me
    }

    /// Replaces the contents, applying the active sort and grouping.
    pub fn render(&self, tasks: &[Task]) {
        let config = crate::config::Config::load();
        let sorted = view::assemble(&self.view, tasks.to_vec(), self.today, config.sort_mode);

        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }

        // Subtasks are shown beneath their parent, so the depth is resolved
        // once and passed down rather than recomputed per row.
        let mut rendered = 0usize;

        match config.group_mode {
            crate::config::GroupMode::ByDue => {
                for (label, section) in super::sidebar::group_due(&sorted) {
                    self.list.append(&header_row(label));
                    for task in &section {
                        self.list
                            .append(&super::sidebar::task_row(task, depth_of(task, &sorted)));
                        rendered += 1;
                    }
                }
            }
            _ => {
                for task in &sorted {
                    self.list
                        .append(&super::sidebar::task_row(task, depth_of(task, &sorted)));
                    rendered += 1;
                }
            }
        }

        self.stack
            .set_visible_child_name(if rendered == 0 { "empty" } else { "list" });
    }

    pub fn focus_search(&self) {
        self.search.grab_focus();
    }

    pub fn focus_quick_add(&self) {
        self.quick_add.grab_focus();
    }

    /// Feeds a search term through to the rows, hiding those that do not match.
    ///
    /// Matches titles and notes, case-insensitively. An empty term restores
    /// every row, so the field doubles as a toggle rather than something that
    /// has to be cleared by hand.
    pub fn apply_filter(&self, term: &str) {
        let needle = term.trim().to_lowercase();
        let mut visible_rows = 0usize;

        for row in self.rows() {
            // Section headers belong to the tasks below them, so they are
            // hidden and restored together with the group they label.
            if row.widget_name() == HEADER_ROW {
                continue;
            }

            let matches = needle.is_empty() || row_text(&row).contains(&needle);
            row.set_visible(matches);
            if matches {
                visible_rows += 1;
            }
        }

        let _ = visible_rows;
        self.update_header_visibility(&needle);
    }

    /// Hides a section header when every task it labels is filtered out, so the
    /// list does not show a heading with nothing under it.
    fn update_header_visibility(&self, needle: &str) {
        if needle.is_empty() {
            for row in self.rows().iter().filter(|r| r.widget_name() == HEADER_ROW) {
                row.set_visible(true);
            }
            return;
        }

        let rows = self.rows();
        for (index, row) in rows.iter().enumerate() {
            if row.widget_name() != HEADER_ROW {
                continue;
            }
            // Any following task row that survived the filter means this header
            // still has content.
            let has_visible_task = rows
                .iter()
                .skip(index + 1)
                .take_while(|next| next.widget_name() == HEADER_ROW || is_task_row(next))
                .any(|next| next.widget_name() != HEADER_ROW && next.is_visible());

            row.set_visible(has_visible_task);
        }
    }

    fn rows(&self) -> Vec<gtk::ListBoxRow> {
        let mut rows = Vec::new();
        let mut current = self.list.first_child();
        while let Some(widget) = current {
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>() {
                rows.push(row.clone());
            }
            current = widget.first_child();
        }
        rows
    }
}

/// Widget name marking a section header row.
const HEADER_ROW: &str = "gtaskbar-header";

fn is_task_row(row: &gtk::ListBoxRow) -> bool {
    row.widget_name() != HEADER_ROW
}

/// The searchable text of a task row: title and notes.
fn row_text(row: &gtk::ListBoxRow) -> String {
    // The child has to be bound before downcasting, because downcast_ref
    // borrows from the temporary.
    let child = row.child();
    let Some(action) = child
        .as_ref()
        .and_then(|c| c.downcast_ref::<adw::ActionRow>())
    else {
        return String::new();
    };
    format!(
        "{} {}",
        action.title(),
        action.subtitle().unwrap_or_default()
    )
    .to_lowercase()
}

fn header_row(label: &str) -> gtk::ListBoxRow {
    let text = gtk::Label::builder().label(label).xalign(0.0).build();
    text.add_css_class("gtaskbar-group-header");
    let row = gtk::ListBoxRow::builder()
        .activatable(false)
        .selectable(false)
        .child(&text)
        .build();
    row.set_widget_name(HEADER_ROW);
    row
}

/// How deeply a task should be indented: 0 for a top-level task, 1 for a direct
/// subtask, and so on for deeper nesting.
fn depth_of(task: &Task, all: &[Task]) -> u32 {
    // A task with no parent is by definition top level, and is not indented.
    if !task.is_subtask() {
        return 0;
    }

    let mut depth = 0;
    let mut parent = task.parent.clone();

    // Bounded by the number of tasks so a cycle in malformed data cannot hang
    // the UI.
    while let Some(id) = parent {
        if depth as usize >= all.len() {
            break;
        }
        depth += 1;
        parent = all
            .iter()
            .find(|candidate| candidate.id == id)
            .and_then(|candidate| candidate.parent.clone());
    }

    depth
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::models::TaskStatus;
    use chrono::NaiveDate;

    fn task(id: &str, parent: Option<&str>) -> Task {
        Task {
            id: id.into(),
            title: id.into(),
            notes: String::new(),
            status: TaskStatus::NeedsAction,
            due: NaiveDate::from_ymd_opt(2026, 10, 1),
            completed: None,
            updated: None,
            parent: parent.map(str::to_string),
            previous: None,
            position: None,
            etag: None,
            hidden: false,
            deleted: false,
        }
    }

    #[test]
    fn a_top_level_task_has_no_indent() {
        let tasks = vec![task("a", None)];
        assert_eq!(depth_of(&tasks[0], &tasks), 0);
    }

    #[test]
    fn a_subtask_is_indented_under_its_parent() {
        let tasks = vec![task("parent", None), task("child", Some("parent"))];
        assert_eq!(depth_of(&tasks[1], &tasks), 1);
    }

    #[test]
    fn nesting_depth_accumulates() {
        let tasks = vec![
            task("a", None),
            task("b", Some("a")),
            task("c", Some("b")),
            task("d", Some("c")),
        ];
        assert_eq!(depth_of(&tasks[3], &tasks), 3);
    }

    #[test]
    fn an_orphan_subtask_does_not_hang() {
        // Malformed data could reference a parent that is not in the set; the
        // walk must terminate rather than spin.
        let tasks = vec![task("lost", Some("nonexistent"))];
        assert_eq!(depth_of(&tasks[0], &tasks), 1);
    }

    #[test]
    fn a_parent_cycle_terminates() {
        // Defensive: a cycle in `parent` would otherwise loop forever while
        // rendering.
        let mut a = task("a", Some("b"));
        let mut b = task("b", Some("a"));
        a.parent = Some("b".into());
        b.parent = Some("a".into());
        let tasks = vec![a, b];
        assert!(depth_of(&tasks[0], &tasks) <= 2);
    }
}
