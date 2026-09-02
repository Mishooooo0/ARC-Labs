//! Markdown: split, scan and render, all without touching the filesystem.

pub mod frontmatter;
pub mod render;
pub mod wikilink;

pub use render::{render, Heading, RenderedNote};
pub use wikilink::WikiLink;
