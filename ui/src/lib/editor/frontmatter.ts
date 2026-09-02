/**
 * Frontmatter detection for the editor.
 *
 * ## Why this exists
 *
 * `@codemirror/lang-markdown` does not know what YAML frontmatter is, so it
 * parses the block as ordinary markdown. That produced a real and nasty bug:
 * a note whose frontmatter read
 *
 *     tags: [perf]
 *
 * displayed in the editor as
 *
 *     tags: perf
 *
 * because `[perf]` was parsed as a link and live preview dutifully hid its
 * brackets. The editor was showing something the file did not say — which is
 * the single worst failure mode available to a program whose whole promise is
 * that the file is the source of truth.
 *
 * ## Why a scan rather than a Lezer block parser
 *
 * The obvious fix is a `parseBlock` extension. The obvious fix is also awkward:
 * detecting an *unterminated* `---` requires consuming lines and then refusing
 * them, which is not something the block-parser API does cleanly.
 *
 * More importantly, this rule has to agree exactly with
 * `arc-labs-core::markdown::frontmatter`, which decides what actually gets
 * rendered and saved. Implementing the same rule twice in two different shapes
 * is how the two drift apart. Implementing it twice in the *same* shape — a
 * direct scan of the document text — is a rule anyone can check by reading both.
 *
 * The rule, identically in both: the document must open with `---` on a line of
 * its own, and the block ends at the next line that is exactly `---` or `...`.
 * An unterminated block is not frontmatter.
 */

import { type Extension, type Range, StateField } from "@codemirror/state";
import type { Text } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView } from "@codemirror/view";

export interface FrontmatterRange {
  /** Start of the document (always 0 when present). */
  from: number;
  /** End of the closing delimiter line. */
  to: number;
  /** Start of the YAML itself, after the opening `---` line. */
  contentFrom: number;
  /** End of the YAML itself, before the closing delimiter. */
  contentTo: number;
}

export function frontmatterRange(doc: Text): FrontmatterRange | null {
  if (doc.lines < 2) return null;
  if (doc.line(1).text !== "---") return null;

  for (let n = 2; n <= doc.lines; n++) {
    const line = doc.line(n);
    if (line.text === "---" || line.text === "...") {
      return {
        from: 0,
        to: line.to,
        contentFrom: doc.line(1).to + 1,
        contentTo: line.from,
      };
    }
  }
  // Opened and never closed. Far more likely to be someone typing `---` at the
  // top of a note than a note that is entirely frontmatter.
  return null;
}

const frontmatterField = StateField.define<FrontmatterRange | null>({
  create: (state) => frontmatterRange(state.doc),
  update: (value, tr) => (tr.docChanged ? frontmatterRange(tr.state.doc) : value),
});

/** The frontmatter range in the current state, or `null`. */
export function currentFrontmatter(state: {
  field: (f: typeof frontmatterField, required?: false) => FrontmatterRange | null | undefined;
}): FrontmatterRange | null {
  return state.field(frontmatterField, false) ?? null;
}

export { frontmatterField };

/**
 * Frontmatter is shown verbatim, in the data face, visually set apart — never
 * markdown-decorated. It is metadata the user typed by hand, and it round-trips
 * to disk byte for byte, so the editor shows exactly its bytes.
 */
const frontmatterLine = Decoration.line({ class: "cm-arc-frontmatter" });
const frontmatterDelim = Decoration.line({ class: "cm-arc-frontmatter cm-arc-frontmatter-delim" });

const frontmatterDecorations = EditorView.decorations.compute(
  [frontmatterField, "doc"],
  (state): DecorationSet => {
    const fm = state.field(frontmatterField, false);
    if (!fm) return Decoration.none;

    const marks: Range<Decoration>[] = [];
    const lastLine = state.doc.lineAt(fm.to).number;
    for (let n = 1; n <= lastLine; n++) {
      const line = state.doc.line(n);
      marks.push((n === 1 || n === lastLine ? frontmatterDelim : frontmatterLine).range(line.from));
    }
    return Decoration.set(marks);
  },
);

export function frontmatterSupport(): Extension {
  return [frontmatterField, frontmatterDecorations];
}
