use adw::prelude::*;

use super::icons;
use crate::auth::credentials::ClientCredentials;
use crate::config::Config;

/// First-run account connection.
///
/// The Google Tasks API has no device-code grant, so authorisation has to go
/// through a loopback redirect. The full PKCE handshake lands in the OAuth
/// step; this dialog collects and stores the client credentials that it needs,
/// and explains the setup so the app is usable without a web search.
pub fn present() {
    let Some(app) = crate::running_app() else {
        log::error!("no running application to present the connect dialog from");
        return;
    };
    if let Some(window) = app
        .windows()
        .first()
        .and_then(|w| w.downcast_ref::<adw::ApplicationWindow>())
    {
        present_for(window.upcast_ref());
    } else {
        // A hidden start has no window yet. The dialog is still useful without
        // a parent, and showing it is what brings the app forward.
        log::warn!("no main window yet; opening the connect dialog on its own");
        present_for_standalone();
    }
}

/// Opens the dialog with no parent window.
///
/// `transient_for` needs a parent, so the dialog is simply not modal against one.
/// This is the path taken when the app was started with `--hidden` and the user
/// reaches the connect flow from the tray or the main menu before any window has
/// been built.
fn present_for_standalone() {
    let dialog = adw::Window::builder()
        .title("Connect Google account")
        .default_width(560)
        .default_height(620)
        .build();
    build_dialog(&dialog);
    dialog.present();
}

pub fn present_for(parent: &gtk::Window) {
    let dialog = adw::Window::builder()
        .title("Connect Google account")
        .modal(true)
        .transient_for(parent)
        .default_width(560)
        .default_height(620)
        .build();

    build_dialog(&dialog);
    dialog.present();
}

/// Fills the dialog with the credential form and the setup instructions.
///
/// Shared by the parented and standalone paths so there is one copy of the form.
fn build_dialog(dialog: &adw::Window) {
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
        "APIs &amp; Services → Library → Google Tasks API → Enable",
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
        "APIs &amp; Services → Credentials → Create credentials → OAuth client ID → Desktop app",
    ));
    page.add(&steps);

    let credentials = adw::PreferencesGroup::builder()
        .title("Client credentials")
        .description("Stored in your desktop keyring, never in a config file.")
        .build();

    // Deliberately empty: a real client id must never be baked into the source.
    // `GTASKBAR_CLIENT_ID` / `GTASKBAR_CLIENT_SECRET` prefill these for local
    // development, and the OAuth step will persist what is entered here.
    let (env_id, env_secret) = credentials_from_env()
        .map(|credentials| (credentials.client_id, credentials.client_secret))
        .unwrap_or_default();

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
                    // Without this the window keeps showing the "Not connected"
                    // page and no sync is requested, so a successful sign-in
                    // looks like it did nothing.
                    match crate::running_app() {
                        Some(app) => {
                            crate::ui::window::rebuild(&app);
                            crate::sync::scheduler::request_sync(&app);
                        }
                        None => log::error!(
                            "authorised, but the running application could not be found, so the                              window was not refreshed"
                        ),
                    }
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
/// sharing the sync engine's. The future must not touch the keyring: secret-
/// service builds and `block_on`s a runtime of its own, and doing that from
/// inside this one aborts the process. `Pending` therefore carries the
/// credentials, read before this point.
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
pub fn credentials_from_env() -> Option<ClientCredentials> {
    credentials_from(
        std::env::var("GTASKBAR_CLIENT_ID").ok(),
        std::env::var("GTASKBAR_CLIENT_SECRET").ok(),
    )
}

/// The credential rules, as a pure function.
///
/// Split out from `credentials_from_env` so the rules are testable without
/// touching the process environment, which is shared state: a test that called
/// `set_var` would race every other test in the binary, and would also wipe the
/// developer's real credentials when they run the suite with `.env` loaded.
fn credentials_from(id: Option<String>, secret: Option<String>) -> Option<ClientCredentials> {
    let id = id?;
    let secret = secret?;
    if id.trim().is_empty() || secret.trim().is_empty() {
        return None;
    }
    Some(ClientCredentials {
        client_id: id,
        client_secret: secret,
    })
}

#[cfg(test)]
mod tests {

    // These tests mutate process-wide environment, so they are combined into
    // one test to keep them from interfering if the suite ever runs in
    // parallel.

    use super::credentials_from;

    #[test]
    fn credentials_need_both_variables() {
        assert!(
            credentials_from(None, None).is_none(),
            "neither variable set is not connected"
        );
        assert!(
            credentials_from(Some("id".into()), None).is_none(),
            "a client id without a secret would fail at the token exchange, which is a
             confusing way to find out, so it is rejected here instead"
        );
        assert!(
            credentials_from(None, Some("secret".into())).is_none(),
            "a secret without a client id identifies nothing"
        );
    }

    #[test]
    fn credentials_are_returned_when_both_are_present() {
        let credentials = credentials_from(
            Some("id.apps.googleusercontent.com".into()),
            Some("secret".into()),
        )
        .expect("both set");
        assert_eq!(credentials.client_id, "id.apps.googleusercontent.com");
        assert_eq!(credentials.client_secret, "secret");
    }

    #[test]
    fn blank_credentials_are_rejected() {
        // A present-but-empty variable is a misconfiguration, not a credential.
        for (id, secret) in [
            (Some("   ".to_string()), Some("secret".to_string())),
            (Some("id".to_string()), Some("   ".to_string())),
            (Some(String::new()), Some(String::new())),
        ] {
            assert!(
                credentials_from(id, secret).is_none(),
                "whitespace-only credentials must not be accepted"
            );
        }
    }
}
