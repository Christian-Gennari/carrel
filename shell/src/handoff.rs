//! The one hand-off between two screens: Sync asks Wi-Fi for a network, and
//! Wi-Fi brings the writer back to Sync when the connection is up.
//!
//! Rule 2 keeps the radio off except on the Wi-Fi screen, so Sync cannot look
//! at the network itself. When a sync fails because there is no network, Sync
//! opens Wi-Fi and remembers that it did; Wi-Fi notices the request and hands
//! the writer back. Nothing here touches the daemon or GTK.

use std::cell::Cell;
use std::rc::Rc;

/// A one-slot request that travels Sync → Wi-Fi → Sync.
#[derive(Clone, Default)]
pub struct WifiHandoff {
    inner: Rc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Sync is waiting for a network.
    asked: Cell<bool>,
    /// Wi-Fi connected while the request was open.
    connected: Cell<bool>,
}

impl WifiHandoff {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sync: the next step is the Wi-Fi screen.
    pub fn ask(&self) {
        self.inner.asked.set(true);
        self.inner.connected.set(false);
    }

    /// Wi-Fi: was this screen opened to fix a sync?
    pub fn is_asked(&self) -> bool {
        self.inner.asked.get()
    }

    /// Wi-Fi: the connection is up. True when Sync is waiting and should be
    /// shown again; the answer is consumed so a second connect does not jump.
    pub fn connect(&self) -> bool {
        let asked = self.inner.asked.replace(false);
        if asked {
            self.inner.connected.set(true);
        }
        asked
    }

    /// Wi-Fi: the writer left without connecting. Nothing is pending any more.
    pub fn abandon(&self) {
        self.inner.asked.set(false);
    }

    /// Sync: a connection happened on its behalf since the screen was shown.
    pub fn resume(&self) -> bool {
        self.inner.connected.replace(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_answered_once() {
        let handoff = WifiHandoff::new();
        assert!(!handoff.is_asked());
        assert!(!handoff.connect(), "nothing was asked");

        handoff.ask();
        assert!(handoff.is_asked());
        assert!(handoff.connect(), "Wi-Fi was opened for a sync");
        assert!(!handoff.is_asked());
        assert!(handoff.resume(), "Sync is told to continue");
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
}
