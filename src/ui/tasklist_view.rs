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
    /// Set by the key controller when its submit fails, so an `apply` from
    /// the same press stands down instead of reporting the same failure
    /// twice. A successful submit clears the entry, which already stops
    /// `apply`; this covers only the failure case, where the text is kept.
    static SUBMIT_FAILED: RefCell<bool> = const { RefCell::new(false) };
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

/// Whether any pane's quick-add holds unsubmitted text.
///
/// A background sync that changed tasks triggers a repaint, but rebuilding
/// while the user is mid-sentence would destroy the entry widget and eat what
/// they typed. Callers skip the repaint in that case; the next sync or any
/// manual refresh picks the changes up.
pub fn any_quick_add_has_text() -> bool {
    PANES.with(|slot| {
        slot.borrow()
            .iter()
            .any(|pane| !pane.quick_add.text().trim().is_empty())
    })
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

        // Return and keypad Enter submit, whatever the modifiers: this is a
        // single-line field, so there is no newline to protect.
        //
        // This controller exists because `apply` does not fire on this system's
        // libadwaita: the key provably reaches the row, but the row never emits
        // the signal, so relying on it alone leaves Enter dead. The controller
        // runs in the capture phase, before the entry's own handling, because
        // the entry consumes Return without emitting `apply`: a bubble-phase
        // controller provably never runs here. A successful submit clears the
        // entry, so a later `apply` from the same press (on systems where it
        // does fire) sees empty text and stands down; a failed submit sets a
        // flag the `apply` handler yields to, so failures report once.
        //
        // Known trade-off: with an IME composing, this submits on the first
        // Enter rather than only committing the composition. A dead Enter for
        // everyone beats a perfect one for IME users, and the common Latin case
        // is unaffected either way.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let row = quick_add.downgrade();
        keys.connect_key_pressed(move |_, keyval, _, _| {
            let submit_key = matches!(keyval.name().as_deref(), Some("Return" | "KP_Enter"));
            if !submit_key {
                return gtk::glib::Propagation::Proceed;
            }
            if let Some(entry) = row.upgrade() {
                let failed = !submit_quick_add(&entry);
                SUBMIT_FAILED.with(|flag| *flag.borrow_mut() = failed);
            }
            gtk::glib::Propagation::Proceed
        });
        quick_add.add_controller(keys);

        quick_add.connect_apply(move |entry| {
            if SUBMIT_FAILED.take() {
                return;
            }
            submit_quick_add(entry);
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

    /// The list this pane shows, if it shows one list rather than a smart view.
    pub fn list_id(&self) -> Option<String> {
        match &self.view {
            View::List(id) => Some(id.clone()),
            _ => None,
        }
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

/// Submits the quick-add entry: queue the task, confirm, and sync now.
///
/// Shared by the `apply` handler and the key controller, which exists because
/// `apply` does not fire on this system's libadwaita even though the key
/// provably reaches the row.
fn submit_quick_add(entry: &adw::EntryRow) -> bool {
    log::debug!("quick-add submit");
    let text = entry.text().trim().to_string();
    if text.is_empty() {
        log::debug!("quick-add text empty; ignoring");
        return true;
    }
    let connected = crate::auth::session::is_connected();
    log::debug!("quick-add connected={connected}");
    if !connected {
        log::info!("no account connected; not adding {text:?}");
        if let Some(app) = crate::running_app() {
            crate::ui::window::set_status(&app, Some("Connect your Google account first"));
        }
        return false;
    }
    // The pane the user typed in is the list they meant. Smart views
    // have no list of their own, so those fall back to the default.
    let preferred = visible_pane().and_then(|pane| pane.list_id());
    log::debug!("quick-add preferred list={preferred:?}");
    match crate::sync::queue::create_task(text, preferred) {
        Ok(list_name) => {
            log::debug!("quick-add queued for {list_name:?}");
            entry.set_text("");
            if let Some(app) = crate::running_app() {
                crate::ui::window::set_status(
                    &app,
                    Some(&format!("Added to {list_name} — syncing…")),
                );
                // Flush now rather than at the next poll, or the task
                // sits invisibly for minutes. The sync completion
                // repaints, which is what makes it appear.
                crate::sync::scheduler::request_sync(&app);
            }
            true
        }
        Err(reason) => {
            log::info!("quick-add failed: {reason}");
            // The text stays so nothing the user typed is lost to a
            // failure they can retry.
            if let Some(app) = crate::running_app() {
                crate::ui::window::set_status(&app, Some(&reason));
            }
            false
        }
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
