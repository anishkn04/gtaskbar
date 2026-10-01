//! The app's icon names.
//!
//! Every icon is an official GNOME icon from `adwaita-icon-theme`, referenced by
//! name and resolved by GTK at runtime. Nothing is bundled.
//!
//! That is the point of libadwaita: an app built on it is supposed to look like
//! it belongs to the desktop it is running on, which means it uses the icon
//! theme the user has chosen. Bundling artwork works against that — it makes the
//! app look identical everywhere, including places where the system theme would
//! have looked better, and it means the icons can never pick up a user's theme,
//! a high-contrast variant, or a custom icon set.
//!
//! Earlier revisions of this file generated a bespoke monochrome set. That was
//! the wrong call twice over: it ignored the platform's icon theme, and GTK's
//! handling of `-symbolic` SVG neither preserves `fill="none"` nor reads
//! `currentColor`, so the stroked outlines rendered as solid blobs while the
//! thin-line ones vanished on a dark background. Both problems are inherent to
//! hand-authored symbolic SVGs rather than to the drawing, and are avoided
//! entirely by not authoring any.
//!
//! The only artwork gtaskbar ships is the application icon, which has to exist
//! independently of any theme. See `docs/icons.md`.

/// The application icon.
///
/// The one piece of artwork gtaskbar must ship, because an application icon has
/// to exist independently of any icon theme. Sourced from an official upstream
/// set rather than drawn here; see docs/icons.md for provenance and licensing.
pub const APP: &str = "gtaskbar";

/// The symbolic variant, for the tray and for notifications, which want a
/// themed `GioIcon` rather than a widget.
pub const APP_SYMBOLIC: &str = "gtaskbar-symbolic";

/// Main window and task list.
pub const LIST: &str = "view-list-symbolic";
/// App menu button.
pub const MENU: &str = "open-menu-symbolic";
/// Window close button.
pub const CLOSE: &str = "window-close-symbolic";
/// Preferences entry.
pub const SETTINGS: &str = "preferences-system-symbolic";
/// Completion tick on a finished task.
pub const CHECK: &str = "object-select-symbolic";
/// Quick-add affordance.
pub const ADD: &str = "list-add-symbolic";
/// Manual and scheduled sync.
pub const SYNC: &str = "view-refresh-symbolic";
/// Account, and the connect state.
pub const ACCOUNT: &str = "avatar-default-symbolic";
// Smart views.
pub const TODAY: &str = "x-office-calendar-symbolic";
pub const UPCOMING: &str = "alarm-symbolic";
pub const OVERDUE: &str = "dialog-warning-symbolic";
pub const ALL: &str = "view-continuous-symbolic";
pub const COMPLETED: &str = "checkbox-symbolic";

/// A themed image widget for one of the app's icons.
///
/// Goes through `Image::from_icon_name` so the widget participates in GTK's
/// icon theme: the icon is recoloured for hover, pressed, insensitive and
/// destructive states, and follows the active theme in light and dark.
pub fn image(name: &str) -> gtk::Image {
    gtk::Image::from_icon_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn all() -> [&'static str; 13] {
        [
            LIST, MENU, CLOSE, SETTINGS, CHECK, ADD, SYNC, ACCOUNT, TODAY, UPCOMING, OVERDUE, ALL,
            COMPLETED,
        ]
    }

    #[test]
    fn icon_names_are_unique() {
        let names = all();
        let unique: HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "icon names must be unique");
    }

    #[test]
    fn every_icon_is_symbolic() {
        // A non-symbolic name would render at full colour and ignore the
        // theme's foreground, which is exactly the inconsistency this module
        // exists to prevent.
        for name in all() {
            assert!(name.ends_with("-symbolic"), "{name} is not symbolic");
        }
    }

    #[test]
    fn every_icon_is_an_upstream_gnome_name() {
        // A gtaskbar-namespaced name would resolve to nothing now that nothing
        // is bundled, and fail silently as a blank space.
        for name in all() {
            assert!(
                !name.starts_with("gtaskbar"),
                "{name} looks like it is namespaced, but no artwork is bundled"
            );
        }
    }
}
