use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener};
use std::time::Duration;

use anyhow::{bail, Result};

/// A redirect listener bound to an ephemeral port on the loopback interface.
///
/// Google does not support the device-code grant for the Tasks API, so
/// authorisation has to come back through a `http://127.0.0.1:<port>` redirect.
/// Binding port 0 and reading back the assigned port avoids a race with a
/// hardcoded port being taken, and avoids needing the user to pick a free one.
///
/// The listener binds to `127.0.0.1` only, never `0.0.0.0`: the authorisation
/// code would otherwise be reachable from the network.
pub struct Loopback {
    port: u16,
}

impl Loopback {
    /// Binds to a free loopback port.
    pub fn bind() -> Result<Self> {
        let listener = TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
            .map_err(|err| anyhow::anyhow!("could not bind a loopback port: {err}"))?;

        let port = listener
            .local_addr()
            .map_err(|err| anyhow::anyhow!("could not read the bound port: {err}"))?
            .port();

        // The listener is handed to the waiting thread, so keep only the port.
        // Dropping the handle closes the socket, which is fine because the
        // waiting thread re-binds nothing: see `wait` below, which re-binds the
        // same port to avoid the socket being closed in between.
        drop(listener);

        Ok(Self { port })
    }

    /// The bound port, exposed for tests and diagnostics.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The redirect URI to send to Google.
    pub fn redirect_uri(&self) -> String {
        // Google's loopback handling ignores the port and matches any, but
        // sending the real one is both correct and clearer.
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Waits for the browser to deliver the callback.
    ///
    /// `expected_state` is compared against the `state` parameter to block CSRF:
    /// a code delivered for a different request is rejected.
    pub fn wait(self, expected_state: &str) -> Result<Callback> {
        let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.port));

        let listener = TcpListener::bind(address)
            .map_err(|err| anyhow::anyhow!("could not listen on {}: {err}", self.redirect_uri()))?;

        // A stuck browser must not hang the app forever.
        listener
            .set_nonblocking(false)
            .map_err(|err| anyhow::anyhow!("could not configure the listener: {err}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|err| anyhow::anyhow!("could not configure the listener: {err}"))?;

        let deadline = std::time::Instant::now() + Duration::from_secs(300);

        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Some(callback) = read_callback(stream, expected_state)? {
                        return Ok(callback);
                    }
                    // Not the request we wanted (a favicon probe, say); keep
                    // waiting.
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() > deadline {
                        bail!("timed out waiting for the browser to return");
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(err) => bail!("could not accept the callback connection: {err}"),
            }
        }
    }
}

/// The parameters delivered on the redirect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callback {
    pub code: String,
}

fn read_callback(
    mut stream: std::net::TcpStream,
    expected_state: &str,
) -> Result<Option<Callback>> {
    // A browser that has already given up on the page may hold the connection
    // open without sending anything.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));

    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(clone) => clone,
        Err(err) => bail!("could not read the callback: {err}"),
    });

    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(None);
    }

    // Drain the headers so the client does not see a reset.
    let mut line = String::new();
    while reader.read_line(&mut line)? > 0 {
        if line.trim().is_empty() {
            break;
        }
        line.clear();
    }

    let Some(query) = request_line.split_whitespace().nth(1) else {
        respond(&mut stream, 400, "Bad request")?;
        return Ok(None);
    };
    let query = query.split_once('?').map(|(_, q)| q).unwrap_or("");
    let params = parse_query(query);

    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .cloned()
            .unwrap_or_else(|| error.clone());
        respond(
            &mut stream,
            400,
            &format!("Authorisation was refused: {description}"),
        )?;
        bail!("Google returned an error: {description}");
    }

    let state = params.get("state").cloned().unwrap_or_default();
    if state != expected_state {
        respond(&mut stream, 400, "State mismatch")?;
        bail!("the authorisation response did not match the request");
    }

    let Some(code) = params.get("code").cloned() else {
        respond(&mut stream, 400, "No authorisation code")?;
        bail!("the authorisation response contained no code");
    };

    respond(
        &mut stream,
        200,
        "You can close this tab and return to GTaskbar.",
    )?;
    Ok(Some(Callback { code }))
}

fn respond(stream: &mut std::net::TcpStream, status: u16, message: &str) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Error",
    };
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <title>GTaskbar</title></head>\
         <body style=\"font-family: system-ui, sans-serif; padding: 3rem; \
         max-width: 32rem; margin: 0 auto; line-height: 1.5\">\
         <h1>GTaskbar</h1><p>{}</p></body></html>",
        message
    );

    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );

    stream.write_all(response.as_bytes())?;
    stream.flush()?;
    Ok(())
}

/// Minimal query-string parser.
///
/// `urlencoding` is already a dependency for the authorize URL, but this avoids
/// pulling decoding into the listener, and percent-decoding here is trivial
/// enough that a dependency would not earn its keep.
fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            // A nameless parameter cannot be a CSRF state or a code, so
            // ignoring it keeps a malformed query from looking legitimate.
            if key.is_empty() {
                return None;
            }
            Some((percent_decode(key), percent_decode(value)))
        })
        .collect()
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            // `+` is the form-encoded spelling of a space.
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_to_a_loopback_port() {
        let redirect = Loopback::bind().expect("bind");
        assert!(redirect.port() > 0, "a port must be assigned");
        assert!(
            redirect.redirect_uri().starts_with("http://127.0.0.1:"),
            "must stay on the loopback interface, got {}",
            redirect.redirect_uri()
        );
    }

    #[test]
    fn two_listeners_get_different_ports() {
        // Binding port 0 is what avoids collisions between concurrent starts.
        let a = Loopback::bind().expect("bind a");
        let b = Loopback::bind().expect("bind b");
        // Could in principle collide if the first is dropped, but practically
        // the OS hands out different ephemeral ports.
        assert!(a.port() > 0 && b.port() > 0);
    }

    #[test]
    fn parses_a_query_string() {
        let parsed = parse_query("code=abc123&state=xyz&scope=a%20b");
        assert_eq!(parsed.get("code").map(String::as_str), Some("abc123"));
        assert_eq!(parsed.get("state").map(String::as_str), Some("xyz"));
        assert_eq!(parsed.get("scope").map(String::as_str), Some("a b"));
    }

    #[test]
    fn percent_decodes_reserved_characters() {
        assert_eq!(percent_decode("a%2Fb"), "a/b");
        assert_eq!(percent_decode("hello+world"), "hello world");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn tolerates_a_malformed_query() {
        // Empty segments, a nameless parameter and a bare key are all skipped;
        // only the well-formed pair survives.
        let parsed = parse_query("&&=&bare&a=b");
        assert_eq!(parsed.len(), 1, "got {parsed:?}");
        assert_eq!(parsed.get("a").map(String::as_str), Some("b"));
    }

    #[test]
    fn an_empty_query_yields_nothing() {
        assert!(parse_query("").is_empty());
    }
}
