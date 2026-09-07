/**
 * The bridge between the flat layout and the 3D scene.
 *
 * `shelves.ts` lays the library out in screen-like units with y increasing
 * *downward*, because that is how a 2D layout reads and how it is tested.
 * Three.js has y increasing *upward* and works in world units. Everything that
 * converts between the two lives here rather than being scattered through the
 * scene, so there is exactly one place where a sign can be wrong.
 *
 * Kept pure and separate from the component for the same reason the layout is:
 * a camera that frames the wrong thing, or a book that sits through its plank
 * instead of on it, is a value that can be asserted rather than a picture
 * someone has to squint at.
 */

import type { Book, Layout, Shelf } from "./shelves";

/** Layout units per world unit. A book ends up roughly 0.7 units tall. */
export const SCALE = 100;

/** How far a book sticks out of the shelf. Constant: depth is not data. */
export const BOOK_DEPTH = 0.16;

/** The plank, in world units. */
export const PLANK_DEPTH = BOOK_DEPTH * 1.55;
export const PLANK_THICK = 0.022;

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
   * Room available along the text's own direction, in pixels: the plank's width
   * for a shelf, the book's on-screen height for a spine. Perspective means that
   * changes as you move, so it is measured per frame rather than assumed.
   */
  len: number;
}

export interface Box {
  x: number;
  y: number;
  z: number;
  w: number;
  h: number;
  d: number;
}

/**
 * Where a book's *centre* sits in the world.
 *
 * Three.js positions a box by its centre, while the layout positions a book by
 * its top-left corner — so both axes shift by half the size, and y flips. Every
 * one of those is a chance to put a book half a shelf out of place, which is
 * why this is one function with a test rather than arithmetic inlined at three
 * call sites.
 */
export function bookBox(b: Book): Box {
  return {
    x: (b.x + b.w / 2) / SCALE,
    y: -(b.y + b.h / 2) / SCALE,
    z: 0,
    w: b.w / SCALE,
    h: b.h / SCALE,
    d: BOOK_DEPTH,
  };
}

/** Where a plank sits: directly under the row of books standing on it. */
export function plankBox(shelf: Shelf, plankY: number): Box {
  const w = Math.max(shelf.w, 40) / SCALE;
  return {
    x: (shelf.x + Math.max(shelf.w, 40) / 2) / SCALE,
    y: -plankY / SCALE - PLANK_THICK / 2,
    z: 0,
    w,
    h: PLANK_THICK,
    d: PLANK_DEPTH,
  };
}

/** The upright at the left end of a bay, which is what makes it read as a bay. */
export function endBox(shelf: Shelf): Box {
  const top = -shelf.y / SCALE;
  const bottom = -(shelf.y + shelf.h) / SCALE - PLANK_THICK;
  return {
    x: shelf.x / SCALE - PLANK_THICK,
    y: (top + bottom) / 2,
    z: 0,
    w: PLANK_THICK * 1.6,
    h: Math.abs(top - bottom),
    d: PLANK_DEPTH,
  };
}

/**
 * Where a shelf's label goes: on its own plank, at the left end.
 *
 * The flat version put the folder name at the *top* of its bay while the plank
 * was at the bottom, so every label sat directly under the previous shelf's
 * plank and read as a caption for the books above it rather than a heading for
 * the ones below. On a real shelf the label is on the plank the books stand on.
 */
export function labelPoint(shelf: Shelf): { x: number; y: number; z: number } {
  const lowest = shelf.planks[shelf.planks.length - 1] ?? shelf.y + shelf.h;
  return {
    x: shelf.x / SCALE,
    y: -lowest / SCALE - PLANK_THICK - 0.055,
    z: PLANK_DEPTH / 2,
  };
}

/**
 * A world point, back in layout coordinates.
 *
 * The inverse of what `bookBox` does to a position, and the reason drag works:
 * the pointer is raycast onto the wall plane, and the hit has to become an
 * `{x, y}` the layout's own `shelfAt` understands. Round-trips exactly, which
 * is asserted — a sign error here would mean books dropping onto the shelf
 * above or below the one under the cursor.
 */
export function layoutPoint(worldX: number, worldY: number): { x: number; y: number } {
  return { x: worldX * SCALE, y: -worldY * SCALE };
}

/**
 * A camera position that frames the whole library.
 *
 * Fits the taller of the two axes against the viewport, so a wide flat vault
 * and a deep narrow one both arrive on screen whole rather than one of them
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
      Margin, and never so close that the near plane clips the books.

      A fifth rather than a tenth: the shelf labels hang below their planks and
      the HUD chips sit in the corners, so a case fitted tight to the window
      puts the bottom-left folder's name underneath the legend. The margin is
      cheaper than arranging the two never to meet, and the case reads better
      with air around it anyway.
    */
    z: Math.max(1.2, Math.max(forH, forW) * 1.2),
  };
}
