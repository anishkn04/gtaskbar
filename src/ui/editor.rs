//! The task editor: title, notes, due date, list and parent in one place.
//!
//! Reads the task and the list catalogue from the cache, writes back through
//! the queue like every other write (apply locally, repaint, sync now, toast
//! on failure), and never touches the network itself.

use adw::prelude::*;
use gtk::glib;

use crate::store::models::{Task, TaskPatch};

/// Opens the editor for one task.
///
/// Looks the task's list up from the cache, so callers only need the id. A
/// task that vanished since the menu was built reports instead of opening an
/// empty form.
pub fn present(parent: &gtk::Window, task_id: &str) {
    let Some((list_id, task)) = find_task(task_id) else {
        if let Some(app) = crate::running_app() {
            crate::ui::window::set_status(&app, Some("That task is no longer here"));
        }
        return;
    };
    let lists = cached_lists();
    if lists.is_empty() {
        if let Some(app) = crate::running_app() {
            crate::ui::window::set_status(&app, Some("No task list available"));
        }
        return;
    }

    let dialog = adw::Window::builder()
        .title("Edit task")
        .modal(true)
        .transient_for(parent)
        .default_width(480)
        .default_height(660)
        .build();

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new("Edit task", "")));
    toolbar.add_top_bar(&header);

    let page = adw::PreferencesPage::new();
    page.add(&details_group(&task));

    let due = DueState::new(task.due);
    page.add(&due_group(&due));

    let list_row = list_combo(&lists, &list_id);
    let parents = parent_options(&list_id, &task.id);
    let parent_row = parent_combo(&parents, task.parent.as_deref());
    let placement = adw::PreferencesGroup::builder().title("Placement").build();
    placement.add(&list_row);
    placement.add(&parent_row);
    page.add(&placement);

    // The parent options belong to one list. When the list changes, rebuild
    // them for the new one rather than leaving positions that now mean
    // something else; the selection resets to top level.
    {
        let parent_row = parent_row.clone();
        let task_id = task.id.clone();
        let lists = lists.clone();
        list_row.connect_selected_notify(move |row| {
            let Some(dest) = lists.get(row.selected() as usize) else {
                return;
            };
            let options = parent_options(&dest.id, &task_id);
            let titles: Vec<&str> = options.iter().map(|(_, title)| title.as_str()).collect();
            parent_row.set_model(Some(&gtk::StringList::new(&titles)));
            parent_row.set_selected(0);
        });
    }

    let actions = action_bar(&dialog, &task, &list_id, &list_row, &parent_row, &due);
    let layout = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&page)
        .build();
    layout.append(&scroller);
    layout.append(&actions);
    toolbar.set_content(Some(&layout));

    // The rows the save handler reads back by id lookups below.
    dialog.set_content(Some(&toolbar));
    dialog.present();
}

/// The task plus the list it lives in, from the cache.
fn find_task(task_id: &str) -> Option<(String, Task)> {
    let lists = cached_lists();
    lists.iter().find_map(|list| {
        crate::sync::scheduler::with_store(|store| store.task(&list.id, task_id))
            .and_then(|result| result.ok())
            .flatten()
            .map(|task| (list.id.clone(), task))
    })
}

#[derive(Clone)]
struct CachedList {
    id: String,
    title: String,
}
fn cached_lists() -> Vec<CachedList> {
    crate::sync::scheduler::with_store(|store| {
        store
            .task_lists()
            .map(|lists| {
                lists
                    .into_iter()
                    .map(|list| CachedList {
                        id: list.id,
                        title: list.title,
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
    .unwrap_or_default()
}

fn details_group(task: &Task) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("Details").build();

    let title = adw::EntryRow::builder()
        .title("Title")
        .text(&task.title)
        .build();
    title.set_widget_name("editor-title");
    crate::ui::icons::hide_unresolvable_indicators(&title);
    group.add(&title);

    let notes_view = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::Word)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .build();
    notes_view.buffer().set_text(&task.notes);
    notes_view.set_widget_name("editor-notes");
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(120)
        .child(&notes_view)
        .build();
    group.add(&scroller);

    group
}

/// The due-date choice, shared between the buttons that set it and the save
/// handler that reads it.
#[derive(Clone)]
struct DueState {
    selected: std::rc::Rc<std::cell::RefCell<Option<chrono::NaiveDate>>>,
    label: gtk::Label,
}

impl DueState {
    fn new(current: Option<chrono::NaiveDate>) -> Self {
        Self {
            selected: std::rc::Rc::new(std::cell::RefCell::new(current)),
            label: gtk::Label::builder()
                .label(due_text(current))
                .css_classes(["dim-label"])
                .build(),
        }
    }

    fn set(&self, due: Option<chrono::NaiveDate>) {
        *self.selected.borrow_mut() = due;
        self.label.set_label(&due_text(due));
    }

    fn get(&self) -> Option<chrono::NaiveDate> {
        *self.selected.borrow()
    }
}

fn due_text(due: Option<chrono::NaiveDate>) -> String {
    match due {
        None => "No due date".to_string(),
        Some(date) => {
            let today = crate::model::view::today();
            if date < today {
                format!("Overdue · {}", date.format("%-d %b %Y"))
            } else if date == today {
                "Today".to_string()
            } else if date == today + chrono::Duration::days(1) {
                "Tomorrow".to_string()
            } else {
                date.format("%-d %b %Y").to_string()
            }
        }
    }
}

fn due_group(due: &DueState) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("Due date").build();

    let row = adw::ActionRow::builder().title("Due").build();
    row.add_suffix(&due.label);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    for (label, value) in [
        ("Today", Some(crate::model::view::today())),
        (
            "Tomorrow",
            Some(crate::model::view::today() + chrono::Duration::days(1)),
        ),
        ("Clear", None),
    ] {
        let button = gtk::Button::with_label(label);
        let due = due.clone();
        button.connect_clicked(move |_| due.set(value));
        buttons.append(&button);
    }
    row.add_suffix(&buttons);
    group.add(&row);
    group
}

fn list_combo(lists: &[CachedList], current: &str) -> adw::ComboRow {
    let titles: Vec<&str> = lists.iter().map(|list| list.title.as_str()).collect();
    let row = adw::ComboRow::builder()
        .title("Task list")
        .model(&gtk::StringList::new(&titles))
        .build();
    let selected = lists
        .iter()
        .position(|list| list.id == current)
        .unwrap_or(0) as u32;
    row.set_selected(selected);
    row.set_widget_name("editor-list");
    row
}

/// Candidate parents in one list: every task except the edited one and its
/// descendants, since those would cycle the hierarchy.
fn parent_options(list_id: &str, task_id: &str) -> Vec<(Option<String>, String)> {
    let tasks = crate::sync::scheduler::with_store(|store| store.tasks_in_list(list_id))
        .and_then(|result| result.ok())
        .unwrap_or_default();

    // Ids that must not become the parent: the task itself and everything
    // beneath it.
    let mut forbidden = std::collections::HashSet::new();
    forbidden.insert(task_id.to_string());
    let mut changed = true;
    while changed {
        changed = false;
        for task in &tasks {
            if let Some(parent) = task.parent.as_deref() {
                if forbidden.contains(parent) && forbidden.insert(task.id.clone()) {
                    changed = true;
                }
            }
        }
    }

    let mut options = vec![(None, "No parent".to_string())];
    let mut named: Vec<(Option<String>, String)> = tasks
        .iter()
        .filter(|task| !forbidden.contains(&task.id) && !task.is_completed())
        .map(|task| (Some(task.id.clone()), task.title.clone()))
        .collect();
    named.sort_by_key(|(_, title)| title.to_lowercase());
    options.extend(named);
    options
}

fn parent_combo(options: &[(Option<String>, String)], current: Option<&str>) -> adw::ComboRow {
    let titles: Vec<&str> = options.iter().map(|(_, title)| title.as_str()).collect();
    let row = adw::ComboRow::builder()
        .title("Parent task")
        .subtitle("Makes this task a subtask of the chosen one")
        .model(&gtk::StringList::new(&titles))
        .build();
    let selected = current
        .and_then(|id| {
            options
                .iter()
                .position(|(option, _)| option.as_deref() == Some(id))
        })
        .unwrap_or(0) as u32;
    row.set_selected(selected);
    row.set_widget_name("editor-parent");
    row
}

/// Save and Delete. Reads every field back out of the dialog, so the rows
/// above only have to exist, not to report anywhere.
#[allow(clippy::too_many_arguments)]
fn action_bar(
    dialog: &adw::Window,
    task: &Task,
    list_id: &str,
    list_row: &adw::ComboRow,
    parent_row: &adw::ComboRow,
    due: &DueState,
) -> gtk::Box {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    bar.set_halign(gtk::Align::Center);
    bar.set_margin_top(12);
    bar.set_margin_bottom(12);

    let delete = gtk::Button::with_label("Delete");
    delete.add_css_class("destructive-action");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    save.add_css_class("pill");
    bar.append(&delete);
    bar.append(&save);

    let snapshot = EditorSnapshot {
        task_id: task.id.clone(),
        list_id: list_id.to_string(),
        title: task.title.clone(),
        notes: task.notes.clone(),
        due: task.due,
        parent: task.parent.clone(),
    };
    let lists = cached_lists();
    let dialog_weak = dialog.downgrade();

    let delete_snapshot = snapshot.clone();
    delete.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            confirm_delete(&dialog, &delete_snapshot);
        }
    ));

    let due_state = due.clone();
    save.connect_clicked(glib::clone!(
        #[weak]
        list_row,
        #[weak]
        parent_row,
        move |_| {
            save_task(
                &dialog_weak,
                &snapshot,
                &lists,
                &list_row,
                &parent_row,
                &due_state,
            );
        }
    ));

    bar
}

/// Everything save compares against, read once when the dialog opens.
#[derive(Clone)]
struct EditorSnapshot {
    task_id: String,
    list_id: String,
    title: String,
    notes: String,
    due: Option<chrono::NaiveDate>,
    parent: Option<String>,
}

fn dialog_title(dialog: &adw::Window) -> String {
    find_row_text(dialog, "editor-title")
}

fn dialog_notes(dialog: &adw::Window) -> String {
    let mut out = String::new();
    find_widget(dialog.upcast_ref(), &mut |widget| {
        if widget.widget_name() == "editor-notes" {
            if let Some(view) = widget.downcast_ref::<gtk::TextView>() {
                let buffer = view.buffer();
                let (start, end) = buffer.bounds();
                out = buffer.text(&start, &end, false).to_string();
            }
        }
    });
    out
}

fn find_row_text(dialog: &adw::Window, name: &str) -> String {
    let mut out = String::new();
    find_widget(dialog.upcast_ref(), &mut |widget| {
        if widget.widget_name() == name {
            if let Some(row) = widget.downcast_ref::<adw::EntryRow>() {
                out = row.text().to_string();
            }
        }
    });
    out
}

fn find_widget(widget: &gtk::Widget, visit: &mut impl FnMut(&gtk::Widget)) {
    visit(widget);
    let mut child = widget.first_child();
    while let Some(next) = child {
        find_widget(&next, visit);
        child = next.next_sibling();
    }
}

fn save_task(
    dialog_weak: &glib::WeakRef<adw::Window>,
    snapshot: &EditorSnapshot,
    lists: &[CachedList],
    list_row: &adw::ComboRow,
    parent_row: &adw::ComboRow,
    due: &DueState,
) {
    let Some(dialog) = dialog_weak.upgrade() else {
        return;
    };
    let title = dialog_title(&dialog).trim().to_string();
    if title.is_empty() {
        if let Some(app) = crate::running_app() {
            crate::ui::window::set_status(&app, Some("Title cannot be empty"));
        }
        return;
    }
    let notes = dialog_notes(&dialog);
    let due = due.get();

    let dest_list = lists
        .get(list_row.selected() as usize)
        .map(|list| list.id.clone())
        .unwrap_or_else(|| snapshot.list_id.clone());

    // Only changed fields travel: the API treats an omitted field as
    // "leave alone", so sending everything would clobber concurrent edits.
    // `due` is `Some(new)` even when clearing, because the API needs an
    // explicit null to clear and an absent field to leave alone.
    let mut patch = TaskPatch::default();
    let mut dirty = false;
    if title != snapshot.title {
        patch.title = Some(title);
        dirty = true;
    }
    if notes != snapshot.notes {
        patch.notes = Some(notes);
        dirty = true;
    }
    if due != snapshot.due {
        patch.due = Some(due);
        dirty = true;
    }

    // Parent options depend on the list, so they are rebuilt from the
    // destination list here rather than trusted from the widget: the options
    // the combo shows are exactly these, in this order, with "No parent"
    // first. A list change resets the parent rather than pointing at a task
    // that does not live in the new list.
    let options = parent_options(&dest_list, &snapshot.task_id);
    let new_parent = options
        .get(parent_row.selected() as usize)
        .and_then(|(id, _)| id.clone())
        .filter(|_| dest_list == snapshot.list_id);
    let parent_changed = new_parent != snapshot.parent;

    let mut first_error: Option<String> = None;
    let mut changed = false;
    if dirty {
        match crate::sync::queue::patch_task(&snapshot.task_id, patch) {
            Ok(()) => changed = true,
            Err(reason) => first_error = Some(reason),
        }
    }
    if parent_changed {
        match crate::sync::queue::move_task(&snapshot.task_id, Some(new_parent), None) {
            Ok(()) => changed = true,
            Err(reason) => {
                if first_error.is_none() {
                    first_error = Some(reason);
                }
            }
        }
    }
    if dest_list != snapshot.list_id {
        match crate::sync::queue::move_to_list(&snapshot.task_id, &dest_list) {
            Ok(()) => changed = true,
            Err(reason) => {
                if first_error.is_none() {
                    first_error = Some(reason);
                }
            }
        }
    }

    if let Some(app) = crate::running_app() {
        if let Some(reason) = first_error.as_deref() {
            log::warn!("{reason}");
            crate::ui::window::set_status(&app, Some(reason));
        } else {
            if changed {
                crate::ui::window::set_status(&app, Some("Saved — syncing…"));
            }
            dialog.close();
        }
        crate::ui::window::rebuild(&app);
        if changed {
            crate::sync::scheduler::request_sync(&app);
        }
    } else if first_error.is_none() {
        dialog.close();
    }
}

fn confirm_delete(dialog: &adw::Window, snapshot: &EditorSnapshot) {
    let confirm = adw::MessageDialog::new(
        Some(dialog),
        Some("Delete this task?"),
        Some("This cannot be undone."),
    );
    confirm.add_responses(&[("cancel", "_Cancel"), ("delete", "_Delete")]);
    confirm.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    confirm.set_default_response(Some("cancel"));
    confirm.set_close_response("cancel");
    confirm.set_transient_for(Some(dialog));
    confirm.set_modal(true);
    let snapshot_id = snapshot.task_id.clone();
    let dialog_weak = dialog.downgrade();
    confirm.connect_response(None, move |_, response| {
        if response != "delete" {
            return;
        }
        match crate::sync::queue::delete_task(&snapshot_id) {
            Ok(()) => {
                if let Some(app) = crate::running_app() {
                    crate::ui::window::set_status(&app, Some("Deleted — syncing…"));
                    crate::ui::window::rebuild(&app);
                    crate::sync::scheduler::request_sync(&app);
                }
                if let Some(dialog) = dialog_weak.upgrade() {
                    dialog.close();
                }
            }
            Err(reason) => {
                log::warn!("{reason}");
                if let Some(app) = crate::running_app() {
                    crate::ui::window::set_status(&app, Some(&reason));
                }
            }
        }
    });
    confirm.present();
}
