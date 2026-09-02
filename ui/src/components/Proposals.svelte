<script lang="ts">
  /**
   * Agent proposals awaiting a decision.
   *
   * Rendered inline above the note, in agent blue, with Accept and Reject. The
   * note underneath is **unchanged** — that is constraint 4, and this component
   * is where the user finds out it is true: the diff shows what *would* happen,
   * and nothing has happened until they choose.
   *
   * Reject is not a destructive action and is not styled as one. A refused
   * suggestion stays in the ledger, because an audit that shows only accepted
   * changes tells you what an agent did but not what it wanted to do.
   */
  import type { Proposal } from "../lib/types";

  let {
    proposals,
    busy,
    onaccept,
    onreject,
  }: {
    proposals: Proposal[];
    busy: boolean;
    onaccept: (index: number) => void;
    onreject: (index: number) => void;
  } = $props();

  function lines(patch: string) {
    return patch
      .split("\n")
      .filter((l) => !l.startsWith("+++") && !l.startsWith("---") && !l.startsWith("@@"))
      .map((l) => ({
        kind: l.startsWith("+") ? "add" : l.startsWith("-") ? "del" : "ctx",
        text: l.slice(l.startsWith("+") || l.startsWith("-") ? 1 : 0),
      }))
      .slice(0, 40);
  }
</script>

{#each proposals as p (p.index)}
  <section class="proposal" aria-label="Agent proposal">
    <header>
      <span class="dot"></span>
      <span class="who data">{p.actorId}{#if p.model}<span class="model"> · {p.model}</span>{/if}</span>
      <span class="reason">{p.reason}</span>
      <div class="spacer"></div>
      <span class="data counts">+{p.added} −{p.removed}</span>
      <button class="accept" disabled={busy} onclick={() => onaccept(p.index)}>Accept</button>
      <button class="reject" disabled={busy} onclick={() => onreject(p.index)}>Reject</button>
    </header>

    <pre class="patch data">{#each lines(p.patch) as l}<span class="line {l.kind}">{l.text}
</span>{/each}</pre>

    <footer class="data">
      This note has not been changed. Nothing is written until you accept.
    </footer>
  </section>
{/each}

<style>
  .proposal {
    margin: 0 0 var(--arc-space-5);
    border: 1px solid var(--arc-line);
    /* Agent blue, as a left edge. Reserved: it means authorship and nothing else. */
    border-left: 2px solid var(--arc-agent);
    border-radius: var(--arc-radius);
    background: var(--arc-agent-wash);
    overflow: hidden;
    max-width: var(--arc-measure);
  }

  header {
    display: flex;
    align-items: center;
    gap: var(--arc-space-3);
    padding: var(--arc-space-2) var(--arc-space-3);
    border-bottom: 1px solid var(--arc-line);
    font-size: var(--arc-text-sm);
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 1px;
    background: var(--arc-agent);
    flex: none;
  }
  .who {
    color: var(--arc-fg);
    flex: none;
  }
  .model {
    color: var(--arc-fg-faint);
  }
  .reason {
    color: var(--arc-fg-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spacer {
    flex: 1;
  }
  .counts {
    color: var(--arc-fg-faint);
    flex: none;
  }

  .accept,
  .reject {
    flex: none;
    padding: 2px var(--arc-space-3);
    border-radius: var(--arc-radius-sm);
    font-size: var(--arc-text-sm);
    transition: background var(--arc-dur-fast) var(--arc-ease);
  }
  .accept {
    background: var(--arc-accent);
    color: var(--arc-fg-on-accent);
    font-weight: 550;
  }
  .accept:hover:not(:disabled) {
    background: var(--arc-accent-dim);
  }
  /* Not destructive, and not styled as though it were: the suggestion is kept
     in the ledger either way. */
  .reject {
    color: var(--arc-fg-dim);
    border: 1px solid var(--arc-line-strong);
  }
  .reject:hover:not(:disabled) {
    color: var(--arc-fg);
    border-color: var(--arc-fg-faint);
  }
  .accept:disabled,
  .reject:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .patch {
    margin: 0;
    padding: var(--arc-space-2) 0;
    max-height: 260px;
    overflow: auto;
    font-size: var(--arc-text-xs);
    line-height: 1.55;
    color: var(--arc-fg-dim);
  }
  .line {
    display: block;
    padding: 0 var(--arc-space-3);
    white-space: pre-wrap;
    word-break: break-word;
    border-left: 2px solid transparent;
  }
  .line.add {
    background: var(--arc-accent-wash);
    border-left-color: var(--arc-accent-dim);
    color: var(--arc-fg);
  }
  .line.del {
    color: var(--arc-fg-faint);
    text-decoration: line-through;
    text-decoration-color: var(--arc-line-strong);
  }

  footer {
    padding: var(--arc-space-2) var(--arc-space-3);
    border-top: 1px solid var(--arc-line);
    color: var(--arc-fg-faint);
  }
</style>
