/**
 * Turning a vault tree into a bookshelf.
 *
 * Pure: a tree in, geometry out. No canvas, no DOM, no clock. That is what
 * makes the layout testable — every case worth checking here (a folder with one
 * note, a folder with four hundred, a folder nested five deep, a vault that is
 * entirely empty) is a value rather than something to eyeball on screen.
 *
 * ## Why the tree and not the graph
 *
 * The graph payload only knows notes. A folder you just created contains none,
 * so it appears in no graph — and an invisible folder cannot be a drop target,
 * which would make "drag a book onto the shelf you just made" impossible. The
 * tree has folders, has the hierarchy already as parent pointers, needs no
 * index, and is refreshed after every create, rename and delete.
 */

import type { TreeEntry, TreeView } from "./types";

/** One note, as a book standing on a shelf. */
export interface Book {
  path: string;
  /** Filename without its extension — what is printed on the spine. */
  label: string;
  isCanvas: boolean;
  size: number;
  /** Position of the spine's left edge, in world units. */
  x: number;
  y: number;
  w: number;
  h: number;
}

/** One folder, as a shelf. */
export interface Shelf {
  /** Vault path of the folder. `""` is the vault root. */
  path: string;
  label: string;
  depth: number;
  x: number;
  y: number;
  /** Width of the plank. Runs out to the right edge of its column. */
  w: number;
  /** Height of the whole shelf, including every wrapped row. */
  h: number;
  /**
   * The y of every plank in this bay, top row first.
   *
   * One per *row*, not one per shelf. A shelf whose books wrapped used to draw
   * a single plank at the bottom, which left every wrapped row standing on
   * nothing — books floating in mid-air, which is the one thing a bookshelf
   * must not look like.
   */
  planks: number[];
  books: Book[];
}

export interface Layout {
  shelves: Shelf[];
  /** World bounds, for fitting the view. */
  width: number;
  height: number;
}

export interface LayoutOptions {
  /** Wrap a shelf once it passes this many world units wide. */
  maxShelfWidth: number;
  /** Vertical rhythm multiplier — the density control. */
  density: number;
  sort: "name" | "size" | "recent";
  showEmpty: boolean;
  /**
   * How many bays stand side by side. `"auto"` fits the window.
   *
   * Stacking every folder in one column was the layout's real defect: a vault
   * of six folders came out roughly 220 units wide and 740 tall, so in any
   * normal window the view had to shrink it to a third of the height to fit —
   * a thin strip down the middle with the whole width empty either side. That
   * is the "still small" complaint, and no amount of camera work fixes it,
   * because the shape being framed is the wrong shape. A real bookcase puts
   * bays side by side.
   */
  columns: number | "auto";
  /** Window aspect, used only to choose `columns` when it is `"auto"`. */
  aspect: number;
}

export const DEFAULT_LAYOUT: LayoutOptions = {
  // Narrower than a window on purpose: past this a folder's books wrap into a
  // second row of the same bay, which keeps one enormous folder from setting
  // the width of every column.
  maxShelfWidth: 640,
  density: 1,
  sort: "name",
  showEmpty: true,
  columns: "auto",
  aspect: 16 / 9,
};

const INDENT = 34;
const BOOK_H = 76;
const BOOK_GAP = 3;
const ROW_GAP = 10;
const SHELF_PAD_TOP = 26; // room for the folder label above the books
const SHELF_GAP = 26;
const COLUMN_GAP = 54;
const MIN_W = 190;
/** More than this and the bays are too narrow to read whatever the window. */
const MAX_COLUMNS = 6;

/**
 * Spine width from note length.
 *
 * Compressed hard, because note sizes span four orders of magnitude and a
 * linear scale would make one essay as wide as the screen while four hundred
 * stubs shared a pixel. The fourth root keeps a 40 KB note visibly fatter than
 * a 400-byte one without either disappearing.
 *
 * A zero-byte note gets the floor rather than nothing. They exist — two of them
 * are in the Phase 0 fixture — and a book you cannot see is a note you cannot
 * click.
 */
export function spineWidth(size: number): number {
  const w = 9 + Math.pow(Math.max(size, 0), 0.25) * 2.4;
  return Math.round(Math.min(46, Math.max(9, w)));
}

/** Filename minus its extension. */
function labelOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}

function isNote(e: TreeEntry): boolean {
  return !e.is_dir && (e.name.endsWith(".md") || e.name.endsWith(".canvas"));
}

/**
 * A top-level folder and everything under it, laid out from its own origin.
 *
 * The unit the columns are packed from. A subtree is kept whole because that is
 * what makes indentation mean anything: a child shelf reads as belonging to the
 * shelf above it, and splitting a folder across a column break would put a
 * child at the top of the next column with nothing above it to be indented
 * *from*.
 */
interface Group {
  shelves: Shelf[];
  w: number;
  h: number;
}

/** Where each group ends up, and how big the case is. Arithmetic only. */
interface Packing {
  colW: number;
  at: Array<{ col: number; y: number }>;
  width: number;
  height: number;
}

function packing(groups: Group[], cols: number, density: number): Packing {
  const gap = SHELF_GAP * density;
  const colW = Math.max(MIN_W, ...groups.map((g) => g.w));
  const total = groups.reduce((a, g) => a + g.h + gap, 0) - gap;
  const target = total / cols;

  const heights = new Array<number>(cols).fill(0);
  const at: Array<{ col: number; y: number }> = [];

  let col = 0;
  for (const g of groups) {
    // Break to the next column once this one has had its share — but never on
    // an empty column, or a single group taller than the target would push
    // every later one sideways and leave a hole.
    if (col < cols - 1 && heights[col]! > 0 && heights[col]! + g.h > target) col++;
    at.push({ col, y: heights[col]! });
    heights[col] = heights[col]! + g.h + gap;
  }

  const used = heights.filter((h) => h > 0).length;
  return {
    colW,
    at,
    width: used * colW + Math.max(0, used - 1) * COLUMN_GAP,
    height: Math.max(0, Math.max(...heights) - gap),
  };
}

/**
 * How many columns best match the window.
 *
 * Scored on log-aspect so being twice as wide as the window is penalised the
 * same as being half as wide, and ties go to fewer columns — one tall bookcase
 * is easier to read than two short ones when neither fits better.
 */
function fitColumns(groups: Group[], density: number, aspect: number): number {
  let best = 1;
  let bestScore = Infinity;
  for (let c = 1; c <= Math.min(MAX_COLUMNS, groups.length); c++) {
    const p = packing(groups, c, density);
    if (p.height <= 0 || p.width <= 0) continue;
    const score = Math.abs(Math.log(p.width / p.height / Math.max(aspect, 0.05)));
    if (score < bestScore - 1e-9) {
      bestScore = score;
      best = c;
    }
  }
  return best;
}

/**
 * Lay the whole vault out as a bookcase.
 *
 * Depth-first over folders so a child shelf sits directly beneath its parent
 * and indented from it — the arrangement that makes depth legible without
 * drawing a single connecting line — and then those subtrees are packed into
 * columns so the case is roughly the shape of the window it has to fit in.
 */
export function layout(tree: TreeView, vaultName: string, opts: LayoutOptions): Layout {
  const entries = tree.entries;

  // Children of each folder, by index. `-1` stands for the vault root, which is
  // not an entry: nothing has it as a parent, so it needs its own bucket.
  const childrenOf = new Map<number, number[]>();
  for (let i = 0; i < entries.length; i++) {
    const p = entries[i]!.parent ?? -1;
    const list = childrenOf.get(p);
    if (list) list.push(i);
    else childrenOf.set(p, [i]);
  }

  const booksIn = (idx: number): TreeEntry[] => {
    const notes = (childrenOf.get(idx) ?? []).map((i) => entries[i]!).filter(isNote);
    notes.sort((a, b) => {
      if (opts.sort === "size") return b.size - a.size;
      // "recent" has no mtime on the tree, so it falls back to name rather than
      // pretending to an order it cannot compute. Honest, and it keeps the
      // control from silently doing nothing different.
      return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
    });
    return notes;
  };

  const foldersUnder = (parentIdx: number) =>
    (childrenOf.get(parentIdx) ?? [])
      .map((i) => ({ i, e: entries[i]! }))
      .filter(({ e }) => e.is_dir)
      .sort((a, b) => a.e.name.localeCompare(b.e.name, undefined, { sensitivity: "base" }));

  /** One folder as a shelf, `top` units down from its group's origin. */
  const shelfFor = (
    folderIdx: number,
    path: string,
    label: string,
    depth: number,
    top: number,
  ): Shelf | null => {
    const notes = booksIn(folderIdx);
    if (notes.length === 0 && !opts.showEmpty && depth > 0) return null;

    const x = depth * INDENT;
    const books: Book[] = [];

    let bx = 0;
    let row = 0;
    const rowH = (BOOK_H + ROW_GAP) * opts.density;
    for (const n of notes) {
      const w = spineWidth(n.size);
      if (bx > 0 && bx + w > opts.maxShelfWidth) {
        row++;
        bx = 0;
      }
      books.push({
        path: n.path,
        label: labelOf(n.name),
        isCanvas: n.name.endsWith(".canvas"),
        size: n.size,
        x: x + bx,
        y: top + SHELF_PAD_TOP + row * rowH,
        w,
        h: BOOK_H * opts.density,
      });
      bx += w + BOOK_GAP;
    }

    const rows = row + 1;
    const usedW = books.length ? Math.max(...books.map((b) => b.x + b.w)) - x : 0;
    const w = Math.max(MIN_W, usedW);
    const h = SHELF_PAD_TOP + rows * (BOOK_H * opts.density) + (rows - 1) * ROW_GAP * opts.density;

    // A plank under each row, exactly where that row's books end.
    const planks: number[] = [];
    for (let r = 0; r < rows; r++) {
      planks.push(top + SHELF_PAD_TOP + r * rowH + BOOK_H * opts.density);
    }

    return { path, label, depth, x, y: top, w, h, planks, books };
  };

  const groupFor = (
    folderIdx: number,
    path: string,
    label: string,
    depth: number,
    descend: boolean,
  ): Group => {
    const shelves: Shelf[] = [];
    let y = 0;

    const add = (idx: number, p: string, l: string, d: number) => {
      const s = shelfFor(idx, p, l, d, y);
      if (!s) return;
      shelves.push(s);
      y += s.h + SHELF_GAP * opts.density;
    };

    add(folderIdx, path, label, depth);
    if (descend) {
      const walk = (parentIdx: number, d: number) => {
        for (const { i, e } of foldersUnder(parentIdx)) {
          add(i, e.path, e.name, d);
          walk(i, d + 1);
        }
      };
      walk(folderIdx, depth + 1);
    }

    return {
      shelves,
      w: shelves.length ? Math.max(...shelves.map((s) => s.x + s.w)) : MIN_W,
      h: Math.max(0, y - SHELF_GAP * opts.density),
    };
  };

  // The vault root is its own bay — the notes that live in no folder — and each
  // top-level folder brings its whole subtree as one.
  const groups: Group[] = [groupFor(-1, "", vaultName, 0, false)];
  for (const { i, e } of foldersUnder(-1)) {
    const g = groupFor(i, e.path, e.name, 1, true);
    if (g.shelves.length) groups.push(g);
  }

  const cols =
    opts.columns === "auto"
      ? fitColumns(groups, opts.density, opts.aspect)
      : Math.max(1, Math.min(MAX_COLUMNS, Math.round(opts.columns)));
  const p = packing(groups, cols, opts.density);

  const shelves: Shelf[] = [];
  groups.forEach((g, gi) => {
    const { col, y: dy } = p.at[gi]!;
    const dx = col * (p.colW + COLUMN_GAP);
    for (const s of g.shelves) {
      shelves.push({
        ...s,
        x: s.x + dx,
        y: s.y + dy,
        // Every plank in a column runs out to the same edge. Sized to its own
        // books they end raggedly, which reads as a pile of unrelated boards
        // rather than a bookcase — and it makes a short shelf a smaller drop
        // target than a long one for no reason a person could guess.
        w: Math.max(s.w, p.colW - s.x),
        planks: s.planks.map((v) => v + dy),
        books: s.books.map((b) => ({ ...b, x: b.x + dx, y: b.y + dy })),
      });
    }
  });

  return { shelves, width: p.width, height: p.height };
}

/** The shelf a world point falls on, or `null`. Used to pick a drop target. */
export function shelfAt(l: Layout, x: number, y: number): Shelf | null {
  for (const s of l.shelves) {
    if (y >= s.y && y <= s.y + s.h && x >= s.x - 12 && x <= s.x + Math.max(s.w, MIN_W) + 12) {
      return s;
    }
  }
  return null;
}

/** Where a book is right now, as against where the layout wants it. */
export interface Live {
  x: number;
  y: number;
  /** 0 while rising onto the shelf, 1 once it has arrived. */
  spawn: number;
}

/**
 * Move one book a frame closer to where it belongs.
 *
 * Extracted from the draw loop so the accessibility gate is *testable*. CSS
 * transitions inherit `--arc-motion` for free; a canvas loop does not, and
 * "motion 0 stops every animation" is a gate rather than a preference — so it
 * needs to be something a test can assert rather than something someone
 * squints at.
 *
 * Returns whether this book is still moving, which is what tells the loop
 * whether to schedule another frame or stop.
 */
export function stepToward(live: Live, to: { x: number; y: number }, motion: number): boolean {
  if (motion <= 0) {
    live.x = to.x;
    live.y = to.y;
    live.spawn = 1;
    return false;
  }
  live.x += (to.x - live.x) * (0.18 * motion);
  live.y += (to.y - live.y) * (0.18 * motion);
  live.spawn = Math.min(1, live.spawn + (1 - live.spawn) * (0.12 * motion) + 0.01);

  if (Math.abs(to.x - live.x) < 0.3 && Math.abs(to.y - live.y) < 0.3 && live.spawn >= 1) {
    live.x = to.x;
    live.y = to.y;
    return false;
  }
  return true;
}

/** The folder a note currently sits in, from its path. */
export function folderOf(path: string): string {
  const cut = path.lastIndexOf("/");
  return cut === -1 ? "" : path.slice(0, cut);
}

/** Where a note would land if moved to `folder`. */
export function movedInto(path: string, folder: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1);
  return folder ? `${folder}/${name}` : name;
}
