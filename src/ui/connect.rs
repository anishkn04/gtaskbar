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
    let client_secret_for_click = client_secret.clone();
    connect.connect_clicked(glib::clone!(
        #[weak]
        client_id,
        #[weak]
        client_secret_for_click,
        move |button| {
            let id = client_id.text().trim().to_string();
            let secret = client_secret_for_click.text().trim().to_string();

            if id.is_empty() || secret.is_empty() {
                button.set_label("Enter a client ID and secret first");
                return;
            }

            let credentials = crate::auth::credentials::ClientCredentials {
                client_id: id,
                client_secret: secret,
            };
            if let Err(err) = credentials.store() {
                log::error!("could not save the credentials: {err}");
                button.set_label("Could not reach the keyring");
                return;
            }

            button.set_sensitive(false);
            button.set_label("Waiting for the browser…");
            start_authorisation(button.clone());
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

/// Opens the browser and completes the authorisation.
///
/// Runs on a worker thread because the loopback listener blocks until the
/// browser returns, and the GTK main loop must stay responsive. The button is
/// updated from the main context when the flow finishes.
fn start_authorisation(button: gtk::Button) {
    let pending = match crate::auth::flow::begin() {
        Ok(pending) => pending,
        Err(err) => {
            log::error!("could not start the authorisation flow: {err}");
            button.set_sensitive(true);
            button.set_label("Authorise");
            return;
        }
    };

    let url = pending.authorize_url.clone();
    if let Err(err) = open_in_browser(&url) {
        log::error!("could not open a browser: {err}");
        button.set_sensitive(true);
        button.set_label("Authorise");
        return;
    }

    // Neither a widget nor even a weak reference to one is `Send`, so nothing
    // GTK-owned crosses the thread boundary. The worker sends a plain result
    // over a channel, and the main thread polls for it.
    let (tx, rx) = std::sync::mpsc::channel::<Result<crate::auth::flow::Authorized, String>>();

    if let Err(err) = std::thread::Builder::new()
        .name("gtaskbar-oauth".into())
        .spawn(move || {
            let outcome =
                block_on(crate::auth::flow::finish(pending)).map_err(|err| err.to_string());
            let _ = tx.send(outcome);
        })
    {
        log::error!("could not start the authorisation thread: {err}");
        button.set_sensitive(true);
        button.set_label("Authorise");
        return;
    }

    glib::MainContext::default().spawn_local(async move {
        glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
            let outcome = match rx.try_recv() {
                Ok(outcome) => outcome,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    button.set_sensitive(true);
                    button.set_label("Try again");
                    return glib::ControlFlow::Break;
                }
            };

            match outcome {
                Ok(authorized) => {
                    if let Err(err) = authorized.persist() {
                        log::error!("authorised, but could not store the token: {err}");
                        button.set_sensitive(true);
                        button.set_label("Authorise");
                        return glib::ControlFlow::Break;
                    }
                    log::info!("authorised; reloading the window");
                    // A fresh authorisation should not immediately notify about
                    // everything the account has ever been reminded of.
                    crate::sync::queue::reset_notification_history();
                    let app = adw::Application::default();
                    crate::ui::window::rebuild(&app);
                    crate::sync::scheduler::request_sync(&app);
                }
                Err(message) => {
                    log::error!("authorisation failed: {message}");
                    button.set_sensitive(true);
                    button.set_label("Try again");
                }
            }

            glib::ControlFlow::Break
        });
    });
}

/// Opens a URL in the user's browser.
///
/// `xdg-open` is used rather than `GtkUriLauncher`, which is gated behind newer
/// GLib than some distributions ship, and which would additionally route
/// through the desktop portal for no benefit here.
fn open_in_browser(url: &str) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

/// Drives an async future to completion on a fresh runtime.
///
/// The authorisation flow is a one-shot, so a dedicated runtime is simpler than
/// sharing the sync engine's.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(future),
        Err(err) => {
            log::error!("could not start a runtime for the token exchange: {err}");
            // The caller's only job with this value is to format it, so rather
            // than inventing a fake value, block forever: the button stays
            // disabled, which is the honest representation of "not finished".
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
    }
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
