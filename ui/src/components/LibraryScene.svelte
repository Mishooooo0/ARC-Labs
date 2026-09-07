<script lang="ts">
  /**
   * The bookcase, in three dimensions.
   *
   * Everything inside a Threlte `<Canvas>` lives here, because Threlte's
   * context only exists below it. The host owns the DOM, the HUD and the
   * interaction state; this owns the scene.
   *
   * ## A box first, boards second
   *
   * The version before this drew a plank per folder with a stub upright at the
   * left end of each — every board its own object, nothing enclosing anything,
   * which is why it read as a diagram of a bookshelf rather than a bookshelf.
   * A bookcase is a carcass: two sides, a top with a lip, a back panel and a
   * plinth. The boards are what goes *in* it, and the plinth is the ground
   * unfiled notes lie on.
   *
   * The back panel earns its place twice. It closes the box, and it is what
   * stops you reaching through the case from behind once the view can be
   * rotated the whole way round.
   *
   * ## Why books are instanced, and driven by hand
   *
   * A 5,000-note vault is 5,000 boxes. As individual meshes that is 5,000 draw
   * calls and the interactivity gate is gone; as one `InstancedMesh` it is one.
   * The carcass and the boards stay ordinary meshes — there are a handful, and
   * they have to be raycast individually to work as drop targets.
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
   * first 3D render. Widening a security policy that exists to enforce
   * constraint 3, for cosmetics, is the wrong trade.
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
  import type { Book, Compartment, Layout } from "../lib/shelves";
  import { stepToward, type Live } from "../lib/shelves";
  import type { ScreenLabel } from "../lib/scene";
  import {
    boardBox,
    bookPose,
    BOOK_Z,
    caseParts,
    labelPoint,
    spineEnds,
    SCALE,
  } from "../lib/scene";
  import type { Finish } from "../lib/finishes";
  import { grainTexture } from "../lib/grain";

  let {
    model,
    finish,
    accent,
    selected,
    hovered,
    held,
    heldAt,
    target,
    labelled,
    camera,
    motion,
    controls,
    resetKey,
    onhover,
    onpick,
    ondrag,
    onlabels,
    onsize,
  }: {
    model: Layout;
    finish: Finish;
    /** `--arc-accent`, for hover and the highlighted drop target. */
    accent: string;
    selected: string | null;
    hovered: Book | null;
    held: Book | null;
    /** Where the held book currently is, in world units. */
    heldAt: { x: number; y: number } | null;
    target: Compartment | null;
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
    /** Bumped to put the view back where it started. */
    resetKey: number;
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
  const forward = new Vector3();

  const grainUp = grainTexture(false);
  const grainAcross = grainTexture(true);

  const parts = $derived(caseParts(model.bookcase));

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

  const books = $derived(model.bookcase.compartments.flatMap((c) => c.books));

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
    void finish;
    void accent;
    dirty = true;
  });

  function colourOf(b: Book): string {
    if (selected === b.path || hovered?.path === b.path) return accent;
    return b.isCanvas ? finish.bookAlt : finish.book;
  }

  /**
   * Where a book is *this frame*, part-way through rising onto its shelf.
   *
   * One function because the drawing and the label have to agree about it. They
   * did not: the label used the book's final size and simply refused to appear
   * until the spawn finished. Any interruption then left a bookcase with no
   * names on it — and `requestAnimationFrame` stops while a tab is in the
   * background, so \"interrupted\" is just \"switched away and came back\".
   *
   * A book grows about its foot. The foot is `y + h`, so a shorter book has to
   * start further down, or it rises off its own shelf as it grows.
   */
  function livePose(b: Book): Book {
    const l = live.get(b.path);
    if (!l) return b;
    const h = b.h * Math.max(0.02, l.spawn);
    return { ...b, x: l.x, y: l.y + b.h - h, h };
  }

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
        dummy.rotation.set(0, 0, 0);
      } else {
        const pose = bookPose(livePose(b));
        dummy.position.set(pose.x, pose.y, pose.z);
        dummy.rotation.set(0, 0, pose.lean);
        dummy.scale.set(pose.w, pose.h, pose.d);
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

    /*
      Is the case being looked at from the front?

      DOM labels have no idea the back panel exists — they are drawn over the
      canvas, not in it — so from behind the case every shelf name and every
      spine would print straight through the wood. The camera looks down its own
      −Z, so a camera in front of the case has a forward vector pointing away
      from +Z.
    */
    cam.getWorldDirection(forward);
    const fromFront = forward.z < -0.08;

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

    if (fromFront) {
      for (const comp of model.bookcase.compartments) {
        const l = labelPoint(comp);
        const at = project(l.x, l.y, l.z);
        if (!at) continue;
        // How much room the name has, measured the way it is drawn: from the
        // left end of its span to the right end of it.
        const right = project(l.x + comp.w / SCALE, l.y, l.z);
        out.push({
          key: `s:${comp.path}`,
          text: comp.isFloor ? `${comp.label} · loose` : comp.label,
          sub: comp.books.length === 0 ? "empty" : String(comp.books.length),
          kind: target?.path === comp.path ? "shelf-target" : "shelf",
          x: at.x,
          y: at.y,
          len: right ? Math.abs(right.x - at.x) : 200,
          angle: 0,
        });
      }

      for (const b of labelled) {
        if (held?.path === b.path) continue;

        // The book as it is right now, not as it will be. A name that waits for
        // the spawn to finish never appears at all if the spawn is interrupted,
        // and it grows out of the shelf with its book this way instead.
        const at = livePose(b);
        const ends = spineEnds(at);
        const foot = project(ends.foot[0], ends.foot[1], ends.foot[2]);
        const head = project(ends.head[0], ends.head[1], ends.head[2]);
        if (!foot || !head) continue;

        // Along the book's *projected* axis, which is only straight up the
        // screen when you are looking at the case head-on — and the whole point
        // of being able to walk around it is that usually you are not.
        const dx = head.x - foot.x;
        const dy = head.y - foot.y;
        const len = Math.hypot(dx, dy);
        if (len < 26) continue;

        // And the spine has to be thick enough to hold a line of text across
        // it, or the title sits on the two books either side of the one it
        // names.
        const pose = bookPose(at);
        const side = project(pose.x + pose.w / 2, pose.y, pose.z + pose.d / 2);
        const mid = project(pose.x, pose.y, pose.z + pose.d / 2);
        if (!side || !mid || Math.hypot(side.x - mid.x, side.y - mid.y) * 2 < 11) continue;

        out.push({
          key: `b:${b.path}`,
          text: b.label,
          kind: "spine",
          x: (foot.x + head.x) / 2,
          y: (foot.y + head.y) / 2,
          len,
          // Text reads from the foot upward, so the angle is that of the
          // foot→head vector turned into screen degrees.
          angle: (Math.atan2(dy, dx) * 180) / Math.PI,
        });
      }
    }

    onlabels(out);
  });

  // Big enough to catch the pointer anywhere the case reaches, plus room to
  // drag past its edges.
  const planeW = $derived(Math.max(model.width / SCALE, 4) * 3);
  const planeH = $derived(Math.max(model.height / SCALE, 4) * 3);
</script>

<!--
  Re-created when `resetKey` changes, which is the whole of "reset view": a
  fresh camera and fresh controls start at the framing again. Cheaper and more
  obviously correct than persuading OrbitControls to forget where it has been.
-->
{#key resetKey}
  <T.PerspectiveCamera makeDefault fov={42} position={[camera.x, camera.y, camera.z]} near={0.05}>
    <!--
      Free to walk around.

      The azimuth clamp that used to be here (±0.55 rad) is what made this feel
      bolted to a wall. Only the polar angle is limited now, and only enough to
      keep you off the floor and off the top panel — from directly overhead a
      bookcase is a rectangle and nothing about it is legible.
    -->
    <OrbitControls
      enabled={controls}
      enableDamping
      enablePan
      dampingFactor={0.08}
      target={[camera.x, camera.y, 0]}
      minDistance={0.4}
      maxDistance={Math.max(24, camera.z * 3)}
      minPolarAngle={0.15}
      maxPolarAngle={1.75}
    />
  </T.PerspectiveCamera>
{/key}

<T.AmbientLight intensity={1.35} />
<!-- Key light from the upper left, as in the reference photograph. -->
<T.DirectionalLight position={[-3, 4, 6]} intensity={2.0} />
<!-- A cool fill from the other side, so the right-hand side panel is not black. -->
<T.DirectionalLight position={[5, -1, 4]} intensity={0.55} />
<!-- Just enough from behind to separate the case from the ground it floats on. -->
<T.DirectionalLight position={[-2, 2, -5]} intensity={0.4} />

<!--
  The wall a dragged book slides along.

  Invisible, and only listening while something is in hand. Letting the
  raycaster project the pointer onto this plane is what avoids hand-rolling an
  unproject: the hit point arrives already in world units, and `layoutPoint`
  turns it straight back into something `compartmentAt` understands — from any
  camera angle, because the plane is fixed in the world and the camera is what
  moves.
-->
{#if held}
  <T.Mesh
    position={[model.width / (SCALE * 2), -model.height / (SCALE * 2), BOOK_Z]}
    visible={false}
    onpointermove={(e: any) => e.point && ondrag({ x: e.point.x, y: e.point.y })}
  >
    <T.PlaneGeometry args={[planeW, planeH]} />
    <T.MeshBasicMaterial />
  </T.Mesh>
{/if}

<!-- ── The carcass ─────────────────────────────────────────────────────────
     Sides and top take the grain along their length; the back is in shadow and
     the plinth is what unfiled notes stand on. -->
{#each [parts.left, parts.right] as p, i (i)}
  <T.Mesh position={[p.x, p.y, p.z]}>
    <T.BoxGeometry args={[p.w, p.h, p.d]} />
    <T.MeshStandardMaterial
      color={finish.case}
      map={grainUp}
      roughness={0.72}
      metalness={0.02}
    />
  </T.Mesh>
{/each}

<T.Mesh position={[parts.top.x, parts.top.y, parts.top.z]}>
  <T.BoxGeometry args={[parts.top.w, parts.top.h, parts.top.d]} />
  <T.MeshStandardMaterial
    color={finish.edge}
    map={grainAcross}
    roughness={0.7}
    metalness={0.02}
  />
</T.Mesh>

<T.Mesh position={[parts.back.x, parts.back.y, parts.back.z]}>
  <T.BoxGeometry args={[parts.back.w, parts.back.h, parts.back.d]} />
  <T.MeshStandardMaterial color={finish.back} map={grainUp} roughness={0.9} metalness={0} />
</T.Mesh>

<T.Mesh position={[parts.plinth.x, parts.plinth.y, parts.plinth.z]}>
  <T.BoxGeometry args={[parts.plinth.w, parts.plinth.h, parts.plinth.d]} />
  <T.MeshStandardMaterial
    color={finish.case}
    map={grainAcross}
    roughness={0.75}
    metalness={0.02}
  />
</T.Mesh>

<!-- ── The boards. One per folder, and a drop target each. ───────────────── -->
{#each model.bookcase.compartments as comp, i (comp.path)}
  {#if !comp.isFloor}
    <!-- The floor is always last, so a folder's board carries its own index. -->
    {@const board = model.bookcase.boards[i]}
    {#if board}
      {@const b = boardBox(board)}
      <T.Mesh position={[b.x, b.y, b.z]}>
        <T.BoxGeometry args={[b.w, b.h, b.d]} />
        <T.MeshStandardMaterial
          color={target?.path === comp.path ? accent : finish.board}
          map={grainAcross}
          roughness={0.68}
          metalness={0.02}
        />
      </T.Mesh>
    {/if}
  {/if}
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
    <T.MeshStandardMaterial roughness={0.66} metalness={0.02} />
  </T.InstancedMesh>
{/key}

<!--
  The book in your hand.

  Drawn last and without a depth test rather than pushed toward the camera.
  `heldAt` is where the pointer's ray crosses the book plane, so any book
  sitting off that plane is slid away from the cursor by perspective — a few
  pixels near the middle of the view, more at the edges. You would be aiming at
  one shelf and dropping onto another. Sitting it on the plane keeps it exactly
  under the pointer, and drawing it last keeps it visible anyway.
-->
{#if held && heldAt}
  {@const pose = bookPose(held)}
  <T.Mesh position={[heldAt.x, heldAt.y, BOOK_Z]} renderOrder={999}>
    <T.BoxGeometry args={[pose.w, pose.h, pose.d]} />
    <T.MeshStandardMaterial
      color={accent}
      roughness={0.5}
      metalness={0.05}
      depthTest={false}
    />
  </T.Mesh>
{/if}
