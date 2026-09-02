<script lang="ts">
  /**
   * A run in flight.
   *
   * ## The egress indicator is the important part
   *
   * When vault bytes are going off this machine, that fact is on screen for as
   * long as it is true — not a toast that fades, not a line in a log. The spec
   * calls for a *persistent* indicator, and the reason is that a notebook whose
   * headline promise is "works with the cable pulled" has to be unmistakable
   * about the moments when it does not.
   *
   * ## Throughput, not a spinner
   *
   * tok/s is shown rather than an indeterminate progress bar. On hardware with
   * no GPU the difference between 30 tok/s and 3 tok/s is the difference between
   * waiting and going to do something else, and only the number tells you which
   * one you are in.
   */
  import type { RunStatus } from "../lib/types";

  let {
    run,
    oncancel,
    onapprove,
    onclose,
  }: {
    run: RunStatus;
    oncancel: () => void;
    onapprove: () => void;
    onclose: () => void;
  } = $props();

  let running = $derived(run.state === "running");
  let needsApproval = $derived(run.state === "needsEgressApproval");

  function bytes(n: number): string {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / 1048576).toFixed(1)} MB`;
  }
</script>

<section class="run" data-state={run.state}>
  <!-- Persistent while bytes are leaving. Not a toast. -->
  {#if run.egressTo}
    <div class="egress data" role="status">
      <span class="pip" class:live={running}></span>
      <strong>vault content is being sent off this machine</strong>
      <span class="dest">{run.egressTo}</span>
      {#if run.egressBytes}<span>{bytes(run.egressBytes)}</span>{/if}
      <span class="note">recorded in the ledger</span>
    </div>
  {/if}

  <header>
    <span class="data state">{run.state}</span>
    <span class="data target">{run.target}</span>
    <div class="spacer"></div>
    {#if run.totalTokens > 0}
      <span class="data metric">{run.totalTokens} tokens</span>
    {/if}
    {#if run.elapsedMs > 0}
      <span class="data metric">{(run.elapsedMs / 1000).toFixed(1)}s</span>
    {/if}
    {#if running}
      <button class="cancel" onclick={oncancel}>Cancel</button>
    {:else}
      <button class="close" onclick={onclose} aria-label="Close">×</button>
    {/if}
  </header>

  {#if needsApproval}
    <!-- Not an error: a decision only a person can make. -->
    <div class="approval">
      <p>
        This run would send vault content to a model that is not on this machine.
        Nothing has been sent.
      </p>
      <p class="data where">{run.error}</p>
      <div class="actions">
        <button class="approve" onclick={onapprove}>Send it, and record the egress</button>
        <button class="close-text" onclick={onclose}>Cancel this run</button>
      </div>
    </div>
  {:else if run.error}
    <p class="error data">{run.error}</p>
  {/if}

  <ol class="nodes">
    {#each run.nodes as n (n.id)}
      <li class="node" class:active={n.running}>
        <div class="line">
          <span class="pip" class:live={n.running}></span>
          <span class="data id">{n.id}</span>
          <span class="data kind">{n.kind}</span>
          <div class="spacer"></div>
          {#if n.tokensPerSec}
            <span class="data metric">{n.tokensPerSec.toFixed(1)} tok/s</span>
          {/if}
          {#if n.tokens}<span class="data metric">{n.tokens} tok</span>{/if}
          {#if n.peakRssBytes}
            <span class="data metric" title="peak resident memory">
              {bytes(n.peakRssBytes)}
            </span>
          {/if}
        </div>

        {#if n.output}
          <!-- Streaming text. Its own scroll container, so growing output can
               never reflow the canvas behind it or steal the caret. -->
          <pre class="output data">{n.output}</pre>
        {/if}

        {#if n.proposedTo}
          <p class="proposed data">
            proposed to <strong>{n.proposedTo}</strong> — the note is unchanged until you accept
          </p>
        {/if}
      </li>
    {/each}
  </ol>
</section>

<style>
  .run {
    flex: none;
    max-height: 45%;
    display: flex;
    flex-direction: column;
    background: var(--arc-bg-1);
    border-top: 1px solid var(--arc-line);
    /* A run is agent work, so the panel carries the agent colour. */
    box-shadow: inset 0 2px 0 0 var(--arc-agent);
  }

  .egress {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--arc-space-3);
    padding: var(--arc-space-2) var(--arc-space-4);
    background: var(--arc-bg-2);
    border-bottom: 1px solid var(--arc-warn);
    color: var(--arc-warn);
  }
  .egress strong {
    font-weight: 550;
  }
  .egress .dest {
    color: var(--arc-fg);
  }
  .egress .note {
    color: var(--arc-fg-faint);
  }

  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--arc-space-3);
    padding: var(--arc-space-2) var(--arc-space-4);
    border-bottom: 1px solid var(--arc-line);
  }
  .state {
    color: var(--arc-accent);
    border: 1px solid var(--arc-accent-dim);
    border-radius: var(--arc-radius-sm);
    padding: 1px var(--arc-space-2);
    letter-spacing: 0.06em;
  }
  [data-state="failed"] .state {
    color: var(--arc-danger);
    border-color: var(--arc-danger);
  }
  [data-state="needsEgressApproval"] .state {
    color: var(--arc-warn);
    border-color: var(--arc-warn);
  }
  .target {
    color: var(--arc-fg-dim);
  }
  .spacer {
    flex: 1;
  }
  .metric {
    color: var(--arc-fg-faint);
  }

  .pip {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--arc-line-strong);
    flex: none;
  }
  /* Motion only while something is actually happening, per the motion budget. */
  .pip.live {
    background: var(--arc-agent);
    animation: pulse calc(var(--arc-dur-slow) * 5) var(--arc-ease) infinite;
  }
  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.3;
    }
  }

  .cancel,
  .approve,
  .close-text {
    padding: 2px var(--arc-space-3);
    border-radius: var(--arc-radius-sm);
    font-size: var(--arc-text-sm);
  }
  .cancel {
    color: var(--arc-fg-dim);
    border: 1px solid var(--arc-line-strong);
  }
  .cancel:hover {
    color: var(--arc-fg);
  }
  .approve {
    background: var(--arc-warn);
    color: var(--arc-bg-0);
    font-weight: 550;
  }
  .close-text {
    color: var(--arc-fg-dim);
    border: 1px solid var(--arc-line-strong);
  }
  .close {
    color: var(--arc-fg-faint);
    font-size: var(--arc-text-lg);
    line-height: 1;
  }
  .close:hover {
    color: var(--arc-fg);
  }

  .approval {
    padding: var(--arc-space-3) var(--arc-space-4);
    border-bottom: 1px solid var(--arc-line);
  }
  .approval p {
    margin: 0 0 var(--arc-space-2);
    color: var(--arc-fg-dim);
    font-size: var(--arc-text-sm);
  }
  .where {
    color: var(--arc-warn);
  }
  .actions {
    display: flex;
    gap: var(--arc-space-2);
  }

  .error {
    margin: 0;
    padding: var(--arc-space-2) var(--arc-space-4);
    color: var(--arc-danger);
    border-bottom: 1px solid var(--arc-line);
  }

  .nodes {
    list-style: none;
    margin: 0;
    padding: var(--arc-space-2) var(--arc-space-4);
    overflow-y: auto;
  }
  .node {
    padding: var(--arc-space-2) 0;
    border-bottom: 1px solid var(--arc-line);
  }
  .node:last-child {
    border-bottom: none;
  }
  .line {
    display: flex;
    align-items: center;
    gap: var(--arc-space-2);
  }
  .id {
    color: var(--arc-fg);
  }
  .kind {
    color: var(--arc-fg-faint);
  }

  .output {
    margin: var(--arc-space-2) 0 0;
    padding: var(--arc-space-2) var(--arc-space-3);
    max-height: 160px;
    overflow: auto;
    background: var(--arc-bg-2);
    border-radius: var(--arc-radius-sm);
    border-left: 2px solid var(--arc-agent);
    color: var(--arc-fg-dim);
    font-size: var(--arc-text-xs);
    line-height: 1.55;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .proposed {
    margin: var(--arc-space-2) 0 0;
    color: var(--arc-fg-faint);
  }
  .proposed strong {
    color: var(--arc-agent);
    font-weight: 500;
  }
</style>
