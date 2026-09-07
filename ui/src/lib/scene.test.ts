import { describe, expect, it } from "vitest";
import { DEFAULT_LAYOUT, layout, type Layout } from "./shelves";
import type { TreeEntry, TreeView } from "./types";
import {
  boardBox,
  bookPose,
  BOOK_DEPTH,
  BOOK_Z,
  CASE_DEPTH,
  caseParts,
  framing,
  labelPoint,
  layoutPoint,
  SCALE,
  spineEnds,
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
    ["loose.md", false, undefined, 900],
  ]),
  "v",
  opts,
);

const folders = (l: Layout) => l.bookcase.compartments.filter((c) => !c.isFloor);
const shelf = folders(model)[0]!;
const drawer = model.bookcase.compartments.find((c) => c.isFloor)!;

/** A layout stub for the framing tests, which only read the outer bounds. */
const bounds = (width: number, height: number): Layout =>
  ({ width, height }) as unknown as Layout;

describe("placing a book in the world", () => {
  it("centres it, because three.js positions a box by its middle", () => {
    const b = shelf.books[0]!;
    const pose = bookPose(b);
    expect(pose.x).toBeCloseTo((b.x + b.w / 2) / SCALE, 6);
    expect(pose.w).toBeCloseTo(b.w / SCALE, 6);
  });

  it("flips y, because the layout counts downward and the world counts up", () => {
    expect(bookPose(shelf.books[0]!).y).toBeLessThan(0);
  });

  /**
   * The one that matters: a book stands *on* its surface. Half a shelf out and
   * the books hang in the air or sink through the wood, which is exactly what a
   * bookcase must never look like.
   */
  it("stands every upright book exactly on its own surface", () => {
    for (const c of folders(model)) {
      for (const b of c.books) {
        const pose = bookPose(b);
        const foot = pose.y - pose.h / 2;
        expect(foot).toBeCloseTo(-c.surfaceY / SCALE, 6);
      }
    }
  });

  /**
   * A leaning book pivots about its *foot*, not its centre. Get that wrong and
   * unfiled notes sink into the plinth or hover above it — which would read as
   * a bug rather than as mess.
   */
  it("keeps a leaning book's foot on the floor", () => {
    const leaning = drawer.books.find((b) => b.lean !== 0);
    expect(leaning, "the drawer should have at least one leaning note").toBeDefined();

    const b = leaning!;
    const pose = bookPose(b);
    // Undo the tilt about the centre to recover where the foot ended up.
    const footX = pose.x + Math.sin(b.lean) * (pose.h / 2);
    const footY = pose.y - Math.cos(b.lean) * (pose.h / 2);

    expect(footY).toBeCloseTo(-(b.y + b.h) / SCALE, 6);
    expect(footX).toBeCloseTo((b.x + b.w / 2) / SCALE, 6);
  });

  it("puts books inside the case, not proud of it", () => {
    const pose = bookPose(shelf.books[0]!);
    expect(pose.z + pose.d / 2).toBeLessThanOrEqual(CASE_DEPTH / 2);
    expect(pose.z).toBe(BOOK_Z);
    expect(pose.d).toBe(BOOK_DEPTH);
  });
});

describe("the carcass in world space", () => {
  const parts = caseParts(model.bookcase);

  it("stands the sides either side of the interior", () => {
    const innerLeft = model.bookcase.innerX / SCALE;
    const innerRight = (model.bookcase.innerX + model.bookcase.innerW) / SCALE;

    expect(parts.left.x + parts.left.w / 2).toBeCloseTo(innerLeft, 6);
    expect(parts.right.x - parts.right.w / 2).toBeCloseTo(innerRight, 6);
  });

  it("overhangs the top, as the reference does", () => {
    expect(parts.top.w).toBeGreaterThan(model.bookcase.w / SCALE);
  });

  /**
   * The back panel is not decoration. Once the view can be rotated the whole way
   * round it is the thing that stops you picking a book through the back of the
   * case, and it is what the DOM labels' front-facing check exists to agree
   * with.
   */
  it("closes the back, behind every book", () => {
    expect(parts.back.z).toBeLessThan(BOOK_Z);
    expect(parts.back.z - parts.back.d / 2).toBeCloseTo(-CASE_DEPTH / 2, 6);
  });

  it("puts the plinth's top face exactly where the loose notes stand", () => {
    const top = parts.plinth.y + parts.plinth.h / 2;
    expect(top).toBeCloseTo(-model.bookcase.floorY / SCALE, 6);
    for (const b of drawer.books) {
      const pose = bookPose(b);
      const footY = pose.y - Math.cos(b.lean) * (pose.h / 2);
      expect(footY).toBeCloseTo(top, 6);
    }
  });

  it("hangs each board's top face where its folder's books stand", () => {
    model.bookcase.boards.forEach((board, i) => {
      const comp = folders(model)[i]!;
      const box = boardBox(board);
      expect(box.y + box.h / 2).toBeCloseTo(-comp.surfaceY / SCALE, 6);
      expect(box.h).toBeGreaterThan(0);
    });
  });

  it("labels a compartment on the front edge of its own surface", () => {
    const label = labelPoint(shelf);
    expect(label.x).toBeCloseTo(shelf.x / SCALE, 6);
    // Just below the board's top face, and at the front of the case where it
    // can actually be read.
    expect(label.y).toBeLessThan(-shelf.surfaceY / SCALE + 1e-9);
    expect(label.z).toBeCloseTo(CASE_DEPTH / 2, 6);
  });
});

describe("the spine a name is written along", () => {
  it("runs from the book's foot to its head", () => {
    const b = shelf.books[0]!;
    const { foot, head } = spineEnds(b);
    expect(head[1]).toBeGreaterThan(foot[1]);
    expect(Math.hypot(head[0] - foot[0], head[1] - foot[1])).toBeCloseTo(b.h / SCALE, 6);
  });

  it("tilts with a leaning book, so the name stays on the spine", () => {
    const leaning = drawer.books.find((b) => b.lean !== 0)!;
    const { foot, head } = spineEnds(leaning);
    // Not vertical: the head is offset sideways from the foot.
    expect(Math.abs(head[0] - foot[0])).toBeGreaterThan(0);
  });

  it("sits on the front face, so the name is not inside the wood", () => {
    const { foot } = spineEnds(shelf.books[0]!);
    expect(foot[2]).toBeCloseTo(BOOK_Z + BOOK_DEPTH / 2, 6);
  });
});

describe("framing the bookcase", () => {
  it("backs off far enough to hold a wide case", () => {
    expect(framing(bounds(4000, 200), 16 / 9).z).toBeGreaterThan(
      framing(bounds(200, 200), 16 / 9).z,
    );
  });

  it("backs off further on a narrow window than a wide one", () => {
    const l = bounds(4000, 200);
    expect(framing(l, 0.6).z).toBeGreaterThan(framing(l, 2.4).z);
  });

  it("looks at the middle of the case", () => {
    const f = framing(bounds(1000, 600), 16 / 9);
    expect(f.x).toBeCloseTo(1000 / SCALE / 2, 6);
    expect(f.y).toBeCloseTo(-600 / SCALE / 2, 6);
  });

  it("never sits so close that the near plane clips the case", () => {
    expect(framing(bounds(1, 1), 16 / 9).z).toBeGreaterThanOrEqual(1.2);
  });
});

describe("pointer back to layout", () => {
  it("round-trips a book's centre exactly", () => {
    for (const b of shelf.books) {
      const pose = bookPose(b);
      const back = layoutPoint(pose.x, pose.y);
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
    const pose = bookPose(b);
    const p = layoutPoint(pose.x, pose.y);
    expect(p.x).toBeGreaterThanOrEqual(b.x);
    expect(p.x).toBeLessThanOrEqual(b.x + b.w);
    expect(p.y).toBeGreaterThanOrEqual(b.y);
    expect(p.y).toBeLessThanOrEqual(b.y + b.h);
  });
});
