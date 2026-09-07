import { describe, expect, it } from "vitest";
import {
  CASE_SIDE,
  compartmentAt,
  DEFAULT_LAYOUT,
  folderOf,
  INDENT,
  layout,
  movedInto,
  spineWidth,
  stepToward,
  type Layout,
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

/** Every compartment that is a folder's, in order. */
const folders = (l: Layout) => l.bookcase.compartments.filter((c) => !c.isFloor);
/** The one compartment that is the floor of the case. */
const floor = (l: Layout) => l.bookcase.compartments.find((c) => c.isFloor)!;

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

/**
 * The four rules the whole layout exists to satisfy.
 *
 * Written as four tests named after them, because they are a specification
 * someone handed over in four lines and this is the file that says whether the
 * code still means them.
 */
describe("the four rules", () => {
  it("#1 no folders means no boards, and the notes lie on the ground", () => {
    const l = layout(
      tree([
        ["a.md", false, undefined, 400],
        ["b.md", false, undefined, 900],
      ]),
      "v",
      opts,
    );

    expect(l.bookcase.boards).toHaveLength(0);
    expect(folders(l)).toHaveLength(0);

    // Every note stands on the plinth — its foot is exactly the floor.
    const f = floor(l);
    expect(f.surfaceY).toBe(l.bookcase.floorY);
    expect(f.books).toHaveLength(2);
    for (const b of f.books) expect(b.y + b.h).toBeCloseTo(l.bookcase.floorY, 6);
  });

  it("#2 X folders means exactly X boards", () => {
    for (const x of [0, 1, 5, 40]) {
      const rows: Array<[string, boolean, number?, number?]> = [];
      for (let i = 0; i < x; i++) rows.push([`F${i}`, true]);

      const l = layout(tree(rows), "v", opts);
      expect(l.bookcase.boards, `${x} folders`).toHaveLength(x);
      expect(folders(l), `${x} folders`).toHaveLength(x);
      // The floor is always there, and is never one of them.
      expect(l.bookcase.compartments).toHaveLength(x + 1);
    }
  });

  it("#3 a folder with no notes still gets its board", () => {
    const l = layout(tree([["Empty", true]]), "v", opts);
    const empty = folders(l).find((c) => c.path === "Empty");

    expect(empty).toBeDefined();
    expect(empty!.books).toHaveLength(0);
    expect(l.bookcase.boards).toHaveLength(1);
    // And it is still somewhere a book can be dropped, which is the whole
    // reason an empty folder has to be drawn at all.
    expect(compartmentAt(l, empty!.x + 5, empty!.surfaceY - 4)?.path).toBe("Empty");
  });

  it("#4 a folder's notes all stand on that folder's board", () => {
    const l = layout(
      tree([
        ["F", true],
        ["F/a.md", false, 0, 400],
        ["F/b.md", false, 0, 4000],
        ["G", true],
        ["G/c.md", false, 3, 40],
      ]),
      "v",
      opts,
    );

    for (const c of folders(l)) {
      const board = l.bookcase.boards[l.bookcase.compartments.indexOf(c)]!;
      expect(board.y).toBeCloseTo(c.surfaceY, 6);
      for (const b of c.books) {
        // The foot of every book meets the top face of its own board.
        expect(b.y + b.h, `${b.path} on ${c.path}`).toBeCloseTo(c.surfaceY, 6);
      }
    }

    expect(folders(l).find((c) => c.path === "F")!.books.map((b) => b.path)).toEqual([
      "F/a.md",
      "F/b.md",
    ]);
    expect(folders(l).find((c) => c.path === "G")!.books.map((b) => b.path)).toEqual(["G/c.md"]);
  });
});

describe("the carcass", () => {
  it("keeps every book inside the interior", () => {
    const rows: Array<[string, boolean, number?, number?]> = [["F", true]];
    for (let i = 0; i < 40; i++) rows.push([`F/n${i}.md`, false, 0, i * 800]);

    const l = layout(tree(rows), "v", opts);
    const c = l.bookcase;

    for (const comp of c.compartments) {
      for (const b of comp.books) {
        expect(b.x).toBeGreaterThanOrEqual(c.innerX - 0.001);
        expect(b.x + b.w).toBeLessThanOrEqual(c.innerX + c.innerW + 0.001);
        expect(b.y).toBeGreaterThanOrEqual(c.innerY - 0.001);
        expect(b.y + b.h).toBeLessThanOrEqual(c.floorY + 0.001);
      }
    }
  });

  it("puts the interior inside the sides, and the plinth under it", () => {
    const l = layout(tree([["F", true]]), "v", opts);
    const c = l.bookcase;

    expect(c.innerX).toBe(c.x + CASE_SIDE);
    expect(c.innerW).toBe(c.w - CASE_SIDE * 2);
    expect(c.floorY).toBeGreaterThan(c.innerY);
    expect(c.h).toBeGreaterThan(c.floorY);
  });

  it("never overlaps two boards", () => {
    const rows: Array<[string, boolean, number?, number?]> = [];
    for (let i = 0; i < 8; i++) rows.push([`F${i}`, true]);

    const boards = layout(tree(rows), "v", opts).bookcase.boards;
    for (let i = 1; i < boards.length; i++) {
      expect(boards[i]!.y).toBeGreaterThan(boards[i - 1]!.y);
    }
  });

  /** A layout that moved on its own would make every animation a lie. */
  it("is deterministic", () => {
    const t = tree([
      ["B", true],
      ["B/two.md", false, 0, 30],
      ["A", true],
      ["A/one.md", false, 2, 10],
      ["loose.md", false, undefined, 55],
    ]);
    expect(JSON.stringify(layout(t, "v", opts))).toBe(JSON.stringify(layout(t, "v", opts)));
  });
});

describe("compression", () => {
  /**
   * The choice made deliberately: a shelf too full for its books shrinks them
   * rather than wrapping onto a second board, because a second board would mean
   * one folder had two shelves and rule #2 would stop being true.
   */
  it("fits a folder of five hundred notes onto its one board", () => {
    const rows: Array<[string, boolean, number?, number?]> = [["F", true]];
    for (let i = 0; i < 557; i++) rows.push([`F/n${i}.md`, false, 0, 2000]);

    const l = layout(tree(rows), "v", opts);
    const comp = folders(l)[0]!;

    expect(l.bookcase.boards).toHaveLength(1);
    expect(comp.books).toHaveLength(557);

    const last = comp.books[comp.books.length - 1]!;
    expect(last.x + last.w).toBeLessThanOrEqual(comp.x + comp.w + 0.001);
    // Every book is still a positive size, so nothing becomes a degenerate
    // matrix that three would refuse to draw.
    for (const b of comp.books) expect(b.w).toBeGreaterThan(0);
  });

  it("keeps a long note wider than a short one even when squeezed", () => {
    const rows: Array<[string, boolean, number?, number?]> = [["F", true]];
    for (let i = 0; i < 300; i++) rows.push([`F/stub${i}.md`, false, 0, 10]);
    rows.push(["F/essay.md", false, 0, 400_000]);

    const comp = folders(layout(tree(rows), "v", opts))[0]!;
    const essay = comp.books.find((b) => b.path === "F/essay.md")!;
    const stub = comp.books.find((b) => b.path === "F/stub0.md")!;

    // One factor for the whole shelf, so the size signal survives the squeeze.
    expect(essay.w).toBeGreaterThan(stub.w);
  });

  it("does not squeeze a shelf that fits", () => {
    const l = layout(
      tree([
        ["F", true],
        ["F/a.md", false, 0, 400],
      ]),
      "v",
      opts,
    );
    expect(folders(l)[0]!.books[0]!.w).toBe(spineWidth(400));
  });
});

describe("nesting", () => {
  it("indents a nested folder's books by one step per level", () => {
    const l = layout(
      tree([
        ["ARC", true],
        ["ARC/Deep", true, 0],
        ["ARC/Deep/Deeper", true, 1],
        ["ARC/Deep/Deeper/n.md", false, 2, 20],
      ]),
      "v",
      opts,
    );

    const arc = folders(l).find((c) => c.path === "ARC")!;
    const deep = folders(l).find((c) => c.path === "ARC/Deep")!;
    const deeper = folders(l).find((c) => c.path === "ARC/Deep/Deeper")!;

    expect(deep.x - arc.x).toBe(INDENT);
    expect(deeper.x - deep.x).toBe(INDENT);
    // A child's board sits below its parent's, which is what makes depth
    // readable without drawing a single connecting line.
    expect(deep.surfaceY).toBeGreaterThan(arc.surfaceY);
    expect(deeper.surfaceY).toBeGreaterThan(deep.surfaceY);

    // The indent comes out of the space its books have, not out of the board.
    expect(deeper.w).toBe(arc.w - INDENT * 2);
  });

  it("gives the floor the vault's own name", () => {
    const l = layout(tree([["a.md", false, undefined, 10]]), "my-vault", opts);
    expect(floor(l).label).toBe("my-vault");
    expect(floor(l).books.map((b) => b.path)).toEqual(["a.md"]);
  });

  it("can hide empty folders without hiding the floor", () => {
    const l = layout(tree([["Empty", true]]), "v", { ...opts, showEmpty: false });
    expect(l.bookcase.boards).toHaveLength(0);
    // The floor survives even with nothing on it: it is where a note with no
    // folder would land, so it has to stay a target.
    expect(floor(l)).toBeDefined();
  });
});

describe("the messy drawer", () => {
  /**
   * The one visual difference between filed and unfiled. Notes on a board stand
   * straight; notes on the floor lean, which is what makes the bottom of the
   * case read as a drawer rather than as another shelf.
   */
  it("leans notes on the floor and nothing else", () => {
    const l = layout(
      tree([
        ["F", true],
        ["F/filed.md", false, 0, 400],
        ["loose-one.md", false, undefined, 400],
        ["loose-two.md", false, undefined, 400],
      ]),
      "v",
      opts,
    );

    for (const b of folders(l)[0]!.books) expect(b.lean).toBe(0);
    expect(floor(l).books.some((b) => b.lean !== 0)).toBe(true);
  });

  it("leans the same way every time, so mess is not jitter", () => {
    const t = tree([
      ["a.md", false, undefined, 400],
      ["b.md", false, undefined, 400],
      ["c.md", false, undefined, 400],
    ]);
    const first = floor(layout(t, "v", opts)).books.map((b) => b.lean);
    const again = floor(layout(t, "v", opts)).books.map((b) => b.lean);
    expect(first).toEqual(again);
    // And it is a lean, not a topple.
    for (const lean of first) expect(Math.abs(lean)).toBeLessThan(0.2);
  });
});

describe("hit testing", () => {
  /**
   * Only compartments are hit-tested here. A *book* is picked by three's own
   * raycaster against the instanced mesh, which reports the instance it hit —
   * so a hand-written rectangle test for books would be a second answer to a
   * question already answered, and the two would drift.
   */
  const l = layout(
    tree([
      ["F", true],
      ["F/n.md", false, 0, 400],
      ["loose.md", false, undefined, 400],
    ]),
    "v",
    opts,
  );

  it("finds a folder's compartment above its board", () => {
    const f = folders(l)[0]!;
    expect(compartmentAt(l, f.x + 20, f.surfaceY - 10)?.path).toBe("F");
  });

  it("finds the floor, so a note can be moved back to the vault root", () => {
    const g = floor(l);
    expect(compartmentAt(l, g.x + 20, g.surfaceY - 10)?.isFloor).toBe(true);
  });

  it("finds nothing outside the case", () => {
    expect(compartmentAt(l, -500, l.bookcase.innerY + 10)).toBeNull();
    expect(compartmentAt(l, l.bookcase.innerX + 10, -500)).toBeNull();
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
   * which forces that token to 0 — must stop every animation dead. A frame
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

describe("edge cases", () => {
  it("survives an entirely empty vault", () => {
    const l = layout(tree([]), "v", opts);
    expect(l.bookcase.boards).toHaveLength(0);
    expect(floor(l).books).toHaveLength(0);
    expect(l.height).toBeGreaterThan(0);
    expect(l.width).toBeGreaterThan(0);
  });

  it("strips the extension for the spine but keeps the path", () => {
    const l = layout(tree([["Note name.md", false, undefined, 5]]), "v", opts);
    const b = floor(l).books[0]!;
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
    const books = floor(l).books;
    expect(books.find((b) => b.path === "b.canvas")!.isCanvas).toBe(true);
    expect(books.find((b) => b.path === "a.md")!.isCanvas).toBe(false);
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
    expect(folders(layout(t, "v", opts))[0]!.books.map((b) => b.label)).toEqual([
      "apple",
      "zebra",
    ]);
    expect(
      folders(layout(t, "v", { ...opts, sort: "size" }))[0]!.books.map((b) => b.label),
    ).toEqual(["zebra", "apple"]);
  });

  /**
   * Furniture does not shrink to fit what you put in it. Without a floor on the
   * height, a vault with no folders came out three and a half times wider than
   * it was tall — a shoe rack, not the empty bookcase the rules describe.
   */
  it("is a bookcase even with nothing in it", () => {
    const bare = layout(tree([]), "v", opts);
    expect(bare.height).toBeGreaterThan(bare.width * 0.7);

    // The extra height is headroom over the loose notes, not a stretched shelf:
    // it is the part of the case you have not put a shelf in yet.
    const f = floor(bare);
    expect(f.h).toBeGreaterThan(400);
    expect(f.surfaceY).toBe(bare.bookcase.floorY);
  });

  it("stops adding headroom once there are enough shelves to fill the case", () => {
    const rows: Array<[string, boolean, number?, number?]> = [];
    for (let i = 0; i < 8; i++) rows.push([`F${i}`, true]);

    const full = layout(tree(rows), "v", opts);
    const bare = layout(tree([]), "v", opts);

    // Eight shelves are taller than the minimum, so the floor is an ordinary
    // compartment again rather than a hall.
    expect(full.height).toBeGreaterThan(bare.height);
    expect(floor(full).h).toBeLessThan(floor(bare).h);
  });

  it("grows the case taller as folders are added", () => {
    const one = layout(tree([["A", true]]), "v", opts);
    const five = layout(
      tree([
        ["A", true],
        ["B", true],
        ["C", true],
        ["D", true],
        ["E", true],
      ]),
      "v",
      opts,
    );
    expect(five.height).toBeGreaterThan(one.height);
    // But not wider: the width comes from the fullest shelf, not the count.
    expect(five.width).toBe(one.width);
  });
});
