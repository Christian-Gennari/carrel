//! The one hand-off between two screens: Sync asks Wi-Fi for a network, and
//! Wi-Fi brings the writer back to Sync when the connection is up.
//!
//! Rule 2 keeps the radio off except on the Wi-Fi screen, so Sync cannot look
//! at the network itself. When a sync fails because there is no network, Sync
//! opens Wi-Fi and remembers that it did; Wi-Fi notices the request and hands
//! the writer back. Nothing here touches the daemon or GTK.

use std::cell::RefCell;
use std::rc::Rc;

use crate::daemon::Backend;

/// What Sync was trying to do when it sent the writer to Wi-Fi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoffAction {
    Sync,
    Pair { backend: Backend, url: String },
}

/// A one-slot request that travels Sync → Wi-Fi → Sync.
#[derive(Clone, Default)]
pub struct WifiHandoff {
    inner: Rc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Sync is waiting for a network.
    asked: RefCell<Option<HandoffAction>>,
    /// Wi-Fi connected while the request was open.
    connected: RefCell<Option<HandoffAction>>,
}

impl WifiHandoff {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sync: the next step is the Wi-Fi screen, to retry a sync afterwards.
    pub fn ask(&self) {
        *self.inner.asked.borrow_mut() = Some(HandoffAction::Sync);
        *self.inner.connected.borrow_mut() = None;
    }

    /// Sync: the next step is the Wi-Fi screen, to retry pairing afterwards.
    pub fn ask_pair(&self, backend: Backend, url: String) {
        *self.inner.asked.borrow_mut() = Some(HandoffAction::Pair { backend, url });
        *self.inner.connected.borrow_mut() = None;
    }

    /// Wi-Fi: was this screen opened to fix a sync or pairing error?
    pub fn is_asked(&self) -> bool {
        self.inner.asked.borrow().is_some()
    }

    /// Wi-Fi: what Sync will continue once a network connects.
    pub fn asked_action(&self) -> Option<HandoffAction> {
        self.inner.asked.borrow().clone()
    }

    /// Wi-Fi: the connection is up. True when Sync is waiting and should be
    /// shown again; the answer is consumed so a second connect does not jump.
    pub fn connect(&self) -> bool {
        let asked = self.inner.asked.borrow_mut().take();
        let was_asked = asked.is_some();
        if let Some(action) = asked {
            *self.inner.connected.borrow_mut() = Some(action);
        }
        was_asked
    }

    /// Wi-Fi: the writer left without connecting. Nothing is pending any more.
    pub fn abandon(&self) {
        *self.inner.asked.borrow_mut() = None;
        *self.inner.connected.borrow_mut() = None;
    }

    /// Sync: a connection happened on its behalf since the screen was shown.
    pub fn resume(&self) -> bool {
        self.resume_action().is_some()
    }

    /// Sync: consume and return what to continue after Wi-Fi connected.
    pub fn resume_action(&self) -> Option<HandoffAction> {
        self.inner.connected.borrow_mut().take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_answered_once() {
        let handoff = WifiHandoff::new();
        assert!(!handoff.is_asked());
        assert_eq!(handoff.asked_action(), None);
        assert!(!handoff.connect(), "nothing was asked");

        handoff.ask();
        assert!(handoff.is_asked());
        assert_eq!(handoff.asked_action(), Some(HandoffAction::Sync));
        assert!(handoff.connect(), "Wi-Fi was opened for a sync");
        assert!(!handoff.is_asked());
        assert_eq!(handoff.resume_action(), Some(HandoffAction::Sync));
        assert!(!handoff.resume(), "the answer is consumed");
        assert!(!handoff.connect(), "a later connect is not ours");
    }

    #[test]
    fn leaving_without_connecting_cancels_the_request() {
        let handoff = WifiHandoff::new();
        handoff.ask();
        handoff.abandon();
        assert!(!handoff.is_asked());
        assert!(!handoff.connect());
        assert!(!handoff.resume());
    }

    #[test]
    fn a_pairing_request_preserves_backend_and_url_and_is_consumed_once() {
        let handoff = WifiHandoff::new();
        handoff.ask_pair(Backend::SelfHosted, "https://studio.local:8443".into());
        assert!(handoff.is_asked());
        assert_eq!(
            handoff.asked_action(),
            Some(HandoffAction::Pair {
                backend: Backend::SelfHosted,
                url: "https://studio.local:8443".into(),
            })
        );
        assert!(handoff.connect());
        assert!(!handoff.is_asked());
        assert_eq!(
            handoff.resume_action(),
            Some(HandoffAction::Pair {
                backend: Backend::SelfHosted,
                url: "https://studio.local:8443".into(),
            })
        );
        assert_eq!(handoff.resume_action(), None);
    }

    #[test]
    fn abandoning_a_pairing_request_clears_it() {
        let handoff = WifiHandoff::new();
        handoff.ask_pair(Backend::Nostos, String::new());
        handoff.abandon();
        assert!(!handoff.is_asked());
        assert_eq!(handoff.asked_action(), None);
        assert!(!handoff.connect());
        assert_eq!(handoff.resume_action(), None);
    }
}
