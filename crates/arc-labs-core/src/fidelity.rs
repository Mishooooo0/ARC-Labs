//! Byte fidelity: reading a vault file without losing how it was written.
//!
//! Constraint 1 says files are the source of truth, and constraint 2 says a
//! vault ARC-LABS touches must still open correctly in Obsidian. Together they
//! mean something sharper than "don't corrupt files": **opening a note, typing a
//! character, undoing it and saving must produce a byte-identical file.** That
//! is a Phase 1 acceptance gate, and it is the gate most likely to fail
//! silently, because every layer between disk and editor wants to normalise
//! something.
//!
//! The real vault this is built against proves the point: its markdown notes use
//! CRLF and its `.canvas` files use LF, in the same folder. CodeMirror 6
//! normalises documents to `\n` on load. `String::lines()` drops the distinction.
//! Any one of those, left alone, rewrites every line of a file the user did not
//! edit — and git would show the whole note as changed.
//!
//! So [`NoteText`] carries the normalised text *and* the evidence needed to undo
//! the normalisation exactly. Two rules make it airtight:
//!
//! 1. **Unchanged text re-encodes to the original bytes verbatim.** Not
//!    "equivalent bytes" — the same bytes. Mixed line endings, a stray lone
//!    `\r`, an unusual BOM position: none of it has to be modelled, because
//!    unmodified content never goes through the encoder's reconstruction path.
//! 2. **Changed text re-encodes with the file's own conventions** — its dominant
//!    line ending and its BOM — so an edit to line 40 of a 300-line CRLF file
//!    touches line 40 and nothing else.

use serde::{Deserialize, Serialize};

/// UTF-8 byte-order mark. Windows editors add it; Obsidian tolerates it; and if
/// we drop it, every line of the file appears to change.
const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
        }
    }
}

/// How a file was written, recorded at read time so it can be reproduced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFidelity {
    /// The ending used for the majority of line breaks. Used only when content
    /// actually changed; unchanged content is written back verbatim.
    dominant: LineEnding,
    /// True when the file mixes CRLF and LF. Kept for reporting — the encoder
    /// does not need it, because rule 1 covers the unchanged case and there is
    /// no sane way to preserve a mix through an arbitrary edit.
    mixed: bool,
    had_bom: bool,
    ends_with_newline: bool,
    /// Exact bytes as read. This is what makes rule 1 exact rather than
    /// approximate. Only open notes hold one, and notes are small.
    original: Vec<u8>,
}

impl FileFidelity {
    pub fn line_ending(&self) -> LineEnding {
        self.dominant
    }
    pub fn is_mixed(&self) -> bool {
        self.mixed
    }
    pub fn had_bom(&self) -> bool {
        self.had_bom
    }
    pub fn ends_with_newline(&self) -> bool {
        self.ends_with_newline
    }
}

/// A note's text, normalised to `\n`, plus the fidelity record needed to write
/// it back the way it was found.
#[derive(Debug, Clone)]
pub struct NoteText {
    text: String,
    fidelity: FileFidelity,
}

impl NoteText {
    /// Decode raw file bytes.
    ///
    /// Returns `None` for non-UTF-8 input rather than replacing invalid
    /// sequences: silently substituting U+FFFD would destroy the user's bytes,
    /// which is the one thing this module exists to prevent.
    pub fn decode(bytes: &[u8]) -> Option<NoteText> {
        let had_bom = bytes.starts_with(BOM);
        let body = if had_bom { &bytes[BOM.len()..] } else { bytes };
        let raw = std::str::from_utf8(body).ok()?;

        let crlf = raw.matches("\r\n").count();
        // Lone LF count = total LF minus those that are part of a CRLF.
        let lf = raw.matches('\n').count() - crlf;

        let text = raw.replace("\r\n", "\n");

        NoteText {
            fidelity: FileFidelity {
                // An empty or single-line file has no evidence either way. LF is
                // the safer default: it is what Obsidian writes for .canvas, what
                // git prefers, and it is what every platform reads correctly.
                dominant: if crlf > lf { LineEnding::Crlf } else { LineEnding::Lf },
                mixed: crlf > 0 && lf > 0,
                had_bom,
                ends_with_newline: text.ends_with('\n'),
                original: bytes.to_vec(),
            },
            text,
        }
        .into()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn fidelity(&self) -> &FileFidelity {
        &self.fidelity
    }

    /// Bytes to write for `new_text`.
    ///
    /// If `new_text` is what was read, this returns the original bytes exactly —
    /// see rule 1. Otherwise it reconstructs using the file's own conventions.
    pub fn encode(&self, new_text: &str) -> Vec<u8> {
        if new_text == self.text {
            return self.fidelity.original.clone();
        }

        let body = match self.fidelity.dominant {
            LineEnding::Lf => new_text.to_owned(),
            LineEnding::Crlf => new_text.replace('\n', "\r\n"),
        };

        let mut out = Vec::with_capacity(body.len() + BOM.len());
        if self.fidelity.had_bom {
            out.extend_from_slice(BOM);
        }
        out.extend_from_slice(body.as_bytes());
        out
    }

    /// Content hash of the normalised text.
    ///
    /// Deliberately hashes the *text*, not the bytes: a note is "the same note"
    /// whether it was saved with CRLF or LF, and the Phase 3 ledger should not
    /// record an edit because a line ending changed.
    pub fn content_hash(&self) -> String {
        format!("blake3:{}", blake3::hash(self.text.as_bytes()).to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Phase 1 acceptance gate, in miniature: read, no change, write.
    #[test]
    fn unchanged_content_reencodes_to_identical_bytes() {
        let cases: &[&[u8]] = &[
            b"# Title\r\nbody\r\n",                       // CRLF, as his notes are
            b"# Title\nbody\n",                           // LF, as his .canvas files are
            b"\xEF\xBB\xBF# Title\r\nbody\r\n",           // CRLF with a BOM
            b"# Title\r\nmixed\nendings\r\n",             // mixed, the nasty one
            b"no trailing newline",                       // no terminator
            b"",                                          // zero-byte note (his vault has two)
            b"\r\n\r\n\r\n",                              // only line breaks
            b"lone \r carriage return",                   // a bare CR, preserved as a char
        ];
        for raw in cases {
            let note = NoteText::decode(raw).expect("valid utf-8");
            let out = note.encode(note.text());
            assert_eq!(out, *raw, "round-trip changed bytes for {raw:?}");
        }
    }

    #[test]
    fn edited_content_keeps_the_files_own_conventions() {
        let note = NoteText::decode(b"\xEF\xBB\xBF# Title\r\nbody\r\n").unwrap();
        let edited = note.encode("# Title\nbody\nnew line\n");
        assert_eq!(edited, b"\xEF\xBB\xBF# Title\r\nbody\r\nnew line\r\n");

        let lf = NoteText::decode(b"# Title\nbody\n").unwrap();
        assert_eq!(lf.encode("# Title\nbody\nnew\n"), b"# Title\nbody\nnew\n");
    }

    #[test]
    fn detects_conventions() {
        let crlf = NoteText::decode(b"a\r\nb\r\n").unwrap();
        assert_eq!(crlf.fidelity().line_ending(), LineEnding::Crlf);
        assert!(!crlf.fidelity().is_mixed());
        assert!(crlf.fidelity().ends_with_newline());

        let mixed = NoteText::decode(b"a\r\nb\nc\r\n").unwrap();
        assert!(mixed.fidelity().is_mixed());
        // Two CRLF to one LF.
        assert_eq!(mixed.fidelity().line_ending(), LineEnding::Crlf);

        let bom = NoteText::decode(b"\xEF\xBB\xBFa\n").unwrap();
        assert!(bom.fidelity().had_bom());
        // The BOM is not part of the text the editor sees.
        assert_eq!(bom.text(), "a\n");

        // No evidence either way defaults to LF.
        assert_eq!(NoteText::decode(b"").unwrap().fidelity().line_ending(), LineEnding::Lf);
    }

    #[test]
    fn text_is_normalised_for_the_editor() {
        let note = NoteText::decode(b"# T\r\n\r\nbody\r\n").unwrap();
        assert_eq!(note.text(), "# T\n\nbody\n");
        assert!(!note.text().contains('\r'));
    }

    #[test]
    fn rejects_non_utf8_rather_than_corrupting_it() {
        // Latin-1 'é'. Lossy decoding would turn this into U+FFFD and the next
        // save would write the replacement character to the user's file.
        assert!(NoteText::decode(b"caf\xE9\n").is_none());
    }

    #[test]
    fn content_hash_ignores_line_endings_but_not_content() {
        let crlf = NoteText::decode(b"# T\r\nbody\r\n").unwrap();
        let lf = NoteText::decode(b"# T\nbody\n").unwrap();
        assert_eq!(crlf.content_hash(), lf.content_hash());

        let other = NoteText::decode(b"# T\nbody!\n").unwrap();
        assert_ne!(crlf.content_hash(), other.content_hash());
    }
}
