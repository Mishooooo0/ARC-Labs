<script lang="ts">
  /**
   * A unified diff, with the same authorship colours as the timeline.
   *
   * ## The gutter carries the colour, not the text
   *
   * Added and removed lines are shown with a neutral tint for *what changed*,
   * and a 2px gutter stripe in the actor's colour for *who changed it*. Those
   * are two different questions and they must not fight: colouring the text
   * itself amber or blue would make a diff unreadable and would spend the
   * reserved hues on syntax rather than on authorship.
   *
   * This is the same rule the timeline follows, which is why a diff opened from
   * a blue bar is visibly the same blue.
   */
  import type { TimelineEntry } from "../lib/types";

  let {
    entry,
    patch,
    onrestore,
    onclose,
  }: {
    entry: TimelineEntry;
    patch: string;
    onrestore: (index: number) => void;
    onclose: () => void;
  } = $props();

  interface Line {
    kind: "add" | "del" | "ctx" | "hunk";
    text: string;
  }

  let lines = $derived.by((): Line[] =>
    patch
      .split("\n")
      // The `--- before` / `+++ after` headers say nothing a reader needs.
      .filter((l) => !l.startsWith("+++") && !l.startsWith("---"))
      .map((l) => {
        if (l.startsWith("@@")) return { kind: "hunk" as const, text: l };
        if (l.startsWith("+")) return { kind: "add" as const, text: l.slice(1) };
        if (l.startsWith("-")) return { kind: "del" as const, text: l.slice(1) };
        return { kind: "ctx" as const, text: l.replace(/^ /, "") };
      })
      .filter((l, i, all) => !(l.kind === "ctx" && l.text === "" && i === all.length - 1)),
  );

  let isAgent = $derived(entry.actorKind === "agent");
</script>

<div class="diff" class:agent={isAgent}>
  <header>
    <span class="who data">
      <span class="dot"></span>
      {entry.actorId}{#if entry.model}<span class="model"> · {entry.model}</span>{/if}
    </span>
    <span class="data op">{entry.op}</span>
    <span class="data ts">{entry.ts}</span>
    <span class="data counts">+{entry.added} −{entry.removed}</span>
    <div class="spacer"></div>
    {#if entry.touchedFile}
      <button class="restore data" onclick={() => onrestore(entry.index)}>
        restore to here
      </button>
    {:else}
      <span class="data untouched">did not touch the file</span>
    {/if}
    <button class="close" onclick={onclose} aria-label="Close">×</button>
  </header>

  <p class="reason">{entry.reason}</p>

  {#if lines.length}
    <pre class="body data">{#each lines as l}<span class="line {l.kind}">{l.text}
</span>{/each}</pre>
  {:else}
    <p class="reason quiet">No content change was recorded for this entry.</p>
  {/if}
</div>

<style>
  .diff {
    flex: none;
    max-height: 45%;
    display: flex;
    flex-direction: column;
    background: var(--arc-bg-1);
    border-top: 1px solid var(--arc-line);
    /* The authorship colour, once, as a top edge. Everything else stays neutral. */
    box-shadow: inset 0 2px 0 0 var(--arc-human);
  }
  .diff.agent {
    box-shadow: inset 0 2px 0 0 var(--arc-agent);
  }

  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--arc-space-3);
    padding: var(--arc-space-3) var(--arc-space-4);
    border-bottom: 1px solid var(--arc-line);
    font-size: var(--arc-text-xs);
  }
  .who {
    display: inline-flex;
    align-items: center;
    gap: var(--arc-space-2);
    color: var(--arc-fg);
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 1px;
    background: var(--arc-human);
  }
  .agent .dot {
    background: var(--arc-agent);
  }
  .model {
    color: var(--arc-fg-faint);
  }
  .op,
  .ts,
  .counts,
  .untouched {
    color: var(--arc-fg-faint);
  }
  .spacer {
    flex: 1;
  }
  .restore {
    color: var(--arc-accent);
    border: 1px solid var(--arc-accent-dim);
    border-radius: var(--arc-radius-sm);
    padding: 1px var(--arc-space-2);
  }
  .restore:hover {
    background: var(--arc-accent-wash);
  }
  .close {
    color: var(--arc-fg-faint);
    font-size: var(--arc-text-lg);
    line-height: 1;
  }
  .close:hover {
    color: var(--arc-fg);
  }

  .reason {
    flex: none;
    margin: 0;
    padding: var(--arc-space-2) var(--arc-space-4);
    color: var(--arc-fg-dim);
    font-size: var(--arc-text-sm);
    border-bottom: 1px solid var(--arc-line);
  }
  .quiet {
    color: var(--arc-fg-faint);
    border-bottom: none;
  }

  .body {
    margin: 0;
    padding: var(--arc-space-2) 0;
    overflow: auto;
    font-size: var(--arc-text-xs);
    line-height: 1.55;
    color: var(--arc-fg-dim);
  }
  .line {
    display: block;
    padding: 0 var(--arc-space-4);
    white-space: pre-wrap;
    word-break: break-word;
    border-left: 2px solid transparent;
  }
  /* What changed: neutral. Who changed it: the header stripe above. */
  .line.add {
    background: var(--arc-accent-wash);
    border-left-color: var(--arc-accent-dim);
    color: var(--arc-fg);
  }
  .line.del {
    background: var(--arc-bg-2);
    border-left-color: var(--arc-line-strong);
    color: var(--arc-fg-faint);
    text-decoration: line-through;
    text-decoration-color: var(--arc-line-strong);
  }
  .line.hunk {
    color: var(--arc-fg-faint);
    padding-top: var(--arc-space-2);
  }
</style>
