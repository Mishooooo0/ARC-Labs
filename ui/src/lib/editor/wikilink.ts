/**
 * A Lezer markdown extension for Obsidian's non-CommonMark syntax.
 *
 * Live preview decorates by walking the syntax tree, so the tree has to *know*
 * about `[[wikilinks]]`, `![[embeds]]` and `#tags`. Matching them with a regex
 * over visible text instead would reintroduce the exact bug the Rust renderer
 * was built to avoid: `[[NotALink]]` inside a fenced code block would decorate,
 * because a regex cannot see that it is inside a code block and the tree can.
 *
 * The node names here mirror what `arc-labs-core::markdown::wikilink` produces,
 * so the editor and the renderer agree about what a link is. If they ever
 * disagree, a note would look different while being edited and after being
 * saved — which is the kind of difference that makes an editor feel untrustworthy.
 */

import type { InlineContext, MarkdownConfig } from "@lezer/markdown";
import { tags as t, Tag } from "@lezer/highlight";

/** Custom highlight tags so the theme can style these without inventing CSS. */
export const wikiTags = {
  link: Tag.define(),
  embed: Tag.define(),
  tag: Tag.define(),
  mark: Tag.define(),
};

const OPEN = 0x5b; // [
const BANG = 0x21; // !
const HASH = 0x23; // #

/**
 * `[[Target#anchor|alias]]` and `![[Target]]`.
 *
 * Parsed as one node with child marks, so a decoration can hide the brackets
 * while leaving the label editable — the whole point of live preview.
 */
const WikiLink: MarkdownConfig = {
  defineNodes: [
    { name: "ArcWikiLink", style: wikiTags.link },
    { name: "ArcEmbed", style: wikiTags.embed },
    { name: "ArcLinkMark", style: wikiTags.mark },
  ],
  parseInline: [
    {
      name: "ArcWikiLink",
      before: "Link",
      parse(cx: InlineContext, next: number, pos: number): number {
        const embed = next === BANG;
        const start = embed ? pos + 1 : pos;
        if (cx.char(start) !== OPEN || cx.char(start + 1) !== OPEN) return -1;

        // Scan for `]]` on the same line. A wikilink never spans a line, and
        // bailing keeps `[[a` on one line from swallowing the rest of the note.
        let i = start + 2;
        for (; i < cx.end; i++) {
          const c = cx.char(i);
          if (c === 10) return -1; // newline
          if (c === 0x5d && cx.char(i + 1) === 0x5d) break;
        }
        if (i >= cx.end) return -1;
        if (i === start + 2) return -1; // `[[]]` names nothing

        const end = i + 2;
        return cx.addElement(
          cx.elt(embed ? "ArcEmbed" : "ArcWikiLink", pos, end, [
            cx.elt("ArcLinkMark", pos, start + 2),
            cx.elt("ArcLinkMark", i, end),
          ]),
        );
      },
    },
  ],
};

/**
 * `#tag`, `#nested/tag`.
 *
 * The rules match the Rust scanner exactly: a `#` only opens a tag at the start
 * of a line or after a non-word character, the name may not be all digits, and
 * a leading or trailing `/` is a typo rather than a nesting level. Keeping these
 * in sync matters — a tag that highlights here but is not indexed there is worse
 * than one that does neither.
 */
const Tags: MarkdownConfig = {
  defineNodes: [{ name: "ArcTag", style: wikiTags.tag }],
  parseInline: [
    {
      name: "ArcTag",
      parse(cx: InlineContext, next: number, pos: number): number {
        if (next !== HASH) return -1;

        const prev = pos > cx.offset ? cx.char(pos - 1) : -1;
        if (prev >= 0 && isWordChar(prev)) return -1;

        let i = pos + 1;
        let digitsOnly = true;
        while (i < cx.end) {
          const c = cx.char(i);
          if (!isTagChar(c)) break;
          if (c < 0x30 || c > 0x39) digitsOnly = false;
          i++;
        }
        if (i === pos + 1) return -1;
        if (digitsOnly) return -1;
        if (cx.char(pos + 1) === 0x2f || cx.char(i - 1) === 0x2f) return -1;

        return cx.addElement(cx.elt("ArcTag", pos, i));
      },
    },
  ],
};

function isWordChar(c: number): boolean {
  return (
    (c >= 0x30 && c <= 0x39) || // 0-9
    (c >= 0x41 && c <= 0x5a) || // A-Z
    (c >= 0x61 && c <= 0x7a) || // a-z
    c === 0x5f || // _
    c === 0x2d || // -
    c === 0x2f || // /
    c === HASH ||
    c > 0x7f // any non-ASCII letter
  );
}

function isTagChar(c: number): boolean {
  return (
    (c >= 0x30 && c <= 0x39) ||
    (c >= 0x41 && c <= 0x5a) ||
    (c >= 0x61 && c <= 0x7a) ||
    c === 0x5f ||
    c === 0x2d ||
    c === 0x2f ||
    c > 0x7f
  );
}

export const arcMarkdown: MarkdownConfig[] = [WikiLink, Tags];

/** Re-exported so the theme file does not need to import from @lezer/highlight. */
export const highlightTags = t;
