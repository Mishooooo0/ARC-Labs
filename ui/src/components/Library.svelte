<script lang="ts">
  /**
   * The vault as a library, in 3D.
   *
   * ## What this replaced, and why twice
   *
   * First it was a d3-force graph. A force layout has no idea what a folder is
   * — it arranges by link topology, so a vault whose notes are barely linked
   * came out as a cloud of dots in no order. It could not be dragged either:
   * `onPointerDown` always started a *pan*, and a node was only hit-tested on
   * release, so the gesture did not exist to be buggy.
   *
   * Then it was flat shelves on a 2D canvas. Structurally right and visually
   * wrong: grey rectangles of one height standing on a hairline read as a bar
   * chart, not a bookshelf. Depth is not decoration here — it is the thing that
   * makes the metaphor legible.
   *
   * ## The division of labour
   *
   * - `lib/shelves.ts` decides *where everything goes*. Pure, no DOM, tested.
   * - `lib/scene.ts` converts that to world space. Pure, tested — one place for
   *   a sign to be wrong instead of three.
   * - `LibraryScene.svelte` draws it.
   * - This owns the DOM, the pointer, and what to ask the app for.
   */
  import { onMount } from "svelte";
  import { Canvas } from "@threlte/core";
  import LibraryScene from "./LibraryScene.svelte";
  import type { TreeView } from "../lib/types";
  import {
    DEFAULT_LAYOUT,
    folderOf,
    layout as computeLayout,
    shelfAt,
    type Book,
    type LayoutOptions,
    type Shelf,
  } from "../lib/shelves";
  import { framing, layoutPoint, SCALE, type ScreenLabel } from "../lib/scene";

  let {
    tree,
    vaultName,
    selected,
    onopen,
    onmove,
  }: {
    tree: TreeView;
    vaultName: string;
    selected: string | null;
    onopen: (path: string) => void;
    /** The caller confirms before doing anything. This only ever asks. */
    onmove: (from: string, toFolder: string) => void;
  } = $props();

  let host = $state<HTMLDivElement | null>(null);

  // ── Controls ──────────────────────────────────────────────────────────────
  // The flat view had none, which is what "geometry settings cannot be chosen"
  // meant. These change the layout rather than decorating it.
  let density = $state(1);
  let sort = $state<LayoutOptions["sort"]>("name");
  let showEmpty = $state(true);
  let columns = $state<LayoutOptions["columns"]>("auto");

  /**
   * The window's shape, reported by the scene from Threlte's own canvas size
   * rather than measured here.
   *
   * Measuring this element is wrong whenever it has not been laid out — a pane
   * that is still hidden measures 0×0, the guard skips it, and the aspect stays
   * at a hardcoded guess. That guess framed the library off the edge of the
   * screen, and now it would also pick the wrong number of columns.
   */
  let aspect = $state(16 / 9);

  function onsize(w: number, h: number) {
    if (w > 0 && h > 0) aspect = w / h;
  }

  const opts = $derived<LayoutOptions>({
    ...DEFAULT_LAYOUT,
    density,
    sort,
    showEmpty,
    columns,
    aspect,
  });
  const model = $derived(computeLayout(tree, vaultName, opts));

  // ── Palette ───────────────────────────────────────────────────────────────
  /**
   * Three.js materials need colours, not CSS variables, so the tokens are read
   * out once and handed to the scene. Re-read when the theme changes, which is
   * what keeps `lint-tokens` honest: this component still holds no literal.
   */
  let palette = $state<Record<string, string>>({});

  /**
   * `--arc-motion`, the accessibility gate.
   *
   * CSS transitions multiply by this token for free; the scene's frame loop
   * cannot, so the value is read here — the one place that already reads
   * tokens — and honoured explicitly there. Motion 0, and the
   * `prefers-reduced-motion` rule that forces the token to 0, must stop the
   * spawn and settle dead.
   *
   * An absent token means the stylesheet has not loaded, not that motion is
   * off, so it falls back to 1 rather than silently disabling animation
   * everywhere.
   */
  let motion = $state(1);

  function readPalette() {
    if (!host) return;
    const s = getComputedStyle(host);
    const get = (n: string) => s.getPropertyValue(n).trim();

    const raw = get("--arc-motion");
    const m = raw === "" ? 1 : Number(raw);
    motion = Number.isFinite(m) ? Math.max(0, m) : 1;

    palette = {
      bg: get("--arc-bg-0"),
      plank: get("--arc-line-strong"),
      shelfLabel: get("--arc-fg-faint"),
      book: get("--arc-fg-dim"),
      canvasBook: get("--arc-fg-faint"),
      accent: get("--arc-accent"),
      fg: get("--arc-fg"),
    };
  }

  // ── Camera ────────────────────────────────────────────────────────────────
  const camera = $derived(framing(model, aspect));

  // ── Labels ────────────────────────────────────────────────────────────────
  let labels = $state<ScreenLabel[]>([]);
  function onlabels(next: ScreenLabel[]) {
    labels = next;
  }

  // ── Interaction ───────────────────────────────────────────────────────────
  let hovered = $state<Book | null>(null);
  let held = $state<Book | null>(null);
  let heldAt = $state<{ x: number; y: number } | null>(null);
  let target = $state<Shelf | null>(null);
  /** Distinguishes a click from a drag, in world units travelled. */
  let travelled = 0;

  function onhover(b: Book | null) {
    hovered = b;
  }

  function onpick(b: Book) {
    held = b;
    travelled = 0;
    target = model.shelves.find((s) => s.path === folderOf(b.path)) ?? null;
    heldAt = { x: (b.x + b.w / 2) / SCALE, y: -(b.y + b.h / 2) / SCALE };
  }

  /**
   * The pointer, projected onto the wall by the raycaster and handed back in
   * world units. Turning it into layout units is what lets the *same*
   * `shelfAt` the flat version used pick the drop target.
   */
  function ondrag(world: { x: number; y: number }) {
    if (!held || !heldAt) return;
    travelled += Math.abs(world.x - heldAt.x) + Math.abs(world.y - heldAt.y);
    heldAt = world;
    const p = layoutPoint(world.x, world.y);
    const over = shelfAt(model, p.x, p.y);
    if (over?.path !== target?.path) target = over;
  }

  /**
   * A fresh press while a book is still in hand: drop it, move nothing.
   *
   * `release` runs on pointerup, and a pointerup can go missing — the pointer
   * leaves the window, another element captures it, the browser eats it. The
   * book then stays in hand with a drop target still set, and the *next* click
   * anywhere completes a move nobody asked for. Found exactly that way: a drag
   * whose release never arrived, then an unrelated click, and a confirm to move
   * a note into a folder the pointer was nowhere near.
   *
   * Confirming meant it was caught rather than done, but the dialog should not
   * be the thing standing between a stray click and a moved file. This runs in
   * the capture phase so it lands before the scene picks up the new book.
   */
  function dropStale() {
    if (!held) return;
    held = null;
    heldAt = null;
    target = null;
  }

  function release() {
    const book = held;
    const drop = target;
    held = null;
    heldAt = null;
    target = null;
    if (!book) return;

    // A press that went nowhere is a click. The threshold is in world units, so
    // it scales with the zoom rather than with the screen.
    if (travelled < 0.02) {
      onopen(book.path);
      return;
    }
    // Dropping a book back on its own shelf does nothing, deliberately. Order
    // comes from the filename and the vault stores no manual ordering;
    // pretending to reorder would invent state that does not survive a reload.
    if (drop && drop.path !== folderOf(book.path)) {
      onmove(book.path, drop.path);
    }
  }

  /**
   * Which books get a spine title.
   *
   * Each one is a DOM node repositioned whenever the camera moves, so 5,000 is
   * not an option. Capped, and biased to the widest books, which are the ones
   * whose spines have room for a word anyway. Below the cap everything is
   * labelled and the vault reads like a shelf you can browse.
   */
  const LABEL_CAP = 90;
  const labelled = $derived.by(() => {
    const all: Book[] = [];
    for (const s of model.shelves) for (const b of s.books) all.push(b);
    if (all.length <= LABEL_CAP) return all;
    return [...all].sort((a, b) => b.w - a.w).slice(0, LABEL_CAP);
  });

  onMount(() => {
    readPalette();

    // The theme is an attribute on the root; when it changes every colour here
    // is stale.
    const themes = new MutationObserver(readPalette);
    themes.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme", "style"],
    });
    return () => themes.disconnect();
  });
</script>

<div
  class="library"
  bind:this={host}
  onpointerdowncapture={dropStale}
  onpointerup={release}
  onpointerleave={release}
  role="presentation"
>
  {#if palette.accent}
    <Canvas>
      <LibraryScene
        {model}
        {palette}
        {selected}
        {hovered}
        {held}
        {heldAt}
        {target}
        {labelled}
        {camera}
        {motion}
        controls={!held}
        {onhover}
        {onpick}
        {ondrag}
        {onlabels}
        {onsize}
      />
    </Canvas>

    <!--
      Labels, over the canvas rather than in it.

      Not decoration: this is the only text in the view, so it is also the only
      thing a screen reader can read here. `aria-hidden` is deliberately absent
      for that reason, and `pointer-events: none` keeps it out of the way of a
      drag.
    -->
    <div class="labels">
      {#each labels as l (l.key)}
        {#if l.kind === "spine"}
          <span class="spine" style="left:{l.x}px; top:{l.y}px; width:{l.len}px">{l.text}</span>
        {:else}
          <span
            class="shelf"
            class:is-target={l.kind === "shelf-target"}
            style="left:{l.x}px; top:{l.y}px; max-width:{l.len}px"
          >
            {l.text}<span class="count">{l.sub}</span>
          </span>
        {/if}
      {/each}
    </div>
  {/if}

  <div class="controls data">
    <label>
      <span>density</span>
      <input type="range" min="0.7" max="1.6" step="0.1" bind:value={density} />
    </label>
    <label>
      <span>order</span>
      <select bind:value={sort}>
        <option value="name">name</option>
        <option value="size">size</option>
      </select>
    </label>
    <label>
      <span>bays</span>
      <select bind:value={columns}>
        <option value="auto">auto</option>
        <option value={1}>1</option>
        <option value={2}>2</option>
        <option value={3}>3</option>
        <option value={4}>4</option>
      </select>
    </label>
    <label class="check">
      <input type="checkbox" bind:checked={showEmpty} />
      <span>empty folders</span>
    </label>
  </div>

  <div class="legend data">
    <span>{tree.note_count.toLocaleString()} notes</span>
    <span>{model.shelves.length.toLocaleString()} shelves</span>
    {#if held}
      <span class="hint">
        {#if target && target.path !== folderOf(held.path)}
          drop to move into <strong>{target.label || "the vault root"}</strong>
        {:else}
          drag to another shelf to move it
        {/if}
      </span>
    {:else if hovered}
      <span class="hover">{hovered.label}</span>
      <span class="dim">{hovered.size.toLocaleString()} bytes</span>
    {/if}
  </div>
</div>

<style>
  .library {
    position: relative;
    height: 100%;
    background: var(--arc-bg-0);
    overflow: hidden;
    touch-action: none;
  }

  /* ── Labels ───────────────────────────────────────────────────────────── */
  .labels {
    position: absolute;
    inset: 0;
    pointer-events: none;
    overflow: hidden;
  }
  .labels span {
    position: absolute;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* A folder name sits on its own plank, at the left end, reading normally. */
  .shelf {
    color: var(--arc-fg-faint);
    font-size: var(--arc-text-xs);
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .shelf.is-target {
    color: var(--arc-accent);
  }
  .count {
    margin-left: var(--arc-space-2);
    opacity: 0.55;
  }

  /*
    A spine title runs up the book, the way it does on a real shelf. Rotating
    about the book's own centre is what keeps it on the spine at any zoom; the
    width is the book's on-screen height, so a title too long for the book is
    clipped rather than spilling across its neighbours.
  */
  .spine {
    color: var(--arc-fg-dim);
    font-size: var(--arc-text-xs);
    line-height: 1;
    transform: translate(-50%, -50%) rotate(-90deg);
    transform-origin: center;
    text-align: center;
  }

  .controls {
    position: absolute;
    top: var(--arc-space-4);
    right: var(--arc-space-4);
    display: flex;
    gap: var(--arc-space-4);
    align-items: center;
    background: var(--arc-bg-1);
    padding: var(--arc-space-2) var(--arc-space-3);
    border-radius: var(--arc-radius-pill);
    color: var(--arc-fg-faint);
    font-size: var(--arc-text-xs);
  }
  .controls label {
    display: flex;
    gap: var(--arc-space-2);
    align-items: center;
    cursor: pointer;
  }
  .controls input[type="range"] {
    width: 68px;
    accent-color: var(--arc-accent);
  }
  .controls select {
    background: var(--arc-bg-2);
    color: var(--arc-fg-dim);
    border: 0;
    border-radius: var(--arc-radius-sm);
    padding: 1px var(--arc-space-2);
    font: inherit;
  }
  .controls .check input {
    accent-color: var(--arc-accent);
  }

  /*
    On its own chip, like the controls. Without one it sits directly on the
    scene, and a shelf label that happens to land in the bottom-left corner
    prints straight through it.
  */
  .legend {
    position: absolute;
    left: var(--arc-space-4);
    bottom: var(--arc-space-4);
    display: flex;
    gap: var(--arc-space-3);
    align-items: baseline;
    background: var(--arc-bg-1);
    padding: var(--arc-space-2) var(--arc-space-3);
    border-radius: var(--arc-radius-pill);
    color: var(--arc-fg-faint);
    pointer-events: none;
    flex-wrap: wrap;
    max-width: calc(100% - var(--arc-space-8));
  }
  .hover {
    color: var(--arc-fg-dim);
    max-width: 40ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dim {
    opacity: 0.7;
  }
  .hint {
    color: var(--arc-accent);
  }
</style>
