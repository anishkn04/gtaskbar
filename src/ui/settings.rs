use std::cell::RefCell;

use adw::prelude::*;

use crate::config::{Config, GroupMode, SortMode};

/// Preferences dialog. Deliberately not a GObject subclass: every control reads
/// from and writes straight to `Config` on disk, so there is no widget state to
/// mirror and no need for an `imp` struct.
pub fn present(app: &adw::Application) {
    // `PreferencesDialog` is an `AdwDialog`, not a `GtkWindow`, so it is not
    // reachable through `Application::windows()`. Track the live instance here
    // instead, so a second invocation re-raises the existing dialog.
    thread_local! {
        static OPEN: RefCell<Option<adw::PreferencesDialog>> = const { RefCell::new(None) };
    }

    if OPEN.with(|slot| slot.borrow().is_some()) {
        if let Some(window) = app.active_window() {
            if let Some(dialog) = OPEN.with(|slot| slot.borrow().clone()) {
                dialog.present(Some(&window));
                return;
            }
        }
        // No parent window to present against (headless or tray-only): fall
        // through and rebuild.
    }

    let parent = app.active_window();
    let dialog = adw::PreferencesDialog::builder()
        .title("Preferences")
        .search_enabled(true)
        .content_width(600)
        .content_height(560)
        .build();

    let page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();

    let display = adw::PreferencesGroup::builder()
        .title("Display")
        .description("How tasks are ordered and grouped in the list view.")
        .build();
    display.add(&sort_row());
    display.add(&group_row());
    page.add(&display);

    let notifications = adw::PreferencesGroup::builder()
        .title("Notifications")
        .description("GTaskbar can notify you when tasks become due.")
        .build();
    notifications.add(&notify_hour_row());
    notifications.add(&notify_overdue_row());
    page.add(&notifications);

    let behaviour = adw::PreferencesGroup::builder().title("Behaviour").build();
    behaviour.add(&close_to_tray_row());
    page.add(&behaviour);

    dialog.add(&page);
    OPEN.with(|slot| *slot.borrow_mut() = Some(dialog.clone()));
    dialog.present(parent.as_ref());
}

fn combo_row(
    title: &str,
    subtitle: &str,
    options: &[(&str, &str)],
    selected: &str,
) -> adw::ComboRow {
    let labels: Vec<&str> = options.iter().map(|(_, label)| *label).collect();
    let row = adw::ComboRow::builder()
        .title(title)
        .subtitle(subtitle)
        .model(&gtk::StringList::new(&labels))
        .build();

    if let Some(index) = options.iter().position(|(id, _)| *id == selected) {
        row.set_selected(index as u32);
    }
    row
}

fn sort_row() -> adw::ComboRow {
    let config = Config::load();
    let options: Vec<(&str, &str)> = SortMode::ALL.iter().map(|m| (m.id(), m.label())).collect();
    let row = combo_row(
        "Sort tasks by",
        "Manual reordering syncs back to Google.",
        &options,
        config.sort_mode.id(),
    );

    row.connect_selected_notify(move |row| {
        let Some(mode) = SortMode::ALL.get(row.selected() as usize) else {
            return;
        };
        persist(|config| config.sort_mode = *mode);
    });

    row.upcast()
}

fn group_row() -> adw::ComboRow {
    let config = Config::load();
    let options: Vec<(&str, &str)> = GroupMode::ALL.iter().map(|m| (m.id(), m.label())).collect();
    let row = combo_row(
        "Group tasks by",
        "Section headers within the list.",
        &options,
        config.group_mode.id(),
    );

    row.connect_selected_notify(move |row| {
        let Some(mode) = GroupMode::ALL.get(row.selected() as usize) else {
            return;
        };
        persist(|config| config.group_mode = *mode);
    });

    row.upcast()
}

fn notify_hour_row() -> adw::SpinRow {
    let config = Config::load();
    let row = adw::SpinRow::builder()
        .title("Notify at")
        .subtitle("Hour of the day to notify about tasks due today.")
        .adjustment(&gtk::Adjustment::new(
            config.notify_hour as f64,
            0.0,
            23.0,
            1.0,
            1.0,
            0.0,
        ))
        .build();

    row.connect_value_notify(move |row| {
        let hour = row.value() as u32;
        persist(|config| config.notify_hour = hour);
    });

    row.upcast()
}

fn notify_overdue_row() -> adw::SwitchRow {
    let config = Config::load();
    let row = adw::SwitchRow::builder()
        .title("Notify when overdue")
        .subtitle("Fire a notification when a due date passes without completion.")
        .active(config.notify_overdue)
        .build();

    row.connect_active_notify(move |row| {
        let active = row.is_active();
        persist(|config| config.notify_overdue = active);
    });

    row.upcast()
}

fn close_to_tray_row() -> adw::SwitchRow {
    let config = Config::load();
    let row = adw::SwitchRow::builder()
        .title("Close to tray")
        .subtitle("Keep syncing in the background instead of quitting when the window closes.")
        .active(config.close_to_tray)
        .build();

    row.connect_active_notify(move |row| {
        let active = row.is_active();
        persist(|config| config.close_to_tray = active);
    });

    row.upcast()
}

fn persist(mutate: impl FnOnce(&mut Config)) {
    let mut config = Config::load();
    mutate(&mut config);
    if let Err(err) = config.save() {
        log::warn!("failed to save config: {err}");
    }
}
