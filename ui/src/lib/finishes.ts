/**
 * The bookcase's finish.
 *
 * A finish is a choice the person makes about their furniture — walnut, painted
 * white, or the logo's emerald — and not a consequence of the theme. Someone who
 * picks walnut and then switches to the light theme should still have walnut.
 * So the values live once in `tokens.css` under `:root`, with no per-theme
 * override, and this reads them back out.
 *
 * Reading them rather than writing them here is what keeps `lint-tokens` honest:
 * three.js materials need a colour value and cannot resolve a CSS variable, so
 * *something* has to turn `var(--arc-finish-walnut-case)` into `#6b4a2f` — and
 * doing it through `getComputedStyle` means no component holds a literal.
 */

export type FinishName = "walnut" | "white" | "emerald";

export const FINISHES: readonly FinishName[] = ["walnut", "white", "emerald"] as const;

export function isFinish(v: unknown): v is FinishName {
  return typeof v === "string" && (FINISHES as readonly string[]).includes(v);
}

export interface Finish {
  name: FinishName;
  /** Sides and top. */
  case: string;
  /** Interior boards. Lighter than the case: they catch more light. */
  board: string;
  /** The back panel, in shadow. */
  back: string;
  /** Front edges, where the light lands. */
  edge: string;
  /**
   * The six book cloths. A note takes one by a hash of its path.
   *
   * All low-saturation, and that is load-bearing rather than taste: the two
   * reserved authorship hues are a *saturated* amber and a saturated blue, and
   * book cloth is mostly warm neutrals that share amber's hue family. Drawing
   * the line at saturation is what lets books look like books without any of
   * them being mistaken for a claim about who wrote it.
   */
  spines: string[];
  /** Canvases, kept apart from the six so a board still reads as a board. */
  canvas: string;
  /** Folder names, printed on the front edge of each board — read against wood. */
  label: string;
  /** Note names, printed up a spine — read against a book, not against wood. */
  ink: string;
  /**
   * How strongly the grain shows, 0–1.
   *
   * A number, not a colour, so it belongs here rather than in the token file.
   * Painted oak has almost none — paint fills the grain, which is what makes
   * painted furniture read as painted.
   */
  grain: number;
}

const GRAIN: Record<FinishName, number> = {
  walnut: 1,
  white: 0.35,
  emerald: 0.55,
};

/**
 * Read one finish out of the tokens, resolved against `el`.
 *
 * `el` has to be an element that is actually in the document, because a custom
 * property is inherited and `getComputedStyle` on a detached node resolves
 * nothing.
 */
export function readFinish(el: HTMLElement, name: FinishName): Finish {
  const s = getComputedStyle(el);
  const get = (part: string) => s.getPropertyValue(`--arc-finish-${name}-${part}`).trim();
  return {
    name,
    case: get("case"),
    board: get("board"),
    back: get("back"),
    edge: get("edge"),
    spines: [1, 2, 3, 4, 5, 6].map((i) => get(`spine-${i}`)),
    canvas: get("canvas"),
    label: get("label"),
    ink: get("ink"),
    grain: GRAIN[name],
  };
}

const KEY = "arc-labs-finish";

/** The finish this person last chose, or walnut. */
export function storedFinish(): FinishName {
  try {
    const v = localStorage.getItem(KEY);
    if (isFinish(v)) return v;
  } catch {
    // A private window, or site data blocked. A default is a fine answer.
  }
  return "walnut";
}

export function storeFinish(name: FinishName): void {
  try {
    localStorage.setItem(KEY, name);
  } catch {
    // Not worth surfacing: the finish still applies for this session.
  }
}
