use ksni::menu::{MenuItem, StandardItem};
use ksni::{Category, Status, TrayMethods};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// Handle to the tray icon, owned by the scheduler.
///
/// `ksni` is used rather than `tray-icon`'s `libappindicator` feature, which
/// needs a package that is not installed here. `ksni` is pure Rust over D-Bus,
/// and the shell already provides `org.kde.StatusNotifierWatcher`.
pub struct TrayIcon {
    service: ksni::Handle<Service>,
    /// Where the service sends its intents. Drained on the main thread, because
    /// the callbacks run on the runtime and must not touch widgets.
    events: UnboundedReceiver<Event>,
    /// Held rather than looked up with `Handle::current`, which panics: the GTK
    /// thread is not inside a runtime context, and every update is requested
    /// from there.
    runtime: Handle,
}

#[derive(Debug)]
struct Service {
    /// Tasks due today plus overdue. Drives the attention state and the tooltip.
    attention: usize,
    syncing: bool,
    connected: bool,
    events: UnboundedSender<Event>,
}

/// An intent from the tray, to be carried out on the main thread.
#[derive(Debug)]
pub enum Event {
    AddTask,
    SyncNow,
    ShowWindow,
    Quit,
}

impl TrayIcon {
    /// Registers the tray icon.
    ///
    /// Returns `None` when no status-notifier host is available, which is a
    /// supported state rather than an error: the app then quits on close
    /// instead of hiding to a tray that is not there.
    pub fn spawn(runtime: &Handle) -> Option<Self> {
        let (tx, events) = unbounded_channel();
        let service = Service {
            attention: 0,
            syncing: false,
            connected: false,
            events: tx,
        };

        // The spawn future has to be driven, but it must not block the GLib
        // thread, so it runs on the sync runtime.
        let service = runtime.block_on(async { service.spawn().await.ok() })?;

        Some(Self {
            service,
            events,
            runtime: runtime.clone(),
        })
    }

    /// Whether the icon is still registered with the shell. A watcher restart
    /// drops it silently, and the app needs to know so it can stop pretending
    /// there is a tray to hide to.
    pub fn is_alive(&self) -> bool {
        !self.service.is_closed()
    }

    /// Queues a state change. Never blocks: the update is applied on the
    /// runtime, so the main thread is not held up by the shell.
    pub fn set_attention(&self, count: usize) {
        self.apply(move |service| service.attention = count);
    }

    pub fn set_syncing(&self, syncing: bool) {
        self.apply(move |service| service.syncing = syncing);
    }

    pub fn set_connected(&self, connected: bool) {
        self.apply(move |service| service.connected = connected);
    }

    fn apply<F>(&self, change: F)
    where
        F: FnOnce(&mut Service) + Send + 'static,
    {
        // `Handle::update` is async and the caller is the GTK thread, so the
        // change is handed to the runtime the service lives on rather than run
        // inline.
        let service = self.service.clone();
        let runtime = self.runtime.clone();
        runtime.spawn(async move {
            service.update(change).await;
        });
    }

    /// Dispatches whatever the tray asked for. Called from the main loop.
    pub fn dispatch<F>(&mut self, mut on_event: F)
    where
        F: FnMut(Event),
    {
        while let Ok(event) = self.events.try_recv() {
            on_event(event);
        }
    }
}

impl ksni::Tray for Service {
    fn id(&self) -> String {
        "gtaskbar".to_owned()
    }

    fn category(&self) -> Category {
        Category::ApplicationStatus
    }

    fn status(&self) -> Status {
        // `NeedsAttention` is what makes the shell badge the icon while
        // something is overdue; it clears again once the count reaches zero.
        if self.attention > 0 {
            Status::NeedsAttention
        } else {
            Status::Active
        }
    }

    fn title(&self) -> String {
        "GTaskbar".to_owned()
    }

    fn icon_name(&self) -> String {
        // A themed symbolic name rather than a pixmap. A generated pixmap could
        // show a number, but it could not be recoloured and would ignore the
        // shell's icon theme, so the count goes in the tooltip and the
        // attention state instead.
        crate::ui::icons::APP_SYMBOLIC.to_owned()
    }

    fn attention_icon_name(&self) -> String {
        crate::ui::icons::APP_SYMBOLIC.to_owned()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let title = if self.attention > 0 {
            format!("{} task{} due", self.attention, plural(self.attention))
        } else {
            "GTaskbar".to_owned()
        };

        let description = match (self.connected, self.syncing) {
            (false, _) => "Not connected".to_owned(),
            (true, true) => "Syncing…".to_owned(),
            (true, false) if self.attention > 0 => {
                format!("{} due today or overdue", self.attention)
            }
            (true, false) => "Nothing due".to_owned(),
        };

        ksni::ToolTip {
            title,
            description,
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Add Task".to_owned(),
                icon_name: crate::ui::icons::ADD.to_owned(),
                activate: Box::new(|service: &mut Self| {
                    let _ = service.events.send(Event::AddTask);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Sync Now".to_owned(),
                icon_name: crate::ui::icons::SYNC.to_owned(),
                enabled: self.connected,
                activate: Box::new(|service: &mut Self| {
                    let _ = service.events.send(Event::SyncNow);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Open GTaskbar".to_owned(),
                activate: Box::new(|service: &mut Self| {
                    let _ = service.events.send(Event::ShowWindow);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".to_owned(),
                activate: Box::new(|service: &mut Self| {
                    let _ = service.events.send(Event::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.events.send(Event::ShowWindow);
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        let _ = self.events.send(Event::ShowWindow);
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    use crate::model::view::{self, View};

    fn task(id: &str, due: Option<NaiveDate>) -> crate::store::models::Task {
        crate::store::models::Task {
            id: id.into(),
            title: id.into(),
            notes: String::new(),
            status: crate::store::models::TaskStatus::NeedsAction,
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
    fn pluralisation_is_correct() {
        assert_eq!(plural(1), "");
        assert_eq!(plural(0), "s");
        assert_eq!(plural(2), "s");
    }

    #[test]
    fn attention_is_due_today_plus_overdue() {
        let today = view::today();
        let tasks = [
            task("late", Some(today - chrono::Duration::days(1))),
            task("now", Some(today)),
            task("later", Some(today + chrono::Duration::days(4))),
            task("undated", None),
        ];

        let count = tasks
            .iter()
            .filter(|t| View::Overdue.matches(t, today) || View::Today.matches(t, today))
            .count();

        assert_eq!(count, 2, "later and undated tasks are not attention-worthy");
    }

    #[test]
    fn a_completed_task_is_never_counted() {
        let today = view::today();
        let mut done = task("done", Some(today));
        done.status = crate::store::models::TaskStatus::Completed;
        assert!(!View::Today.matches(&done, today));
    }
}
