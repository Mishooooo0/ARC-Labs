/**
 * The editor's appearance, entirely in terms of the token file.
 *
 * Not one colour literal appears here — `xtask lint-tokens` would fail the build
 * if one did. That is what keeps the editor inside the same three themes as the
 * rest of the app: switching to ARC Terminal restyles CodeMirror without this
 * file knowing the theme exists.
 *
 * Type sizes deliberately do **not** change between a heading's source form and
 * its rendered form. Phase 1's gate says entering a widget must cause zero
 * layout shift, and a heading that grows when you click into it is the most
 * obvious way to fail it.
 */

import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { tags as t } from "@lezer/highlight";

import { wikiTags } from "./wikilink";

const base = EditorView.theme({
  "&": {
    color: "var(--arc-fg)",
    backgroundColor: "transparent",
    fontFamily: "var(--arc-font-prose)",
    fontSize: "var(--arc-text-md)",
    height: "100%",
  },
  ".cm-scroller": {
    fontFamily: "inherit",
    lineHeight: "var(--arc-leading)",
    padding: "var(--arc-space-5) 0 var(--arc-space-8)",
  },
  ".cm-content": {
    padding: "0",
    maxWidth: "var(--arc-measure)",
    caretColor: "var(--arc-accent)",
  },
  ".cm-line": { padding: "0 var(--arc-space-2)" },

  "&.cm-focused": { outline: "none" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--arc-accent)", borderLeftWidth: "2px" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
    backgroundColor: "var(--arc-accent-wash)",
  },
  ".cm-activeLine": { backgroundColor: "transparent" },

  ".cm-gutters": {
    backgroundColor: "transparent",
    color: "var(--arc-fg-faint)",
    border: "none",
    fontFamily: "var(--arc-font-data)",
    fontSize: "var(--arc-text-xs)",
  },
  ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--arc-fg-dim)" },

  // ── Block context ─────────────────────────────────────────────────────────
  // Weight and colour carry the hierarchy; size changes are kept modest so the
  // reveal transition cannot reflow the paragraph below.
  ".cm-arc-h1": { fontSize: "var(--arc-text-2xl)", fontWeight: "600", lineHeight: "1.3" },
  ".cm-arc-h2": { fontSize: "var(--arc-text-xl)", fontWeight: "600", lineHeight: "1.3" },
  ".cm-arc-h3": { fontSize: "var(--arc-text-lg)", fontWeight: "600" },
  ".cm-arc-h4, .cm-arc-h5, .cm-arc-h6": { fontWeight: "600", color: "var(--arc-fg-dim)" },
  ".cm-arc-quote": {
    borderLeft: "2px solid var(--arc-line-strong)",
    color: "var(--arc-fg-dim)",
    paddingLeft: "var(--arc-space-3)",
  },

  // Frontmatter: the data face, set apart, never markdown-styled. What is on
  // screen is exactly the bytes on disk.
  ".cm-arc-frontmatter": {
    fontFamily: "var(--arc-font-data)",
    fontSize: "var(--arc-text-sm)",
    color: "var(--arc-fg-dim)",
    backgroundColor: "var(--arc-bg-1)",
    borderLeft: "2px solid var(--arc-line-strong)",
  },
  ".cm-arc-frontmatter-delim": { color: "var(--arc-fg-faint)" },

  ".cm-arc-rule": {
    display: "inline-block",
    width: "100%",
    borderTop: "1px solid var(--arc-line)",
    verticalAlign: "middle",
  },
  ".cm-arc-task": { accentColor: "var(--arc-accent)", marginRight: "var(--arc-space-1)" },

  ".cm-arc-link": { color: "var(--arc-accent)" },
  ".cm-arc-tag": {
    color: "var(--arc-fg-dim)",
    fontFamily: "var(--arc-font-data)",
    fontSize: "0.92em",
  },

  ".cm-searchMatch": { backgroundColor: "var(--arc-accent-wash)" },
  ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "var(--arc-bg-4)" },
  ".cm-panels": {
    backgroundColor: "var(--arc-bg-2)",
    color: "var(--arc-fg)",
    borderTop: "1px solid var(--arc-line)",
  },
});

const highlight = HighlightStyle.define([
  { tag: t.heading, fontWeight: "600" },
  { tag: t.strong, fontWeight: "650", color: "var(--arc-fg)" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strikethrough, textDecoration: "line-through", color: "var(--arc-fg-faint)" },
  { tag: t.link, color: "var(--arc-accent)" },
  { tag: t.url, color: "var(--arc-accent-dim)" },
  { tag: t.quote, color: "var(--arc-fg-dim)" },

  // Code is data, so it gets the data face.
  {
    tag: t.monospace,
    fontFamily: "var(--arc-font-data)",
    fontSize: "0.92em",
    color: "var(--arc-fg-dim)",
  },

  { tag: wikiTags.link, color: "var(--arc-accent)" },
  { tag: wikiTags.embed, color: "var(--arc-accent-dim)" },
  { tag: wikiTags.tag, color: "var(--arc-fg-dim)" },
  { tag: wikiTags.mark, color: "var(--arc-fg-faint)" },

  // Fenced-code highlighting, kept to a narrow, low-chroma set. A rainbow here
  // would put five accent hues on screen and break the one-accent rule — and
  // would put arbitrary colours next to the reserved amber and blue.
  { tag: [t.keyword, t.moduleKeyword], color: "var(--arc-accent)" },
  { tag: [t.string, t.special(t.string)], color: "var(--arc-fg-dim)" },
  { tag: [t.comment, t.lineComment, t.blockComment], color: "var(--arc-fg-faint)", fontStyle: "italic" },
  { tag: [t.number, t.bool, t.null], color: "var(--arc-fg-dim)" },
  { tag: [t.function(t.variableName), t.definition(t.variableName)], color: "var(--arc-fg)" },
  { tag: [t.typeName, t.className], color: "var(--arc-fg)" },
  { tag: t.invalid, color: "var(--arc-danger)" },
]);

export function arcEditorTheme(): Extension {
  return [base, syntaxHighlighting(highlight)];
}
