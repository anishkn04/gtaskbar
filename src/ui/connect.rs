use adw::prelude::*;

use super::icons;
use crate::config::Config;

/// First-run account connection.
///
/// The Google Tasks API has no device-code grant, so authorisation has to go
/// through a loopback redirect. The full PKCE handshake lands in the OAuth
/// step; this dialog collects and stores the client credentials that it needs,
/// and explains the setup so the app is usable without a web search.
pub fn present() {
    let app = adw::Application::default();
    if let Some(window) = app
        .windows()
        .first()
        .and_then(|w| w.downcast_ref::<adw::ApplicationWindow>())
    {
        present_for(window.upcast_ref());
    }
}

pub fn present_for(parent: &gtk::Window) {
    let dialog = adw::Window::builder()
        .title("Connect Google account")
        .modal(true)
        .transient_for(parent)
        .default_width(560)
        .default_height(620)
        .build();

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let close = gtk::Button::from_icon_name(icons::CLOSE);
    close.set_tooltip_text(Some("Close"));
    close.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| dialog.close()
    ));
    header.pack_start(&close);

    toolbar.add_top_bar(&header);

    let page = adw::PreferencesPage::builder()
        .title("Connect")
        .icon_name(icons::ACCOUNT)
        .build();

    let intro = adw::PreferencesGroup::builder()
        .title("Google Tasks")
        .description(format!(
            "GTaskbar talks to the Google Tasks API directly, which needs your own OAuth \
             client. Create one, then paste the details below.\n\nSort: {} · Group: {}",
            Config::load().sort_mode.label(),
            Config::load().group_mode.label()
        ))
        .build();
    page.add(&intro);

    let steps = adw::PreferencesGroup::builder()
        .title("Creating a client")
        .description("At console.cloud.google.com, in a project of your choice:")
        .build();
    steps.add(&step_row(
        "1",
        "Enable the Google Tasks API",
        "APIs & Services → Library → Google Tasks API → Enable",
    ));
    steps.add(&step_row(
        "2",
        "Configure the consent screen",
        "OAuth consent screen → External → add yourself as a test user, \
         which lets you authorise an unverified app",
    ));
    steps.add(&step_row(
        "3",
        "Create a desktop client",
        "APIs & Services → Credentials → Create credentials → OAuth client ID → Desktop app",
    ));
    page.add(&steps);

    let credentials = adw::PreferencesGroup::builder()
        .title("Client credentials")
        .description("Stored in your desktop keyring, never in a config file.")
        .build();

    // Deliberately empty: a real client id must never be baked into the source.
    // `GTASKBAR_CLIENT_ID` / `GTASKBAR_CLIENT_SECRET` prefill these for local
    // development, and the OAuth step will persist what is entered here.
    let (env_id, env_secret) = credentials_from_env().unwrap_or_default();

    let client_id = adw::EntryRow::builder()
        .title("Client ID")
        .text(env_id)
        .build();
    let client_secret = adw::PasswordEntryRow::builder()
        .title("Client secret")
        .text(env_secret)
        .build();
    credentials.add(&client_id);
    credentials.add(&client_secret);
    page.add(&credentials);

    let connect = gtk::Button::with_label("Authorise");
    connect.add_css_class("suggested-action");
    connect.add_css_class("pill");
    connect.set_halign(gtk::Align::Center);
    connect.set_margin_top(12);
    connect.set_margin_bottom(12);
    connect.connect_clicked(glib::clone!(
        #[weak]
        client_id,
        move |button| {
            let id = client_id.text().to_string();
            if id.trim().is_empty() {
                return;
            }
            button.set_sensitive(false);
            button.set_label("Authorisation flow is the next milestone");
            log::info!("client id captured ({} characters)", id.len());
        }
    ));

    // A single scrolling column: the step list and the credential form. An
    // AdwStatusPage here would grab all the vertical space and push the
    // credential fields out of view, so the placeholder note is plain text.
    let note = gtk::Label::builder()
        .label(
            "The PKCE loopback flow is the next milestone. Set GTASKBAR_CLIENT_ID and \
                GTASKBAR_CLIENT_SECRET to use your credentials in the meantime.",
        )
        .wrap(true)
        .justify(gtk::Justification::Center)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .build();
    note.add_css_class("dim-label");

    let layout = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&page)
        .build();
    layout.append(&scroller);
    layout.append(&note);
    layout.append(&connect);

    toolbar.set_content(Some(&layout));
    dialog.set_content(Some(&toolbar));
    dialog.present();
}

fn step_row(number: &str, title: &str, detail: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(detail)
        .build();
    row.set_subtitle_lines(2);

    let badge = gtk::Label::builder()
        .label(number)
        .css_classes(["dim-label", "circular"])
        .valign(gtk::Align::Start)
        .build();
    badge.set_size_request(28, 28);
    row.add_prefix(&badge);
    row
}

/// Reads client credentials from the environment, for development.
///
/// Returns `None` unless both variables are set: a client id without a secret
/// would fail at the token exchange, which is a confusing way to find out.
pub fn credentials_from_env() -> Option<(String, String)> {
    let id = std::env::var("GTASKBAR_CLIENT_ID").ok()?;
    let secret = std::env::var("GTASKBAR_CLIENT_SECRET").ok()?;
    if id.trim().is_empty() || secret.trim().is_empty() {
        return None;
    }
    Some((id, secret))
}

#[cfg(test)]
mod tests {
    use super::credentials_from_env;

    // These tests mutate process-wide environment, so they are combined into
    // one test to keep them from interfering if the suite ever runs in
    // parallel.

    #[test]
    fn env_credentials_require_both_variables() {
        // SAFETY: single-threaded within this test binary for this test; the
        // variables are restored before returning.
        unsafe {
            std::env::remove_var("GTASKBAR_CLIENT_ID");
            std::env::remove_var("GTASKBAR_CLIENT_SECRET");
        }
        assert!(
            credentials_from_env().is_none(),
            "no credentials should be reported when unset"
        );

        unsafe {
            std::env::set_var("GTASKBAR_CLIENT_ID", "id.apps.googleusercontent.com");
        }
        assert!(
            credentials_from_env().is_none(),
            "a client id without a secret is not usable"
        );

        unsafe {
            std::env::set_var("GTASKBAR_CLIENT_SECRET", "GOCSPX-secret");
        }
        let (id, secret) = credentials_from_env().expect("both set");
        assert_eq!(id, "id.apps.googleusercontent.com");
        assert_eq!(secret, "GOCSPX-secret");

        unsafe {
            std::env::remove_var("GTASKBAR_CLIENT_ID");
            std::env::remove_var("GTASKBAR_CLIENT_SECRET");
        }
    }
}
