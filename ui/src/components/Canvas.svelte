<script lang="ts">
  /**
   * The canvas.
   *
   * ## Why DOM on a transform layer, and not a drawing API
   *
   * The spec rules out canvas/WebGL libraries for one reason: from Phase 5 a
   * `file` card has to host a **live CodeMirror instance**, and you cannot put a
   * real editor inside a `<canvas>`. So cards are absolutely positioned DOM
   * elements on a single layer with one CSS `matrix()`, and pan and zoom move
   * that one layer rather than 300 elements.
   *
   * Edges are SVG *beneath* the cards, in the same transformed space. SVG is
   * fine here — an edge count is small next to a node count, and vector strokes
   * scale without going soft.
   *
   * ## Virtualisation, and why the distinction is visible
   *
   * Only cards intersecting the viewport render their contents; the rest are an
   * outline and a title. That is what keeps 300 cards at 60 fps. The spec is
   * explicit that the user must never wonder whether a card is editable, so a
   * virtualised card is visibly a placeholder rather than a card that looks
   * ready and silently is not.
   *
   * ## Authorship (constraint 6)
   *
   * Each card carries a 2px left border in its author's colour — amber human,
   * blue agent — and a card with no recorded history gets **no** border rather
   * than a default one. The border is a fixed screen-space width, so it stays
   * readable when the whole layer is scaled to 40%.
   */
  import type { CanvasView } from "../lib/types";

  let {
    canvas,
    onopen,
    onmove,
  }: {
    canvas: CanvasView;
    onopen: (path: string) => void;
    onmove: (moves: { id: string; x: number; y: number }[]) => void;
  } = $props();

  const GRID = 20;

  let host = $state<HTMLDivElement | null>(null);
  let scale = $state(1);
  let tx = $state(0);
  let ty = $state(0);
  let selection = $state<Set<string>>(new Set());
  let viewport = $state({ w: 0, h: 0 });

  /** Local position overrides while dragging, so the DOM moves at pointer speed
   *  and the file is written once at the end. */
  let dragOffset = $state<{ dx: number; dy: number } | null>(null);

  let nodes = $derived(canvas.nodes);

  const MIN_SCALE = 0.05;
  const MAX_SCALE = 4;

  /**
   * Fit the whole canvas in view.
   *
   * Bails when the host has not been laid out yet. Without that guard it
   * divides by a zero-width rect, sets the scale to 0, and the canvas renders
   * as an empty grid with "0%" in the corner — which looks exactly like a
   * failure to load rather than a layout race.
   */
  function fit() {
    if (!host || nodes.length === 0) return;
    const r = host.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return;
    viewport = { w: r.width, h: r.height };

    const minX = Math.min(...nodes.map((n) => n.x));
    const minY = Math.min(...nodes.map((n) => n.y));
    const maxX = Math.max(...nodes.map((n) => n.x + n.width));
    const maxY = Math.max(...nodes.map((n) => n.y + n.height));
    const pad = 60;

    const sx = r.width / Math.max(1, maxX - minX + pad * 2);
    const sy = r.height / Math.max(1, maxY - minY + pad * 2);
    scale = clamp(Math.min(1.2, Math.min(sx, sy)));
    tx = r.width / 2 - ((minX + maxX) / 2) * scale;
    ty = r.height / 2 - ((minY + maxY) / 2) * scale;
  }

  function clamp(v: number): number {
    return Math.min(MAX_SCALE, Math.max(MIN_SCALE, v));
  }

  let fittedFor = $state<string | null>(null);

  $effect(() => {
    // Fit once per canvas, and keep trying until the element has real geometry.
    // rAF alone is not enough: it never fires in an occluded window, and the
    // canvas would stay unfitted.
    const path = canvas.path;
    if (fittedFor === path) return;
    let tries = 0;
    const attempt = () => {
      if (fittedFor === path) return;
      const r = host?.getBoundingClientRect();
      if (r && r.width >= 1 && r.height >= 1) {
        fit();
        fittedFor = path;
        return;
      }
      if (tries++ < 30) setTimeout(attempt, 16);
    };
    attempt();
  });

  $effect(() => {
    const onResize = () => {
      if (!host) return;
      const r = host.getBoundingClientRect();
      viewport = { w: r.width, h: r.height };
    };
    onResize();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  });

  /**
   * Whether a card is close enough to the viewport to render its contents.
   *
   * Generous margin: a card that pops in exactly at the edge reads as a glitch,
   * and one viewport of slack costs little.
   */
  function isLive(n: { x: number; y: number; width: number; height: number }): boolean {
    if (!viewport.w) return true;
    const left = n.x * scale + tx;
    const top = n.y * scale + ty;
    const right = left + n.width * scale;
    const bottom = top + n.height * scale;
    const m = Math.max(viewport.w, viewport.h) * 0.5;
    return right > -m && bottom > -m && left < viewport.w + m && top < viewport.h + m;
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const r = host!.getBoundingClientRect();
    const cx = e.clientX - r.left;
    const cy = e.clientY - r.top;
    const next = clamp(scale * Math.exp(-e.deltaY * 0.0015));
    // Zoom about the pointer.
    tx = cx - ((cx - tx) * next) / scale;
    ty = cy - ((cy - ty) * next) / scale;
    scale = next;
  }

  type Mode = null | { kind: "pan"; x: number; y: number } | { kind: "drag"; x: number; y: number };
  let mode: Mode = null;

  function onPointerDown(e: PointerEvent) {
    const card = (e.target as HTMLElement).closest<HTMLElement>("[data-node]");
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);

    if (card) {
      const id = card.dataset.node!;
      if (e.shiftKey) {
        const next = new Set(selection);
        next.has(id) ? next.delete(id) : next.add(id);
        selection = next;
      } else if (!selection.has(id)) {
        selection = new Set([id]);
      }
      mode = { kind: "drag", x: e.clientX, y: e.clientY };
      dragOffset = { dx: 0, dy: 0 };
    } else {
      if (!e.shiftKey) selection = new Set();
      mode = { kind: "pan", x: e.clientX, y: e.clientY };
    }
  }

  function onPointerMove(e: PointerEvent) {
    if (!mode) return;
    const dx = e.clientX - mode.x;
    const dy = e.clientY - mode.y;

    if (mode.kind === "pan") {
      tx += dx;
      ty += dy;
      mode = { kind: "pan", x: e.clientX, y: e.clientY };
    } else {
      // Divided by scale: a pointer moving 10 screen pixels at 40% zoom should
      // move the card 25 canvas units, or dragging feels wrong when zoomed.
      dragOffset = { dx: dx / scale, dy: dy / scale };
    }
  }

  function onPointerUp() {
    if (mode?.kind === "drag" && dragOffset) {
      const { dx, dy } = dragOffset;
      if (Math.abs(dx) > 0.5 || Math.abs(dy) > 0.5) {
        const moves = nodes
          .filter((n) => selection.has(n.id))
          .map((n) => ({
            id: n.id,
            // Snap to grid, as the spec asks. Rounded on the final position so
            // a drag does not feel like it is fighting the grid on the way.
            x: Math.round((n.x + dx) / GRID) * GRID,
            y: Math.round((n.y + dy) / GRID) * GRID,
          }));
        if (moves.length) onmove(moves);
      }
    }
    mode = null;
    dragOffset = null;
  }

  function position(n: { id: string; x: number; y: number }) {
    if (dragOffset && selection.has(n.id)) {
      return { x: n.x + dragOffset.dx, y: n.y + dragOffset.dy };
    }
    return { x: n.x, y: n.y };
  }

  /** A bezier between two cards, in canvas space. */
  function edgePath(edge: { fromNode: string; toNode: string; fromSide?: string; toSide?: string }) {
    const a = nodes.find((n) => n.id === edge.fromNode);
    const b = nodes.find((n) => n.id === edge.toNode);
    if (!a || !b) return "";

    const pa = position(a);
    const pb = position(b);
    const from = anchor(pa.x, pa.y, a.width, a.height, edge.fromSide);
    const to = anchor(pb.x, pb.y, b.width, b.height, edge.toSide);

    // Control points pushed out along each side's normal, so an edge leaves a
    // card perpendicular to the face it starts on rather than cutting a corner.
    const k = Math.max(40, Math.hypot(to.x - from.x, to.y - from.y) * 0.4);
    const c1 = { x: from.x + from.nx * k, y: from.y + from.ny * k };
    const c2 = { x: to.x + to.nx * k, y: to.y + to.ny * k };
    return `M ${from.x} ${from.y} C ${c1.x} ${c1.y}, ${c2.x} ${c2.y}, ${to.x} ${to.y}`;
  }

  function anchor(x: number, y: number, w: number, h: number, side?: string) {
    switch (side) {
      case "top":
        return { x: x + w / 2, y, nx: 0, ny: -1 };
      case "bottom":
        return { x: x + w / 2, y: y + h, nx: 0, ny: 1 };
      case "left":
        return { x, y: y + h / 2, nx: -1, ny: 0 };
      case "right":
        return { x: x + w, y: y + h / 2, nx: 1, ny: 0 };
      default:
        return { x: x + w / 2, y: y + h / 2, nx: 0, ny: 0 };
    }
  }

  let liveCount = $derived(nodes.filter(isLive).length);
</script>

<div
  class="canvas"
  bind:this={host}
  onwheel={onWheel}
  onpointerdown={onPointerDown}
  onpointermove={onPointerMove}
  onpointerup={onPointerUp}
  onpointercancel={onPointerUp}
  role="application"
  aria-label="Canvas"
>
  <!-- One transform layer. Pan and zoom move this, not 300 elements. -->
  <div class="layer" style="transform: matrix({scale}, 0, 0, {scale}, {tx}, {ty})">
    <svg class="edges" aria-hidden="true">
      {#each canvas.edges as e (e.id)}
        <path d={edgePath(e)} />
      {/each}
    </svg>

    {#each nodes as n (n.id)}
      {@const p = position(n)}
      {@const live = isLive(n)}
      <div
        class="card"
        class:selected={selection.has(n.id)}
        class:live
        class:group={n.kind === "group"}
        data-node={n.id}
        data-author={n.author ?? "none"}
        style="left:{p.x}px; top:{p.y}px; width:{n.width}px; height:{n.height}px;
               --edge: {2 / scale}px"
        ondblclick={() => n.file && onopen(n.file)}
        role="presentation"
      >
        <div class="head" style="font-size: {Math.max(9, 11 / scale)}px">
          {#if n.arcKind}<span class="badge">{n.arcKind}</span>{/if}
          <span class="title">
            {n.file ? (n.file.split("/").pop() ?? n.file) : n.kind === "link" ? n.url : ""}
          </span>
        </div>

        {#if live}
          <div class="body" style="font-size: {Math.max(8, 12 / scale)}px">
            {#if n.text}{n.text}{:else if n.file}{n.file}{:else if n.url}{n.url}{/if}
          </div>
        {:else}
          <!-- Visibly a placeholder. The spec requires that the user never has
               to wonder whether a card is editable. -->
          <div class="placeholder" aria-hidden="true"></div>
        {/if}
      </div>
    {/each}
  </div>

  <div class="hud data">
    <span>{nodes.length} cards</span>
    <span>{canvas.edges.length} edges</span>
    <span>{Math.round(scale * 100)}%</span>
    <span class="quiet">{liveCount} live</span>
    {#if selection.size}<span class="sel">{selection.size} selected</span>{/if}
  </div>
</div>

<style>
  .canvas {
    position: relative;
    height: 100%;
    overflow: hidden;
    background: var(--arc-bg-0);
    cursor: grab;
    touch-action: none;
    /* A faint grid, so snapping has something to snap to visually. */
    background-image:
      linear-gradient(var(--arc-bg-1) 1px, transparent 1px),
      linear-gradient(90deg, var(--arc-bg-1) 1px, transparent 1px);
    background-size: 40px 40px;
  }

  .layer {
    position: absolute;
    inset: 0;
    transform-origin: 0 0;
    will-change: transform;
  }

  .edges {
    position: absolute;
    /* Large enough to cover canvas coordinates in both directions without a
       per-frame bounds calculation. */
    left: -20000px;
    top: -20000px;
    width: 40000px;
    height: 40000px;
    overflow: visible;
    pointer-events: none;
  }
  .edges path {
    fill: none;
    stroke: var(--arc-line-strong);
    stroke-width: 2;
    /* Shifted because the SVG origin is offset from canvas origin. */
    transform: translate(20000px, 20000px);
  }

  .card {
    position: absolute;
    display: flex;
    flex-direction: column;
    background: var(--arc-bg-2);
    border: 1px solid var(--arc-line);
    border-radius: var(--arc-radius);
    overflow: hidden;
    cursor: default;
  }

  /* Constraint 6 on the canvas. A fixed *screen-space* width (--edge is
     1/scale), so the authorship stripe stays readable at 40% zoom, which is the
     gate. A card with no recorded history gets no stripe rather than a default
     one — inventing authorship would be worse than omitting it. */
  .card[data-author="human"] {
    border-left: var(--edge) solid var(--arc-human);
  }
  .card[data-author="agent"] {
    border-left: var(--edge) solid var(--arc-agent);
  }

  .card.selected {
    border-color: var(--arc-accent);
    box-shadow: 0 0 0 var(--edge) var(--arc-accent-dim);
  }
  .card.group {
    background: transparent;
    border-style: dashed;
  }

  .head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 6px;
    border-bottom: 1px solid var(--arc-line);
    color: var(--arc-fg-dim);
    font-family: var(--arc-font-data);
    white-space: nowrap;
    overflow: hidden;
  }
  .title {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .badge {
    flex: none;
    color: var(--arc-accent);
    border: 1px solid var(--arc-accent-dim);
    border-radius: 2px;
    padding: 0 3px;
  }

  .body {
    flex: 1;
    padding: 6px;
    overflow: hidden;
    color: var(--arc-fg-dim);
    white-space: pre-wrap;
    word-break: break-word;
    line-height: 1.4;
  }

  /* Not a card that looks ready and is not. Deliberately inert-looking. */
  .placeholder {
    flex: 1;
    margin: 6px;
    border-radius: 2px;
    background: repeating-linear-gradient(
      -45deg,
      var(--arc-bg-3) 0 6px,
      transparent 6px 12px
    );
    opacity: 0.5;
  }

  .hud {
    position: absolute;
    left: var(--arc-space-4);
    bottom: var(--arc-space-4);
    display: flex;
    gap: var(--arc-space-3);
    color: var(--arc-fg-faint);
    pointer-events: none;
  }
  .quiet {
    opacity: 0.6;
  }
  .sel {
    color: var(--arc-accent);
  }
</style>
