//! Fixed, invocation-local browser ABI. Plan once over metadata/ignore files,
//! then supply one selected file's bytes at a time. No file bodies are retained.
//! CLI parsing, ignore walking, binary detection, matching and printing remain
//! ripgrep's implementations. `_start` remains the ordinary command entry.
use std::{cell::RefCell, io, io::Write, rc::Rc};
use termcolor::{ColorSpec, WriteColor};
#[path = "automa_messages.rs"]
mod capture;
pub(crate) use capture::capture_message;

use crate::{
    flags::{HiArgs, Mode, ParseResult},
    haystack::Haystack,
    search::SearchWorker,
};

#[derive(Clone, Debug)]
struct Output {
    bytes: Rc<RefCell<Vec<u8>>>,
    limit: usize,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut output = self.bytes.borrow_mut();
        if bytes.len() > self.limit.saturating_sub(output.len()) {
            return Err(io::Error::other("Automa rg output limit exceeded"));
        }
        output.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl WriteColor for Output {
    fn supports_color(&self) -> bool {
        false
    }
    fn set_color(&mut self, _: &ColorSpec) -> io::Result<()> {
        Ok(())
    }
    fn reset(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Session {
    args: HiArgs,
    haystacks: Vec<Haystack>,
    paths: Vec<u8>,
    diagnostics: Vec<u8>,
    worker: Option<SearchWorker<Output>>,
    output: Output,
    matched: bool,
    resource_failure: bool,
    finished: Option<i32>,
}

impl Session {
    fn new(args: HiArgs, limit: usize) -> anyhow::Result<Session> {
        let output = Output { bytes: Rc::new(RefCell::new(vec![])), limit };
        let worker = match args.mode() {
            Mode::Search(mode) => Some(args.search_worker(
                args.matcher()?,
                args.searcher()?,
                args.printer(mode, output.clone()),
            )?),
            Mode::Files => None,
            _ => anyhow::bail!("Unsupported Automa rg library mode"),
        };
        let builder = args.haystack_builder();
        let walker = args.walk_builder()?.build();
        let mut haystacks = vec![];
        capture::start(limit);
        for entry in walker {
            capture::position(haystacks.len());
            if let Some(haystack) = builder.build_from_result(entry) {
                haystacks.push(haystack);
            }
        }
        let (diagnostics, resource_failure) = capture::finish();
        let diagnostics = serde_json::to_vec(&diagnostics)?;
        let haystacks: Vec<_> = args.sort(haystacks.into_iter()).collect();
        let mut paths = vec![];
        for haystack in &haystacks {
            paths.extend_from_slice(
                haystack
                    .path()
                    .to_str()
                    .ok_or_else(|| {
                        anyhow::anyhow!("Automa rg requires UTF-8 paths")
                    })?
                    .as_bytes(),
            );
            paths.push(0);
        }
        Ok(Session {
            args,
            haystacks,
            paths,
            diagnostics,
            worker,
            output,
            matched: false,
            resource_failure,
            finished: None,
        })
    }

    fn search(&mut self, id: usize, bytes: &[u8]) -> i32 {
        let Some(haystack) = self.haystacks.get(id) else { return 2 };
        let Some(worker) = self.worker.as_mut() else { return 2 };
        match worker.search_bytes(haystack, bytes) {
            Ok(result) => {
                self.matched |= result.has_match();
                if self.matched && self.args.quit_after_match() {
                    1
                } else {
                    0
                }
            }
            Err(error) => {
                self.resource_failure = true;
                err_message!("{}: {}", haystack.path().display(), error);
                126
            }
        }
    }

    fn list(&mut self) -> i32 {
        let mut printer =
            self.args.path_printer_builder().build(self.output.clone());
        for haystack in &self.haystacks {
            self.matched = true;
            if self.args.quit_after_match() {
                break;
            }
            if let Err(error) = printer.write(haystack.path()) {
                self.resource_failure = true;
                err_message!("{error}");
                return 126;
            }
        }
        self.finish()
    }

    fn finish(&mut self) -> i32 {
        if let Some(status) = self.finished {
            return status;
        }
        if !self.resource_failure
            && matches!(self.args.mode(), Mode::Search(_))
            && self.args.matches_possible()
            && self.args.has_implicit_path()
            && self.haystacks.is_empty()
        {
            crate::eprint_nothing_searched();
        }
        let status = if self.resource_failure {
            126
        } else if self.matched
            && (self.args.quiet() || !crate::messages::errored())
        {
            0
        } else if crate::messages::errored() {
            2
        } else {
            1
        };
        self.finished = Some(status);
        status
    }

    fn list_one(&mut self, id: usize) -> i32 {
        let Some(haystack) = self.haystacks.get(id) else { return 2 };
        self.matched = true;
        if self.args.quit_after_match() {
            return 1;
        }
        let mut printer =
            self.args.path_printer_builder().build(self.output.clone());
        if let Err(error) = printer.write(haystack.path()) {
            self.resource_failure = true;
            err_message!("{error}");
            return 126;
        }
        0
    }
}

thread_local! { static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) }; }

#[cfg(target_arch = "wasm32")]
mod exports {
    use super::*;
    #[unsafe(no_mangle)]
    pub extern "C" fn automa_rg_abi() -> u32 {
        1
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn automa_rg_init(limit: u32) -> i32 {
        let args = match crate::flags::parse() {
            ParseResult::Ok(args) => args,
            ParseResult::Err(error) => {
                err_message!("{error:#}");
                return 2;
            }
            ParseResult::Special(_) => return 2,
        };
        match Session::new(args, limit as usize) {
            Ok(session) => {
                let result = if session.resource_failure { 126 } else { 0 };
                SESSION.with(|slot| *slot.borrow_mut() = Some(session));
                result
            }
            Err(error) => {
                err_message!("{error:#}");
                2
            }
        }
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn automa_rg_alloc(length: u32) -> *mut u8 {
        Box::into_raw(vec![0; length as usize].into_boxed_slice()) as *mut u8
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn automa_rg_free(pointer: *mut u8, length: u32) {
        unsafe {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                pointer,
                length as usize,
            )));
        }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn automa_rg_search(
        id: u32,
        pointer: *const u8,
        length: u32,
    ) -> i32 {
        let bytes =
            unsafe { std::slice::from_raw_parts(pointer, length as usize) };
        SESSION.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .map_or(2, |session| session.search(id as usize, bytes))
        })
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn automa_rg_paths_ptr() -> *const u8 {
        SESSION.with(|slot| {
            slot.borrow()
                .as_ref()
                .map_or(std::ptr::null(), |s| s.paths.as_ptr())
        })
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn automa_rg_diagnostics_ptr() -> *const u8 {
        SESSION.with(|slot| {
            slot.borrow()
                .as_ref()
                .map_or(std::ptr::null(), |s| s.diagnostics.as_ptr())
        })
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn automa_rg_diagnostics_len() -> u32 {
        SESSION.with(|slot| {
            slot.borrow().as_ref().map_or(0, |s| s.diagnostics.len() as u32)
        })
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn automa_rg_list_one(id: u32) -> i32 {
        SESSION.with(|slot| {
            slot.borrow_mut().as_mut().map_or(2, |s| s.list_one(id as usize))
        })
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn automa_rg_paths_len() -> u32 {
        SESSION.with(|slot| {
            slot.borrow().as_ref().map_or(0, |s| s.paths.len() as u32)
        })
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn automa_rg_output_ptr() -> *const u8 {
        SESSION.with(|slot| {
            slot.borrow()
                .as_ref()
                .map_or(std::ptr::null(), |s| s.output.bytes.borrow().as_ptr())
        })
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn automa_rg_output_len() -> u32 {
        SESSION.with(|slot| {
            slot.borrow()
                .as_ref()
                .map_or(0, |s| s.output.bytes.borrow().len() as u32)
        })
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn automa_rg_clear_output() {
        SESSION.with(|slot| {
            if let Some(s) = slot.borrow_mut().as_mut() {
                s.output.bytes.borrow_mut().clear();
            }
        });
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn automa_rg_list() -> i32 {
        SESSION.with(|slot| slot.borrow_mut().as_mut().map_or(2, |s| s.list()))
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn automa_rg_finish() -> i32 {
        SESSION
            .with(|slot| slot.borrow_mut().as_mut().map_or(2, |s| s.finish()))
    }
}
