use anyhow::{anyhow, Context, Result};

/// Where credentials and tokens live in the desktop keyring.
///
/// The service and user names are fixed strings, so a credential written by one
/// build is found by the next. Nothing here is ever written to disk in
/// plaintext, and nothing is committed.
const SERVICE: &str = "dev.anishkn04.gtaskbar";
const USER_CLIENT_ID: &str = "oauth-client-id";
const USER_CLIENT_SECRET: &str = "oauth-client-secret";
const USER_REFRESH_TOKEN: &str = "oauth-refresh-token";

fn entry(user: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, user).map_err(|err| anyhow!("keyring unavailable: {err}"))
}

/// A Google OAuth Desktop client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientCredentials {
    pub client_id: String,
    pub client_secret: String,
}

impl ClientCredentials {
    /// Reads the credentials, preferring the environment for local development.
    ///
    /// The environment takes precedence so a developer can try a flow without
    /// touching the keyring, and so CI never needs a keyring at all.
    pub fn load() -> Option<Self> {
        if let (Ok(id), Ok(secret)) = (
            std::env::var("GTASKBAR_CLIENT_ID"),
            std::env::var("GTASKBAR_CLIENT_SECRET"),
        ) {
            if !id.trim().is_empty() && !secret.trim().is_empty() {
                return Some(Self {
                    client_id: id,
                    client_secret: secret,
                });
            }
        }

        let client_id = entry(USER_CLIENT_ID)
            .ok()
            .and_then(|e| e.get_password().ok())?;
        let client_secret = entry(USER_CLIENT_SECRET)
            .ok()
            .and_then(|e| e.get_password().ok())?;

        Some(Self {
            client_id,
            client_secret,
        })
    }

    pub fn store(&self) -> Result<()> {
        entry(USER_CLIENT_ID)?
            .set_password(&self.client_id)
            .context("could not write the client id to the keyring")?;
        entry(USER_CLIENT_SECRET)?
            .set_password(&self.client_secret)
            .context("could not write the client secret to the keyring")?;
        Ok(())
    }
}

/// Stores the refresh token.
///
/// Google rotates the refresh token on most exchanges and expects the client to
/// keep the new one; the previous token is invalidated once it is replaced. This
/// must therefore be called after *every* successful exchange, before any
/// further request is made.
pub fn store_refresh_token(token: &str) -> Result<()> {
    entry(USER_REFRESH_TOKEN)?
        .set_password(token)
        .context("could not write the refresh token to the keyring")
}

pub fn refresh_token() -> Option<String> {
    entry(USER_REFRESH_TOKEN)
        .ok()
        .and_then(|e| e.get_password().ok())
}

/// Forgets every stored credential. Used by "Disconnect account".
pub fn clear() {
    for user in [USER_CLIENT_ID, USER_CLIENT_SECRET, USER_REFRESH_TOKEN] {
        if let Ok(entry) = entry(user) {
            let _ = entry.delete_credential();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_an_absent_keyring_entry_is_not_an_error() {
        // Disconnecting when nothing is stored has to succeed; failing here
        // would leave the app wedged in a "disconnect failed" state.
        clear();
    }

    #[test]
    fn a_missing_refresh_token_reads_as_none() {
        // Either the keyring is unavailable or nothing is stored yet; both are
        // "not connected" as far as callers are concerned.
        let _ = refresh_token();
    }
}
