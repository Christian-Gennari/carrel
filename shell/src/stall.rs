//! Opt-in log of GTK main-loop stalls, for measuring typing on real hardware
//! and on the QEMU image (`tests/boot/run.py`, `typing-slow-disk`).
//!
//! Off unless `<writing>/.carrel/stall-log` exists or `CARREL_STALL_LOG=1`.
//! A timer asks to run every [`TICK`]; when it runs more than [`LIMIT`] late,
//! something held the main loop for that long and a line goes to the journal.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;

/// The longest a key may wait behind other main-loop work.
pub const LIMIT: Duration = Duration::from_millis(50);
const TICK: Duration = Duration::from_millis(5);

thread_local! {
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    static TYPED: Cell<bool> = const { Cell::new(false) };
}

/// Start the log when it was asked for.
pub fn install(root: &Path) {
    let wanted = std::env::var("CARREL_STALL_LOG").as_deref() == Ok("1")
        || root.join(".carrel").join("stall-log").exists();
    if !wanted {
        return;
    }
    eprintln!("carrel-stall: watching");
    WATCHING.set(true);
    watch(|late| eprintln!("carrel-stall: {} ms", late.as_millis()));
}

/// Note the first edit, so start-up work can be told apart from typing.
pub fn edited() {
    if WATCHING.get() && !TYPED.replace(true) {
        eprintln!("carrel-stall: typing");
    }
}

/// Call `report` with the lateness of every tick that is over [`LIMIT`], for
/// as long as the returned handle is not set.
pub fn watch(report: impl Fn(Duration) + 'static) -> Rc<Cell<bool>> {
    let stop = Rc::new(Cell::new(false));
    let stopped = Rc::clone(&stop);
    let last = Cell::new(Instant::now());
    glib::timeout_add_local(TICK, move || {
        let now = Instant::now();
        let late = now.duration_since(last.replace(now)).saturating_sub(TICK);
        if late > LIMIT {
            report(late);
        }
        if stopped.get() {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
    stop
}
