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
  /** Width of the plank — as wide as the books it carries, at least `MIN_W`. */
  w: number;
  /** Height of the whole shelf, including every wrapped row. */
  h: number;
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
}

export const DEFAULT_LAYOUT: LayoutOptions = {
  maxShelfWidth: 1100,
  density: 1,
  sort: "name",
  showEmpty: true,
};

const INDENT = 34;
const BOOK_H = 76;
const BOOK_GAP = 3;
const ROW_GAP = 10;
const SHELF_PAD_TOP = 26; // room for the folder label above the books
const SHELF_GAP = 26;
const MIN_W = 190;

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
 * Lay the whole vault out as shelves.
 *
 * Depth-first over folders so a child shelf sits directly beneath its parent
 * and indented from it — the arrangement that makes depth legible without
 * drawing a single connecting line.
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

  const shelves: Shelf[] = [];
  let cursorY = 0;
  let widest = MIN_W;

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

  const emit = (folderIdx: number, path: string, label: string, depth: number) => {
    const notes = booksIn(folderIdx);
    if (notes.length === 0 && !opts.showEmpty && depth > 0) return;

    const x = depth * INDENT;
    const y = cursorY;
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
        y: y + SHELF_PAD_TOP + row * rowH,
        w,
        h: BOOK_H * opts.density,
      });
      bx += w + BOOK_GAP;
    }

    const rows = row + 1;
    const usedW = books.length
      ? Math.max(...books.map((b) => b.x + b.w)) - x
      : 0;
    const w = Math.max(MIN_W, usedW);
    const h = SHELF_PAD_TOP + rows * (BOOK_H * opts.density) + (rows - 1) * ROW_GAP * opts.density;

    shelves.push({ path, label, depth, x, y, w, h, books });
    widest = Math.max(widest, x + w);
    cursorY += h + SHELF_GAP * opts.density;
  };

  // The root shelf first: notes that live directly in the vault.
  emit(-1, "", vaultName, 0);

  const walk = (parentIdx: number, depth: number) => {
    const kids = (childrenOf.get(parentIdx) ?? [])
      .map((i) => ({ i, e: entries[i]! }))
      .filter(({ e }) => e.is_dir)
      .sort((a, b) => a.e.name.localeCompare(b.e.name, undefined, { sensitivity: "base" }));

    for (const { i, e } of kids) {
      emit(i, e.path, e.name, depth);
      walk(i, depth + 1);
    }
  };
  walk(-1, 1);

  return {
    shelves,
    width: widest,
    height: Math.max(0, cursorY - SHELF_GAP * opts.density),
  };
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

/** The book at a world point, or `null`. */
export function bookAt(l: Layout, x: number, y: number): Book | null {
  for (const s of l.shelves) {
    if (y < s.y || y > s.y + s.h) continue;
    for (const b of s.books) {
      if (x >= b.x && x <= b.x + b.w && y >= b.y && y <= b.y + b.h) return b;
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
