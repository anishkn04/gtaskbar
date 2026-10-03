use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use adw::prelude::*;
use glib::subclass::prelude::*;
use gtk::{gdk, gio, glib};

use super::icons;
use crate::config::GroupMode;
use crate::model::view::{self, View};
use crate::store::models::Task;

/// The main content pane: a toolbar, a quick-add row, and the task list.
#[derive(Clone)]
pub struct TaskListView {
    pub root: adw::NavigationPage,
    search: gtk::SearchEntry,
    stack: adw::ViewStack,
    scroller: gtk::ScrolledWindow,
    quick_add: adw::EntryRow,
    view: View,
    today: chrono::NaiveDate,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    /// The last inputs render ran on, so search text and collapse toggles can
    /// repaint without going back to the cache.
    source: Rc<RefCell<Source>>,
    search_text: Rc<RefCell<String>>,
    collapsed: Rc<RefCell<HashSet<String>>>,
}

/// Everything a repaint needs, kept so later repaints do not re-query.
#[derive(Default)]
struct Source {
    /// `(list id, task)` pairs in no particular order; sorting happens in
    /// `render` through `assemble`, exactly as before.
    items: Vec<(String, Task)>,
    lists: Vec<(String, String)>,
    group: GroupMode,
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
///
/// This checks `mapped`, not `visible`: `AdwViewStack` unmaps hidden pages
/// without clearing their visibility flag, so every pane reports
/// `is_visible() == true` and a lookup on it always returns the first pane.
pub fn visible_pane() -> Option<TaskListView> {
    PANES.with(|slot| {
        slot.borrow()
            .iter()
            .find(|pane| pane.root.is_mapped())
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
        // The row template carries an apply button and indicator icons under
        // `adw-*-symbolic` names, none of which this system's icon theme ships
        // (see docs/icons.md). Unresolvable, they paint as a stray glyph at the
        // row's edge. Submission is Enter-driven through the key controller
        // below, so the button is hidden rather than left broken.
        quick_add.set_show_apply_button(false);
        icons::hide_unresolvable_indicators(&quick_add);

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
                return glib::Propagation::Proceed;
            }
            if let Some(entry) = row.upgrade() {
                let failed = !submit_quick_add(&entry);
                SUBMIT_FAILED.with(|flag| *flag.borrow_mut() = failed);
            }
            glib::Propagation::Proceed
        });
        quick_add.add_controller(keys);

        // Escape cancels: clears whatever was typed, so a half-written
        // thought does not linger in the row.
        let cancel = gtk::EventControllerKey::new();
        cancel.set_propagation_phase(gtk::PropagationPhase::Capture);
        let cancel_row = quick_add.downgrade();
        cancel.connect_key_pressed(move |_, keyval, _, _| {
            if !matches!(keyval.name().as_deref(), Some("Escape")) {
                return gtk::glib::Propagation::Proceed;
            }
            if let Some(entry) = cancel_row.upgrade() {
                entry.set_text("");
            }
            gtk::glib::Propagation::Proceed
        });
        quick_add.add_controller(cancel);

        quick_add.connect_apply(move |entry| {
            if SUBMIT_FAILED.take() {
                return;
            }
            submit_quick_add(entry);
        });

        // --- list ------------------------------------------------------
        // A real ListView rather than a ListBox: the model holds the rows, so
        // a list of any size costs one widget per visible row instead of one
        // per task.
        let store = gio::ListStore::new::<TaskRowObject>();
        let selection = gtk::SingleSelection::new(Some(store.clone()));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let Some(list_item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            list_item.set_child(Option::<&gtk::Widget>::None);
        });
        factory.connect_bind(|_, item| {
            let Some(list_item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(row) = list_item
                .item()
                .and_then(|object| object.downcast::<TaskRowObject>().ok())
            else {
                return;
            };
            fill_row(list_item, &row);
        });

        let list_view = gtk::ListView::new(Some(selection.clone()), Some(factory.clone()));
        // Drops on empty area append at the end of the dragged task's
        // siblings; drops onto rows are handled per row.
        let end_target = gtk::DropTarget::new(glib::types::Type::STRING, gdk::DragAction::MOVE);
        end_target.connect_drop(|_, value, _, _| {
            let Ok(dragged) = value.get::<String>() else {
                return false;
            };
            drop_at_end(&dragged)
        });
        list_view.add_controller(end_target);
        list_view.add_css_class("gtaskbar-task-list");
        // Open the same menu as a secondary click.
        list_view.connect_activate({
            let selection = selection.clone();
            let list_view = list_view.clone();
            move |_, position| {
                popup_for_selected(&list_view, &selection, position);
            }
        });

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list_view)
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
        // A persistent state surface: offline, failed syncs and dead sessions
        // show here rather than only as transient toasts.
        if let Some(banner) = super::window::banner() {
            toolbar.add_bottom_bar(&banner);
        }
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&quick_add);
        content.append(&stack);
        toolbar.set_content(Some(&content));

        let page = adw::NavigationPage::builder()
            .title(title)
            .child(&toolbar)
            .build();

        let search_text = Rc::new(RefCell::new(String::new()));
        let collapsed = Rc::new(RefCell::new(HashSet::new()));

        let me = Self {
            root: page,
            search: search.clone(),
            stack,
            scroller,
            quick_add,
            view,
            today,
            store,
            selection,
            source: Rc::new(RefCell::new(Source::default())),
            search_text: search_text.clone(),
            collapsed: collapsed.clone(),
        };
        me.render(&[], &[]);

        // Wired after construction because the handler needs the pane, which
        // only exists once `me` is built.
        let pane = me.clone();
        search.connect_search_changed(move |entry| {
            pane.set_search(&entry.text());
        });

        me
    }

    /// Replaces the contents, applying the active sort and grouping.
    pub fn render(&self, items: &[(String, Task)], lists: &[(String, String, usize)]) {
        let config = crate::config::Config::load();
        {
            let mut source = self.source.borrow_mut();
            source.items = items.to_vec();
            source.lists = lists
                .iter()
                .map(|(id, title, _)| (id.clone(), title.clone()))
                .collect();
            source.group = config.group_mode;
        }
        self.refresh_store(&config);
    }

    /// Rebuilds the model from the last inputs: search text, collapse state
    /// and config. The scroll position and the selection survive, by task id
    /// rather than by row, so typing in search or toggling an expander does
    /// not throw the view back to the top.
    fn refresh_store(&self, config: &crate::config::Config) {
        let (tasks, lists, group, search, collapsed) = {
            let source = self.source.borrow();
            (
                source.items.clone(),
                source.lists.clone(),
                source.group,
                self.search_text.borrow().clone(),
                self.collapsed.borrow().clone(),
            )
        };

        let assembled = {
            let plain: Vec<Task> = tasks.iter().map(|(_, task)| task.clone()).collect();
            view::assemble(&self.view, plain, self.today, config.sort_mode)
        };
        // The assembled order is authoritative; re-attach each task's list by
        // id so grouping and row wiring cannot disagree about membership.
        let id_to_list: HashMap<&str, &str> = tasks
            .iter()
            .map(|(list_id, task)| (task.id.as_str(), list_id.as_str()))
            .collect();
        let ordered: Vec<(String, Task)> = assembled
            .into_iter()
            .filter_map(|task| {
                id_to_list
                    .get(task.id.as_str())
                    .map(|list_id| (list_id.to_string(), task))
            })
            .collect();

        let rows = build_items(
            &ordered,
            &lists,
            group,
            &search,
            &collapsed,
            self.view.include_completed(),
        );

        let selected_id = self
            .selection
            .selected_item()
            .and_then(|object| object.downcast::<TaskRowObject>().ok())
            .and_then(|row| row.task_id());
        let scroll = self.scroller.vadjustment().value();

        self.store.remove_all();
        for row in &rows {
            self.store.append(&TaskRowObject::new(row.clone()));
        }

        if let Some(wanted) = selected_id {
            for (index, row) in rows.iter().enumerate() {
                if row.task_id() == Some(wanted.as_str()) {
                    self.selection.set_selected(index as u32);
                    break;
                }
            }
        }
        let adjustment = self.scroller.vadjustment();
        adjustment.set_value(
            scroll
                .min(adjustment.upper() - adjustment.page_size())
                .max(0.0),
        );

        self.stack
            .set_visible_child_name(if rows.is_empty() { "empty" } else { "list" });
    }

    /// Whether the user is typing in this pane's text fields.
    ///
    /// Global shortcuts that act on tasks (notably Delete) stand down while
    /// this is true: otherwise editing text could delete whatever happens to
    /// be selected.
    pub fn editing_text(&self) -> bool {
        self.quick_add.has_focus() || self.search.has_focus()
    }

    /// The selected task, if the selection is on a task row rather than a
    /// header or nothing.
    pub fn selected_task_id(&self) -> Option<String> {
        self.selection
            .selected_item()
            .and_then(|object| object.downcast::<TaskRowObject>().ok())
            .and_then(|row| row.task_id())
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

    fn set_search(&self, term: &str) {
        *self.search_text.borrow_mut() = term.trim().to_lowercase();
        let config = crate::config::Config::load();
        self.refresh_store(&config);
    }

    /// Collapses or expands a parent task, hiding or revealing its subtasks.
    pub fn toggle_collapsed(&self, task_id: &str) {
        {
            let mut collapsed = self.collapsed.borrow_mut();
            if !collapsed.remove(task_id) {
                collapsed.insert(task_id.to_string());
            }
        }
        let config = crate::config::Config::load();
        self.refresh_store(&config);
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

/// One row of the list model: either a section header or a task.
///
/// Headers carry only a label; tasks carry everything the factory needs so a
/// rebind never has to look anything up.
#[derive(Debug, Clone)]
pub enum RowItem {
    Header { label: String },
    Task(Box<TaskRow>),
}

/// A task row with everything the factory binds, resolved once at model time.
#[derive(Debug, Clone)]
pub struct TaskRow {
    task: Task,
    list_id: String,
    depth: u32,
    has_children: bool,
    collapsed: bool,
}

impl RowItem {
    fn task_id(&self) -> Option<&str> {
        match self {
            RowItem::Header { .. } => None,
            RowItem::Task(row) => Some(row.task.id.as_str()),
        }
    }
}

mod imp {
    use super::RowItem;
    use std::cell::RefCell;

    use glib::subclass::prelude::*;

    #[derive(Default)]
    pub struct TaskRowObject {
        pub item: RefCell<Option<RowItem>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TaskRowObject {
        const NAME: &'static str = "GtaskbarRow";
        type Type = super::TaskRowObject;
    }

    impl ObjectImpl for TaskRowObject {}
}

glib::wrapper! {
    pub struct TaskRowObject(ObjectSubclass<imp::TaskRowObject>);
}

impl TaskRowObject {
    fn new(item: RowItem) -> Self {
        let object: Self = glib::Object::new();
        *object.imp().item.borrow_mut() = Some(item);
        object
    }

    fn item(&self) -> RowItem {
        self.imp()
            .item
            .borrow()
            .clone()
            .expect("row items are set at construction")
    }

    fn task_id(&self) -> Option<String> {
        self.item().task_id().map(str::to_string)
    }
}

/// Assembles the model rows: sorted tasks plus the section headers grouping
/// demands, minus whatever search and collapse hide.
///
/// Pure: no widgets, no database. Headers are emitted only for sections that
/// still hold a visible task, so filtering can never strand a heading.
fn build_items(
    ordered: &[(String, Task)],
    lists: &[(String, String)],
    group: GroupMode,
    search: &str,
    collapsed: &HashSet<String>,
    include_completed: bool,
) -> Vec<RowItem> {
    // Children by parent id, so each row knows whether it can expand.
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for (_, task) in ordered {
        if let Some(parent) = task.parent.as_deref() {
            children.entry(parent).or_default().push(task.id.as_str());
        }
    }

    let visible = |task: &Task| {
        // Completed tasks are hidden everywhere except the Completed view,
        // which is the one place they belong. Filtering them unconditionally
        // left that view permanently empty while its badge counted them.
        if task.is_completed() && !include_completed {
            return false;
        }
        if search.is_empty() {
            return true;
        }
        let haystack = format!("{} {}", task.title, task.notes).to_lowercase();
        haystack.contains(search)
    };

    // A task hides when any ancestor is collapsed. Orphans (a parent outside
    // the set) cannot be hidden by something invisible, so they stay.
    let in_set: HashSet<&str> = ordered.iter().map(|(_, t)| t.id.as_str()).collect();
    let hidden = |task: &Task| {
        let mut parent = task.parent.as_deref();
        let mut guard = 0usize;
        while let Some(id) = parent {
            if collapsed.contains(id) {
                return true;
            }
            if !in_set.contains(id) {
                return false;
            }
            guard += 1;
            if guard > ordered.len() {
                return false;
            }
            parent = ordered
                .iter()
                .find(|(_, t)| t.id.as_str() == id)
                .and_then(|(_, t)| t.parent.as_deref());
        }
        false
    };

    let mut rows = Vec::new();
    let push_section =
        |label: Option<&str>, section: &[(String, Task)], rows: &mut Vec<RowItem>| {
            let shown: Vec<&(String, Task)> = section
                .iter()
                .filter(|(_, task)| visible(task) && !hidden(task))
                .collect();
            if shown.is_empty() {
                return;
            }
            if let Some(label) = label.filter(|label| !label.is_empty()) {
                rows.push(RowItem::Header {
                    label: label.to_string(),
                });
            }
            for (list_id, task) in shown {
                rows.push(RowItem::Task(Box::new(TaskRow {
                    task: (*task).clone(),
                    list_id: list_id.clone(),
                    depth: depth_of(task, &section_tasks(section)),
                    has_children: children.contains_key(task.id.as_str()),
                    collapsed: collapsed.contains(task.id.as_str()),
                })));
            }
        };

    match group {
        GroupMode::ByDue => {
            for (label, section) in super::sidebar::group_due_pairs(ordered) {
                push_section(Some(label), &section, &mut rows);
            }
        }
        GroupMode::ByList => {
            for (id, title) in lists {
                let section: Vec<(String, Task)> = ordered
                    .iter()
                    .filter(|(list_id, _)| list_id == id)
                    .cloned()
                    .collect();
                push_section(Some(title), &section, &mut rows);
            }
        }
        GroupMode::ByStatus => {
            let section = |completed: bool| {
                ordered
                    .iter()
                    .filter(|(_, task)| task.is_completed() == completed)
                    .cloned()
                    .collect::<Vec<_>>()
            };
            // Completed tasks never reach a non-Completed view, so this is
            // usually a single "Open" section; the Completed view mirrors it.
            push_section(Some("Open"), &section(false), &mut rows);
            push_section(Some("Done"), &section(true), &mut rows);
        }
        GroupMode::None => {
            push_section(None, ordered, &mut rows);
        }
    }

    rows
}

/// The tasks of a section, for depth resolution.
fn section_tasks(section: &[(String, Task)]) -> Vec<Task> {
    section.iter().map(|(_, task)| task.clone()).collect()
}

/// Fills a row widget from its item.
///
/// The content is rebuilt on every bind rather than updated in place: rows are
/// recycled across items, and rebuilding means handlers always capture the
/// current task instead of a stale one, with no guard bookkeeping. Unparented
/// widgets are finalised with their handlers, so nothing leaks.
fn fill_row(list_item: &gtk::ListItem, row: &TaskRowObject) {
    list_item.set_child(Option::<&gtk::Widget>::None);
    match row.item() {
        RowItem::Header { label } => {
            list_item.set_selectable(false);
            list_item.set_activatable(false);
            let text = gtk::Label::builder().label(label).xalign(0.0).build();
            text.add_css_class("gtaskbar-group-header");
            list_item.set_child(Some(&text));
        }
        RowItem::Task(task_row) => {
            list_item.set_selectable(true);
            list_item.set_activatable(true);
            list_item.set_child(Some(&build_task_row(
                &task_row.task,
                task_row.depth,
                task_row.has_children,
                task_row.collapsed,
            )));
        }
    }
}

/// Builds one task row: expander, checkbox, and the title/notes/due content.
///
/// Everything is fresh per bind, so every handler below captures the task it
/// was built for. There is deliberately no shared or recycled state here.
fn build_task_row(task: &Task, depth: u32, has_children: bool, collapsed: bool) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_margin_top(6);
    row.set_margin_bottom(6);
    row.set_margin_start(6);
    row.set_margin_end(6);

    if has_children {
        let expander = gtk::Button::builder()
            .icon_name(if collapsed {
                "pan-end-symbolic"
            } else {
                "pan-down-symbolic"
            })
            .tooltip_text(if collapsed {
                "Expand subtasks"
            } else {
                "Collapse subtasks"
            })
            .css_classes(["flat", "gtaskbar-expander"])
            .valign(gtk::Align::Center)
            .build();
        let task_id = task.id.clone();
        expander.connect_clicked(move |_| {
            if let Some(pane) = visible_pane() {
                pane.toggle_collapsed(&task_id);
            }
        });
        row.append(&expander);
    }
    if depth > 0 {
        row.add_css_class("gtaskbar-subtask");
    }

    let check = gtk::CheckButton::builder()
        .active(task.is_completed())
        .tooltip_text(if task.is_completed() {
            "Mark as not done"
        } else {
            "Mark as done"
        })
        .valign(gtk::Align::Center)
        .build();
    check.add_css_class("gtaskbar-check");
    check.add_css_class("flat");
    if task.is_completed() {
        // A themed tick reads as "done" more clearly than a dimmed empty box.
        let tick = icons::image(icons::CHECK);
        tick.add_css_class("gtaskbar-done-tick");
        row.append(&tick);
    } else {
        let task_id = task.id.clone();
        check.connect_toggled(move |button| {
            if !button.is_active() {
                return;
            }
            match crate::sync::queue::set_status(
                &task_id,
                crate::store::models::TaskStatus::Completed,
            ) {
                Ok(()) => {
                    if let Some(app) = crate::running_app() {
                        crate::ui::window::rebuild(&app);
                        crate::sync::scheduler::request_sync(&app);
                    }
                }
                Err(reason) => {
                    log::warn!("{reason}");
                    if let Some(app) = crate::running_app() {
                        crate::ui::window::set_status(&app, Some(&reason));
                        crate::ui::window::rebuild(&app);
                    }
                }
            }
        });
        row.append(&check);
    }

    let content = adw::ActionRow::builder()
        .title(&task.title)
        .activatable(false)
        .hexpand(true)
        .build();
    content.add_css_class("gtaskbar-task-row");
    if task.is_completed() {
        content.add_css_class("done");
    }
    if !task.notes.trim().is_empty() {
        let first_line = task
            .notes
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        if !first_line.is_empty() {
            content.set_subtitle(&first_line);
            content.set_subtitle_lines(1);
        }
    }
    if let Some(chip) = super::sidebar::due_chip(task) {
        content.add_suffix(&chip);
    }
    row.append(&content);

    // Unchecking a completed task: the tick is display-only, so the row needs
    // a way back. A secondary click opens the same menu as every row, and the
    // menu is also where due dates and deletion live.
    let press = gtk::GestureClick::new();
    press.set_button(gdk::BUTTON_SECONDARY);
    let press_task = task.id.clone();
    let press_completed = task.is_completed();
    let press_row = row.clone();
    press.connect_pressed(move |gesture, n_press, _, _| {
        if n_press != 1 {
            return;
        }
        gesture.set_state(gtk::EventSequenceState::Claimed);
        popup_for_task(&press_row, &press_task, press_completed);
    });
    row.add_controller(press);

    // Drag to reorder, Manual sort only: the drop target below validates the
    // mode, the list and the parent, so starting a drag is always harmless.
    // Positions are the server's opaque ordering key and cannot be computed
    // locally, so the row visibly moves when the flush comes back.
    let drag_task = task.id.clone();
    let drag = gtk::DragSource::new();
    drag.set_actions(gdk::DragAction::MOVE);
    drag.set_content(Some(&gdk::ContentProvider::for_value(
        &drag_task.to_value(),
    )));
    row.add_controller(drag);

    // Drop onto a row means "insert before it". The handler validates the
    // mode, the list and the parent, so an invalid drop is silently refused
    // the way drag-and-drop targets normally behave.
    let drop_task = task.id.clone();
    let drop_target = gtk::DropTarget::new(glib::types::Type::STRING, gdk::DragAction::MOVE);
    drop_target.connect_drop(move |_, value, _, _| {
        let Ok(dragged) = value.get::<String>() else {
            return false;
        };
        drop_before(&dragged, &drop_task)
    });
    row.add_controller(drop_target);

    row.upcast()
}

/// Rows of the visible pane in display order: task id, list id, parent.
fn ordered_rows(pane: &TaskListView) -> Vec<(String, String, Option<String>)> {
    let Some(model) = pane.selection.model() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for index in 0..model.n_items() {
        let Some(row) = model
            .item(index)
            .and_then(|object| object.downcast::<TaskRowObject>().ok())
        else {
            continue;
        };
        if let RowItem::Task(task_row) = row.item() {
            out.push((
                task_row.task.id.clone(),
                task_row.list_id.clone(),
                task_row.task.parent.clone(),
            ));
        }
    }
    out
}

/// Moves the dragged task to just before the target row.
///
/// Only sibling reordering within one list and one parent, in Manual sort:
/// positions are the server's opaque key, anything else has no well-defined
/// meaning. Returns whether the drop was accepted.
fn drop_before(dragged_id: &str, target_id: &str) -> bool {
    if dragged_id == target_id {
        return false;
    }
    if crate::config::Config::load().sort_mode != crate::config::SortMode::Manual {
        return false;
    }
    let Some(pane) = visible_pane() else {
        return false;
    };
    let rows = ordered_rows(&pane);
    let locate = |id: &str| rows.iter().find(|(task_id, _, _)| task_id == id).cloned();
    let (Some((_, drag_list, drag_parent)), Some((_, target_list, target_parent))) =
        (locate(dragged_id), locate(target_id))
    else {
        return false;
    };
    if drag_list != target_list || drag_parent != target_parent {
        return false;
    }

    // Siblings in display order without the dragged task; the task before the
    // target is what it must follow, or nothing to move first.
    let siblings: Vec<&String> = rows
        .iter()
        .filter(|(task_id, list_id, parent)| {
            task_id != dragged_id && list_id == &drag_list && parent == &drag_parent
        })
        .map(|(task_id, _, _)| task_id)
        .collect();
    let Some(position) = siblings.iter().position(|id| *id == target_id) else {
        return false;
    };
    let previous: Option<String> = if position == 0 {
        None
    } else {
        Some(siblings[position - 1].clone())
    };

    // Already there: accept without a pointless round-trip.
    let full: Vec<&String> = rows
        .iter()
        .filter(|(_, list_id, parent)| list_id == &drag_list && parent == &drag_parent)
        .map(|(task_id, _, _)| task_id)
        .collect();
    if let Some(dragged_at) = full.iter().position(|id| *id == dragged_id) {
        let current: Option<&String> = if dragged_at == 0 {
            None
        } else {
            Some(full[dragged_at - 1])
        };
        if current == previous.as_ref() {
            return true;
        }
    }

    match crate::sync::queue::move_task(dragged_id, None, Some(previous)) {
        Ok(()) => {
            if let Some(app) = crate::running_app() {
                crate::ui::window::set_status(&app, Some("Moved — syncing…"));
                crate::sync::scheduler::request_sync(&app);
            }
            true
        }
        Err(reason) => {
            log::warn!("{reason}");
            if let Some(app) = crate::running_app() {
                crate::ui::window::set_status(&app, Some(&reason));
            }
            true
        }
    }
}

/// Moves the dragged task to the end of its siblings.
///
/// The drop target for empty list area. Same rules as `drop_before`: Manual
/// sort, one list, one parent.
fn drop_at_end(dragged_id: &str) -> bool {
    if crate::config::Config::load().sort_mode != crate::config::SortMode::Manual {
        return false;
    }
    let Some(pane) = visible_pane() else {
        return false;
    };
    let rows = ordered_rows(&pane);
    let Some((_, drag_list, drag_parent)) = rows
        .iter()
        .find(|(task_id, _, _)| task_id == dragged_id)
        .cloned()
    else {
        return false;
    };
    let siblings: Vec<&String> = rows
        .iter()
        .filter(|(task_id, list_id, parent)| {
            task_id != dragged_id && list_id == &drag_list && parent == &drag_parent
        })
        .map(|(task_id, _, _)| task_id)
        .collect();
    // Already last (or alone): accept without a round-trip.
    let full: Vec<&String> = rows
        .iter()
        .filter(|(_, list_id, parent)| list_id == &drag_list && parent == &drag_parent)
        .map(|(task_id, _, _)| task_id)
        .collect();
    if full.last().is_some_and(|last| *last == dragged_id) {
        return true;
    }
    let previous = siblings.last().map(|id| id.to_string());

    match crate::sync::queue::move_task(dragged_id, None, Some(previous)) {
        Ok(()) => {
            if let Some(app) = crate::running_app() {
                crate::ui::window::set_status(&app, Some("Moved — syncing…"));
                crate::sync::scheduler::request_sync(&app);
            }
            true
        }
        Err(reason) => {
            log::warn!("{reason}");
            if let Some(app) = crate::running_app() {
                crate::ui::window::set_status(&app, Some(&reason));
            }
            true
        }
    }
}

/// Opens the menu for the activated row, selecting it first so keyboard and
/// pointer activation agree on the target.
fn popup_for_selected(
    parent: &impl IsA<gtk::Widget>,
    selection: &gtk::SingleSelection,
    position: u32,
) {
    selection.set_selected(position);
    let Some(row) = selection
        .selected_item()
        .and_then(|object| object.downcast::<TaskRowObject>().ok())
    else {
        return;
    };
    if let RowItem::Task(task_row) = row.item() {
        popup_for_task(parent, &task_row.task.id, task_row.task.is_completed());
    }
}

/// Opens the task menu: due-date changes, edit, reopening, delete.
///
/// The menu carries the task id as each action's target; the queue resolves
/// the list from the cache, which also covers tasks that moved since the menu
/// was built. Completed rows get a way back, since their checkbox is replaced
/// by a display-only tick.
fn popup_for_task(parent: &impl IsA<gtk::Widget>, task_id: &str, completed: bool) {
    let target = task_id.to_variant();

    let menu = gio::Menu::new();
    if completed {
        let reopen = gio::Menu::new();
        reopen.append_item(&targeted_item(
            "Mark as not done",
            "win.reopen-task",
            &target,
        ));
        menu.append_section(None, &reopen);
    }
    let due_section = gio::Menu::new();
    due_section.append_item(&targeted_item("Due today", "win.due-today", &target));
    due_section.append_item(&targeted_item("Due tomorrow", "win.due-tomorrow", &target));
    due_section.append_item(&targeted_item("Clear due date", "win.clear-due", &target));
    menu.append_section(None, &due_section);

    let edit = gio::Menu::new();
    edit.append_item(&targeted_item("Edit…", "win.edit-task", &target));
    menu.append_section(None, &edit);

    let danger = gio::Menu::new();
    danger.append_item(&targeted_item("Delete", "win.delete-task", &target));
    menu.append_section(None, &danger);

    let popup = gtk::PopoverMenu::from_model(Some(&menu));
    popup.set_parent(parent);
    popup.set_has_arrow(false);
    let unparent = popup.downgrade();
    popup.connect_closed(move |_| {
        if let Some(popup) = unparent.upgrade() {
            popup.unparent();
        }
    });
    popup.popup();
}

fn targeted_item(label: &str, action: &str, target: &glib::Variant) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some(action), Some(target));
    item
}

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

    fn pair(id: &str, parent: Option<&str>) -> (String, Task) {
        ("@a".into(), task(id, parent))
    }

    fn done_pair(id: &str) -> (String, Task) {
        let mut completed = task(id, None);
        completed.status = crate::store::models::TaskStatus::Completed;
        ("@a".into(), completed)
    }

    #[test]
    fn the_visible_pane_lookup_uses_mapping() {
        let source = include_str!("tasklist_view.rs");
        let start = source
            .find("pub fn visible_pane")
            .expect("visible_pane exists");
        let body = &source[start..];
        let end = body.find("\n}\n").expect("function ends at column zero");
        let body = &body[..end];
        assert!(
            body.contains("is_mapped()"),
            "visible_pane must match on mapped pages; is_visible() is true for every page"
        );
        assert!(
            !body.contains("is_visible()"),
            "is_visible() matches hidden stack pages and always returns the first pane"
        );
    }

    /// The Completed view must show completed tasks. The `visible`
    /// closure used to filter them out unconditionally, which left the pane
    /// permanently empty while its badge counted them.
    #[test]
    fn the_completed_view_shows_completed_tasks() {
        let items = build_items(
            &[done_pair("a"), done_pair("b")],
            &[],
            GroupMode::None,
            "",
            &HashSet::new(),
            true,
        );
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| item.task_id().is_some()));
    }

    #[test]
    fn other_views_still_hide_completed_tasks() {
        // `assemble` already excludes them everywhere else; this pins that the
        // row builder agrees when told the view excludes them.
        let items = build_items(
            &[done_pair("a"), pair("b", None)],
            &[],
            GroupMode::None,
            "",
            &HashSet::new(),
            false,
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].task_id(), Some("b"));
    }

    #[test]
    fn flat_lists_emit_no_headers() {
        let items = build_items(
            &[pair("a", None), pair("b", None)],
            &[],
            GroupMode::None,
            "",
            &HashSet::new(),
            false,
        );
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| item.task_id().is_some()));
    }

    #[test]
    fn due_grouping_emits_only_nonempty_sections() {
        let items = build_items(
            &[pair("a", None), pair("b", None)],
            &[],
            GroupMode::ByDue,
            "",
            &HashSet::new(),
            false,
        );
        // Both tasks are due 2026-10-01, so one section plus two tasks.
        assert_eq!(items.len(), 3);
        assert!(matches!(items[0], RowItem::Header { .. }));
    }

    #[test]
    fn collapsing_a_parent_hides_its_subtasks_but_keeps_it() {
        let ordered = vec![pair("p", None), pair("c", Some("p"))];
        let mut collapsed = HashSet::new();
        collapsed.insert("p".to_string());
        let items = build_items(&ordered, &[], GroupMode::None, "", &collapsed, false);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].task_id(), Some("p"));
    }

    #[test]
    fn search_prunes_tasks_and_empties_sections() {
        let mut first = pair("a", None);
        first.1.title = "Buy milk".into();
        let mut second = pair("b", None);
        second.1.title = "Write report".into();
        let items = build_items(
            &[first, second],
            &[],
            GroupMode::None,
            "milk",
            &HashSet::new(),
            false,
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].task_id(), Some("a"));
    }

    #[test]
    fn by_list_sections_carry_their_own_tasks() {
        let items = build_items(
            &[
                ("@a".into(), task("x", None)),
                ("@b".into(), task("y", None)),
            ],
            &[
                ("@a".into(), "First".into()),
                ("@b".into(), "Second".into()),
            ],
            GroupMode::ByList,
            "",
            &HashSet::new(),
            false,
        );
        let labels: Vec<_> = items
            .iter()
            .filter_map(|item| match item {
                RowItem::Header { label } => Some(label.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(labels, vec!["First", "Second"]);
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
        // Defensive: a cycle in malformed data could otherwise loop forever while
        // rendering.
        let mut a = task("a", Some("b"));
        let mut b = task("b", Some("a"));
        a.parent = Some("b".into());
        b.parent = Some("a".into());
        let tasks = vec![a, b];
        assert!(depth_of(&tasks[0], &tasks) <= 2);
    }
}
