import { describe, expect, it } from "vitest";
import { DEFAULT_LAYOUT, layout } from "./shelves";
import type { TreeEntry, TreeView } from "./types";
import {
  bookBox,
  endBox,
  framing,
  labelPoint,
  layoutPoint,
  plankBox,
  PLANK_THICK,
  SCALE,
} from "./scene";

function tree(rows: Array<[string, boolean, number?, number?]>): TreeView {
  const entries: TreeEntry[] = rows.map(([path, isDir, parent, size]) => ({
    path,
    name: path.slice(path.lastIndexOf("/") + 1),
    is_dir: isDir,
    parent: parent ?? null,
    size: size ?? 0,
  }));
  return { entries, note_count: entries.length, canvas_count: 0, skipped: [] };
}

const opts = DEFAULT_LAYOUT;
const model = layout(
  tree([
    ["F", true],
    ["F/a.md", false, 0, 400],
    ["F/b.md", false, 0, 4000],
  ]),
  "v",
  opts,
);
const shelf = model.shelves.find((s) => s.path === "F")!;

describe("placing a book in the world", () => {
  it("centres it, because three.js positions a box by its middle", () => {
    const b = shelf.books[0]!;
    const box = bookBox(b);
    expect(box.x).toBeCloseTo((b.x + b.w / 2) / SCALE, 6);
    expect(box.w).toBeCloseTo(b.w / SCALE, 6);
  });

  it("flips y, because the layout counts downward and the world counts up", () => {
    const [first, second] = [shelf.books[0]!, model.shelves[0]!.books[0]];
    void second;
    expect(bookBox(first).y).toBeLessThan(0);
  });

  /**
   * The one that matters: a book stands *on* its plank. Half a shelf out and
   * the books hang in the air or sink through the wood, which is exactly what
   * a bookshelf must never look like.
   */
  it("stands every book exactly on a plank", () => {
    for (const s of model.shelves) {
      for (const b of s.books) {
        const box = bookBox(b);
        const underside = box.y - box.h / 2;
        const plank = s.planks
          .map((p) => plankBox(s, p))
          .map((pb) => pb.y + pb.h / 2);
        expect(plank.some((top) => Math.abs(top - underside) < 1e-6)).toBe(true);
      }
    }
  });
});

describe("the shelf furniture", () => {
  it("labels a shelf on its own lowest plank, not floating above the next one", () => {
    const label = labelPoint(shelf);
    const lowest = Math.min(...shelf.planks.map((p) => plankBox(shelf, p).y));
    expect(label.y).toBeLessThan(lowest);
    expect(label.x).toBeCloseTo(shelf.x / SCALE, 6);
  });

  it("puts the end upright to the left of the first book", () => {
    const end = endBox(shelf);
    const firstBook = bookBox(shelf.books[0]!);
    expect(end.x).toBeLessThan(firstBook.x - firstBook.w / 2);
    expect(end.h).toBeGreaterThan(0);
  });

  it("gives a plank real thickness, so it is a shelf and not a line", () => {
    expect(PLANK_THICK).toBeGreaterThan(0);
    expect(plankBox(shelf, shelf.planks[0]!).h).toBeCloseTo(PLANK_THICK, 6);
  });
});

describe("framing the library", () => {
  it("backs off far enough to hold a wide vault", () => {
    const wide = framing({ shelves: [], width: 4000, height: 200 }, 16 / 9);
    const narrow = framing({ shelves: [], width: 200, height: 200 }, 16 / 9);
    expect(wide.z).toBeGreaterThan(narrow.z);
  });

  it("backs off further on a narrow window than a wide one", () => {
    const l = { shelves: [], width: 4000, height: 200 };
    expect(framing(l, 0.6).z).toBeGreaterThan(framing(l, 2.4).z);
  });

  it("looks at the middle of the library", () => {
    const l = { shelves: [], width: 1000, height: 600 };
    const f = framing(l, 16 / 9);
    expect(f.x).toBeCloseTo(1000 / SCALE / 2, 6);
    expect(f.y).toBeCloseTo(-600 / SCALE / 2, 6);
  });

  it("never sits so close that the near plane clips the shelves", () => {
    expect(framing({ shelves: [], width: 1, height: 1 }, 16 / 9).z).toBeGreaterThanOrEqual(1.2);
  });
});

describe("pointer back to layout", () => {
  it("round-trips a book's centre exactly", () => {
    for (const b of shelf.books) {
      const box = bookBox(b);
      const back = layoutPoint(box.x, box.y);
      expect(back.x).toBeCloseTo(b.x + b.w / 2, 6);
      expect(back.y).toBeCloseTo(b.y + b.h / 2, 6);
    }
  });

  /** A sign error here drops books onto the shelf above or below the cursor. */
  it("keeps y pointing the right way", () => {
    expect(layoutPoint(0, -1).y).toBeGreaterThan(layoutPoint(0, -0.5).y);
  });

  it("lands a point inside a book back inside that same book", () => {
    const b = shelf.books[1]!;
    const box = bookBox(b);
    const p = layoutPoint(box.x, box.y);
    expect(p.x).toBeGreaterThanOrEqual(b.x);
    expect(p.x).toBeLessThanOrEqual(b.x + b.w);
    expect(p.y).toBeGreaterThanOrEqual(b.y);
    expect(p.y).toBeLessThanOrEqual(b.y + b.h);
  });
});
