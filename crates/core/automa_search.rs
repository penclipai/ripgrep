//! Byte-reader entry for the fixed browser ABI. The ordinary CLI keeps its
//! path/decompression/preprocessor entry. Matching and printing are shared.
use super::{SearchResult, SearchWorker};
use crate::haystack::Haystack;
use std::io;
use termcolor::WriteColor;

impl<W: WriteColor> SearchWorker<W> {
    pub(crate) fn search_bytes(
        &mut self,
        haystack: &Haystack,
        mut bytes: &[u8],
    ) -> io::Result<SearchResult> {
        let binary = if haystack.is_explicit() {
            self.config.binary_explicit.clone()
        } else {
            self.config.binary_implicit.clone()
        };
        self.searcher.set_binary_detection(binary);
        self.search_reader(haystack.path(), &mut bytes)
    }
}
