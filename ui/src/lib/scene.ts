/**
 * The bridge between the flat layout and the 3D scene.
 *
 * `shelves.ts` lays the bookcase out in screen-like units with y increasing
 * *downward*, because that is how a 2D layout reads and how it is tested.
 * Three.js has y increasing *upward* and works in world units. Everything that
 * converts between the two lives here rather than being scattered through the
 * scene, so there is exactly one place where a sign can be wrong.
 *
 * Kept pure and separate from the component for the same reason the layout is:
 * a camera that frames the wrong thing, or a book that sits through its board
 * instead of on it, is a value that can be asserted rather than a picture
 * someone has to squint at.
 */

import type { Board, Book, Bookcase, Compartment, Layout } from "./shelves";
import { BOOK_H, CASE_SIDE, CASE_TOP, BOARD_T, OVERHANG, PLINTH } from "./shelves";

/** Layout units per world unit. A book ends up roughly 0.7 units tall. */
export const SCALE = 100;

/**
 * How deep the carcass is, and how deep a book is — both measured in books.
 *
 * Guessing multipliers got this wrong twice. 0.34 was a facade; 1.5× that was
 * 0.51, still only two-thirds of a book-height deep, and still read flat. The
 * mistake was scaling a number rather than asking what the number should *be*.
 *
 * A book is the one object here whose real size everyone knows. A hardback is
 * roughly 24 cm tall and 16 cm front to back, and it stands on a shelf about
 * 30 cm deep — so a shelf is about 1.25 book-heights deep and a book about
 * 0.7. Those two ratios are the whole of it, and they are what these are now,
 * which is why the case finally has a side worth lighting.
 *
 * That the case is a *tenth* as deep as it is wide is not a contradiction: at
 * this scale it is a two-metre wall unit, and a two-metre bookcase is still
 * only thirty centimetres deep.
 */
const BOOK_HEIGHT = BOOK_H / SCALE;
export const CASE_DEPTH = BOOK_HEIGHT * 1.25;
/** The back panel, which is thin and is what stops you reaching in from behind. */
export const BACK_T = 0.022;
/** How far a book sticks out. Less than the case, so books sit *inside* it. */
export const BOOK_DEPTH = BOOK_HEIGHT * 0.7;
/** A book's front face sits this far behind the case's front edge. */
const RECESS = 0.03;

/** The z every book — and the plane a dragged book slides along — lives at. */
export const BOOK_Z = CASE_DEPTH / 2 - RECESS - BOOK_DEPTH / 2;

/**
 * Everything but the back panel stops short of the rear, so the back reads as a
 * separate panel let into the carcass rather than the sides being solid to the
 * wall. Cabinetry does the same thing, for the same reason.
 */
const BODY_DEPTH = CASE_DEPTH - BACK_T;
const BODY_Z = BACK_T / 2;

/**
 * A label drawn over the canvas rather than inside it, in screen pixels.
 *
 * Text is DOM here because the app's CSP refuses the blob-URL worker that
 * troika — and therefore `@threlte/extras`'s `<Text>` — needs to build a font
 * atlas. See the note at the top of `LibraryScene.svelte`. The projection is in
 * the scene, where the camera is; the shape it produces is here, next to the
 * geometry it is derived from.
 */
export interface ScreenLabel {
  key: string;
  text: string;
  /** A second, dimmer part — the note count on a shelf. */
  sub?: string;
  kind: "shelf" | "shelf-target" | "spine";
  x: number;
  y: number;
  /**
   * Room available along the text's own direction, in pixels: the board's width
   * for a shelf, the book's on-screen length for a spine. Perspective means
   * that changes as you move, so it is measured per frame rather than assumed.
   */
  len: number;
  /**
   * Degrees to rotate the text, clockwise from horizontal.
   *
   * Zero for a shelf name. For a spine it follows the book's *projected* long
   * axis, which is only straight up the screen when you are looking at the case
   * head-on — the whole point of being able to walk around it is that usually
   * you are not.
   */
  angle: number;
  /**
   * Type size in pixels, for a spine only.
   *
   * Off the type scale on purpose, and the one place in the app that is. A
   * spine's width on screen changes continuously with distance and angle, and a
   * six-step scale cannot follow that: pick a fixed size and the name either
   * overflows a thin book onto its neighbours or rattles around inside a fat
   * one. Sizing the type to the spine is also what lets a *thin* book be named
   * at all — the version before this simply refused to draw a title narrower
   * than 11px, so a zero-byte note, which gets the minimum spine width, never
   * had a name.
   */
  size?: number;
}

export interface Box {
  x: number;
  y: number;
  z: number;
  w: number;
  h: number;
  d: number;
}

/** A layout rectangle as a world-space box at a given depth. */
function box(x: number, y: number, w: number, h: number, z: number, d: number): Box {
  return {
    x: (x + w / 2) / SCALE,
    y: -(y + h / 2) / SCALE,
    z,
    w: w / SCALE,
    h: h / SCALE,
    d,
  };
}

/**
 * The carcass: two sides, a top, a back and a plinth.
 *
 * This is what the previous version did not have. It drew a plank per folder
 * with a stub upright at the left end of each — every board its own object,
 * nothing enclosing anything. A bookcase is a box first; the boards are what
 * goes *in* it.
 */
export function caseParts(c: Bookcase): {
  left: Box;
  right: Box;
  top: Box;
  back: Box;
  plinth: Box;
} {
  return {
    left: box(c.x, CASE_TOP, CASE_SIDE, c.floorY - CASE_TOP, BODY_Z, BODY_DEPTH),
    right: box(
      c.x + c.w - CASE_SIDE,
      CASE_TOP,
      CASE_SIDE,
      c.floorY - CASE_TOP,
      BODY_Z,
      BODY_DEPTH,
    ),
    // Wider than the case and a touch deeper: the lip that reads as a cap
    // rather than as the top of a box.
    top: box(c.x - OVERHANG, c.y, c.w + OVERHANG * 2, CASE_TOP, BODY_Z + 0.008, BODY_DEPTH + 0.02),
    back: box(c.x, c.y, c.w, c.h, -CASE_DEPTH / 2 + BACK_T / 2, BACK_T),
    plinth: box(c.x, c.floorY, c.w, PLINTH, BODY_Z, BODY_DEPTH),
  };
}

/** One interior board, spanning the full interior width. */
export function boardBox(b: Board): Box {
  return box(b.x, b.y, b.w, BOARD_T, BODY_Z, BODY_DEPTH);
}

/**
 * Where a book's *centre* sits in the world, and how far it leans.
 *
 * Three.js positions a box by its centre while the layout positions a book by
 * its top-left corner, so both axes shift by half the size and y flips. A
 * leaning book adds one more thing to get wrong: it pivots about its *foot*,
 * not its centre, so the centre swings out along the tilted axis. Every one of
 * those is a chance to put a book half a shelf out of place, which is why this
 * is one function with a test rather than arithmetic inlined at three call
 * sites.
 */
export function bookPose(b: Book): { x: number; y: number; z: number; w: number; h: number; d: number; lean: number } {
  const footX = (b.x + b.w / 2) / SCALE;
  const footY = -(b.y + b.h) / SCALE;
  const h = b.h / SCALE;

  // The centre is half a book up the book's own axis, which the lean rotates.
  return {
    x: footX - Math.sin(b.lean) * (h / 2),
    y: footY + Math.cos(b.lean) * (h / 2),
    z: BOOK_Z,
    w: b.w / SCALE,
    h,
    d: BOOK_DEPTH,
    lean: b.lean,
  };
}

/**
 * The two ends of a book's spine, in world space: where it meets its surface
 * and where its head is.
 *
 * The spine label is drawn along the line between them, so it stays on the book
 * at any camera angle and at any lean.
 */
export function spineEnds(b: Book): { foot: [number, number, number]; head: [number, number, number] } {
  const footX = (b.x + b.w / 2) / SCALE;
  const footY = -(b.y + b.h) / SCALE;
  const h = b.h / SCALE;
  const z = BOOK_Z + BOOK_DEPTH / 2;
  return {
    foot: [footX, footY, z],
    head: [footX - Math.sin(b.lean) * h, footY + Math.cos(b.lean) * h, z],
  };
}

/**
 * Where a compartment's name goes: on the front edge of its own surface, at the
 * left end.
 *
 * On its *own* surface, which is the fix for the thing the flat version got
 * wrong — it put the folder name at the top of its bay while the board was at
 * the bottom, so every name sat directly under the previous shelf's board and
 * read as a caption for the books above it rather than a heading for the ones
 * below. On a real shelf the label is on the board the books stand on.
 */
export function labelPoint(c: Compartment): { x: number; y: number; z: number } {
  return {
    x: c.x / SCALE,
    y: -c.surfaceY / SCALE - 0.012,
    z: CASE_DEPTH / 2,
  };
}

/**
 * A world point, back in layout coordinates.
 *
 * The inverse of what `bookPose` does to a position, and the reason drag works:
 * the pointer is raycast onto the wall plane, and the hit has to become an
 * `{x, y}` the layout's own `compartmentAt` understands. Round-trips exactly,
 * which is asserted — a sign error here would mean books dropping onto the
 * shelf above or below the one under the cursor.
 *
 * Unaffected by rotating the view: the layout plane is fixed in the world and
 * the camera is what moves, so this stays correct from any angle.
 */
export function layoutPoint(worldX: number, worldY: number): { x: number; y: number } {
  return { x: worldX * SCALE, y: -worldY * SCALE };
}

/**
 * A camera position that frames the whole bookcase.
 *
 * Fits the taller of the two axes against the viewport, so a wide squat case
 * and a tall narrow one both arrive on screen whole rather than one of them
 * starting off the edge.
 */
export function framing(
  l: Layout,
  aspect: number,
  fovDeg = 42,
): { x: number; y: number; z: number } {
  const w = Math.max(l.width, 1) / SCALE;
  const h = Math.max(l.height, 1) / SCALE;
  const fov = (fovDeg * Math.PI) / 180;

  // Distance needed for the height, and for the width at this aspect.
  const forH = h / 2 / Math.tan(fov / 2);
  const forW = w / 2 / Math.tan(fov / 2) / Math.max(aspect, 0.2);

  return {
    x: w / 2,
    y: -h / 2,
    /*
      Margin, and never so close that the near plane clips the case.

      A fifth rather than a tenth: the shelf names hang below their boards and
      the HUD chips sit in the corners, so a case fitted tight to the window
      puts the bottom folder's name underneath the legend. The margin is cheaper
      than arranging the two never to meet, and the case reads better with air
      around it anyway.
    */
    z: Math.max(1.2, Math.max(forH, forW) * 1.2),
  };
}
