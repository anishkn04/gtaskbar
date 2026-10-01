// Google account connection.
//
// The Tasks API offers no device-code grant, so authorisation goes through a
// loopback redirect with PKCE: the browser is opened at an authorize URL, and
// the response arrives on a temporary 127.0.0.1 port. PKCE is required because
// that port is not exclusively ours.
pub mod credentials;
pub mod flow;
pub mod loopback;
pub mod session;
