<script lang="ts">
  /**
   * The library, in three dimensions.
   *
   * Everything inside a Threlte `<Canvas>` lives here, because Threlte's
   * context only exists below it. The host owns the DOM, the HUD and the
   * interaction state; this owns the scene.
   *
   * ## Dimensional, not decorative
   *
   * Real geometry and real light: books with thickness, planks with a visible
   * edge, one key light and a soft fill. No wood grain, no coloured cloth, no
   * warm lamps. The token file's stated direction is "an oscilloscope, not a
   * spaceship cockpit — no neon, no glow, no gradients", and a photoreal
   * library would read as a different product bolted onto this one. Depth is
   * what makes a bookshelf legible as a bookshelf; texture is what would make
   * it someone else's.
   *
   * Every colour still comes from `tokens.css`, so the light and terminal
   * themes are correct without a second palette.
   *
   * ## Why books are instanced, and driven by hand
   *
   * A 5,000-note vault is 5,000 boxes. As individual meshes that is 5,000 draw
   * calls and the interactivity gate is gone; as one `InstancedMesh` it is one.
   * Planks stay ordinary meshes — there are tens of them, and they have to be
   * raycast individually to work as drop targets.
   *
   * The instance matrices are written in the frame loop rather than declared as
   * 5,000 `<Instance>` components. Declared, every animated frame is 5,000
   * component updates through Svelte's reactivity, which is what makes a spawn
   * or a settle unaffordable at the size this view is gated on. Written by
   * hand it is one array upload, and the pointer still resolves to a book
   * because three reports `instanceId` on the hit.
   *
   * ## Why no text is drawn in here
   *
   * `@threlte/extras` draws text with troika, which builds its font atlas in a
   * web worker spawned from a `blob:` URL. This app's CSP is `script-src
   * 'self'` with no `worker-src`, so the browser refuses that worker and every
   * label silently fails to appear — which is exactly what happened on the
   * first render. Widening a security policy that exists to enforce constraint
   * 3, for cosmetics, is the wrong trade.
   *
   * So labels are projected to screen coordinates here and drawn as DOM by the
   * host. That is better on its own merits: the text is real, so it is
   * selectable and reachable by a screen reader — which nothing drawn into a
   * canvas in this app has ever been — and it takes its colour straight from
   * `tokens.css` instead of through an extracted hex.
   */
  import { T, useTask, useThrelte } from "@threlte/core";
  import { interactivity, OrbitControls } from "@threlte/extras";
  import { Color, InstancedMesh, Object3D, Vector3 } from "three";
  import type { Book, Layout, Shelf } from "../lib/shelves";
  import { stepToward, type Live } from "../lib/shelves";
  import type { ScreenLabel } from "../lib/scene";
  import { BOOK_DEPTH, bookBox, endBox, labelPoint, plankBox, PLANK_DEPTH, SCALE } from "../lib/scene";

  let {
    model,
    palette,
    selected,
    hovered,
    held,
    heldAt,
    target,
    labelled,
    camera,
    motion,
    controls,
    onhover,
    onpick,
    ondrag,
    onlabels,
    onsize,
  }: {
    model: Layout;
    palette: Record<string, string>;
    selected: string | null;
    hovered: Book | null;
    held: Book | null;
    /** Where the held book currently is, in world units. */
    heldAt: { x: number; y: number } | null;
    target: Shelf | null;
    /** Books worth a spine title. */
    labelled: Book[];
    camera: { x: number; y: number; z: number };
    /**
     * `--arc-motion`, read from the tokens by the host.
     *
     * A CSS transition multiplies by this token for free. A frame loop does
     * not, so it is passed in and honoured explicitly — Appearance → Motion 0,
     * and the `prefers-reduced-motion` rule that forces the token to 0, are a
     * gate rather than a preference. `stepToward` is where it is obeyed, and it
     * lives in `shelves.ts` so a test can assert it rather than someone
     * squinting at a screen.
     */
    motion: number;
    /** Off while a book is in hand, or the camera fights the drag. */
    controls: boolean;
    onhover: (b: Book | null) => void;
    onpick: (b: Book) => void;
    ondrag: (world: { x: number; y: number }) => void;
    /** Screen positions for the DOM labels. */
    onlabels: (labels: ScreenLabel[]) => void;
    /**
     * The drawing surface's real size.
     *
     * The host used to measure its own element to frame the camera, which is
     * wrong whenever that element has not been laid out yet — a hidden pane
     * measures 0×0 and the aspect silently falls back to a guess. Threlte
     * already tracks the canvas it created, so this is the authority.
     */
    onsize: (w: number, h: number) => void;
  } = $props();

  // Pointer events on meshes come from here: it installs the raycaster into
  // Threlte's context. Without it every `onpointerdown` below is dead markup.
  interactivity();

  const ctx = useThrelte();
  const v = new Vector3();

  /**
   * Labels only move when the camera moves or the layout changes, and rebuilding
   * a hundred DOM nodes sixty times a second for a still picture is waste. The
   * camera's world matrix is the cheapest honest test — OrbitControls damping
   * keeps it changing for a moment after you let go, which is exactly when the
   * labels do need to keep up.
   */
  const lastCam = new Float64Array(16);
  let lastW = -1;
  let lastH = -1;
  let dirty = true;

  // ── The books ─────────────────────────────────────────────────────────────
  let mesh = $state<InstancedMesh | undefined>();
  const dummy = new Object3D();
  const colour = new Color();

  const books = $derived(model.shelves.flatMap((s) => s.books));

  /**
   * Where each book is *now*, as against where the layout wants it.
   *
   * A plain `Map`, not reactive state: it is written sixty times a second and
   * nothing should re-render because of it. Keyed by path, so a book keeps its
   * position across a re-layout and eases to the new one instead of jumping —
   * which is the difference between a diagram and a place.
   */
  const live = new Map<string, Live>();
  let animating = true;

  $effect(() => {
    // A new layout means a settle, and a chance to forget books that are gone.
    const paths = new Set(books.map((b) => b.path));
    for (const k of live.keys()) if (!paths.has(k)) live.delete(k);
    dirty = true;
    animating = true;
  });

  $effect(() => {
    // Colour and labels only — no movement.
    void hovered;
    void selected;
    void target;
    void held;
    void palette;
    dirty = true;
  });

  /**
   * Write every book's matrix and colour. Returns whether anything is still
   * moving, which is what decides if the next frame is worth drawing.
   */
  function draw(): boolean {
    const m = mesh;
    if (!m) return false;

    let busy = false;
    for (let i = 0; i < books.length && i < m.count; i++) {
      const b = books[i]!;

      let l = live.get(b.path);
      if (!l) {
        // A book that has just appeared rises out of its own shelf rather than
        // blinking into existence, so creating a note lands somewhere visible.
        l = { x: b.x, y: b.y + b.h, spawn: 0 };
        live.set(b.path, l);
      }
      if (stepToward(l, { x: b.x, y: b.y }, motion)) busy = true;

      if (held?.path === b.path) {
        // In hand, and drawn separately. Collapsed rather than removed so every
        // later book keeps its instance index.
        dummy.scale.set(0, 0, 0);
        dummy.position.set(0, 0, 0);
      } else {
        // Growing upward off the plank it stands on, so its feet never leave.
        const grow = Math.max(0.02, l.spawn);
        const foot = -(l.y + b.h) / SCALE;
        const h = (b.h * grow) / SCALE;
        dummy.position.set((l.x + b.w / 2) / SCALE, foot + h / 2, 0);
        dummy.scale.set(b.w / SCALE, h, BOOK_DEPTH);
      }
      dummy.updateMatrix();
      m.setMatrixAt(i, dummy.matrix);
      m.setColorAt(i, colour.set(colourOf(b)));
    }

    m.instanceMatrix.needsUpdate = true;
    if (m.instanceColor) m.instanceColor.needsUpdate = true;
    // Raycasting rejects against this first, so a stale one makes books
    // unclickable exactly where they have just moved to.
    m.computeBoundingSphere();
    return busy;
  }

  useTask(() => {
    const cam = ctx.camera.current;
    const { width: w, height: h } = ctx.size.current;
    if (!cam || w <= 0 || h <= 0) return;

    if (w !== lastW || h !== lastH) {
      lastW = w;
      lastH = h;
      dirty = true;
      onsize(w, h);
    }

    if (dirty || animating) {
      animating = draw();
      if (animating) {
        dirty = true;
        // Rendering is on demand, and an instance matrix written by hand is not
        // something Threlte can notice. Without this the first frame of an
        // animation is the only one drawn.
        ctx.invalidate();
      }
    }

    const m = cam.matrixWorld.elements;
    for (let i = 0; i < 16; i++) {
      if (lastCam[i] !== m[i]) {
        lastCam.set(m);
        dirty = true;
        break;
      }
    }
    if (!dirty) return;
    dirty = false;

    const out: ScreenLabel[] = [];

    /** A world point in screen pixels, or `null` if it is not on screen. */
    const project = (x: number, y: number, z: number): { x: number; y: number } | null => {
      v.set(x, y, z).project(cam);
      // Behind the camera, or off the edge: not worth a node in the DOM.
      if (v.z > 1) return null;
      const sx = (v.x * 0.5 + 0.5) * w;
      const sy = (-v.y * 0.5 + 0.5) * h;
      if (sx < -240 || sy < -60 || sx > w + 240 || sy > h + 60) return null;
      return { x: sx, y: sy };
    };

    for (const shelf of model.shelves) {
      const l = labelPoint(shelf);
      const at = project(l.x, l.y, l.z);
      if (!at) continue;
      // How much room the name has, measured the way it is drawn: from the left
      // end of the plank to the right end of it.
      const right = project(l.x + Math.max(shelf.w, 40) / SCALE, l.y, l.z);
      out.push({
        key: `s:${shelf.path}`,
        text: shelf.label,
        sub: shelf.books.length === 0 ? "empty" : String(shelf.books.length),
        kind: target?.path === shelf.path ? "shelf-target" : "shelf",
        x: at.x,
        y: at.y,
        len: right ? Math.abs(right.x - at.x) : 200,
      });
    }

    for (const b of labelled) {
      if (held?.path === b.path) continue;
      // A book still rising onto its shelf gets no title yet: it is the wrong
      // height, and a name sliding up the screen reads as a glitch.
      const l = live.get(b.path);
      if (l && l.spawn < 1) continue;
      const box = bookBox(l ? { ...b, x: l.x, y: l.y } : b);
      const at = project(box.x, box.y, PLANK_DEPTH / 2);
      if (!at) continue;
      // A spine title reads along the book, so its room is the book's height on
      // screen — which shrinks with distance, and below a few characters' worth
      // a label is a smear rather than a word.
      const top = project(box.x, box.y + box.h / 2, PLANK_DEPTH / 2);
      const len = top ? Math.abs(at.y - top.y) * 2 : 0;
      if (len < 26) continue;
      // And the spine has to be thick enough to hold a line of text across it,
      // or the title sits on the two books either side of the one it names.
      const side = project(box.x + box.w / 2, box.y, PLANK_DEPTH / 2);
      if (!side || Math.abs(side.x - at.x) * 2 < 11) continue;
      out.push({ key: `b:${b.path}`, text: b.label, kind: "spine", x: at.x, y: at.y, len });
    }

    onlabels(out);
  });

  function colourOf(b: Book): string {
    if (selected === b.path || hovered?.path === b.path) return palette.accent!;
    return b.isCanvas ? palette.canvasBook! : palette.book!;
  }

  // Big enough to catch the pointer anywhere the library reaches, plus room to
  // drag past its edges.
  const planeW = $derived(Math.max(model.width / SCALE, 4) * 3);
  const planeH = $derived(Math.max(model.height / SCALE, 4) * 3);
</script>

<T.PerspectiveCamera makeDefault fov={42} position={[camera.x, camera.y, camera.z]} near={0.05}>
  <!-- Constrained on purpose. Enough rotation for depth to read, not enough to
       end up behind the shelves wondering which way is out. -->
  <OrbitControls
    enabled={controls}
    enableDamping
    dampingFactor={0.08}
    target={[camera.x, camera.y, 0]}
    minDistance={0.6}
    maxDistance={Math.max(24, camera.z * 2.5)}
    minPolarAngle={Math.PI / 2 - 0.42}
    maxPolarAngle={Math.PI / 2 + 0.3}
    minAzimuthAngle={-0.55}
    maxAzimuthAngle={0.55}
  />
</T.PerspectiveCamera>

<T.AmbientLight intensity={1.5} color={palette.fg} />
<T.DirectionalLight position={[-3, 4, 6]} intensity={2.1} color={palette.fg} />
<T.DirectionalLight position={[4, -2, 3]} intensity={0.5} color={palette.accent} />

<!--
  The wall a dragged book slides along.

  Invisible, and only listening while something is in hand. Letting the
  raycaster project the pointer onto this plane is what avoids hand-rolling an
  unproject: the hit point arrives already in world units, and `layoutPoint`
  turns it straight back into something `shelfAt` understands.
-->
{#if held}
  <T.Mesh
    position={[model.width / (SCALE * 2), -model.height / (SCALE * 2), 0]}
    visible={false}
    onpointermove={(e: any) => e.point && ondrag({ x: e.point.x, y: e.point.y })}
  >
    <T.PlaneGeometry args={[planeW, planeH]} />
    <T.MeshBasicMaterial />
  </T.Mesh>
{/if}

{#each model.shelves as shelf (shelf.path)}
  {@const isTarget = target?.path === shelf.path}

  <!-- One plank per row, so wrapped books stand on something. -->
  {#each shelf.planks as plankY, i (i)}
    {@const p = plankBox(shelf, plankY)}
    <T.Mesh position={[p.x, p.y, p.z]}>
      <T.BoxGeometry args={[p.w, p.h, p.d]} />
      <T.MeshStandardMaterial
        color={isTarget ? palette.accent : palette.plank}
        roughness={0.85}
        metalness={0}
      />
    </T.Mesh>
  {/each}

  <!-- The upright at the left end: what turns a plank into a bay. -->
  {@const e = endBox(shelf)}
  <T.Mesh position={[e.x, e.y, e.z]}>
    <T.BoxGeometry args={[e.w, e.h, e.d]} />
    <T.MeshStandardMaterial
      color={isTarget ? palette.accent : palette.plank}
      roughness={0.85}
      metalness={0}
    />
  </T.Mesh>
{/each}

<!--
  Every book, in one mesh.

  Re-created when the count changes, because an `InstancedMesh` fixes its
  capacity at construction. `live` is keyed by path and survives that, so a
  rebuild does not restart every book's animation.

  `frustumCulled` is off: the bounding volume is derived from instance matrices
  this component writes by hand, and a stale one culls books that are on screen.
-->
{#key books.length}
  <T.InstancedMesh
    bind:ref={mesh}
    args={[undefined, undefined, Math.max(1, books.length)]}
    frustumCulled={false}
    onpointermove={(e: any) => {
      const b = books[e.instanceId ?? -1];
      if (b && b.path !== hovered?.path) onhover(b);
    }}
    onpointerleave={() => onhover(null)}
    onpointerdown={(e: any) => {
      const b = books[e.instanceId ?? -1];
      if (b) onpick(b);
    }}
  >
    <T.BoxGeometry args={[1, 1, 1]} />
    <T.MeshStandardMaterial roughness={0.62} metalness={0.04} />
  </T.InstancedMesh>
{/key}

<!--
  The book in your hand.

  Drawn last and without a depth test rather than pushed toward the camera. The
  lift was the obvious way to keep it clear of the bay it came from, and it was
  wrong: `heldAt` is where the pointer's ray crosses the wall at z = 0, so any
  book sitting off that plane is slid away from the cursor by perspective — a
  few pixels near the middle of the view, more at the edges. You would be aiming
  at one shelf and dropping onto another. Sitting it on the plane keeps it
  exactly under the pointer, and drawing it last keeps it visible anyway.
-->
{#if held && heldAt}
  {@const box = bookBox(held)}
  <T.Mesh position={[heldAt.x, heldAt.y, 0]} renderOrder={999}>
    <T.BoxGeometry args={[box.w, box.h, box.d]} />
    <T.MeshStandardMaterial
      color={palette.accent}
      roughness={0.5}
      metalness={0.05}
      depthTest={false}
    />
  </T.Mesh>
{/if}
