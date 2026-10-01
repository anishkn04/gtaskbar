use std::cell::RefCell;

// The OAuth session, held in memory for now.
//
// The full loopback + PKCE flow lands in the next step. Until then this
// provides the shape the rest of the app codes against: a single place that
// owns "are we connected, and with what token", so the sync engine, the
// scheduler and the UI never have to know how authentication works.
thread_local! {
    static TOKEN: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn access_token() -> Option<String> {
    TOKEN.with(|slot| slot.borrow().clone())
}

pub fn is_connected() -> bool {
    TOKEN.with(|slot| slot.borrow().is_some())
}

pub fn set_access_token(token: String) {
    TOKEN.with(|slot| *slot.borrow_mut() = Some(token));
}

pub fn clear() {
    TOKEN.with(|slot| *slot.borrow_mut() = None);
}

#[cfg(test)]
mod tests {
    use super::*;

    // The session is thread-local, so these tests must not share state. Each
    // one clears first and asserts on a known-empty session.

    #[test]
    fn an_empty_session_is_not_connected() {
        clear();
        assert!(!is_connected());
        assert!(access_token().is_none());
    }

    #[test]
    fn setting_a_token_connects_the_session() {
        clear();
        set_access_token("token".into());
        assert!(is_connected());
        assert_eq!(access_token().as_deref(), Some("token"));
        clear();
    }

    #[test]
    fn clearing_disconnects() {
        set_access_token("token".into());
        clear();
        assert!(!is_connected());
    }
}
