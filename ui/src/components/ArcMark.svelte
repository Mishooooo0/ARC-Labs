<script lang="ts">
  /**
   * The mark: a segmented triangle around a vault door.
   *
   * Redrawn rather than imported, which is the same decision the old bracket
   * mark was built on and still the right one — it inherits `currentColor`, so
   * it is correct in dark, light and terminal themes without three assets; and
   * it needs no asset pipeline, no network fetch and no CSP exception.
   *
   * It is a simplification of the full logo on purpose. That artwork carries
   * brushed metal, a drop shadow and a six-spoke handle, none of which survive
   * being drawn at 16 px — they turn to mud. What reads at this size is the
   * silhouette: three ribbon segments that do not quite close, and a hexagon
   * held inside them. The raster logo is still what ships as the app icon,
   * where there are 256 px to spend on the detail.
   *
   * The gaps at the corners are the point rather than a shortcut. They are what
   * makes the triangle read as three pieces meeting rather than one outline,
   * and they survive all the way down to 16 px where every interior detail has
   * already gone.
   */
  let { size = 16 }: { size?: number } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  aria-hidden="true"
  class="mark"
>
  <!-- Three segments, not a closed path: the corners stay open. -->
  <g stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
    <path d="M11.05 4.2 L3.45 17.8" />
    <path d="M12.95 4.2 L20.55 17.8" />
    <path d="M4.4 19.5 L19.6 19.5" />
  </g>

  <!-- The vault door, flat-topped as in the logo, and inset so it never touches
       the ribbons. Its handle is deliberately absent: this is drawn at 14, 20
       and 22 px, and at 14 the handle is under one and a half pixels and merges
       with the hexagon's own stroke. The favicon keeps it, having 32. -->
  <path
    d="M16.2 12.7 L14.1 9.06 L9.9 9.06 L7.8 12.7 L9.9 16.34 L14.1 16.34 Z"
    stroke="currentColor"
    stroke-width="1.4"
    stroke-linejoin="round"
  />
</svg>

<style>
  .mark {
    display: block;
    flex: none;
  }
</style>
