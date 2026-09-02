//! YAML frontmatter: split, never parsed and re-serialised.
//!
//! Constraint 1 makes the file authoritative, which means ARC-LABS must be able
//! to hand a note back to disk unchanged. Round-tripping frontmatter through a
//! YAML parser cannot do that: key order is lost, quoting style is normalised,
//! comments are dropped, and `2026-09-02` comes back as a date rather than the
//! string the user typed. Every one of those is a diff the user did not ask for.
//!
//! So the frontmatter is a byte range, held as a string and written back
//! verbatim. A *parsed view* of it can be layered on later for querying — but
//! the parsed view never becomes the thing that gets written.

/// The result of splitting a document into frontmatter and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split<'a> {
    /// The YAML between the delimiters, exactly as written, without the
    /// delimiter lines themselves. `None` when the document has no frontmatter.
    pub frontmatter: Option<&'a str>,
    /// Everything after the closing delimiter.
    pub body: &'a str,
}

/// Split a document that has already been normalised to `\n` line endings.
///
/// Frontmatter is recognised only when the document *opens* with a `---` line.
/// A `---` anywhere else is a thematic break or a setext heading underline and
/// is left for the markdown parser, which is what Obsidian does too.
pub fn split(text: &str) -> Split<'_> {
    let none = Split {
        frontmatter: None,
        body: text,
    };

    let Some(rest) = text.strip_prefix("---") else {
        return none;
    };
    // The opening delimiter must be a line of its own: `---` then a newline.
    // `----` and `--- title` are not frontmatter.
    let Some(rest) = rest.strip_prefix('\n') else {
        return none;
    };

    let open_len = 4; // "---\n"
    let mut offset = 0usize;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        // YAML permits `...` as a document terminator; Obsidian writes `---`.
        if trimmed == "---" || trimmed == "..." {
            let yaml = &rest[..offset];
            let body_start = open_len + offset + line.len();
            return Split {
                frontmatter: Some(yaml),
                body: &text[body_start..],
            };
        }
        offset += line.len();
    }

    // Opened but never closed. Treat the whole document as body: an unterminated
    // block is far more likely to be a user typing `---` at the top of a note
    // than a note whose entire content is frontmatter.
    none
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_ordinary_frontmatter() {
        let doc = "---\ntitle: Note\ntags: [a, b]\n---\n# Heading\n";
        let s = split(doc);
        assert_eq!(s.frontmatter, Some("title: Note\ntags: [a, b]\n"));
        assert_eq!(s.body, "# Heading\n");
    }

    #[test]
    fn reassembles_to_the_original_byte_for_byte() {
        // This is the property that matters: whatever we split, we can put back.
        let docs = [
            "---\ntitle: A\n---\nbody\n",
            "---\n---\nempty frontmatter\n",
            "---\nkey: 'single quoted'   # trailing comment\nz: 1\na: 2\n---\nbody",
            "no frontmatter at all\n",
            "---\nunterminated\nbody\n",
            "",
        ];
        for doc in docs {
            let s = split(doc);
            let rebuilt = match s.frontmatter {
                Some(fm) => format!("---\n{fm}---\n{}", s.body),
                None => s.body.to_string(),
            };
            assert_eq!(rebuilt, doc, "failed to reassemble {doc:?}");
        }
    }

    #[test]
    fn preserves_key_order_and_formatting_because_it_never_parses() {
        // A YAML round-trip would reorder these keys and requote the value.
        let doc = "---\nzeta: 1\nalpha: 'quoted'\n# a comment\nmiddle: 2026-09-02\n---\nx";
        let s = split(doc);
        assert_eq!(
            s.frontmatter,
            Some("zeta: 1\nalpha: 'quoted'\n# a comment\nmiddle: 2026-09-02\n")
        );
    }

    #[test]
    fn does_not_treat_a_thematic_break_as_frontmatter() {
        for doc in [
            "----\nnot frontmatter\n",
            "--- title\nx\n",
            "text\n---\nmore\n",
            "---",
        ] {
            assert_eq!(
                split(doc).frontmatter,
                None,
                "misread {doc:?} as frontmatter"
            );
        }
    }

    #[test]
    fn accepts_the_yaml_document_terminator() {
        let s = split("---\na: 1\n...\nbody\n");
        assert_eq!(s.frontmatter, Some("a: 1\n"));
        assert_eq!(s.body, "body\n");
    }
}
