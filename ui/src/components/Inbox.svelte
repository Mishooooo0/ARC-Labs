<script lang="ts">
  /**
   * The agent inbox.
   *
   * Everything on this surface is **inferred**, and the surface is built so that
   * a person who arrives here without explanation cannot come away thinking
   * otherwise. Constraint 7 is not satisfied by a tooltip.
   *
   * Four mechanisms, all visible in a screenshot:
   *
   * - The connector between two notes is **dashed**, never solid. Every observed
   *   link in this app is drawn solid; nothing inferred ever is.
   * - The score is on the card, in data type, as a number. Not a bar, not a
   *   label like "strong" — a number the reader can disagree with.
   * - The model that said it is named on every row.
   * - The card sits in the **agent** register: blue rail, blue wash. Amber is
   *   reserved for what a person did, and nothing here was done by a person.
   *
   * Accepting writes the link into the source note through the normal save path,
   * so it is atomic, fidelity-preserving and ledgered — twice: Weave proposed,
   * you accepted.
   */
  import type { LinkSuggestion, WeaveStatus } from "../lib/types";
  import EmptyState from "./EmptyState.svelte";

  interface Props {
    suggestions: LinkSuggestion[];
    status: WeaveStatus | null;
    busy?: number | null;
    onaccept: (id: number) => void;
    ondismiss: (id: number) => void;
    onopen: (path: string) => void;
    onpass: () => void;
    working?: boolean;
  }

  let {
    suggestions,
    status,
    busy = null,
    onaccept,
    ondismiss,
    onopen,
    onpass,
    working = false,
  }: Props = $props();

  const progress = $derived(
    status && status.total > 0 ? Math.round((status.embedded / status.total) * 100) : 0,
  );
  const behind = $derived(status ? status.total - status.embedded : 0);
</script>

<section class="inbox">
  <header>
    <div class="head">
      <h1>Agent inbox</h1>
      <p class="lede">
        Links ARC-LABS <em>thinks</em> might belong. Nothing here has touched a file, and nothing
        will until you say so.
      </p>
    </div>

    <div class="actions">
      <button class="run" onclick={onpass} disabled={working || (status?.coolingSecs ?? 0) > 0}>
        {working ? "looking…" : (status?.coolingSecs ?? 0) > 0 ? `resting ${status?.coolingSecs}s` : "look now"}
      </button>
    </div>
  </header>

  {#if status}
    <!-- The state of the thing that produced these, in data type, because it is
         data: how much of the vault it has read, what read it, and what share of
         a core it is taking while it does. -->
    <div class="meter data">
      <span class="model">{status.model}</span>
      <span class="sep">·</span>
      <span>{status.embedded} / {status.total} notes embedded</span>
      {#if behind > 0}
        <span class="bar" style:--pct="{progress}%" aria-hidden="true"></span>
        <span class="pct">{progress}%</span>
      {/if}
      <span class="spacer"></span>
      {#if status.running}
        <span class="cpu" title="Averaged over the last minute. The ceiling is 15%.">
          {(status.cpuFraction * 100).toFixed(1)}% of a core
        </span>
      {:else if !status.enabled}
        <span class="off">off — set <code>weave.enabled</code> in config.toml to run it in the background</span>
      {/if}
    </div>
    <!-- A pass that declined to run has to say why. Silence after a button
         press reads as a bug, and these two reasons are the system working. -->
    {#if status.lastPass?.stoppedBecause === "UserActive"}
      <p class="stood-down data">
        The last pass stood down because you were typing. That is the rule, not a failure.
      </p>
    {:else if status.coolingSecs > 0}
      <p class="stood-down data">
        Resting for {status.coolingSecs}s to stay inside its 15%-of-a-core budget.
      </p>
    {:else if status.lastPass?.stoppedBecause === "QueueBacked"}
      <p class="stood-down data">
        Paused: the index has writes to catch up on first.
      </p>
    {/if}
  {/if}

  {#if suggestions.length === 0}
    <EmptyState
      title={behind > 0 ? "Still reading the vault" : "Nothing to review"}
      description={behind > 0
        ? `${behind} notes left to embed. Suggestions appear once the whole vault has been read — a partial pass would only ever suggest links between whichever notes happened to be indexed first.`
        : "No note is close enough to another unlinked note to be worth your attention. That is the normal state; this pane is meant to be empty most of the time."}
    />
  {:else}
    <ul class="list">
      {#each suggestions as s (s.id)}
        <li class="card" class:busy={busy === s.id}>
          <span class="rail" aria-hidden="true"></span>

          <div class="pair">
            <button class="note" onclick={() => onopen(s.srcPath)}>
              <span class="title">{s.srcTitle}</span>
              <span class="path data">{s.srcPath}</span>
            </button>

            <!-- Dashed, always. An observed link is drawn solid everywhere in
                 this app; this connector must never be able to pass for one. -->
            <span class="connector" aria-label="inferred, not observed">
              <svg width="46" height="12" viewBox="0 0 46 12" aria-hidden="true">
                <line
                  x1="1" y1="6" x2="38" y2="6"
                  stroke="var(--arc-agent)" stroke-width="1.5"
                  stroke-dasharray="3 3" opacity="0.7"
                />
                <path d="M38 2 L44 6 L38 10" fill="none" stroke="var(--arc-agent)"
                      stroke-width="1.5" opacity="0.7" />
              </svg>
            </span>

            <button class="note" onclick={() => onopen(s.dstPath)}>
              <span class="title">{s.dstTitle}</span>
              <span class="path data">{s.dstPath}</span>
            </button>
          </div>

          <div class="provenance data">
            <span class="badge">inferred</span>
            <span class="score" title="Cosine similarity between the two notes' embeddings">
              {s.score.toFixed(3)}
            </span>
            <span class="by">{s.model}</span>
          </div>

          <div class="choices">
            <button class="accept" onclick={() => onaccept(s.id)} disabled={busy === s.id}>
              Add the link
            </button>
            <button class="dismiss" onclick={() => ondismiss(s.id)} disabled={busy === s.id}>
              No
            </button>
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .inbox {
    /* Each view owns its own scroll — `.pane` is a flex column holding the
       banner as well, so scrolling the pane would carry the banner away with
       it. Without this the document itself grows and the app chrome scrolls
       off the top, which is what a long settings panel did. */
    height: 100%;
    overflow-y: auto;
    padding: var(--arc-space-5) var(--arc-space-5) var(--arc-space-7);
    max-width: 900px;
    margin: 0 auto;
    width: 100%;
  }

  header {
    display: flex;
    align-items: flex-start;
    gap: var(--arc-space-4);
    margin-bottom: var(--arc-space-4);
  }
  .head {
    min-width: 0;
  }
  h1 {
    margin: 0;
    font-size: var(--arc-text-xl);
    font-weight: 500;
    letter-spacing: -0.01em;
  }
  .lede {
    margin: var(--arc-space-1) 0 0;
    color: var(--arc-fg-dim);
    font-size: var(--arc-text-md);
    line-height: var(--arc-leading);
    max-width: 62ch;
  }
  .lede em {
    font-style: italic;
    color: var(--arc-fg);
  }

  .actions {
    margin-left: auto;
    flex: none;
  }
  .run {
    background: var(--arc-bg-2);
    border: 1px solid var(--arc-line-strong);
    color: var(--arc-fg);
    border-radius: var(--arc-radius);
    padding: var(--arc-space-2) var(--arc-space-3);
    font-family: var(--arc-font-data);
    font-size: var(--arc-text-sm);
    cursor: pointer;
    transition: background var(--arc-dur-fast) var(--arc-ease);
  }
  .run:hover:not(:disabled) {
    background: var(--arc-bg-3);
  }
  .run:disabled {
    color: var(--arc-fg-faint);
    cursor: default;
  }

  .meter {
    display: flex;
    align-items: center;
    gap: var(--arc-space-2);
    flex-wrap: wrap;
    font-size: var(--arc-text-xs);
    color: var(--arc-fg-dim);
    border-radius: var(--arc-radius);
    padding: var(--arc-space-2) var(--arc-space-3);
    background: var(--arc-bg-2);
  }
  .meter .model {
    color: var(--arc-agent);
  }
  .meter .sep {
    color: var(--arc-fg-faint);
  }
  .meter .spacer {
    flex: 1;
  }
  .bar {
    display: inline-block;
    width: 90px;
    height: 3px;
    border-radius: 2px;
    background: var(--arc-bg-4);
    position: relative;
    overflow: hidden;
  }
  .bar::after {
    content: "";
    position: absolute;
    inset: 0 auto 0 0;
    width: var(--pct);
    background: var(--arc-agent);
  }
  .cpu {
    color: var(--arc-fg-faint);
  }
  .off code {
    font-family: var(--arc-font-data);
    color: var(--arc-fg);
  }

  .stood-down {
    margin: var(--arc-space-2) 0 0;
    font-size: var(--arc-text-xs);
    color: var(--arc-fg-faint);
  }

  .list {
    list-style: none;
    margin: var(--arc-space-4) 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--arc-space-2);
  }

  .card {
    position: relative;
    display: grid;
    grid-template-columns: 1fr auto;
    grid-template-areas: "pair choices" "prov choices";
    gap: var(--arc-space-2) var(--arc-space-4);
    align-items: center;
    /* The agent register. Amber is for what a person did; nothing here is. */
    background: var(--arc-agent-wash);
    border: 1px solid var(--arc-line);
    border-radius: var(--arc-radius);
    padding: var(--arc-space-3) var(--arc-space-4) var(--arc-space-3) var(--arc-space-5);
    transition: opacity var(--arc-dur-fast) var(--arc-ease);
  }
  .card.busy {
    opacity: 0.5;
  }
  .rail {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 3px;
    background: var(--arc-agent);
    border-radius: var(--arc-radius) 0 0 var(--arc-radius);
  }

  .pair {
    grid-area: pair;
    display: flex;
    align-items: center;
    gap: var(--arc-space-2);
    min-width: 0;
  }
  .note {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
    min-width: 0;
    flex: 1;
    background: none;
    border: 0;
    padding: 0;
    text-align: left;
    cursor: pointer;
    color: inherit;
  }
  .note .title {
    font-size: var(--arc-text-md);
    color: var(--arc-fg);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }
  .note:hover .title {
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .note .path {
    font-size: var(--arc-text-xs);
    color: var(--arc-fg-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
    /* Truncate the *front* of a long path: the file name is what identifies a
       note, and it lives at the end. */
    direction: rtl;
    text-align: left;
  }
  .connector {
    flex: none;
    line-height: 0;
  }

  .provenance {
    grid-area: prov;
    display: flex;
    align-items: center;
    gap: var(--arc-space-2);
    font-size: var(--arc-text-xs);
  }
  .badge {
    color: var(--arc-agent);
    border: 1px dashed var(--arc-agent);
    border-radius: var(--arc-radius-sm);
    padding: 0 var(--arc-space-1);
    opacity: 0.85;
    letter-spacing: 0.04em;
  }
  .score {
    color: var(--arc-fg-dim);
  }
  .by {
    color: var(--arc-fg-faint);
  }

  .choices {
    grid-area: choices;
    display: flex;
    gap: var(--arc-space-2);
    flex: none;
  }
  .choices button {
    border-radius: var(--arc-radius);
    padding: var(--arc-space-2) var(--arc-space-3);
    font-size: var(--arc-text-sm);
    font-family: var(--arc-font-ui);
    cursor: pointer;
    border: 1px solid var(--arc-line-strong);
    background: var(--arc-bg-2);
    color: var(--arc-fg);
    transition: background var(--arc-dur-fast) var(--arc-ease);
  }
  .choices .accept {
    border-color: var(--arc-agent);
    color: var(--arc-agent);
  }
  .choices button:hover:not(:disabled) {
    background: var(--arc-bg-3);
  }
  .choices button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  @media (max-width: 640px) {
    .card {
      grid-template-columns: 1fr;
      grid-template-areas: "pair" "prov" "choices";
    }
    .pair {
      flex-wrap: wrap;
    }
  }
</style>
