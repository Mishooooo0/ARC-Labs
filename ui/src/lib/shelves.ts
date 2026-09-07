/**
 * Turning a vault tree into a bookcase.
 *
 * Pure: a tree in, geometry out. No canvas, no DOM, no clock. That is what
 * makes the layout testable — every case worth checking here (a vault with no
 * folders at all, a folder with four hundred notes, a folder nested five deep,
 * a vault that is entirely empty) is a value rather than something to eyeball.
 *
 * ## The four rules, and why they need no special cases
 *
 * | | |
 * |---|---|
 * | bookcase + no folders | no boards; notes lie on the ground |
 * | bookcase + X folders | X boards |
 * | board + 0 notes | an empty board |
 * | board + X notes | that board holds those X notes |
 *
 * All four fall out of one arrangement: **a folder's board is the surface its
 * notes stand on, and the plinth is the surface the unfiled notes stand on.**
 * X folders therefore need exactly X boards — the vault root needs none,
 * because a bookcase already has a floor. "No folders means no boards" is then
 * the absence of a loop iteration rather than a branch, which is the reason to
 * build it this way round.
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

/** One note, as a book standing on a surface. */
export interface Book {
  path: string;
  /** Filename without its extension — what is printed on the spine. */
  label: string;
  isCanvas: boolean;
  size: number;
  /** Left edge of the spine, in layout units. */
  x: number;
  /** Top edge. The book's foot is `y + h`, and that rests on its surface. */
  y: number;
  w: number;
  h: number;
  /**
   * Tilt, in radians, about the book's foot.
   *
   * Zero for anything on a board. Non-zero only for unfiled notes lying in the
   * bottom of the case, which is the whole visual difference between filed and
   * unfiled — a shelf is tidy, the drawer is not.
   */
  lean: number;
}

/**
 * One storage surface: a folder's board, or the plinth for unfiled notes.
 *
 * The unit the whole layout is built from, and the reason the rules hold. A
 * folder gets one; the vault root gets one for free, because the case has a
 * floor.
 */
export interface Compartment {
  /** Vault path of the folder. `""` is the vault root, on the floor. */
  path: string;
  label: string;
  /** Nesting depth. `0` for the floor and for top-level folders. */
  depth: number;
  isFloor: boolean;
  /** The surface books stand on: the top face of the board, or of the plinth. */
  surfaceY: number;
  /** Where this compartment's books may go, after the indent for its depth. */
  x: number;
  w: number;
  /** Clear height above the surface, up to whatever is over it. */
  h: number;
  books: Book[];
}

/** One interior board. There is exactly one per folder. */
export interface Board {
  /** The top face — the surface its folder's books stand on. */
  y: number;
  x: number;
  w: number;
}

/** The piece of furniture. */
export interface Bookcase {
  x: number;
  y: number;
  w: number;
  h: number;
  /** The interior box: inside the sides, below the top, above the plinth. */
  innerX: number;
  innerY: number;
  innerW: number;
  innerH: number;
  /** The top face of the plinth — the ground unfiled notes stand on. */
  floorY: number;
  boards: Board[];
  compartments: Compartment[];
}

export interface Layout {
  bookcase: Bookcase;
  /** World bounds, for fitting the view. Includes the top's overhang. */
  width: number;
  height: number;
}

export interface LayoutOptions {
  /** Vertical rhythm multiplier — the density control. */
  density: number;
  sort: "name" | "size" | "recent";
  showEmpty: boolean;
}

export const DEFAULT_LAYOUT: LayoutOptions = {
  density: 1,
  sort: "name",
  showEmpty: true,
};

// ── The furniture, in layout units ──────────────────────────────────────────
/** How far a nested folder's books and name step in from the interior edge. */
export const INDENT = 34;
/**
 * A book's height, and the yardstick for the case's depth.
 *
 * Books are the one object here whose real-world size everybody knows, so the
 * carcass is proportioned against them rather than against itself.
 */
export const BOOK_H = 76;
const BOOK_GAP = 3;
/** Clear height of one compartment. Books take `BOOK_H`; the rest is headroom. */
const BAY_H = 110;

export const CASE_SIDE = 20;
export const CASE_TOP = 22;
export const PLINTH = 52;
export const BOARD_T = 14;
/** The top panel is wider than the case, as on the reference. */
export const OVERHANG = 7;

/**
 * The interior widens to hold the fullest folder before anything is squeezed,
 * within these bounds. The lower one is what keeps a five-note vault looking
 * like a bookcase rather than a plank; the upper one is what stops a
 * five-hundred-note folder turning it into a wall you have to pan along.
 */
const MIN_INNER = 620;
const MAX_INNER = 1400;

/**
 * A bookcase is a piece of furniture before it is a container, and furniture
 * does not shrink to fit what you put in it.
 *
 * Without this, a vault with no folders came out 674 wide and 184 tall — a shoe
 * rack with some notes in it, not the empty bookcase the rules describe. The
 * case is now always at least this many bays tall, and the shortfall goes to
 * the floor compartment: the empty space *above* the loose notes is the part of
 * the case you have not put a shelf in yet, which is exactly what it is.
 */
const MIN_BAYS = 5;

/** Small enough to be a hairline, large enough not to be a degenerate matrix. */
const MIN_SPINE = 0.02;

/** How far an unfiled note may lean, in radians. About six degrees. */
const MAX_LEAN = 0.105;

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
 *
 * This is a book's *natural* width. A shelf too full for its books scales every
 * one of them by a single factor, so the relative sizes survive.
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
 * FNV-1a over a note's path.
 *
 * Everything derived from a note's identity goes through this, so it is the
 * same every time the same vault is drawn. A lean that reshuffled on every
 * keystroke that refreshes the tree would read as a bug rather than as mess,
 * and a book that changed colour when you renamed its neighbour would be worse.
 * Six lines, and no dependency.
 */
function hashOf(path: string): number {
  let h = 2166136261;
  for (let i = 0; i < path.length; i++) {
    h ^= path.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

/** A stable number in −1..1 from a note's path. */
function jitter(path: string): number {
  return (hashOf(path) % 2001) / 1000 - 1;
}

/**
 * Which of `n` book cloths this note is bound in.
 *
 * By path, so a note keeps its colour for as long as it keeps its name — and
 * gets a new one when it moves, which is honest: it is a different note on a
 * different shelf.
 */
export function spineOf(path: string, n: number): number {
  return n <= 0 ? 0 : hashOf(path) % n;
}

/**
 * One frame of a 0→1 arrival.
 *
 * Shared by the two things that arrive — a board sliding into the case and a
 * book being set down on it — so there is one place the motion gate is obeyed
 * rather than two that can drift apart. `motion` 0 lands it immediately, which
 * is the accessibility gate and is why this is a function a test can call
 * instead of arithmetic buried in a draw loop.
 */
export function easeIn(progress: number, motion: number): number {
  if (motion <= 0) return 1;
  // The whole step scales, the constant nudge included. Written as
  // `… * (0.16 * motion) + 0.015` the nudge ignored the multiplier, so halving
  // motion barely slowed anything down: only 0 did anything, and every value
  // between was decoration. A multiplier that only means something at one end
  // is not a multiplier.
  const next = progress + ((1 - progress) * 0.16 + 0.015) * motion;
  return next >= 0.999 ? 1 : next;
}

/** What a compartment will be built from, before any geometry exists. */
interface Plan {
  path: string;
  label: string;
  depth: number;
  isFloor: boolean;
  notes: TreeEntry[];
  /** The width these books want, at their natural spine widths. */
  natural: number;
}

function naturalWidth(notes: TreeEntry[]): number {
  if (notes.length === 0) return 0;
  let w = 0;
  for (const n of notes) w += spineWidth(n.size) + BOOK_GAP;
  return w - BOOK_GAP;
}

/**
 * Lay the whole vault out as one bookcase.
 *
 * Depth-first over folders, so a child's board sits directly beneath its
 * parent's and its books step in one indent — the arrangement that makes depth
 * legible without drawing a single connecting line. The floor comes last,
 * because unfiled notes belong at the bottom of the case.
 *
 * One case, always. Splitting a tall vault across two would mean X folders
 * stopped being X boards — every extra case brings its own floor — and the
 * rules are the point.
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

  const notesIn = (idx: number): TreeEntry[] => {
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

  // ── One plan per folder, then the floor ───────────────────────────────────
  const plans: Plan[] = [];

  const walk = (parentIdx: number, depth: number) => {
    for (const { i, e } of foldersUnder(parentIdx)) {
      const notes = notesIn(i);
      if (notes.length > 0 || opts.showEmpty) {
        plans.push({
          path: e.path,
          label: e.name,
          depth,
          isFloor: false,
          notes,
          natural: naturalWidth(notes),
        });
      }
      walk(i, depth + 1);
    }
  };
  walk(-1, 0);

  const loose = notesIn(-1);
  plans.push({
    path: "",
    label: vaultName,
    depth: 0,
    isFloor: true,
    notes: loose,
    natural: naturalWidth(loose),
  });

  // ── Size the case ─────────────────────────────────────────────────────────
  // Wide enough for the fullest folder if it can be, so nothing is squeezed
  // that does not have to be.
  const wanted = Math.max(0, ...plans.map((p) => p.depth * INDENT + p.natural));
  const innerW = Math.round(Math.min(MAX_INNER, Math.max(MIN_INNER, wanted)));

  const bayH = BAY_H * opts.density;
  const bookH = BOOK_H * opts.density;
  const caseX = OVERHANG;
  const innerX = caseX + CASE_SIDE;

  // How much taller the case has to be than its contents need, all of which
  // becomes headroom over the loose notes at the bottom.
  const natural = plans.length * bayH + (plans.length - 1) * BOARD_T;
  const smallest = MIN_BAYS * bayH + (MIN_BAYS - 1) * BOARD_T;
  const headroom = Math.max(0, smallest - natural);

  // ── Fill it, top down ─────────────────────────────────────────────────────
  const compartments: Compartment[] = [];
  const boards: Board[] = [];
  let y = CASE_TOP;

  for (const p of plans) {
    const indent = p.depth * INDENT;
    const x = innerX + indent;
    const w = innerW - indent;
    const h = p.isFloor ? bayH + headroom : bayH;
    const surfaceY = y + h;

    // Every book fits. A shelf whose books want more room than it has scales
    // all of them by one factor, so a long note stays visibly wider than a stub
    // — the size signal survives the squeeze, which it would not if each book
    // were clamped on its own.
    const scale = p.natural > w && p.natural > 0 ? w / p.natural : 1;

    const books: Book[] = [];
    let bx = 0;
    for (const n of p.notes) {
      const bw = Math.max(MIN_SPINE, spineWidth(n.size) * scale);
      books.push({
        path: n.path,
        label: labelOf(n.name),
        isCanvas: n.name.endsWith(".canvas"),
        size: n.size,
        x: x + bx,
        y: surfaceY - bookH,
        w: bw,
        h: bookH,
        lean: p.isFloor ? jitter(n.path) * MAX_LEAN : 0,
      });
      bx += bw + BOOK_GAP * scale;
    }

    compartments.push({
      path: p.path,
      label: p.label,
      depth: p.depth,
      isFloor: p.isFloor,
      surfaceY,
      x,
      w,
      h,
      books,
    });

    if (p.isFloor) {
      // The plinth is the surface, and the case ends below it.
      y = surfaceY;
    } else {
      boards.push({ y: surfaceY, x: innerX, w: innerW });
      y = surfaceY + BOARD_T;
    }
  }

  const floorY = y;
  const caseW = CASE_SIDE * 2 + innerW;
  const caseH = floorY + PLINTH;

  return {
    bookcase: {
      x: caseX,
      y: 0,
      w: caseW,
      h: caseH,
      innerX,
      innerY: CASE_TOP,
      innerW,
      innerH: floorY - CASE_TOP,
      floorY,
      boards,
      compartments,
    },
    width: caseW + OVERHANG * 2,
    height: caseH,
  };
}

/**
 * The compartment a point falls in, or `null`. Used to pick a drop target.
 *
 * Spans the full interior width rather than only where the books are, so an
 * empty shelf is as easy to drop onto as a full one — and the floor is a target
 * too, which is how a note gets moved back out to the vault root.
 */
export function compartmentAt(l: Layout, x: number, y: number): Compartment | null {
  const c = l.bookcase;
  if (x < c.innerX - 8 || x > c.innerX + c.innerW + 8) return null;
  for (const comp of c.compartments) {
    // From the surface up through the clear height above it. Deliberately not
    // the indented span: dropping to the left of an indented shelf still means
    // that shelf, because nothing else is there.
    if (y <= comp.surfaceY && y >= comp.surfaceY - comp.h) return comp;
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
 * transitions inherit `--arc-motion` for free; a frame loop does not, and
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
  // Scaled whole, for the reason in `easeIn`: a nudge outside the multiplier
  // makes every setting between 0 and 1 behave almost the same.
  live.spawn = Math.min(1, live.spawn + ((1 - live.spawn) * 0.12 + 0.01) * motion);

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
