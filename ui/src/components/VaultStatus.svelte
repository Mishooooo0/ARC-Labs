<script lang="ts">
  /**
   * The vault indicator.
   *
   * Load-bearing from Phase 0 onward, which is why it reads a real state enum
   * rather than a string: Phase 2 drives it while indexing, Phase 5 while a
   * canvas node is running, Phase 6 while Weave is embedding. A component that
   * started as a hardcoded label would have to be rewritten three times.
   *
   * The dot animates only while work is happening. Motion here explains a state
   * change; it is never decoration.
   */
  import type { VaultStatus } from "../lib/types";

  let {
    status,
    name,
    readOnly = false,
  }: { status: VaultStatus; name?: string; readOnly?: boolean } = $props();

  const LABEL: Record<VaultStatus, string> = {
    offline: "NO VAULT",
    scanning: "SCANNING",
    online: "VAULT ONLINE",
    indexing: "INDEXING",
  };

  let busy = $derived(status === "scanning" || status === "indexing");
</script>

<div class="status" data-status={status} title={name ?? ""}>
  <span class="dot" class:busy></span>
  <span class="data label">{LABEL[status]}</span>
  {#if name && status !== "offline"}
    <span class="data name">{name}</span>
  {/if}
  {#if readOnly && status !== "offline"}
    <!-- Everything else about the app looks normal when the vault cannot be
         written, and the first sign otherwise would be a save that fails. It is
         a legitimate state, so it is labelled rather than treated as an error. -->
    <span
      class="data readonly"
      title="This process cannot write to the vault. Reading works; saving will fail."
      >READ-ONLY</span>
  {/if}
</div>

<style>
  .status {
    display: flex;
    align-items: center;
    gap: var(--arc-space-2);
    min-width: 0;
  }

  .readonly {
    color: var(--arc-warn);
    border: 1px solid var(--arc-warn);
    border-radius: var(--arc-radius-sm);
    padding: 0 var(--arc-space-1);
    font-size: var(--arc-text-xs);
    letter-spacing: 0.04em;
    flex: none;
  }

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--arc-fg-faint);
    flex: none;
  }
  [data-status="online"] .dot {
    background: var(--arc-accent);
  }
  [data-status="scanning"] .dot,
  [data-status="indexing"] .dot {
    background: var(--arc-warn);
  }

  /* Only while something is actually happening, and cheap enough to leave on. */
  .dot.busy {
    animation: pulse calc(var(--arc-dur-slow) * 6) var(--arc-ease) infinite;
  }
  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.35;
    }
  }

  .label {
    letter-spacing: 0.06em;
    color: var(--arc-fg-dim);
  }
  [data-status="online"] .label {
    color: var(--arc-fg-dim);
  }

  .name {
    color: var(--arc-fg-faint);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name::before {
    content: "· ";
  }
</style>
