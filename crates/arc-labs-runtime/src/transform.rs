//! `transform` nodes: small, deterministic operations on text.
//!
//! # Deterministic is the whole point
//!
//! The Phase 5 gate says **the same graph run twice produces identical
//! `transform` output**. Everything here is therefore a pure function of its
//! input: no clock, no randomness, no iteration over a hash map, no locale.
//! A transform that sorted by a `HashSet`'s order would pass a hundred runs and
//! fail the hundred-and-first.
//!
//! It is also what makes a pipeline debuggable. When a run produces something
//! surprising, the transforms can be reasoned about by reading them, and only
//! the model is a source of variance.

use std::collections::BTreeSet;

/// The operations a `transform` node can perform.
///
/// A small, closed set on purpose. The spec calls them "small deterministic
/// operations", and the moment this becomes an expression language it becomes a
/// thing to maintain, sandbox and version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transform {
    /// The YAML frontmatter of each input, concatenated.
    ExtractFrontmatter,
    /// Split markdown into sections at headings of the given level or above.
    SplitByHeading { level: u8 },
    /// Remove duplicate lines, keeping the first occurrence and the order.
    Dedupe,
    /// Turn lines into a markdown bullet list.
    ToList,
    /// Sort lines. Case-insensitively, so it is stable regardless of locale.
    Sort,
    /// Keep the first `n` lines.
    Take { n: usize },
    /// Keep lines containing a substring, case-insensitively.
    Filter { contains: String },
    /// Concatenate inputs with a separator. The default when nothing is named.
    Join { separator: String },
    /// Strip markdown headings, leaving prose.
    StripHeadings,
    /// Collapse runs of blank lines to one.
    Squeeze,
}

impl Transform {
    /// Parse the node's text.
    ///
    /// The first word is the operation; the rest are arguments. Deliberately
    /// not a syntax: a `transform` card should be readable at a glance on a
    /// canvas, and `dedupe` is readable in a way that `{"op":"dedupe"}` is not.
    pub fn parse(text: &str) -> Transform {
        let text = text.trim();
        let (head, rest) = match text.split_once(char::is_whitespace) {
            Some((h, r)) => (h.trim(), r.trim()),
            None => (text, ""),
        };

        match head.to_lowercase().as_str() {
            "extract-frontmatter" | "frontmatter" => Transform::ExtractFrontmatter,
            "split-by-heading" | "split" => Transform::SplitByHeading {
                level: rest
                    .parse()
                    .ok()
                    .filter(|l| (1..=6).contains(l))
                    .unwrap_or(2),
            },
            "dedupe" | "unique" => Transform::Dedupe,
            "to-list" | "list" => Transform::ToList,
            "sort" => Transform::Sort,
            "take" | "head" => Transform::Take {
                n: rest.parse().unwrap_or(10),
            },
            "filter" | "grep" => Transform::Filter {
                contains: rest.to_string(),
            },
            "strip-headings" => Transform::StripHeadings,
            "squeeze" => Transform::Squeeze,
            "join" => Transform::Join {
                separator: if rest.is_empty() {
                    "\n\n".into()
                } else {
                    unescape(rest)
                },
            },
            // An unrecognised transform joins its inputs rather than failing the
            // run. A canvas is a workspace; a typo in one card should not throw
            // away the work of every card upstream of it.
            _ => Transform::Join {
                separator: "\n\n".into(),
            },
        }
    }

    pub fn apply(&self, inputs: &[String]) -> String {
        match self {
            Transform::Join { separator } => inputs.join(separator),

            Transform::ExtractFrontmatter => inputs
                .iter()
                .filter_map(|s| arc_labs_core::markdown::frontmatter::split(s).frontmatter)
                .collect::<Vec<_>>()
                .join("\n"),

            Transform::SplitByHeading { level } => {
                let joined = inputs.join("\n\n");
                let mut out: Vec<String> = Vec::new();
                let mut current = String::new();
                for line in joined.lines() {
                    if heading_level(line).is_some_and(|l| l <= *level)
                        && !current.trim().is_empty()
                    {
                        out.push(current.trim_end().to_string());
                        current = String::new();
                    }
                    current.push_str(line);
                    current.push('\n');
                }
                if !current.trim().is_empty() {
                    out.push(current.trim_end().to_string());
                }
                out.join("\n\n---\n\n")
            }

            Transform::Dedupe => {
                // BTreeSet, not HashSet: iteration order of a HashSet is not
                // stable across runs, and this must be.
                let mut seen: BTreeSet<&str> = BTreeSet::new();
                lines(inputs)
                    .into_iter()
                    .filter(|l| l.trim().is_empty() || seen.insert(l))
                    .collect::<Vec<_>>()
                    .join("\n")
            }

            Transform::ToList => lines(inputs)
                .into_iter()
                .filter(|l| !l.trim().is_empty())
                .map(|l| {
                    // Already a bullet or a task? Leave it alone rather than
                    // producing `- - item`.
                    let t = l.trim_start();
                    if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") {
                        l.to_string()
                    } else {
                        format!("- {}", l.trim())
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),

            Transform::Sort => {
                let mut ls: Vec<&str> = lines(inputs)
                    .into_iter()
                    .filter(|l| !l.trim().is_empty())
                    .collect();
                // Case-insensitive primary, exact secondary: two lines differing
                // only in case must still have a defined order.
                ls.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));
                ls.join("\n")
            }

            Transform::Take { n } => lines(inputs)
                .into_iter()
                .take(*n)
                .collect::<Vec<_>>()
                .join("\n"),

            Transform::Filter { contains } => {
                let needle = contains.to_lowercase();
                lines(inputs)
                    .into_iter()
                    .filter(|l| l.to_lowercase().contains(&needle))
                    .collect::<Vec<_>>()
                    .join("\n")
            }

            Transform::StripHeadings => lines(inputs)
                .into_iter()
                .filter(|l| heading_level(l).is_none())
                .collect::<Vec<_>>()
                .join("\n"),

            Transform::Squeeze => {
                let mut out: Vec<&str> = Vec::new();
                let mut blank = false;
                for l in lines(inputs) {
                    let is_blank = l.trim().is_empty();
                    if is_blank && blank {
                        continue;
                    }
                    blank = is_blank;
                    out.push(l);
                }
                out.join("\n")
            }
        }
    }
}

fn lines(inputs: &[String]) -> Vec<&str> {
    inputs.iter().flat_map(|s| s.lines()).collect()
}

fn heading_level(line: &str) -> Option<u8> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    // `#Tag` is not a heading; `# Heading` is.
    if (1..=6).contains(&hashes) && line.as_bytes().get(hashes) == Some(&b' ') {
        Some(hashes as u8)
    } else {
        None
    }
}

/// `\n` and `\t` in a separator argument, so `join \n` does what it looks like.
fn unescape(s: &str) -> String {
    s.replace("\\n", "\n").replace("\\t", "\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(spec: &str, inputs: &[&str]) -> String {
        let owned: Vec<String> = inputs.iter().map(|s| s.to_string()).collect();
        Transform::parse(spec).apply(&owned)
    }

    /// **The Phase 5 determinism gate**, at the transform level.
    #[test]
    fn every_transform_is_a_pure_function_of_its_input() {
        let inputs = [
            "---\ntitle: A\n---\n# One\n\nalpha\nbeta\nalpha\n\n## Two\n\ngamma\n".to_string(),
            "---\ntitle: B\n---\n# Three\n\ndelta\nbeta\n".to_string(),
        ];
        let specs = [
            "extract-frontmatter",
            "split-by-heading 1",
            "dedupe",
            "to-list",
            "sort",
            "take 3",
            "filter alpha",
            "strip-headings",
            "squeeze",
            "join ---",
        ];
        for spec in specs {
            let first = Transform::parse(spec).apply(&inputs);
            for _ in 0..50 {
                assert_eq!(
                    Transform::parse(spec).apply(&inputs),
                    first,
                    "transform {spec:?} varied between runs"
                );
            }
        }
    }

    #[test]
    fn extract_frontmatter_takes_the_yaml_and_nothing_else() {
        let out = t(
            "extract-frontmatter",
            &["---\na: 1\nb: 2\n---\n# Body\n\ntext\n"],
        );
        assert_eq!(out, "a: 1\nb: 2\n");
        // A note without frontmatter contributes nothing rather than erroring.
        assert_eq!(t("frontmatter", &["# No frontmatter\n"]), "");
    }

    #[test]
    fn split_by_heading_breaks_at_the_requested_level() {
        let out = t(
            "split-by-heading 2",
            &["# A\n\none\n\n## B\n\ntwo\n\n### C\n\nthree\n"],
        );
        assert_eq!(
            out.matches("---").count(),
            1,
            "one break, at the ## heading"
        );
        assert!(
            out.contains("### C"),
            "a deeper heading stays inside its section"
        );
    }

    #[test]
    fn dedupe_keeps_the_first_occurrence_and_the_order() {
        assert_eq!(t("dedupe", &["b\na\nb\nc\na\n"]), "b\na\nc");
        // Blank lines are structure, not content, so they are not deduped away.
        assert_eq!(t("dedupe", &["a\n\nb\n\nc\n"]), "a\n\nb\n\nc");
    }

    #[test]
    fn to_list_does_not_double_bullet_an_existing_list() {
        assert_eq!(t("to-list", &["one\ntwo\n"]), "- one\n- two");
        assert_eq!(
            t("list", &["- already\n* starred\n"]),
            "- already\n* starred"
        );
    }

    #[test]
    fn sort_is_case_insensitive_but_still_total() {
        assert_eq!(
            t("sort", &["Banana\napple\nCherry\n"]),
            "apple\nBanana\nCherry"
        );
        // Two lines differing only in case still get a defined order.
        assert_eq!(t("sort", &["b\nB\n"]), "B\nb");
    }

    #[test]
    fn take_filter_strip_and_squeeze() {
        assert_eq!(t("take 2", &["a\nb\nc\n"]), "a\nb");
        assert_eq!(t("filter BET", &["alpha\nbeta\ngamma\n"]), "beta");
        assert_eq!(
            t("strip-headings", &["# H\ntext\n## H2\nmore\n"]),
            "text\nmore"
        );
        assert_eq!(t("squeeze", &["a\n\n\n\nb\n"]), "a\n\nb");
    }

    #[test]
    fn join_uses_the_separator_and_understands_escapes() {
        assert_eq!(t("join", &["a", "b"]), "a\n\nb");
        assert_eq!(t("join ---", &["a", "b"]), "a---b");
        assert_eq!(t("join \\n", &["a", "b"]), "a\nb");
    }

    #[test]
    fn an_unknown_transform_joins_rather_than_failing_the_run() {
        // A typo in one card must not throw away the work of every card
        // upstream of it.
        assert_eq!(t("dedupeeee", &["a", "b"]), "a\n\nb");
        assert_eq!(t("", &["a", "b"]), "a\n\nb");
    }

    #[test]
    fn a_tag_is_not_a_heading() {
        // `#rust` must not be stripped as a heading.
        assert_eq!(
            t("strip-headings", &["#rust\n# Real\ntext\n"]),
            "#rust\ntext"
        );
    }

    #[test]
    fn empty_input_produces_empty_output_rather_than_an_error() {
        for spec in [
            "dedupe",
            "sort",
            "to-list",
            "extract-frontmatter",
            "split-by-heading 2",
        ] {
            assert_eq!(t(spec, &[]), "", "{spec} did not handle empty input");
            assert_eq!(t(spec, &[""]), "", "{spec} did not handle an empty string");
        }
    }
}
