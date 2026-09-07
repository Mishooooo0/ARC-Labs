/**
 * Wood grain, computed rather than downloaded.
 *
 * Procedural because it has to be. The app's CSP allows no image from anywhere
 * — `img-src 'self' data: blob:` and `connect-src 'self'` — so a texture file
 * would need either an exception or a bundled asset and an asset pipeline. A
 * bitmap built here needs neither and costs one 256×512 buffer for the life of
 * the page.
 *
 * ## Why the pixels are written by hand
 *
 * The first version drew streaks with `ctx.stroke()`, which was wrong twice.
 *
 * `lint-tokens` caught it first, and was right to: `#ffffff` and `rgba(v,v,v,a)`
 * are colour literals outside the token file. It could not know they were
 * greyscale, and the rule is worth more than the exception would be.
 *
 * The deeper problem is that canvas stroke rasterisation — antialiasing, line
 * joins, sub-pixel coverage — is not specified precisely enough to be identical
 * across engines. This is meant to be the *same* bookcase every time you open
 * it, on every platform this ships to. Accumulating luminance into a
 * `Float32Array` is exact arithmetic, so it is.
 *
 * What comes out is a luminance mask, not a colour. Each finish multiplies it
 * by its own token colour, so three finishes share one buffer instead of
 * needing three.
 */

import { CanvasTexture, ClampToEdgeWrapping, RepeatWrapping, SRGBColorSpace } from "three";

/** Deterministic. Mulberry32 — three lines, no dependency, good enough for grain. */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const W = 256;
const H = 512;

/**
 * One streak running the length of the board.
 *
 * It wanders as it goes, so the grain does not read as pinstripe, and it fades
 * out at its edges rather than ending on a hard line — which is the difference
 * between timber and corrugated iron.
 */
function streak(
  lum: Float32Array,
  rand: () => number,
  halfWidth: number,
  depth: number,
  wander: number,
): void {
  let cx = rand() * W;
  for (let y = 0; y < H; y++) {
    cx += (rand() - 0.5) * wander;
    const from = Math.floor(cx - halfWidth);
    const to = Math.ceil(cx + halfWidth);
    for (let x = from; x <= to; x++) {
      // Wrap, so the texture tiles without a seam down one edge.
      const px = ((x % W) + W) % W;
      const d = Math.abs(x - cx) / halfWidth;
      if (d > 1) continue;
      // Smoothstep falloff: 1 at the centre of the streak, 0 at its edge.
      const fade = 1 - d * d * (3 - 2 * d);
      const i = y * W + px;
      lum[i] = lum[i]! - depth * fade;
    }
  }
}

/**
 * Build the mask.
 *
 * Values stay in the top third of the range: this multiplies a finish colour,
 * and anything darker reads as dirt rather than as timber.
 */
function build(): HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  canvas.width = W;
  canvas.height = H;

  const ctx = canvas.getContext("2d");
  if (!ctx) return canvas;

  const rand = rng(0x5f9e8c);
  const lum = new Float32Array(W * H).fill(1);

  /*
    Broad and shallow, not fine and dark.

    The first attempt used two hundred narrow high-contrast lines, and it came
    out as corrugated iron: at any distance the eye reads a regular row of hard
    stripes rather than a surface. Real walnut is mostly a soft, wide variation
    in tone with only a few strong lines in it, so the widths here are five
    times what they were and the depths a quarter.
  */
  for (let i = 0; i < 26; i++) {
    streak(lum, rand, 8 + rand() * 22, 0.02 + rand() * 0.035, 0.5);
  }

  // Fewer, finer lines over the top: enough to read as grain close up, not
  // enough to organise themselves into a pattern from across the room.
  for (let i = 0; i < 60; i++) {
    streak(lum, rand, 1.2 + rand() * 2.6, 0.012 + rand() * 0.022, 0.22);
  }

  // A couple of growth rings — the darker bands that make it read as a board
  // cut from a log rather than as a painted panel.
  for (let i = 0; i < 3; i++) {
    streak(lum, rand, 2 + rand() * 4, 0.03 + rand() * 0.03, 0.35);
  }

  const img = ctx.createImageData(W, H);
  const d = img.data;
  for (let i = 0; i < lum.length; i++) {
    // Fine noise, so flat areas are never perfectly flat.
    const v = lum[i]! + (rand() - 0.5) * 0.018;
    const byte = Math.max(0, Math.min(255, Math.round(v * 255)));
    const o = i * 4;
    d[o] = byte;
    d[o + 1] = byte;
    d[o + 2] = byte;
    d[o + 3] = 255;
  }
  ctx.putImageData(img, 0, 0);

  return canvas;
}

let along: CanvasTexture | null = null;

/**
 * The grain, running along a part's long axis.
 *
 * `rotate` turns it a quarter turn for boards, whose grain runs across the case
 * rather than up it. Clones share the one bitmap and carry their own rotation,
 * which is the only reason two orientations do not cost two buffers.
 */
export function grainTexture(rotate = false): CanvasTexture {
  if (!along) {
    along = new CanvasTexture(build());
    along.wrapS = RepeatWrapping;
    along.wrapT = RepeatWrapping;
    along.colorSpace = SRGBColorSpace;
  }
  if (!rotate) return along;

  const across = along.clone();
  across.center.set(0.5, 0.5);
  across.rotation = Math.PI / 2;
  across.wrapS = ClampToEdgeWrapping;
  across.wrapT = ClampToEdgeWrapping;
  across.needsUpdate = true;
  return across;
}
