<script lang="ts">
  /**
   * The vault as a bookcase.
   *
   * ## What this replaced, and why three times
   *
   * First it was a d3-force graph. A force layout has no idea what a folder is
   * — it arranges by link topology, so a vault whose notes are barely linked
   * came out as a cloud of dots in no order. It could not be dragged either:
   * `onPointerDown` always started a *pan*, and a node was only hit-tested on
   * release, so the gesture did not exist to be buggy.
   *
   * Then it was flat shelves on a 2D canvas. Structurally right and visually
   * wrong: grey rectangles of one height standing on a hairline read as a bar
   * chart.
   *
   * Then it was those same planks in 3D — real depth, but still a plank per
   * folder with a stub upright at the left end of each. Nothing enclosed
   * anything, so it read as a diagram *of* a bookshelf. Now it is a carcass
   * with boards in it, and the arrangement carries the meaning:
   *
   * - a folder's **board** is the surface its notes stand on
   * - the **plinth** is the surface unfiled notes stand on
   *
   * which is why no folders means no boards, and X folders means X boards,
   * with no branch anywhere that says so.
   *
   * ## The division of labour
   *
   * - `lib/shelves.ts` decides *where everything goes*. Pure, no DOM, tested.
   * - `lib/scene.ts` converts that to world space. Pure, tested — one place for
   *   a sign to be wrong instead of three.
   * - `lib/finishes.ts` reads the three finishes out of the tokens.
   * - `LibraryScene.svelte` draws it.
   * - This owns the DOM, the pointer, and what to ask the app for.
   */
  import { onMount } from "svelte";
  import { Canvas } from "@threlte/core";
  import LibraryScene from "./LibraryScene.svelte";
  import type { TreeView } from "../lib/types";
  import {
    compartmentAt,
    DEFAULT_LAYOUT,
    folderOf,
    layout as computeLayout,
    type Book,
    type Compartment,
    type LayoutOptions,
  } from "../lib/shelves";
  import { framing, layoutPoint, SCALE, type ScreenLabel } from "../lib/scene";
  import {
    FINISHES,
    readFinish,
    storedFinish,
    storeFinish,
    type Finish,
    type FinishName,
  } from "../lib/finishes";

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
  // meant. These change the furniture rather than decorating it.
  let density = $state(1);
  let sort = $state<LayoutOptions["sort"]>("name");
  let showEmpty = $state(true);
  let finishName = $state<FinishName>("walnut");
  /** Bumped to put the camera back at the framing. */
  let resetKey = $state(0);

  /**
   * The window's shape, reported by the scene from Threlte's own canvas size
   * rather than measured here.
   *
   * Measuring this element is wrong whenever it has not been laid out — a pane
   * that is still hidden measures 0×0, the guard skips it, and the aspect stays
   * at a hardcoded guess that frames the case off the edge of the screen.
   */
  let aspect = $state(16 / 9);

  function onsize(w: number, h: number) {
    if (w > 0 && h > 0) aspect = w / h;
  }

  const opts = $derived<LayoutOptions>({ ...DEFAULT_LAYOUT, density, sort, showEmpty });
  const model = $derived(computeLayout(tree, vaultName, opts));

  const folderCount = $derived(
    model.bookcase.compartments.filter((c) => !c.isFloor).length,
  );

  // ── Finish and palette ────────────────────────────────────────────────────
  /**
   * three.js materials need colours, not CSS variables, so the tokens are read
   * out and handed to the scene. Re-read when the theme changes, which is what
   * keeps `lint-tokens` honest: this component still holds no literal.
   *
   * The finish is deliberately *not* re-read on a theme change — it is declared
   * once in `:root` with no per-theme override, because picking walnut and then
   * switching to the light theme should still give you walnut.
   */
  let finish = $state<Finish | null>(null);
  let accent = $state("");

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
   * off, so it falls back to 1 rather than silently disabling animation.
   */
  let motion = $state(1);

  function readTokens() {
    if (!host) return;
    const s = getComputedStyle(host);

    const raw = s.getPropertyValue("--arc-motion").trim();
    const m = raw === "" ? 1 : Number(raw);
    motion = Number.isFinite(m) ? Math.max(0, m) : 1;

    accent = s.getPropertyValue("--arc-accent").trim();
    finish = readFinish(host, finishName);
  }

  function pickFinish(name: FinishName) {
    finishName = name;
    storeFinish(name);
    if (host) finish = readFinish(host, name);
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
  let target = $state<Compartment | null>(null);
  /** Distinguishes a click from a drag, in world units travelled. */
  let travelled = 0;

  function onhover(b: Book | null) {
    hovered = b;
  }

  function onpick(b: Book) {
    held = b;
    travelled = 0;
    target = model.bookcase.compartments.find((c) => c.path === folderOf(b.path)) ?? null;
    heldAt = { x: (b.x + b.w / 2) / SCALE, y: -(b.y + b.h / 2) / SCALE };
  }

  /**
   * The pointer, projected onto the book plane by the raycaster and handed back
   * in world units. Turning it into layout units is what lets the layout's own
   * `compartmentAt` pick the drop target — and it stays correct at any camera
   * angle, because the plane is fixed in the world and the camera is what moves.
   */
  function ondrag(world: { x: number; y: number }) {
    if (!held || !heldAt) return;
    travelled += Math.abs(world.x - heldAt.x) + Math.abs(world.y - heldAt.y);
    heldAt = world;
    const p = layoutPoint(world.x, world.y);
    const over = compartmentAt(model, p.x, p.y);
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
    // Dropping a book back where it came from does nothing, deliberately. Order
    // comes from the filename and the vault stores no manual ordering;
    // pretending to reorder would invent state that does not survive a reload.
    // Dropping onto the floor is a real move, though — that is how a note gets
    // taken back out to the vault root.
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
    for (const c of model.bookcase.compartments) for (const b of c.books) all.push(b);
    if (all.length <= LABEL_CAP) return all;
    return [...all].sort((a, b) => b.w - a.w).slice(0, LABEL_CAP);
  });

  onMount(() => {
    finishName = storedFinish();
    readTokens();

    // The theme is an attribute on the root; when it changes the accent and the
    // motion multiplier here are stale.
    const themes = new MutationObserver(readTokens);
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
  {#if finish && accent}
    <Canvas>
      <LibraryScene
        {model}
        {finish}
        {accent}
        {selected}
        {hovered}
        {held}
        {heldAt}
        {target}
        {labelled}
        {camera}
        {motion}
        {resetKey}
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

      A spine's angle comes from the scene, which projects the book's own long
      axis. Fixing it at −90° only looks right head-on, and the point of being
      able to walk around the case is that usually you are not.
    -->
    <div class="labels" style="--label:{finish.label}; --ink:{finish.ink}">
      {#each labels as l (l.key)}
        {#if l.kind === "spine"}
          <span
            class="spine"
            style="left:{l.x}px; top:{l.y}px; width:{l.len}px; --a:{l.angle}deg"
          >{l.text}</span>
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
      <span>finish</span>
      <select value={finishName} onchange={(e) => pickFinish(e.currentTarget.value as FinishName)}>
        {#each FINISHES as f (f)}
          <option value={f}>{f}</option>
        {/each}
      </select>
    </label>
    <label>
      <span>height</span>
      <input type="range" min="0.7" max="1.6" step="0.1" bind:value={density} />
    </label>
    <label>
      <span>order</span>
      <select bind:value={sort}>
        <option value="name">name</option>
        <option value="size">size</option>
      </select>
    </label>
    <label class="check">
      <input type="checkbox" bind:checked={showEmpty} />
      <span>empty folders</span>
    </label>
    <button type="button" onclick={() => resetKey++}>reset view</button>
  </div>

  <div class="legend data">
    <span>{tree.note_count.toLocaleString()} notes</span>
    <span>{folderCount.toLocaleString()} {folderCount === 1 ? "shelf" : "shelves"}</span>
    {#if held}
      <span class="hint">
        {#if target && target.path !== folderOf(held.path)}
          drop to move into <strong>{target.isFloor ? "the vault root" : target.label}</strong>
        {:else}
          drag to another shelf to move it
        {/if}
      </span>
    {:else if hovered}
      <span class="hover">{hovered.label}</span>
      <span class="dim">{hovered.size.toLocaleString()} bytes</span>
    {:else if folderCount === 0}
      <!-- The arrangement is the explanation, and this is the one moment it can
           be said in six words. -->
      <span class="dim">No folders yet — every folder you make becomes a shelf.</span>
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

  /*
    A folder name sits on the front edge of its own board, and a note's name
    runs up its spine — two different grounds, so two different inks. The app's
    own greys were used for both at first, which put slate-grey text on walnut
    and on cream paper and left neither readable. Both now come from the finish,
    which is the only thing that knows what they are printed on.
  */
  .shelf {
    color: var(--label);
    font-size: var(--arc-text-xs);
    letter-spacing: 0.04em;
    text-transform: uppercase;
    transform: translateY(2px);
  }
  .shelf.is-target {
    color: var(--arc-accent);
  }
  .count {
    margin-left: var(--arc-space-2);
    opacity: 0.55;
  }

  /*
    A spine title runs up the book, the way it does on a real shelf. `--a` is
    the projected angle of the book's own long axis, so the name stays on the
    spine from any angle you have walked round to; the width is the book's
    on-screen length, so a title too long for the book is clipped rather than
    spilling across its neighbours.
  */
  .spine {
    color: var(--ink);
    font-size: var(--arc-text-xs);
    line-height: 1;
    text-align: center;
    transform: translate(-50%, -50%) rotate(var(--a));
    transform-origin: center;
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
  .controls button {
    background: var(--arc-bg-2);
    color: var(--arc-fg-dim);
    border: 0;
    border-radius: var(--arc-radius-pill);
    padding: 2px var(--arc-space-3);
    font: inherit;
    cursor: pointer;
  }
  .controls button:hover {
    background: var(--arc-bg-3);
    color: var(--arc-fg);
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
