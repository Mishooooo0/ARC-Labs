<script lang="ts">
  /**
   * The vault as a library: folders are shelves, notes are books standing on
   * them.
   *
   * ## Why this replaced a force graph
   *
   * The old view ran a d3-force simulation. A force layout has no idea what a
   * folder is — it arranges by link topology, so a vault whose notes are barely
   * linked came out as a cloud of unconnected dots in no order at all. It could
   * not be dragged either: `onPointerDown` always started a *pan*, and a node
   * was only hit-tested on release. There was no code path in which a node
   * moved.
   *
   * Here the layout is arithmetic, not simulation — see `lib/shelves.ts`, which
   * is pure and tested. The same vault always lays out the same way, which is
   * most of "no order" fixed on its own, and it means an animation can be
   * trusted: a book that moves on screen moved because something changed.
   *
   * ## Still a canvas
   *
   * The 5,000-note interactivity gate is a phase gate rather than a preference,
   * and 5,000 DOM elements loses it. Canvas also gives the zoom-out to the vault
   * map a shared coordinate space to cross-fade through, which two DOM views
   * could not.
   */
  import { onMount } from "svelte";
  import type { TreeView } from "../lib/types";
  import {
    bookAt,
    DEFAULT_LAYOUT,
    folderOf,
    layout as computeLayout,
    shelfAt,
    stepToward,
    type Book,
    type LayoutOptions,
    type Live,
    type Shelf,
  } from "../lib/shelves";

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
  let canvas = $state<HTMLCanvasElement | null>(null);

  // ── Controls ──────────────────────────────────────────────────────────────
  // The old view had none, which is what "geometry settings cannot be chosen"
  // meant. These change the layout rather than decorating it.
  let density = $state(1);
  let sort = $state<LayoutOptions["sort"]>("name");
  let showEmpty = $state(true);

  const opts = $derived<LayoutOptions>({
    ...DEFAULT_LAYOUT,
    density,
    sort,
    showEmpty,
  });
  const model = $derived(computeLayout(tree, vaultName, opts));

  // ── View transform ────────────────────────────────────────────────────────
  let scale = 1;
  let tx = 24;
  let ty = 24;

  // ── Animation ─────────────────────────────────────────────────────────────
  /**
   * Where each book is *right now*, as against where the layout says it should
   * be. Books ease toward their target instead of teleporting, which is what
   * makes a move read as a move; and a book seen for the first time rises onto
   * its shelf rather than blinking into existence.
   */
  const live = new Map<string, Live>();
  let animating = false;

  /**
   * The motion budget, honoured by hand.
   *
   * CSS transitions inherit `--arc-motion` for free; a canvas draw loop does
   * not. Appearance → Motion 0 has to stop every one of these, and
   * `prefers-reduced-motion` already forces that token to 0 — so it is read
   * here and obeyed explicitly. This is an accessibility gate, not a taste.
   */
  function motion(): number {
    if (!host) return 1;
    const raw = getComputedStyle(host).getPropertyValue("--arc-motion").trim();
    const n = Number.parseFloat(raw);
    return Number.isFinite(n) ? Math.max(0, n) : 1;
  }

  function tokens(el: HTMLElement) {
    const s = getComputedStyle(el);
    return {
      bg: s.getPropertyValue("--arc-bg-0").trim(),
      plank: s.getPropertyValue("--arc-line-strong").trim(),
      shelfLabel: s.getPropertyValue("--arc-fg-faint").trim(),
      book: s.getPropertyValue("--arc-fg-dim").trim(),
      canvasBook: s.getPropertyValue("--arc-fg-faint").trim(),
      accent: s.getPropertyValue("--arc-accent").trim(),
      accentWash: s.getPropertyValue("--arc-accent-wash").trim(),
      fg: s.getPropertyValue("--arc-fg").trim(),
      bg2: s.getPropertyValue("--arc-bg-2").trim(),
    };
  }

  // ── Drag ──────────────────────────────────────────────────────────────────
  // The whole fix for "you cannot drag and move": pointer-down on a *book*
  // grabs it, pointer-down on empty space pans. One distinction, and the
  // gesture that never existed becomes possible.
  let panning = $state(false);
  let held = $state<Book | null>(null);
  let heldAt = { dx: 0, dy: 0, x: 0, y: 0 };
  let target = $state<Shelf | null>(null);
  let hovered = $state<Book | null>(null);
  let lastX = 0;
  let lastY = 0;
  let moved = 0;

  function world(clientX: number, clientY: number) {
    const rect = canvas!.getBoundingClientRect();
    return {
      x: (clientX - rect.left - tx) / scale,
      y: (clientY - rect.top - ty) / scale,
    };
  }

  function draw() {
    if (!canvas || !host) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    if (canvas.width !== w * dpr || canvas.height !== h * dpr) {
      canvas.width = w * dpr;
      canvas.height = h * dpr;
    }
    const t = tokens(host);

    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    ctx.save();
    ctx.translate(tx, ty);
    ctx.scale(scale, scale);

    for (const shelf of model.shelves) {
      // Viewport cull: on a deep vault most shelves are off screen, and the
      // cheapest shelf is one that is never drawn.
      const top = shelf.y * scale + ty;
      const bottom = (shelf.y + shelf.h) * scale + ty;
      if (bottom < -40 || top > h + 40) continue;

      const isTarget = target?.path === shelf.path;

      // The plank.
      const plankY = shelf.y + shelf.h;
      ctx.strokeStyle = isTarget ? t.accent : t.plank;
      ctx.lineWidth = (isTarget ? 2.5 : 1.5) / scale;
      ctx.beginPath();
      ctx.moveTo(shelf.x, plankY);
      ctx.lineTo(shelf.x + shelf.w, plankY);
      ctx.stroke();

      // A shelf being dropped onto lights up its whole bay, so the target is
      // obvious while a book is in the air.
      if (isTarget) {
        ctx.fillStyle = t.accentWash;
        ctx.globalAlpha = 0.5;
        ctx.fillRect(shelf.x - 6, shelf.y, shelf.w + 12, shelf.h);
        ctx.globalAlpha = 1;
      }

      // Folder name, and a count so an empty shelf still says what it is.
      ctx.fillStyle = isTarget ? t.accent : t.shelfLabel;
      ctx.font = `${12 / scale}px ui-monospace, monospace`;
      ctx.textAlign = "left";
      ctx.textBaseline = "alphabetic";
      const count = shelf.books.length;
      ctx.fillText(
        `${shelf.label}  ${count === 0 ? "empty" : count}`,
        shelf.x,
        shelf.y + 14 / scale,
      );

      for (const b of shelf.books) {
        if (held?.path === b.path) continue; // drawn last, under the cursor
        const l = live.get(b.path);
        const x = l ? l.x : b.x;
        const y = l ? l.y : b.y;
        const grow = l ? l.spawn : 1;
        drawBook(ctx, t, b, x, y, grow);
      }
    }

    // The held book last, so it rides above everything.
    if (held) {
      ctx.globalAlpha = 0.92;
      drawBook(ctx, t, held, heldAt.x, heldAt.y, 1, true);
      ctx.globalAlpha = 1;
    }

    ctx.restore();
  }

  function drawBook(
    ctx: CanvasRenderingContext2D,
    t: ReturnType<typeof tokens>,
    b: Book,
    x: number,
    y: number,
    grow: number,
    lifted = false,
  ) {
    const h = b.h * grow;
    const yy = y + (b.h - h); // grows upward, as if sliding onto the shelf
    const isSelected = selected === b.path;
    const isHovered = hovered?.path === b.path;

    ctx.fillStyle = isSelected || isHovered ? t.accent : b.isCanvas ? t.canvasBook : t.book;
    ctx.globalAlpha = lifted ? 1 : b.isCanvas ? 0.75 : 0.92;
    roundRect(ctx, x, yy, b.w, h, Math.min(3, b.w / 3) / 1);
    ctx.fill();
    ctx.globalAlpha = 1;

    if (isSelected || isHovered || lifted) {
      ctx.strokeStyle = t.accent;
      ctx.lineWidth = 1.5 / scale;
      roundRect(ctx, x, yy, b.w, h, Math.min(3, b.w / 3));
      ctx.stroke();
    }

    // The spine title, rotated, once there is room for it to be legible.
    const px = 10 / scale;
    if (b.w * scale > 13 && h * scale > 44 && grow > 0.85) {
      ctx.save();
      ctx.translate(x + b.w / 2, yy + h - 6 / scale);
      ctx.rotate(-Math.PI / 2);
      ctx.fillStyle = t.bg;
      ctx.font = `${px}px ui-sans-serif, system-ui, sans-serif`;
      ctx.textAlign = "left";
      ctx.textBaseline = "middle";
      const max = h - 12 / scale;
      let label = b.label;
      while (label.length > 3 && ctx.measureText(label).width > max) {
        label = label.slice(0, -1);
      }
      ctx.fillText(label, 0, 0);
      ctx.restore();
    }
  }

  function roundRect(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    w: number,
    h: number,
    r: number,
  ) {
    ctx.beginPath();
    ctx.moveTo(x + r, y);
    ctx.arcTo(x + w, y, x + w, y + h, r);
    ctx.arcTo(x + w, y + h, x, y + h, r);
    ctx.arcTo(x, y + h, x, y, r);
    ctx.arcTo(x, y, x + w, y, r);
    ctx.closePath();
  }

  /**
   * Step every book toward where the layout says it belongs.
   *
   * Runs only while something is actually moving, and stops itself when
   * everything has arrived — a permanent `requestAnimationFrame` loop on a view
   * at rest is a battery cost for no picture change.
   */
  function tick() {
    const m = motion();
    let busy = false;
    const seen = new Set<string>();

    for (const shelf of model.shelves) {
      for (const b of shelf.books) {
        seen.add(b.path);
        let l = live.get(b.path);
        if (!l) {
          // First sight of this note. With motion off it simply appears.
          l = m === 0
            ? { x: b.x, y: b.y, spawn: 1 }
            : { x: b.x, y: b.y + 18, spawn: 0 };
          live.set(b.path, l);
        }
        // The easing lives in `shelves.ts` so the motion budget is a thing a
        // test can assert rather than something to squint at on screen.
        if (stepToward(l, b, m)) busy = true;
      }
    }
    // A note that has gone stops being animated. Without this the map grows for
    // the life of the session on a vault that is edited a lot.
    for (const key of live.keys()) if (!seen.has(key)) live.delete(key);

    draw();
    if (busy || held) {
      requestAnimationFrame(tick);
    } else {
      animating = false;
    }
  }

  /**
   * Repaint now, and ease from here if anything is still moving.
   *
   * The synchronous `draw()` is not an optimisation. `requestAnimationFrame` is
   * starved in an occluded window, a background tab and a hidden pane, so a
   * loop that only ever paints from a rAF callback shows a stale picture in all
   * three — a control you just moved appears to do nothing. Painting first
   * means the view is correct wherever it is, and the animation is the
   * enhancement on top rather than the only path to a pixel.
   *
   * This is the same reasoning the force graph carried before it, and dropping
   * it was how the density control came to change nothing at all while the
   * pane was hidden.
   */
  function kick() {
    draw();
    if (animating) return;
    animating = true;
    requestAnimationFrame(tick);
  }

  // Any change to the model or the view restarts the loop; it stops itself.
  $effect(() => {
    void model;
    void selected;
    void target;
    void hovered;
    kick();
  });

  onMount(() => {
    const onResize = () => kick();
    window.addEventListener("resize", onResize);
    document.addEventListener("visibilitychange", onResize);
    kick();
    return () => {
      window.removeEventListener("resize", onResize);
      document.removeEventListener("visibilitychange", onResize);
    };
  });

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const rect = canvas!.getBoundingClientRect();
    const cx = e.clientX - rect.left;
    const cy = e.clientY - rect.top;
    const next = Math.min(4, Math.max(0.15, scale * Math.exp(-e.deltaY * 0.0015)));
    tx = cx - ((cx - tx) * next) / scale;
    ty = cy - ((cy - ty) * next) / scale;
    scale = next;
    kick();
  }

  function onPointerDown(e: PointerEvent) {
    const p = world(e.clientX, e.clientY);
    const hit = bookAt(model, p.x, p.y);
    moved = 0;
    lastX = e.clientX;
    lastY = e.clientY;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);

    if (hit) {
      held = hit;
      heldAt = { dx: p.x - hit.x, dy: p.y - hit.y, x: hit.x, y: hit.y };
      target = shelfAt(model, p.x, p.y);
    } else {
      panning = true;
    }
    kick();
  }

  function onPointerMove(e: PointerEvent) {
    // Deltas first: `lastX`/`lastY` are the *previous* point, and overwriting
    // them before reading them loses the movement entirely.
    const dx = e.clientX - lastX;
    const dy = e.clientY - lastY;
    moved += Math.abs(dx) + Math.abs(dy);
    lastX = e.clientX;
    lastY = e.clientY;

    if (held) {
      const p = world(e.clientX, e.clientY);
      heldAt.x = p.x - heldAt.dx;
      heldAt.y = p.y - heldAt.dy;
      const over = shelfAt(model, p.x, p.y);
      if (over?.path !== target?.path) target = over;
      kick();
      return;
    }
    if (panning) {
      tx += dx;
      ty += dy;
      kick();
      return;
    }
    const p = world(e.clientX, e.clientY);
    const over = bookAt(model, p.x, p.y);
    if (over?.path !== hovered?.path) {
      hovered = over;
      kick();
    }
  }

  function onPointerUp() {
    const book = held;
    const drop = target;
    held = null;
    target = null;
    panning = false;

    if (!book) return;

    // A short press is a click, not a drag. 4px of slop so a slightly shaky
    // click still opens the note.
    if (moved < 4) {
      onopen(book.path);
      kick();
      return;
    }
    // Dropping a book back on its own shelf does nothing, deliberately. Order
    // within a shelf comes from the filename, and the vault stores no manual
    // ordering — pretending to reorder would invent state that does not
    // survive a reload.
    if (drop && drop.path !== folderOf(book.path)) {
      onmove(book.path, drop.path);
    }
    kick();
  }
</script>

<div class="library" bind:this={host}>
  <canvas
    bind:this={canvas}
    onwheel={onWheel}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointercancel={onPointerUp}
    class:grabbing={panning}
    class:carrying={!!held}
  ></canvas>

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
          drop to move into <strong>{target.label}</strong>
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
  }
  canvas {
    display: block;
    width: 100%;
    height: 100%;
    cursor: grab;
    touch-action: none;
  }
  canvas.grabbing {
    cursor: grabbing;
  }
  canvas.carrying {
    cursor: grabbing;
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

  .legend {
    position: absolute;
    left: var(--arc-space-4);
    bottom: var(--arc-space-4);
    display: flex;
    gap: var(--arc-space-3);
    align-items: baseline;
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
