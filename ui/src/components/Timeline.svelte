<script lang="ts">
  /**
   * The timeline rail.
   *
   * ## This component is constraint 6
   *
   * *"Screenshot the timeline and hand it to someone who has not used the app —
   * they identify agent activity without reading a word."*
   *
   * Everything here serves that. One thin bar per entry down the edge of the
   * editor, **amber for human, blue for agent**, and nothing else on the rail
   * carries colour at all. Bar length encodes how much changed, so a run of
   * agent work reads as a block of blue with weight to it rather than a list you
   * have to parse.
   *
   * The two hues are spent on nothing else in the entire product — the token
   * lint enforces that — which is what makes them legible as *meaning* rather
   * than as decoration.
   *
   * ## Entries that did not touch the file look different
   *
   * A proposal, a rejection and an egress record are drawn hollow: outlined in
   * their actor's colour rather than filled with it. Something an agent
   * *suggested* must never look like something an agent *did*.
   */
  import type { TimelineEntry } from "../lib/types";

  let {
    entries,
    selected,
    onselect,
    onrestore,
  }: {
    entries: TimelineEntry[];
    selected: number | null;
    onselect: (index: number | null) => void;
    onrestore: (index: number) => void;
  } = $props();

  /** Bar length from lines changed, compressed so one huge edit cannot
   *  flatten every other entry into an invisible sliver. */
  function weight(e: TimelineEntry): number {
    const changed = e.added + e.removed;
    if (changed === 0) return 0.25;
    return Math.min(1, 0.25 + Math.log10(1 + changed) / 2.2);
  }

  function label(e: TimelineEntry): string {
    const who = e.actorKind === "agent" ? `${e.actorId}${e.model ? ` (${e.model})` : ""}` : e.actorId;
    const change = e.added || e.removed ? ` · +${e.added} −${e.removed}` : "";
    return `${e.ts} · ${who} · ${e.op}${change}\n${e.reason}`;
  }

  let agentCount = $derived(entries.filter((e) => e.actorKind === "agent").length);
</script>

<div class="rail" aria-label="Change history">
  <div class="bars">
    {#each entries as e (e.index)}
      <button
        class="bar"
        class:agent={e.actorKind === "agent"}
        class:hollow={!e.touchedFile}
        class:selected={selected === e.index}
        style="--w: {weight(e) * 100}%"
        title={label(e)}
        onclick={() => onselect(selected === e.index ? null : e.index)}
        ondblclick={() => onrestore(e.index)}
        aria-label={label(e)}
      ></button>
    {/each}
  </div>

  {#if entries.length}
    <div class="key data" aria-hidden="true">
      <span class="swatch human"></span>{entries.length - agentCount}
      <span class="swatch agent"></span>{agentCount}
    </div>
  {/if}
</div>

<style>
  .rail {
    width: 30px;
    flex: none;
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: stretch;
    border-left: 1px solid var(--arc-line);
    background: var(--arc-bg-1);
    padding: var(--arc-space-3) 0;
    gap: var(--arc-space-2);
  }

  .bars {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 0 var(--arc-space-2);
  }

  .bar {
    height: 4px;
    flex: none;
    width: var(--w);
    min-width: 4px;
    border-radius: 1px;
    /* RESERVED. Amber is human, everywhere in the product, and nowhere else. */
    background: var(--arc-human);
    transition:
      opacity var(--arc-dur-fast) var(--arc-ease),
      transform var(--arc-dur-fast) var(--arc-ease);
    transform-origin: left center;
  }
  .bar.agent {
    /* RESERVED. Blue is agent. */
    background: var(--arc-agent);
  }

  /* Suggested, not done. Outlined rather than filled, so a proposal can never
     be mistaken for a change that actually happened. */
  .bar.hollow {
    background: transparent;
    box-shadow: inset 0 0 0 1px var(--arc-human);
  }
  .bar.hollow.agent {
    box-shadow: inset 0 0 0 1px var(--arc-agent);
  }

  .bar:hover {
    transform: scaleY(1.6);
  }
  .bar.selected {
    transform: scaleY(2);
  }

  .key {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-wrap: wrap;
    gap: 3px;
    font-size: var(--arc-text-xs);
    color: var(--arc-fg-faint);
    padding: 0 var(--arc-space-1);
    border-top: 1px solid var(--arc-line);
    padding-top: var(--arc-space-2);
  }
  .swatch {
    width: 6px;
    height: 6px;
    border-radius: 1px;
    display: inline-block;
  }
  .swatch.human {
    background: var(--arc-human);
  }
  .swatch.agent {
    background: var(--arc-agent);
  }
</style>
