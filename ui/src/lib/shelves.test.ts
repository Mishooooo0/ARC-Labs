import { describe, expect, it } from "vitest";
import {
  bookAt,
  DEFAULT_LAYOUT,
  folderOf,
  layout,
  movedInto,
  shelfAt,
  spineWidth,
  stepToward,
} from "./shelves";
import type { TreeEntry, TreeView } from "./types";

/**
 * A tree, written the way the scanner produces one: parents come before their
 * children, and `parent` is an index into this same array.
 */
function tree(rows: Array<[string, boolean, number?, number?]>): TreeView {
  const entries: TreeEntry[] = rows.map(([path, isDir, parent, size]) => ({
    path,
    name: path.slice(path.lastIndexOf("/") + 1),
    is_dir: isDir,
    parent: parent ?? null,
    size: size ?? 0,
  }));
  return {
    entries,
    note_count: entries.filter((e) => !e.is_dir).length,
    canvas_count: 0,
    skipped: [],
  };
}

const opts = DEFAULT_LAYOUT;

describe("spineWidth", () => {
  it("never returns a book too thin to click", () => {
    // Zero-byte notes are real — two of them are in the Phase 0 fixture — and a
    // spine of zero width is a note you cannot reach.
    expect(spineWidth(0)).toBeGreaterThanOrEqual(9);
  });

  it("makes a long note visibly fatter than a short one", () => {
    expect(spineWidth(40_000)).toBeGreaterThan(spineWidth(400));
    expect(spineWidth(400)).toBeGreaterThan(spineWidth(0));
  });

  it("clamps, so one enormous note cannot fill the screen", () => {
    // Four orders of magnitude apart, at most a few times wider.
    expect(spineWidth(10_000_000)).toBeLessThanOrEqual(46);
  });

  it("is monotonic", () => {
    let last = 0;
    for (const n of [0, 100, 1_000, 10_000, 100_000, 1_000_000]) {
      const w = spineWidth(n);
      expect(w).toBeGreaterThanOrEqual(last);
      last = w;
    }
  });
});

describe("layout", () => {
  it("puts root notes on a shelf named for the vault", () => {
    const l = layout(tree([["a.md", false, undefined, 10]]), "my-vault", opts);
    expect(l.shelves[0]!.label).toBe("my-vault");
    expect(l.shelves[0]!.books.map((b) => b.path)).toEqual(["a.md"]);
  });

  it("gives every folder a shelf, indented under its parent", () => {
    const l = layout(
      tree([
        ["ARC", true],
        ["ARC/Deep", true, 0],
        ["ARC/Deep/n.md", false, 1, 20],
      ]),
      "v",
      opts,
    );
    const arc = l.shelves.find((s) => s.path === "ARC")!;
    const deep = l.shelves.find((s) => s.path === "ARC/Deep")!;

    expect(arc.depth).toBe(1);
    expect(deep.depth).toBe(2);
    expect(deep.x).toBeGreaterThan(arc.x);
    // A child sits below its parent, which is what makes depth readable
    // without drawing a single connecting line.
    expect(deep.y).toBeGreaterThan(arc.y);
  });

  /**
   * The reason this view reads the tree rather than the graph. A folder with no
   * notes appears in no graph payload, and an invisible folder cannot be a drop
   * target — so "drag a book onto the shelf you just made" would be impossible.
   */
  it("shows a folder that has no notes in it", () => {
    const l = layout(tree([["Empty", true]]), "v", opts);
    const empty = l.shelves.find((s) => s.path === "Empty");
    expect(empty).toBeDefined();
    expect(empty!.books).toHaveLength(0);
  });

  it("can hide empty folders without hiding the root", () => {
    const l = layout(tree([["Empty", true]]), "v", { ...opts, showEmpty: false });
    expect(l.shelves.find((s) => s.path === "Empty")).toBeUndefined();
    // The root shelf survives even with nothing on it: it is where a note with
    // no folder would land, so it has to stay a target.
    expect(l.shelves.find((s) => s.path === "")).toBeDefined();
  });

  it("wraps a shelf that outgrows its width instead of running off screen", () => {
    const rows: Array<[string, boolean, number?, number?]> = [["F", true]];
    for (let i = 0; i < 120; i++) rows.push([`F/n${i}.md`, false, 0, 500]);

    const l = layout(tree(rows), "v", { ...opts, maxShelfWidth: 300 });
    const shelf = l.shelves.find((s) => s.path === "F")!;
    const ys = new Set(shelf.books.map((b) => b.y));

    expect(ys.size).toBeGreaterThan(1);
    expect(shelf.h).toBeGreaterThan(100);
    for (const b of shelf.books) {
      expect(b.x + b.w).toBeLessThanOrEqual(shelf.x + 300 + 46);
    }
  });

  it("never overlaps two books on the same row", () => {
    const rows: Array<[string, boolean, number?, number?]> = [["F", true]];
    for (let i = 0; i < 30; i++) rows.push([`F/n${i}.md`, false, 0, i * 900]);

    const shelf = layout(tree(rows), "v", opts).shelves.find((s) => s.path === "F")!;
    const byRow = new Map<number, typeof shelf.books>();
    for (const b of shelf.books) {
      const list = byRow.get(b.y) ?? [];
      list.push(b);
      byRow.set(b.y, list);
    }
    for (const list of byRow.values()) {
      list.sort((a, b) => a.x - b.x);
      for (let i = 1; i < list.length; i++) {
        expect(list[i]!.x).toBeGreaterThanOrEqual(list[i - 1]!.x + list[i - 1]!.w);
      }
    }
  });

  it("never overlaps two shelves", () => {
    const l = layout(
      tree([
        ["a.md", false, undefined, 10],
        ["A", true],
        ["A/b.md", false, 1, 10],
        ["B", true],
        ["B/c.md", false, 3, 10],
      ]),
      "v",
      opts,
    );
    for (let i = 1; i < l.shelves.length; i++) {
      const prev = l.shelves[i - 1]!;
      expect(l.shelves[i]!.y).toBeGreaterThanOrEqual(prev.y + prev.h);
    }
  });

  /** A layout that moved on its own would make every animation a lie. */
  it("is deterministic", () => {
    const t = tree([
      ["B", true],
      ["B/two.md", false, 0, 30],
      ["A", true],
      ["A/one.md", false, 2, 10],
    ]);
    expect(JSON.stringify(layout(t, "v", opts))).toBe(JSON.stringify(layout(t, "v", opts)));
  });

  it("sorts by size when asked, and by name otherwise", () => {
    // Deliberately named so the two orders disagree. With `big`/`small` the
    // alphabetical and by-size orders coincide, and the test would pass even if
    // the sort control did nothing at all.
    const t = tree([
      ["F", true],
      ["F/apple.md", false, 0, 10],
      ["F/zebra.md", false, 0, 9000],
    ]);
    const byName = layout(t, "v", opts).shelves.find((s) => s.path === "F")!;
    expect(byName.books.map((b) => b.label)).toEqual(["apple", "zebra"]);

    const bySize = layout(t, "v", { ...opts, sort: "size" }).shelves.find((s) => s.path === "F")!;
    expect(bySize.books.map((b) => b.label)).toEqual(["zebra", "apple"]);
  });

  it("survives an entirely empty vault", () => {
    const l = layout(tree([]), "v", opts);
    expect(l.shelves).toHaveLength(1);
    expect(l.shelves[0]!.books).toHaveLength(0);
    expect(l.height).toBeGreaterThanOrEqual(0);
  });

  it("strips the extension for the spine but keeps the path", () => {
    const l = layout(tree([["Note name.md", false, undefined, 5]]), "v", opts);
    const b = l.shelves[0]!.books[0]!;
    expect(b.label).toBe("Note name");
    expect(b.path).toBe("Note name.md");
  });

  it("marks canvases so they can be drawn differently", () => {
    const l = layout(
      tree([
        ["a.md", false, undefined, 5],
        ["b.canvas", false, undefined, 5],
      ]),
      "v",
      opts,
    );
    const books = l.shelves[0]!.books;
    expect(books.find((b) => b.path === "b.canvas")!.isCanvas).toBe(true);
    expect(books.find((b) => b.path === "a.md")!.isCanvas).toBe(false);
  });
});

describe("hit testing", () => {
  const l = layout(
    tree([
      ["F", true],
      ["F/n.md", false, 0, 400],
    ]),
    "v",
    opts,
  );
  const shelf = l.shelves.find((s) => s.path === "F")!;
  const book = shelf.books[0]!;

  it("finds a book under a point inside its spine", () => {
    expect(bookAt(l, book.x + book.w / 2, book.y + book.h / 2)?.path).toBe("F/n.md");
  });

  it("finds nothing in the gap between books", () => {
    expect(bookAt(l, book.x - 500, book.y + book.h / 2)).toBeNull();
  });

  it("finds the shelf even where there is no book, so empty shelves take drops", () => {
    expect(shelfAt(l, shelf.x + 5, shelf.y + shelf.h / 2)?.path).toBe("F");
  });
});

describe("moving", () => {
  it("reads a note's current folder from its path", () => {
    expect(folderOf("ARC/Deep/n.md")).toBe("ARC/Deep");
    expect(folderOf("n.md")).toBe("");
  });

  it("keeps the filename when moving, which is why links survive", () => {
    // Wikilinks resolve by name, not path, so a folder move does not break
    // links pointing at the note. That only holds while the name is untouched.
    expect(movedInto("A/n.md", "B")).toBe("B/n.md");
    expect(movedInto("A/n.md", "")).toBe("n.md");
    expect(movedInto("n.md", "B/C")).toBe("B/C/n.md");
  });
});

describe("the motion budget", () => {
  const target = { x: 100, y: 100 };

  /**
   * The accessibility gate. Appearance → Motion 0 — and `prefers-reduced-motion`,
   * which forces that token to 0 — must stop every animation dead. A canvas
   * loop does not inherit that the way a CSS transition does, so it is asserted
   * here rather than trusted.
   */
  it("snaps in a single step with motion 0, and asks for no more frames", () => {
    const live = { x: 0, y: 0, spawn: 0 };
    const busy = stepToward(live, target, 0);

    expect(live).toEqual({ x: 100, y: 100, spawn: 1 });
    expect(busy).toBe(false);
  });

  it("with motion on it eases rather than jumping", () => {
    const live = { x: 0, y: 0, spawn: 0 };
    expect(stepToward(live, target, 1)).toBe(true);

    expect(live.x).toBeGreaterThan(0);
    expect(live.x).toBeLessThan(target.x);
    expect(live.spawn).toBeGreaterThan(0);
    expect(live.spawn).toBeLessThan(1);
  });

  it("arrives, so the loop stops instead of spinning for ever", () => {
    const live = { x: 0, y: 0, spawn: 0 };
    let frames = 0;
    while (stepToward(live, target, 1) && frames < 600) frames++;

    expect(frames).toBeLessThan(600);
    expect(live.x).toBe(target.x);
    expect(live.y).toBe(target.y);
    expect(live.spawn).toBe(1);
  });

  it("moves faster the higher the multiplier", () => {
    const slow = { x: 0, y: 0, spawn: 0 };
    const fast = { x: 0, y: 0, spawn: 0 };
    stepToward(slow, target, 0.5);
    stepToward(fast, target, 1.5);
    expect(fast.x).toBeGreaterThan(slow.x);
  });
});
