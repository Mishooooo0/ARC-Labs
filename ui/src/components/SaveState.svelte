<script lang="ts">
  /**
   * The save indicator.
   *
   * Deliberately quiet. A notebook that saves continuously should not celebrate
   * doing so — the interesting states are the ones where something is *wrong*,
   * and those are the only ones that get colour. "Saved" fades back to nothing
   * rather than sitting there as a permanent reassurance badge.
   */
  import type { SaveState } from "./Editor.svelte";

  let { state, detail }: { state: SaveState; detail?: string } = $props();

  const LABEL: Record<SaveState, string> = {
    clean: "",
    dirty: "unsaved",
    saving: "saving",
    saved: "saved",
    conflict: "CONFLICT",
    error: "SAVE FAILED",
  };

  let bad = $derived(state === "conflict" || state === "error");
</script>

{#if LABEL[state]}
  <span class="save data" class:bad class:muted={state === "saved"}>
    {LABEL[state]}
    {#if detail && state === "saved"}<span class="detail">· {detail}</span>{/if}
  </span>
{/if}

<style>
  .save {
    color: var(--arc-fg-faint);
    letter-spacing: 0.04em;
    transition: color var(--arc-dur) var(--arc-ease);
  }
  .muted {
    color: var(--arc-fg-faint);
    opacity: 0.7;
  }
  .bad {
    color: var(--arc-danger);
    letter-spacing: 0.06em;
  }
  .detail {
    opacity: 0.8;
  }
</style>
