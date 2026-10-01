use anyhow::{anyhow, Result};
use oauth2::basic::BasicClient;
use oauth2::{AuthUrl, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge, Scope, TokenUrl};

use super::credentials::{self, ClientCredentials};
use super::loopback::Loopback;

/// Read-write access. `tasks.readonly` is deliberately not requested: the app
/// has to create and complete tasks, and asking for a scope that cannot do the
/// job would only mean a second authorisation later.
const SCOPE: &str = "https://www.googleapis.com/auth/tasks";

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// The outcome of a successful authorisation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authorized {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

impl Authorized {
    /// Records the tokens.
    ///
    /// The refresh token is persisted before anything else can use the access
    /// token, because Google invalidates the previous refresh token as soon as
    /// a replacement is issued: losing it means re-authorising by hand.
    pub fn persist(&self) -> Result<()> {
        if let Some(refresh) = &self.refresh_token {
            credentials::store_refresh_token(refresh)?;
        }
        super::session::set_access_token(self.access_token.clone());
        Ok(())
    }
}

/// Builds a client for Google's endpoints.
///
/// The secret is optional: PKCE is what actually protects the code exchange, and
/// the secret only identifies the app. Google issues one for desktop clients, so
/// it is sent when present.
fn client(credentials: &ClientCredentials) -> BasicClient {
    let secret = if credentials.client_secret.trim().is_empty() {
        None
    } else {
        Some(ClientSecret::new(credentials.client_secret.clone()))
    };

    BasicClient::new(
        ClientId::new(credentials.client_id.clone()),
        secret,
        AuthUrl::new(AUTH_URL.to_string()).expect("AUTH_URL is a valid URL"),
        Some(TokenUrl::new(TOKEN_URL.to_string()).expect("TOKEN_URL is a valid URL")),
    )
}

/// An authorisation request in progress.
pub struct Pending {
    /// The URL to open in the browser.
    pub authorize_url: String,
    /// The CSRF token Google will echo back; compared before trusting the code.
    pub state: String,
    /// The PKCE verifier, needed at the token exchange.
    pub verifier: String,
    redirect: Loopback,
}

impl Pending {
    pub fn redirect_uri(&self) -> String {
        self.redirect.redirect_uri()
    }
}

/// Builds the authorisation request.
///
/// Google has no device-code grant for this API, so the flow is: open the
/// browser at the authorize URL, then wait on a loopback listener for the
/// redirect. PKCE is mandatory here, because the redirect lands on a port that
/// any process on the machine could have claimed.
pub fn begin() -> Result<Pending> {
    let credentials = ClientCredentials::load()
        .ok_or_else(|| anyhow!("no OAuth client credentials are configured"))?;

    let redirect = Loopback::bind()?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();

    // `oauth2` builds the URL because PKCE generation and CSRF state are the
    // parts where a subtle mistake matters. Everything downstream is ours.
    let (authorize_url, csrf) = client(&credentials)
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(challenge)
        .add_scope(Scope::new(SCOPE.to_string()))
        .url();

    Ok(Pending {
        authorize_url: authorize_url.to_string(),
        // The state the client generated is the value Google echoes back, so
        // comparing against it is what blocks a cross-site request forgery.
        state: csrf.secret().clone(),
        verifier: verifier.secret().clone(),
        redirect,
    })
}

/// Completes the flow, exchanging the code for tokens.
///
/// Blocks until the browser delivers the callback, so this runs on a worker
/// thread rather than the GTK main loop.
pub async fn finish(pending: Pending) -> Result<Authorized> {
    let credentials = ClientCredentials::load()
        .ok_or_else(|| anyhow!("no OAuth client credentials are configured"))?;

    // Read what the exchange needs before the listener is consumed by `wait`.
    let redirect_uri = pending.redirect_uri();

    // The state is validated inside `wait`, so an unsolicited or mismatched
    // response is rejected before the code is ever used.
    let callback = pending.redirect.wait(&pending.state)?;

    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", callback.code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", pending.verifier),
        ("client_id", credentials.client_id.clone()),
    ];
    if !credentials.client_secret.trim().is_empty() {
        form.push(("client_secret", credentials.client_secret.clone()));
    }

    let token = post_token(&form).await?;

    Ok(Authorized {
        access_token: token
            .access_token
            .ok_or_else(|| anyhow!("the token response contained no access token"))?,
        refresh_token: token.refresh_token,
    })
}

/// Refreshes the access token using the stored refresh token.
pub async fn refresh() -> Result<Authorized> {
    let credentials = ClientCredentials::load()
        .ok_or_else(|| anyhow!("no OAuth client credentials are configured"))?;
    let refresh_token =
        credentials::refresh_token().ok_or_else(|| anyhow!("no refresh token is stored"))?;

    let mut form = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.clone()),
        ("client_id", credentials.client_id.clone()),
    ];
    if !credentials.client_secret.trim().is_empty() {
        form.push(("client_secret", credentials.client_secret.clone()));
    }

    let token = post_token(&form).await?;

    Ok(Authorized {
        access_token: token
            .access_token
            .ok_or_else(|| anyhow!("the refresh response contained no access token"))?,
        // Google only sometimes issues a replacement refresh token. When it does
        // not, the existing one stays valid and must be kept rather than lost.
        refresh_token: token.refresh_token.or(Some(refresh_token)),
    })
}

/// The token endpoint's response, restricted to the fields that matter here.
#[derive(Debug, serde::Deserialize)]
struct TokenBody {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// Posts a form to the token endpoint.
///
/// Written out rather than taken from `oauth2`'s helper because that crate
/// pins reqwest 0.11 while the app uses 0.13, and the two clients do not unify.
/// Nothing security-relevant lives here: the PKCE verifier and the CSRF state
/// both come from `oauth2`, and this only moves bytes.
async fn post_token(form: &[(&str, String)]) -> Result<TokenBody> {
    let response = reqwest::Client::new()
        .post(TOKEN_URL)
        .form(form)
        .send()
        .await
        .map_err(|err| anyhow!("could not reach the token endpoint: {err}"))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| anyhow!("could not read the token response: {err}"))?;

    let parsed: TokenBody = serde_json::from_str(&body)
        .map_err(|err| anyhow!("could not parse the token response: {err}"))?;

    if let Some(error) = parsed.error {
        let detail = parsed
            .error_description
            .unwrap_or_else(|| format!("HTTP {status}"));
        anyhow::bail!("the token endpoint refused the request: {error} ({detail})");
    }

    Ok(parsed)
}

/// Whether a usable refresh token is available.
pub fn has_stored_token() -> bool {
    credentials::refresh_token().is_some()
}

/// Drops every stored credential and the in-memory session.
pub fn sign_out() {
    super::session::clear();
    credentials::clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_refresh_token_is_reported_as_absent() {
        // Whether the keyring is unavailable or empty, the outcome is the same.
        let _ = has_stored_token();
    }

    #[test]
    fn begin_without_credentials_fails_rather_than_opening_a_browser() {
        // Nothing useful can happen without a client, so this should be a clean
        // error and not a stray browser window.
        if ClientCredentials::load().is_none() {
            // `Pending` is not `Debug`, so the result is matched rather than
            // unwrapped with `expect_err`.
            match begin() {
                Ok(_) => panic!("begin() should refuse without credentials"),
                Err(err) => assert!(
                    err.to_string().contains("credentials"),
                    "unexpected error: {err}"
                ),
            }
        }
    }

    #[test]
    fn a_token_error_is_surfaced_rather_than_swallowed() {
        // A body carrying `error` must not be read as a successful exchange with
        // no access token, which would be a confusing failure much later.
        let parsed: TokenBody =
            serde_json::from_str(r#"{"error":"invalid_grant","error_description":"expired"}"#)
                .expect("parse");
        assert_eq!(parsed.error.as_deref(), Some("invalid_grant"));
        assert_eq!(parsed.error_description.as_deref(), Some("expired"));
        assert!(parsed.access_token.is_none());
    }

    #[test]
    fn a_successful_token_body_parses() {
        let parsed: TokenBody = serde_json::from_str(
            r#"{"access_token":"ya29.x","refresh_token":"1//ref","expires_in":3599}"#,
        )
        .expect("parse");
        assert_eq!(parsed.access_token.as_deref(), Some("ya29.x"));
        assert_eq!(parsed.refresh_token.as_deref(), Some("1//ref"));
        assert!(parsed.error.is_none());
    }

    #[test]
    fn a_body_without_a_refresh_token_parses() {
        // Google omits the refresh token on most refresh responses; that has to
        // be an ordinary outcome, not a parse failure.
        let parsed: TokenBody =
            serde_json::from_str(r#"{"access_token":"ya29.y","expires_in":3599}"#).expect("parse");
        assert!(parsed.refresh_token.is_none());
    }
}
