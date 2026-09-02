<script lang="ts">
  // The mark: a filled square inside an open bracket. A specimen under an
  // instrument. Drawn rather than imported so it inherits currentColor and needs
  // no asset pipeline, no network fetch and no CSP exception.
  let { size = 16 }: { size?: number } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 16 16"
  fill="none"
  aria-hidden="true"
  class="mark"
>
  <path
    d="M5.5 1.5H2.5V14.5H5.5"
    stroke="currentColor"
    stroke-width="1.25"
    stroke-linecap="square"
  />
  <path
    d="M10.5 1.5H13.5V14.5H10.5"
    stroke="currentColor"
    stroke-width="1.25"
    stroke-linecap="square"
  />
  <rect x="6.75" y="6.75" width="2.5" height="2.5" fill="currentColor" />
</svg>

<style>
  .mark {
    display: block;
    flex: none;
  }
</style>
