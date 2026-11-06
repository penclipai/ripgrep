//! Deferred walker diagnostics retain their position relative to candidates.
//! Quiet searches never publish diagnostics beyond the first matched file.
use super::Output;
use std::{cell::RefCell, fmt, io::Write, rc::Rc};

#[derive(Default)]
struct Capture {
    active: bool,
    position: usize,
    used: usize,
    limit: usize,
    exhausted: bool,
    messages: Vec<(usize, String)>,
}
thread_local! { static CAPTURE: RefCell<Capture> = RefCell::new(Capture::default()); }

pub(super) fn start(limit: usize) {
    CAPTURE.with(|slot| {
        *slot.borrow_mut() =
            Capture { active: true, limit, ..Capture::default() }
    });
}
pub(super) fn position(index: usize) {
    CAPTURE.with(|slot| slot.borrow_mut().position = index);
}
pub(super) fn finish() -> (Vec<(usize, String)>, bool) {
    CAPTURE.with(|slot| {
        let mut capture = slot.borrow_mut();
        capture.active = false;
        (std::mem::take(&mut capture.messages), capture.exhausted)
    })
}

pub(crate) fn capture_message(arguments: fmt::Arguments<'_>) -> bool {
    CAPTURE.with(|slot| {
        let mut capture = slot.borrow_mut();
        if !capture.active {
            return false;
        }
        if capture.exhausted {
            return true;
        }
        let mut output = Output {
            bytes: Rc::new(RefCell::new(vec![])),
            limit: capture.limit.saturating_sub(capture.used),
        };
        if output.write_fmt(format_args!("rg: {arguments}\n")).is_err() {
            capture.exhausted = true;
        }
        let bytes = Rc::try_unwrap(output.bytes).unwrap().into_inner();
        capture.used += bytes.len();
        let index = capture.position;
        capture.messages.push((
            index,
            String::from_utf8(bytes).expect("Rust formatting is UTF-8"),
        ));
        true
    })
}
