//! The app's icon names.
//!
//! Every icon the app uses is bundled in its own GResource and listed here.
//! Two reasons this is a module rather than scattered string literals:
//!
//! 1. The set stays coherent. All of them share one visual language (16px grid,
//!    1.5px single stroke, round caps, `currentColor` only), so nothing
//!    multi-coloured or emblem-like sneaks in and clashes with the rest.
//! 2. The app never silently falls back to the system icon theme, so it looks
//!    the same on a bare install as on a fully themed desktop.
//!
//! The full-size app icon is `dev.anishkn04.gtaskbar`, resolved by the
//! application id rather than by name.

/// The full-size app icon.
///
/// Deliberately *not* the application id: the bundle registers it as
/// `gtaskbar.svg`, and the desktop entry installs it under the same name. GTK
/// resolves a bundled icon by the file name in the resource, so asking for
/// `dev.anishkn04.gtaskbar` would miss and fall back to a stock placeholder.
pub const APP: &str = "gtaskbar";

/// The symbolic variant, for places that need a `GioIcon` rather than a widget
/// (notifications, in particular).
pub const APP_SYMBOLIC: &str = "gtaskbar-symbolic";

/// Main window and task list.
pub const LIST: &str = "gtaskbar-list-symbolic";
/// App menu button.
pub const MENU: &str = "gtaskbar-menu-symbolic";
/// Window close button.
pub const CLOSE: &str = "gtaskbar-close-symbolic";
/// Preferences entry.
pub const SETTINGS: &str = "gtaskbar-settings-symbolic";
/// Checkbox and completion marks.
pub const CHECK: &str = "gtaskbar-check-symbolic";
/// Quick-add affordance.
pub const ADD: &str = "gtaskbar-add-symbolic";
/// Manual and scheduled sync.
pub const SYNC: &str = "gtaskbar-sync-symbolic";
/// Account / connect.
pub const ACCOUNT: &str = "gtaskbar-account-symbolic";

// Smart views.
pub const TODAY: &str = "gtaskbar-today-symbolic";
pub const UPCOMING: &str = "gtaskbar-upcoming-symbolic";
pub const OVERDUE: &str = "gtaskbar-overdue-symbolic";
pub const ALL: &str = "gtaskbar-all-symbolic";
pub const COMPLETED: &str = "gtaskbar-completed-symbolic";

/// A themed image widget for one of the app's icons.
///
/// Goes through `Image::from_icon_name` so the widget participates in GTK's
/// icon theme: GTK recolours symbolic icons for the current foreground and
/// picks up the size-appropriate variant automatically.
pub fn image(name: &str) -> gtk::Image {
    gtk::Image::from_icon_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn icon_names_are_unique() {
        let all = [
            LIST, MENU, CLOSE, SETTINGS, CHECK, ADD, SYNC, ACCOUNT, TODAY, UPCOMING, OVERDUE, ALL,
            COMPLETED,
        ];
        let unique: HashSet<_> = all.iter().collect();
        assert_eq!(unique.len(), all.len(), "icon names must be unique");
    }

    #[test]
    fn every_icon_is_namespaced_and_symbolic() {
        // A bare name could collide with a system icon of the same name, which
        // would silently swap our artwork for the theme's.
        let all = [
            LIST, MENU, CLOSE, SETTINGS, CHECK, ADD, SYNC, ACCOUNT, TODAY, UPCOMING, OVERDUE, ALL,
            COMPLETED,
        ];
        for name in all {
            assert!(name.starts_with("gtaskbar-"), "{name} is not namespaced");
            assert!(name.ends_with("-symbolic"), "{name} is not symbolic");
        }
    }
}
