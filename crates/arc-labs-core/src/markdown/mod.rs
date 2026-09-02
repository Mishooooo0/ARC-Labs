//! Markdown: split, scan and render, all without touching the filesystem.

pub mod frontmatter;
pub mod render;
pub mod wikilink;

pub use render::{render, RenderedNote};
pub use wikilink::WikiLink;
