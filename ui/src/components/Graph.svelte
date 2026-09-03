<script lang="ts">
  /**
   * The vault graph.
   *
   * Layout runs in a worker; drawing goes to a `<canvas>`. Neither is a
   * preference — the gate is that this stays interactive at 5,000 nodes, and SVG
   * gives up several thousand elements earlier than that while a main-thread
   * simulation would eat the frame budget the editor needs.
   *
   * ## Observed and inferred are two registers, and they never blur
   *
   * A solid grey line is a `[[wikilink]]` someone wrote. Links that resolve to
   * nothing are not drawn at all, because an edge to a note that does not exist
   * would be a relationship the vault does not contain.
   *
   * A dashed blue line is something Weave *guessed*. It arrives in a separate
   * prop, of a different type, from a different table - this renderer could not
   * confuse the two if it tried.
   *
   * The distinction has to survive **every zoom level**, so it is carried three
   * ways at once: colour (agent blue vs. line grey), opacity, and a dash pattern
   * expressed in *screen* pixels rather than graph units, so it neither
   * disappears when zoomed out nor turns into a solid line when zoomed in. Any
   * one of the three could be defeated by a colour-blind viewer, a small screen
   * or a screenshot; all three together cannot.
   */
  import { onMount } from "svelte";
  import type { GraphData, LinkSuggestion } from "../lib/types";
  import GraphWorker from "../lib/graph.worker?worker";

  let { data, selected, inferred = [], onopen }: {
    data: GraphData;
    selected: string | null;
    /** Weave's guesses. A different type from `data.edges`, deliberately. */
    inferred?: LinkSuggestion[];
    onopen: (path: string) => void;
  } = $props();

  /** Inferred edges can be hidden. Observed ones cannot - they are the vault. */
  let showInferred = $state(true);

  let canvas = $state<HTMLCanvasElement | null>(null);
  let host = $state<HTMLDivElement | null>(null);
  let alpha = $state(1);
  let hovered = $state<number | null>(null);

  // View transform. Pan and zoom are applied when drawing rather than by moving
  // the data, so the simulation never learns the viewport exists.
  let scale = 1;
  let tx = 0;
  let ty = 0;

  let positions: Float32Array | null = null;
  let worker: Worker | null = null;
  let raf = 0;

  /** Colours are read from the token file at draw time, so the graph follows the
   *  theme without this component holding a single literal. */
  function tokens(el: HTMLElement) {
    const s = getComputedStyle(el);
    return {
      edge: s.getPropertyValue("--arc-line-strong").trim(),
      node: s.getPropertyValue("--arc-fg-dim").trim(),
      nodeHi: s.getPropertyValue("--arc-accent").trim(),
      canvasNode: s.getPropertyValue("--arc-fg-faint").trim(),
      agent: s.getPropertyValue("--arc-agent").trim(),
      label: s.getPropertyValue("--arc-fg").trim(),
      bg: s.getPropertyValue("--arc-bg-0").trim(),
    };
  }

  function draw() {
    if (!canvas || !host || !positions) return;
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

    // Edges in one path. 57,000 individual strokes would be 57,000 state
    // changes; one path is one.
    ctx.strokeStyle = t.edge;
    ctx.globalAlpha = 0.5;
    ctx.lineWidth = 1 / scale;
    ctx.beginPath();
    for (const e of data.edges) {
      const s = e.source * 2;
      const d = e.target * 2;
      ctx.moveTo(positions[s]!, positions[s + 1]!);
      ctx.lineTo(positions[d]!, positions[d + 1]!);
    }
    ctx.stroke();
    ctx.globalAlpha = 1;

    // Inferred edges, in the agent register. Drawn after the observed ones so
    // they are visible, and dimmer so they never dominate what is actually
    // there.
    if (showInferred && inferredEdges.length) {
      ctx.strokeStyle = t.agent;
      ctx.globalAlpha = 0.55;
      ctx.lineWidth = 1.25 / scale;
      // Dash lengths divided by scale, so the pattern is a constant number of
      // *screen* pixels at any zoom. This is the part that makes the
      // distinction survive zooming out to the whole vault.
      ctx.setLineDash([4 / scale, 4 / scale]);
      ctx.beginPath();
      for (const e of inferredEdges) {
        const a = e.source * 2;
        const b = e.target * 2;
        ctx.moveTo(positions[a]!, positions[a + 1]!);
        ctx.lineTo(positions[b]!, positions[b + 1]!);
      }
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.globalAlpha = 1;
    }

    const selectedIdx = selected ? data.nodes.findIndex((n) => n.path === selected) : -1;

    for (let i = 0; i < data.nodes.length; i++) {
      const n = data.nodes[i]!;
      const x = positions[i * 2]!;
      const y = positions[i * 2 + 1]!;
      // Degree sets radius, so hubs read as hubs without a legend.
      //
      // Two things were wrong here. The floor was 2px, which on a vault whose
      // notes are not yet linked meant every node was a 2px dot — reported as
      // "barely visible", and correctly. And the whole thing was divided by
      // `scale`, which pins the radius to a constant number of SCREEN pixels:
      // zooming in moved the nodes apart but never made one bigger, so zooming
      // to see something better did not work at all.
      //
      // `scale ** 0.6` keeps most of that stability — a 5,000-node graph does
      // not turn into overlapping blobs when you zoom out — while letting a
      // deliberate zoom actually enlarge what you are looking at.
      const base = 5 + Math.min(9, Math.sqrt(n.degree) * 2.1);
      const r = base / Math.pow(scale, 0.6);
      const focused = i === selectedIdx || i === hovered;

      ctx.beginPath();
      ctx.arc(x, y, r, 0, Math.PI * 2);
      ctx.fillStyle = focused ? t.nodeHi : n.isCanvas ? t.canvasNode : t.node;
      ctx.fill();

      // A ring rather than a colour change alone: on a dark ground a hovered
      // node and an ordinary one are two similar greys, and the difference has
      // to survive a screenshot.
      if (focused) {
        ctx.beginPath();
        ctx.arc(x, y, r + 4 / scale, 0, Math.PI * 2);
        ctx.strokeStyle = t.nodeHi;
        ctx.lineWidth = 1.5 / scale;
        ctx.stroke();
      }
    }

    // Labels.
    //
    // These used to require BOTH `scale > 1.4` and `degree >= 2`, which on a
    // small or sparsely linked vault meant no label ever appeared at any zoom:
    // you could zoom all the way in and still be looking at unlabelled dots.
    //
    // A graph small enough to label is labelled at rest, and the degree gate
    // only applies once there are too many to draw — which is what the gate was
    // for. The threshold drops to 1.0 so the first click of zoom does something.
    // A small graph is labelled at rest. A large one keeps the exact threshold
    // it always had, so this change cannot cost the 5,000-node frame budget:
    // above 150 nodes the drawn set is identical to before, and below it the
    // label pass is trivially small.
    const small = data.nodes.length <= 150;
    if (small || scale > 1.4) {
      ctx.fillStyle = t.label;
      ctx.font = `${11 / Math.pow(scale, 0.6)}px ui-sans-serif, system-ui, sans-serif`;
      ctx.textAlign = "center";
      ctx.textBaseline = "top";
      for (let i = 0; i < data.nodes.length; i++) {
        const n = data.nodes[i]!;
        // On a large graph, only hubs and the focused node earn the space.
        if (!small && n.degree < 2 && i !== hovered && i !== selectedIdx) continue;
        const x = positions[i * 2]!;
        const y = positions[i * 2 + 1]!;
        // Skip anything off screen: at 5,000 nodes most of them are.
        const sx = x * scale + tx;
        const sy = y * scale + ty;
        if (sx < -40 || sy < -20 || sx > w + 40 || sy > h + 20) continue;
        const nodeR = (5 + Math.min(9, Math.sqrt(n.degree) * 2.1)) / Math.pow(scale, 0.6);
        ctx.fillText(n.title.slice(0, 28), x, y + nodeR + 3 / scale);
      }
    }

    ctx.restore();
  }

  /**
   * Suggestions resolved to node indices.
   *
   * A suggestion whose endpoints are not both in the graph is dropped rather
   * than drawn at coordinate zero - which is what an unchecked lookup produces,
   * and it looks like a real edge to somewhere.
   */
  const inferredEdges = $derived.by(() => {
    if (!inferred.length) return [] as { source: number; target: number; score: number }[];
    const index = new Map(data.nodes.map((n, i) => [n.path, i]));
    const out: { source: number; target: number; score: number }[] = [];
    for (const s of inferred) {
      const a = index.get(s.srcPath);
      const b = index.get(s.dstPath);
      if (a === undefined || b === undefined) continue;
      out.push({ source: a, target: b, score: s.score });
    }
    return out;
  });

  /** Suggestions touching the hovered note, so the score is reachable here too. */
  const hoveredSuggestions = $derived.by(() => {
    if (hovered === null || !inferred.length) return [] as LinkSuggestion[];
    const path = data.nodes[hovered]?.path;
    if (!path) return [];
    return inferred.filter((s) => s.srcPath === path || s.dstPath === path);
  });

  let painted = false;

  /**
   * Coalesce draws to one per frame.
   *
   * The first frame is drawn **synchronously** rather than waiting for a
   * `requestAnimationFrame`: a graph that appears the instant the layout starts
   * reads as responsive, and one that waits a frame for its first pixel reads as
   * broken. It also means the view is correct in environments where rAF is
   * starved — an occluded window, a background tab — instead of staying blank
   * until something happens to wake it.
   */
  function schedule() {
    if (!painted) {
      painted = true;
      draw();
      return;
    }
    if (raf) return;
    raf = requestAnimationFrame(() => {
      raf = 0;
      draw();
    });
  }

  /** Nearest node to a screen point, in graph coordinates. */
  function nodeAt(clientX: number, clientY: number): number | null {
    if (!canvas || !positions) return null;
    const rect = canvas.getBoundingClientRect();
    const x = (clientX - rect.left - tx) / scale;
    const y = (clientY - rect.top - ty) / scale;
    const threshold = 12 / scale;

    let best: number | null = null;
    let bestDist = threshold * threshold;
    for (let i = 0; i < data.nodes.length; i++) {
      const dx = positions[i * 2]! - x;
      const dy = positions[i * 2 + 1]! - y;
      const d = dx * dx + dy * dy;
      if (d < bestDist) {
        bestDist = d;
        best = i;
      }
    }
    return best;
  }

  onMount(() => {
    const w = new GraphWorker();
    worker = w;
    w.onmessage = (e: MessageEvent) => {
      if (e.data.type === "tick") {
        positions = e.data.positions as Float32Array;
        alpha = e.data.alpha;
        schedule();
      } else if (e.data.type === "settled") {
        alpha = 0;
      }
    };

    const rect = host?.getBoundingClientRect();
    // Both arrays are rebuilt as plain objects before crossing to the worker.
    // `data` arrives as Svelte state, which is a reactive Proxy, and a Proxy
    // cannot be structured-cloned: passing it straight through throws
    // DataCloneError and the worker silently never starts.
    w.postMessage({
      type: "start",
      nodes: data.nodes.map((n) => ({ id: n.id, degree: n.degree })),
      edges: data.edges.map((e) => ({ source: e.source, target: e.target })),
      width: rect?.width ?? 800,
      height: rect?.height ?? 600,
    });

    const onResize = () => {
      // Force the next schedule() to draw even if a previous frame never fired.
      raf = 0;
      schedule();
    };
    // Coming back from an occluded or background state: any rAF queued before
    // is dead, so clear it and repaint from whatever the layout reached.
    const onVisible = () => {
      if (document.visibilityState === "visible") {
        raf = 0;
        draw();
      }
    };
    window.addEventListener("resize", onResize);
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      window.removeEventListener("resize", onResize);
      document.removeEventListener("visibilitychange", onVisible);
      w.postMessage({ type: "stop" });
      w.terminate();
      worker = null;
      if (raf) cancelAnimationFrame(raf);
    };
  });

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const rect = canvas!.getBoundingClientRect();
    const cx = e.clientX - rect.left;
    const cy = e.clientY - rect.top;
    const factor = Math.exp(-e.deltaY * 0.0015);
    const next = Math.min(8, Math.max(0.1, scale * factor));
    // Zoom about the pointer, so the thing under the cursor stays under it.
    tx = cx - ((cx - tx) * next) / scale;
    ty = cy - ((cy - ty) * next) / scale;
    scale = next;
    schedule();
  }

  // Reactive: it drives the grab/grabbing cursor class in the markup.
  let dragging = $state(false);
  let lastX = 0;
  let lastY = 0;
  let moved = 0;

  function onPointerDown(e: PointerEvent) {
    dragging = true;
    moved = 0;
    lastX = e.clientX;
    lastY = e.clientY;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  }
  function onPointerMove(e: PointerEvent) {
    if (dragging) {
      const dx = e.clientX - lastX;
      const dy = e.clientY - lastY;
      moved += Math.abs(dx) + Math.abs(dy);
      tx += dx;
      ty += dy;
      lastX = e.clientX;
      lastY = e.clientY;
      schedule();
      return;
    }
    const hit = nodeAt(e.clientX, e.clientY);
    if (hit !== hovered) {
      hovered = hit;
      schedule();
    }
  }
  function onPointerUp(e: PointerEvent) {
    dragging = false;
    // A drag is a pan, not a click. 4 px of slop so a slightly shaky click on a
    // node still opens it.
    if (moved < 4) {
      const hit = nodeAt(e.clientX, e.clientY);
      if (hit !== null) onopen(data.nodes[hit]!.path);
    }
  }
</script>

<div class="graph" bind:this={host}>
  <canvas
    bind:this={canvas}
    onwheel={onWheel}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    class:grabbing={dragging}
  ></canvas>

  <div class="legend data">
    <span>{data.nodes.length.toLocaleString()} notes</span>
    <span><span class="key observed" aria-hidden="true"></span
      >{data.edges.length.toLocaleString()} links</span>

    {#if inferredEdges.length}
      <!-- A toggle rather than a caption: the surest way to know which lines are
           guesses is to be able to switch them off and watch them go. -->
      <button
        class="key-toggle"
        class:off={!showInferred}
        onclick={() => {
          showInferred = !showInferred;
          schedule();
        }}
        title="Inferred, not observed. Click to hide."
      >
        <span class="key inferred" aria-hidden="true"></span>
        {inferredEdges.length} suggested
      </button>
    {/if}

    {#if alpha > 0.02}<span class="settling">settling…</span>{/if}
    {#if hovered !== null}
      <span class="hover">{data.nodes[hovered]?.title}</span>
      {#each hoveredSuggestions.slice(0, 3) as s (s.id)}
        <!-- Score and model travel with the edge everywhere it appears. -->
        <span class="hover-inferred">
          ⇢ {s.srcPath === data.nodes[hovered]?.path ? s.dstTitle : s.srcTitle}
          {s.score.toFixed(2)} · {s.model}
        </span>
      {/each}
    {/if}
  </div>
</div>

<style>
  .graph {
    position: relative;
    height: 100%;
    background: var(--arc-bg-0);
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
  /* The swatches are the legend. They use the same three cues as the canvas:
     colour, opacity, and solid-versus-dashed. */
  .key {
    display: inline-block;
    width: 14px;
    height: 0;
    margin-right: 6px;
    vertical-align: middle;
  }
  .key.observed {
    border-top: 1px solid var(--arc-line-strong);
  }
  .key.inferred {
    border-top: 1.5px dashed var(--arc-agent);
    opacity: 0.7;
  }
  .key-toggle {
    pointer-events: auto;
    background: none;
    border: 0;
    padding: 0;
    font: inherit;
    color: var(--arc-agent);
    cursor: pointer;
  }
  .key-toggle.off {
    color: var(--arc-fg-faint);
  }
  .key-toggle.off .key.inferred {
    border-top-color: var(--arc-fg-faint);
  }
  .hover-inferred {
    color: var(--arc-agent);
    opacity: 0.8;
    max-width: 34ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .settling {
    color: var(--arc-accent-dim);
  }
  .hover {
    color: var(--arc-fg-dim);
    max-width: 40ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
