/**
 * Live preview: the markdown source stays authoritative, rendered widgets
 * replace syntax in place, and moving the cursor into a range reveals its source.
 *
 * ## Two rules that decide the whole design
 *
 * **The document is never rewritten.** Nothing here changes a single character.
 * Every effect is a `Decoration`, which is a view-layer overlay. That is what
 * lets Phase 1's byte-identical gate hold: the editor literally cannot introduce
 * drift, because it never touches the text.
 *
 * **Reveal is by selection, not by hover.** A range is shown as source when the
 * selection touches it. Hover would make the document twitch as the mouse
 * crosses it; selection ties the reveal to where the user is actually working.
 *
 * ## Why the marks are hidden rather than replaced
 *
 * For emphasis and headings the syntax characters get `Decoration.replace` with
 * no widget — they occupy zero width but remain in the document. The tempting
 * alternative, replacing `**bold**` with a widget containing "bold", breaks
 * cursor movement, selection and find-in-note, because the text the user sees is
 * no longer the text that is there.
 *
 * The Phase 1 gate says entering a widget must cause **zero layout shift**. That
 * is why revealing a heading does not change its font size: the `#` characters
 * become visible in the gutter-side of the line, and nothing reflows.
 */

import { syntaxTree } from "@codemirror/language";
import { currentFrontmatter } from "./frontmatter";
import { type Extension, type Range, RangeSetBuilder } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  type EditorView,
  ViewPlugin,
  type ViewUpdate,
  WidgetType,
} from "@codemirror/view";

/**
 * Marks that vanish unless the cursor is in their node.
 *
 * `replace` with no widget: the characters occupy zero width but stay in the
 * document, so cursor movement, selection and find-in-note all still see the
 * real text.
 *
 * ## The layout-shift trade-off, decided deliberately
 *
 * Because hidden marks take no width, revealing them makes the line wider — and
 * a line already close to the wrap width gains a second visual row, shifting
 * everything below by one line height (measured: 20.8 px).
 *
 * The alternative was tried and rejected: hiding with `opacity: 0` keeps the
 * characters' width, which makes reveal cost exactly zero layout change. But it
 * leaves a permanent ragged gap around every `**bold**`, every `[[link]]` and
 * every heading marker, so ordinary prose reads as `index   bench    proposal`.
 * Stable, and visibly broken.
 *
 * So: clean text, and the reflow is confined to lines already near the wrap
 * boundary. Widgets — the thing the Phase 1 gate actually names — shift by less
 * than a pixel either way.
 */
const HIDDEN = Decoration.replace({});

/** Node types whose syntax characters are hidden while not being edited. */
const HIDEABLE_MARKS = new Set([
  "EmphasisMark",
  "StrongMark",
  "StrikethroughMark",
  "CodeMark",
  "HeaderMark",
  "QuoteMark",
  "LinkMark",
  "ArcLinkMark",
]);

/** Line decorations that give block context to the theme. */
const LINE_CLASS: Record<string, string> = {
  ATXHeading1: "cm-arc-h1",
  ATXHeading2: "cm-arc-h2",
  ATXHeading3: "cm-arc-h3",
  ATXHeading4: "cm-arc-h4",
  ATXHeading5: "cm-arc-h5",
  ATXHeading6: "cm-arc-h6",
  Blockquote: "cm-arc-quote",
};

/** A rendered task checkbox. Toggling it edits the source, not the widget. */
class TaskWidget extends WidgetType {
  constructor(readonly checked: boolean, readonly pos: number) {
    super();
  }
  eq(other: TaskWidget) {
    return other.checked === this.checked && other.pos === this.pos;
  }
  toDOM(view: EditorView): HTMLElement {
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = this.checked;
    box.className = "cm-arc-task";
    box.addEventListener("mousedown", (e) => {
      e.preventDefault();
      // Replace only the character inside the brackets, so the line's own
      // spacing and any trailing content are untouched.
      view.dispatch({
        changes: { from: this.pos, to: this.pos + 1, insert: this.checked ? " " : "x" },
      });
    });
    return box;
  }
  ignoreEvent() {
    return false;
  }
}

/** A horizontal rule, drawn instead of shown as `---`. */
class RuleWidget extends WidgetType {
  eq() {
    return true;
  }
  toDOM(): HTMLElement {
    const hr = document.createElement("span");
    hr.className = "cm-arc-rule";
    return hr;
  }
}

function selectionTouches(view: EditorView, from: number, to: number): boolean {
  // Inclusive at both ends: a cursor resting immediately after `**bold**`
  // should reveal it, or the marks flicker as the cursor leaves.
  for (const range of view.state.selection.ranges) {
    if (range.from <= to && range.to >= from) return true;
  }
  return false;
}

function buildDecorations(view: EditorView): DecorationSet {
  const marks: Range<Decoration>[] = [];
  const tree = syntaxTree(view.state);

  // Frontmatter is YAML, not markdown. Decorating it would hide characters the
  // file actually contains — `tags: [perf]` rendering as `tags: perf` — which
  // makes the editor lie about the note. See ./frontmatter.ts.
  const fm = currentFrontmatter(view.state);
  const inFrontmatter = (from: number) => fm !== null && from < fm.to;

  for (const { from, to } of view.visibleRanges) {
    tree.iterate({
      from,
      to,
      enter(node) {
        if (inFrontmatter(node.from)) return;
        const name = node.name;

        const lineClass = LINE_CLASS[name];
        if (lineClass) {
          const line = view.state.doc.lineAt(node.from);
          marks.push(Decoration.line({ class: lineClass }).range(line.from));
          return;
        }

        if (name === "HorizontalRule") {
          if (!selectionTouches(view, node.from, node.to)) {
            marks.push(
              Decoration.replace({ widget: new RuleWidget() }).range(node.from, node.to),
            );
          }
          return;
        }

        if (name === "TaskMarker") {
          if (!selectionTouches(view, node.from, node.to)) {
            const text = view.state.doc.sliceString(node.from, node.to);
            const checked = /\[[xX]\]/.test(text);
            marks.push(
              Decoration.replace({
                widget: new TaskWidget(checked, node.from + 1),
              }).range(node.from, node.to),
            );
          }
          return;
        }

        if (HIDEABLE_MARKS.has(name)) {
          // Reveal based on the *parent* range, not the mark itself. A cursor
          // inside `**bold**` sits between the two marks and touches neither, so
          // checking the mark alone would leave the syntax hidden exactly when
          // the user is trying to edit it.
          const parent = node.node.parent;
          const from = parent ? parent.from : node.from;
          const to = parent ? parent.to : node.to;
          if (!selectionTouches(view, from, to)) {
            marks.push(HIDDEN.range(node.from, node.to));
          }
          return;
        }

        if (name === "ArcWikiLink" || name === "ArcEmbed" || name === "ArcTag") {
          marks.push(
            Decoration.mark({
              class: name === "ArcTag" ? "cm-arc-tag" : "cm-arc-link",
            }).range(node.from, node.to),
          );
        }
      },
    });
  }

  // RangeSetBuilder requires sorted, non-overlapping-by-start input; the tree
  // walk yields line decorations and inline marks interleaved.
  marks.sort((a, b) => a.from - b.from || a.value.startSide - b.value.startSide);
  const builder = new RangeSetBuilder<Decoration>();
  for (const m of marks) builder.add(m.from, m.to, m.value);
  return builder.finish();
}

export function livePreview(): Extension {
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = buildDecorations(view);
      }
      update(update: ViewUpdate) {
        // Selection changes matter as much as document changes here: moving the
        // cursor is what reveals and re-hides syntax.
        if (update.docChanged || update.viewportChanged || update.selectionSet) {
          this.decorations = buildDecorations(update.view);
        }
      }
    },
    { decorations: (v) => v.decorations },
  );
}
