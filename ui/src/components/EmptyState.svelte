<script lang="ts">
  /**
   * The empty-state primitive.
   *
   * The Surface Contract forbids a blank pane anywhere in the product: every
   * empty surface states what it is, what belongs in it, and offers exactly one
   * primary action. The spec names five distinct empty states — an empty vault,
   * an empty canvas, an empty search, an empty agent inbox and a zero-backlink
   * note — and calls them five designs, not one placeholder.
   *
   * So this component supplies the *shape* they share and nothing else. Each
   * caller writes its own words. A generic "Nothing here yet" would satisfy the
   * component and fail the contract.
   *
   * Exactly one action: `action` is a single optional prop, not an array. Two
   * primary actions is no primary action.
   */
  import type { Snippet } from "svelte";

  let {
    title,
    description,
    actionLabel,
    onaction,
    hint,
    icon,
  }: {
    title: string;
    description: string;
    actionLabel?: string;
    onaction?: () => void;
    /** Secondary line, usually a keyboard shortcut. Monospace: it is data. */
    hint?: string;
    icon?: Snippet;
  } = $props();
</script>

<div class="empty">
  {#if icon}
    <div class="icon">{@render icon()}</div>
  {/if}
  <h2>{title}</h2>
  <p>{description}</p>
  {#if actionLabel && onaction}
    <button class="action" onclick={onaction}>{actionLabel}</button>
  {/if}
  {#if hint}
    <p class="data hint">{hint}</p>
  {/if}
</div>

<style>
  .empty {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    text-align: center;
    padding: var(--arc-space-7) var(--arc-space-5);
    gap: var(--arc-space-3);
  }

  .icon {
    color: var(--arc-fg-faint);
    margin-bottom: var(--arc-space-2);
  }

  h2 {
    margin: 0;
    font-size: var(--arc-text-lg);
    font-weight: 600;
    letter-spacing: -0.005em;
  }

  p {
    margin: 0;
    max-width: 44ch;
    color: var(--arc-fg-dim);
    line-height: var(--arc-leading);
  }

  .action {
    margin-top: var(--arc-space-3);
    padding: var(--arc-space-2) var(--arc-space-4);
    background: var(--arc-accent);
    color: var(--arc-fg-on-accent);
    border-radius: var(--arc-radius);
    font-weight: 550;
    transition: background var(--arc-dur-fast) var(--arc-ease);
  }
  .action:hover {
    background: var(--arc-accent-dim);
  }

  .hint {
    margin-top: var(--arc-space-2);
    color: var(--arc-fg-faint);
  }
</style>
