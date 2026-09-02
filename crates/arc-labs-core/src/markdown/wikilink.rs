//! Scanner for Obsidian's non-CommonMark syntax: `[[wikilinks]]`, `![[embeds]]`
//! and `#tags`.
//!
//! This operates on the *text* of an already-parsed markdown node, never on the
//! raw document. That distinction is the whole point. A regex over the raw
//! document is the classic bug in this category: it linkifies `[[not a link]]`
//! inside a fenced code block, turns the `#` of `https://x/#frag` into a tag,
//! and mangles the contents of an existing `[label](url)`. Running over parsed
//! text nodes makes all three impossible by construction rather than by adding
//! more exceptions to a pattern.

/// A parsed `[[target#anchor|alias]]` or `![[…]]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiLink {
    /// The note being linked to, without extension. Empty for `[[#heading]]`,
    /// which points inside the current note.
    pub target: String,
    /// A heading (`#Heading`) or block id (`#^abc123`) within the target.
    pub anchor: Option<String>,
    /// Display text from `|alias`.
    pub alias: Option<String>,
    /// True for `![[…]]`, which transcludes rather than links.
    pub embed: bool,
}

impl WikiLink {
    /// What the reader sees: the alias if given, else the target, else the
    /// anchor for a same-note link.
    pub fn display(&self) -> &str {
        if let Some(a) = &self.alias {
            return a;
        }
        if self.target.is_empty() {
            return self.anchor.as_deref().unwrap_or("");
        }
        &self.target
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Link(WikiLink),
    Tag(String),
}

/// An [`Item`] and the byte range it occupies in the scanned text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub start: usize,
    pub end: usize,
    pub item: Item,
}

/// Find every wikilink, embed and tag in one text node.
pub fn scan(text: &str) -> Vec<Found> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        // `![[` — an embed. Checked before `[[` so the `!` is consumed.
        if bytes[i] == b'!' && bytes[i + 1..].starts_with(b"[[") {
            if let Some((link, end)) = parse_link(text, i + 3, true) {
                out.push(Found {
                    start: i,
                    end,
                    item: Item::Link(link),
                });
                i = end;
                continue;
            }
        }
        if bytes[i..].starts_with(b"[[") {
            if let Some((link, end)) = parse_link(text, i + 2, false) {
                out.push(Found {
                    start: i,
                    end,
                    item: Item::Link(link),
                });
                i = end;
                continue;
            }
        }
        if bytes[i] == b'#' && tag_may_start_at(text, i) {
            if let Some((name, end)) = parse_tag(text, i + 1) {
                out.push(Found {
                    start: i,
                    end,
                    item: Item::Tag(name),
                });
                i = end;
                continue;
            }
        }
        // Advance one whole character; the guards above only match ASCII, so a
        // multi-byte character can never be mistaken for a delimiter.
        i += utf8_len(bytes[i]);
    }
    out
}

fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// Parse from just after the opening `[[`. Returns the link and the index just
/// past the closing `]]`.
fn parse_link(text: &str, from: usize, embed: bool) -> Option<(WikiLink, usize)> {
    let rest = &text[from..];
    let close = rest.find("]]")?;
    let inner = &rest[..close];

    // A wikilink never spans a line and never nests a bracket. Bailing here is
    // what stops `[[a` on one line and `b]]` on another from linking.
    if inner.contains('\n') || inner.contains('[') || inner.contains(']') {
        return None;
    }

    let (before_alias, alias) = match inner.split_once('|') {
        Some((l, r)) => (l, Some(r.trim().to_string())),
        None => (inner, None),
    };
    let (target, anchor) = match before_alias.split_once('#') {
        Some((l, r)) => (l.trim(), Some(r.trim().to_string())),
        None => (before_alias.trim(), None),
    };

    // `[[]]` and `[[|alias]]` name nothing.
    if target.is_empty() && anchor.is_none() {
        return None;
    }

    Some((
        WikiLink {
            target: target.to_string(),
            anchor: anchor.filter(|a| !a.is_empty()),
            alias: alias.filter(|a| !a.is_empty()),
            embed,
        },
        from + close + 2,
    ))
}

/// A `#` only opens a tag at the start of the text or after a character that
/// cannot be part of a word. This is what keeps `C#`, `issue#42` and the
/// fragment in a bare URL from becoming tags.
fn tag_may_start_at(text: &str, at: usize) -> bool {
    match text[..at].chars().next_back() {
        None => true,
        Some(c) => !(c.is_alphanumeric() || c == '_' || c == '-' || c == '/' || c == '#'),
    }
}

/// Parse from just after the `#`.
fn parse_tag(text: &str, from: usize) -> Option<(String, usize)> {
    let rest = &text[from..];
    let mut end = 0usize;
    for c in rest.chars() {
        if c.is_alphanumeric() || matches!(c, '_' | '-' | '/') {
            end += c.len_utf8();
        } else {
            break;
        }
    }
    let name = &rest[..end];

    if name.is_empty() {
        return None;
    }
    // Obsidian's rule, and a good one: an all-numeric tag is almost always a
    // heading, an issue number or a colour, not a tag.
    if !name.chars().any(|c| !c.is_ascii_digit()) {
        return None;
    }
    // `#tag/` and `#/x` are typos, not nested tags.
    if name.starts_with('/') || name.ends_with('/') {
        return None;
    }
    Some((name.to_string(), from + end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(text: &str) -> Vec<WikiLink> {
        scan(text)
            .into_iter()
            .filter_map(|f| match f.item {
                Item::Link(l) => Some(l),
                _ => None,
            })
            .collect()
    }

    fn tags(text: &str) -> Vec<String> {
        scan(text)
            .into_iter()
            .filter_map(|f| match f.item {
                Item::Tag(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn parses_every_wikilink_shape() {
        let l = &links("see [[Note]]")[0];
        assert_eq!(l.target, "Note");
        assert!(l.anchor.is_none() && l.alias.is_none() && !l.embed);

        let l = &links("[[Note|the alias]]")[0];
        assert_eq!((l.target.as_str(), l.display()), ("Note", "the alias"));

        let l = &links("[[Note#Heading]]")[0];
        assert_eq!(l.anchor.as_deref(), Some("Heading"));

        let l = &links("[[Note#^block-id|Alias]]")[0];
        assert_eq!(l.anchor.as_deref(), Some("^block-id"));
        assert_eq!(l.alias.as_deref(), Some("Alias"));

        let l = &links("![[Diagram.png]]")[0];
        assert!(l.embed);

        // A link into the current note.
        let l = &links("[[#Heading]]")[0];
        assert!(l.target.is_empty());
        assert_eq!(l.display(), "Heading");

        // Folders and spaces are ordinary.
        let l = &links("[[Daily/2026-09-02|today]]")[0];
        assert_eq!(l.target, "Daily/2026-09-02");
    }

    #[test]
    fn rejects_malformed_links() {
        for bad in [
            "[[unclosed",
            "[[]]",
            "[[|alias]]",
            "[[a\nb]]",
            "[[a[b]]",
            "[single]",
        ] {
            assert!(links(bad).is_empty(), "should not parse {bad:?}");
        }
    }

    #[test]
    fn finds_several_in_one_line() {
        let ls = links("[[A]] then ![[B]] then [[C|c]]");
        assert_eq!(ls.len(), 3);
        assert!(ls[1].embed);
        assert_eq!(ls[2].display(), "c");
    }

    #[test]
    fn parses_tags() {
        assert_eq!(tags("a #rust and #arc-labs here"), ["rust", "arc-labs"]);
        assert_eq!(tags("#nested/tag/deep"), ["nested/tag/deep"]);
        assert_eq!(tags("#tag,"), ["tag"]);
        assert_eq!(tags("(#tag)"), ["tag"]);
        assert_eq!(tags("#t2"), ["t2"]);
    }

    #[test]
    fn rejects_hashes_that_are_not_tags() {
        // Each of these is a real false positive a regex would produce.
        for bad in [
            "C# is a language", // trailing hash on a word
            "issue#42",         // hash inside a word
            "#42",              // all-numeric
            "# ",               // bare hash
            "##double",         // second hash follows a hash
            "#/leading-slash",  // typo
            "#trailing/",       // typo
            "a#b",              // hash inside a word
        ] {
            assert!(tags(bad).is_empty(), "should not find a tag in {bad:?}");
        }
    }

    #[test]
    fn byte_ranges_are_exact_so_replacement_cannot_drift() {
        let text = "x [[A|a]] y #tag z";
        let found = scan(text);
        assert_eq!(&text[found[0].start..found[0].end], "[[A|a]]");
        assert_eq!(&text[found[1].start..found[1].end], "#tag");
    }

    #[test]
    fn ranges_stay_valid_across_multibyte_characters() {
        // If the scanner advanced by bytes rather than characters, these ranges
        // would land mid-character and slicing would panic.
        let text = "café ☕ [[Note]] naïve #tag 日本語";
        let found = scan(text);
        assert_eq!(&text[found[0].start..found[0].end], "[[Note]]");
        assert_eq!(&text[found[1].start..found[1].end], "#tag");
    }

    #[test]
    fn embed_marker_is_not_swallowed_by_the_plain_link_branch() {
        let found = scan("![[X]]");
        assert_eq!(
            found[0].start, 0,
            "the `!` must be part of the matched range"
        );
        assert_eq!(found[0].end, 6);
    }
}
