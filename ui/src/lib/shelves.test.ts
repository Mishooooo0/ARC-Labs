import { describe, expect, it } from "vitest";
import {
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
    // Rectangles, not "each one lower than the last": bays stand side by side
    // now, so the second column starts back at the top by design.
    for (let i = 0; i < l.shelves.length; i++) {
      for (let j = i + 1; j < l.shelves.length; j++) {
        const a = l.shelves[i]!;
        const b = l.shelves[j]!;
        const apart =
          a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;
        expect(apart, `${a.path || "root"} overlaps ${b.path || "root"}`).toBe(true);
      }
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

describe("columns", () => {
  /** A vault of `n` folders with one note each — `n + 1` bays with the root. */
  function folders(n: number) {
    const rows: Array<[string, boolean, number?, number?]> = [];
    for (let i = 0; i < n; i++) {
      rows.push([`F${i}`, true]);
      rows.push([`F${i}/n.md`, false, i * 2, 400]);
    }
    return tree(rows);
  }

  /**
   * Which column a shelf stands in, by its origin.
   *
   * Not the raw `x`: a nested shelf is indented from its parent, so counting
   * distinct `x` values counts indent levels as well as columns.
   */
  const columns = (l: ReturnType<typeof layout>) =>
    new Set(l.shelves.map((s) => s.x - s.depth * 34));

  /**
   * The defect this exists to fix. Stacked in one column, a vault of eight
   * folders is roughly three times taller than it is wide, so a wide window has
   * to shrink it to a strip down the middle — which is what "still small" was.
   */
  it("stands bays side by side rather than one thin strip in a wide window", () => {
    const wide = layout(folders(8), "v", { ...opts, aspect: 16 / 9 });
    expect(columns(wide).size).toBeGreaterThan(1);
    // Within a factor of two of the window it has to fill — the point being
    // that it is no longer off by a factor of five.
    expect(wide.width / wide.height).toBeGreaterThan((16 / 9) * 0.5);
    expect(wide.width / wide.height).toBeLessThan((16 / 9) * 2);
  });

  it("uses one column when the window is tall and narrow", () => {
    const tall = layout(folders(8), "v", { ...opts, aspect: 0.4 });
    expect(columns(tall).size).toBe(1);
  });

  it("obeys an explicit column count, because it is a control", () => {
    const two = layout(folders(9), "v", { ...opts, columns: 2 });
    expect(columns(two).size).toBe(2);

    const one = layout(folders(9), "v", { ...opts, columns: 1 });
    expect(columns(one).size).toBe(1);
  });

  /**
   * A child shelf reads as belonging to the one above it because it is indented
   * from it. Split a folder across a column break and the child lands at the
   * top of the next column with nothing to be indented *from*, and the nesting
   * stops meaning anything.
   */
  it("keeps a folder's subtree together in one column", () => {
    const rows: Array<[string, boolean, number?, number?]> = [];
    for (let i = 0; i < 6; i++) {
      rows.push([`F${i}`, true]);
      rows.push([`F${i}/kid`, true, i * 3]);
      rows.push([`F${i}/kid/n.md`, false, i * 3 + 1, 400]);
    }
    const l = layout(tree(rows), "v", { ...opts, aspect: 16 / 9 });

    for (let i = 0; i < 6; i++) {
      const parent = l.shelves.find((s) => s.path === `F${i}`)!;
      const kid = l.shelves.find((s) => s.path === `F${i}/kid`)!;
      // Same column: the child is exactly one indent right of its parent, and
      // below it.
      expect(kid.x - parent.x).toBe(34);
      expect(kid.y).toBeGreaterThan(parent.y);
    }
  });

  it("runs every plank in a column out to the same edge", () => {
    const l = layout(folders(6), "v", { ...opts, columns: 2 });
    const edges = new Map<number, Set<number>>();
    for (const s of l.shelves) {
      const col = s.x - s.depth * 34;
      const set = edges.get(col) ?? new Set<number>();
      set.add(s.x + s.w);
      edges.set(col, set);
    }
    for (const [col, set] of edges) {
      expect(set.size, `column at ${col} has ragged plank ends`).toBe(1);
    }
  });

  it("is still deterministic once columns are in play", () => {
    const t = folders(7);
    const a = layout(t, "v", { ...opts, aspect: 1.9 });
    const b = layout(t, "v", { ...opts, aspect: 1.9 });
    expect(JSON.stringify(a)).toBe(JSON.stringify(b));
  });

  it("never leaves a column empty, which would open a hole in the case", () => {
    const l = layout(folders(5), "v", { ...opts, columns: 3 });
    const xs = [...columns(l)].sort((a, b) => a - b);
    // Column origins are evenly spaced: no gap where a column was skipped.
    for (let i = 2; i < xs.length; i++) {
      expect(xs[i]! - xs[i - 1]!).toBeCloseTo(xs[1]! - xs[0]!, 6);
    }
  });
});

/**
 * Only the shelf is hit-tested here now. A *book* is picked by three's own
 * raycaster against the instanced mesh, which reports the instance it hit — so
 * a hand-written rectangle test for books would be a second answer to a
 * question already answered, and the two would drift.
 */
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

describe("planks", () => {
  it("puts a plank under every row, so wrapped books are not floating", () => {
    const rows: Array<[string, boolean, number?, number?]> = [["F", true]];
    for (let i = 0; i < 60; i++) rows.push([`F/n${i}.md`, false, 0, 500]);

    const shelf = layout(tree(rows), "v", { ...opts, maxShelfWidth: 300 }).shelves.find(
      (s) => s.path === "F",
    )!;
    const rowTops = new Set(shelf.books.map((b) => b.y));

    expect(shelf.planks).toHaveLength(rowTops.size);
    // Every book stands on one of them: its underside meets a plank exactly.
    for (const b of shelf.books) {
      expect(shelf.planks.some((p) => Math.abs(b.y + b.h - p) < 0.001)).toBe(true);
    }
  });

  it("gives an empty shelf a plank anyway, so it still reads as a shelf", () => {
    const shelf = layout(tree([["Empty", true]]), "v", opts).shelves.find(
      (s) => s.path === "Empty",
    )!;
    expect(shelf.planks).toHaveLength(1);
  });
});
