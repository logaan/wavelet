//! Byte-offset ⇆ LSP `Position` conversion.
//!
//! The Wavelet reader reports spans and errors as byte offsets into the source
//! (see `form::Arena::spans` and `lexer::ReadError::at`), but LSP positions are
//! `(line, character)` pairs where `character` counts **UTF-16 code units**
//! (the default position encoding). This module bridges the two.

use lsp_types::Position;

/// Precomputed byte offsets of each line start, including a leading `0`.
pub struct LineIndex {
    line_starts: Vec<usize>,
    len: usize,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self { line_starts, len: text.len() }
    }

    /// Map a byte offset to an LSP position. The offset is floored to a UTF-8
    /// char boundary first, so an error reported mid-codepoint still resolves.
    pub fn position(&self, text: &str, offset: usize) -> Position {
        let offset = floor_char_boundary(text, offset.min(self.len));
        let line = match self.line_starts.binary_search(&offset) {
            Ok(l) => l,
            Err(l) => l - 1,
        };
        let line_start = self.line_starts[line];
        let character = text[line_start..offset].encode_utf16().count();
        Position::new(line as u32, character as u32)
    }

    /// Map an LSP position back to a byte offset, clamping out-of-range input.
    pub fn offset(&self, text: &str, pos: Position) -> usize {
        let line = pos.line as usize;
        if line >= self.line_starts.len() {
            return self.len;
        }
        let line_start = self.line_starts[line];
        let mut utf16 = 0usize;
        let mut offset = line_start;
        for ch in text[line_start..].chars() {
            if ch == '\n' || utf16 >= pos.character as usize {
                break;
            }
            utf16 += ch.len_utf16();
            offset += ch.len_utf8();
        }
        offset
    }
}

fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    if offset >= text.len() {
        return text.len();
    }
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}
