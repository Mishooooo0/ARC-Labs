<script lang="ts">
  /**
   * Vault search, and the tag browser, which is the same surface filtered a
   * different way.
   *
   * Snippets come back from FTS5 with matched terms wrapped in guillemets, and
   * they are turned into marks here rather than in SQL. That keeps the index
   * free of anything HTML-shaped: what it stores is text, and how text is
   * presented is this layer's problem.
   */
  import type { NoteRef, SearchHit } from "../lib/types";
  import EmptyState from "./EmptyState.svelte";

  let {
    query = $bindable(""),
    hits,
    tagFilter,
    tagNotes,
    searching,
    onopen,
    onclear,
  }: {
    query: string;
    hits: SearchHit[];
    tagFilter: string | null;
    tagNotes: NoteRef[];
    searching: boolean;
    onopen: (path: string) => void;
    onclear: () => void;
  } = $props();

  let field = $state<HTMLInputElement | null>(null);
  export function focus() {
    field?.focus();
    field?.select();
  }

  const OPEN = "«";
  const CLOSE = "»";

  /** Split an FTS5 snippet into plain and highlighted runs. */
  function runs(snippet: string): { text: string; hit: boolean }[] {
    const out: { text: string; hit: boolean }[] = [];
    let rest = snippet;
    for (;;) {
      const open = rest.indexOf(OPEN);
      if (open < 0) break;
      const close = rest.indexOf(CLOSE, open);
      if (close < 0) break;
      if (open > 0) out.push({ text: rest.slice(0, open), hit: false });
      out.push({ text: rest.slice(open + 1, close), hit: true });
      rest = rest.slice(close + 1);
    }
    if (rest) out.push({ text: rest, hit: false });
    return out;
  }
</script>

<div class="search">
  {#if tagFilter}
    <div class="head">
      <span class="data label">tag</span>
      <span class="tag data">#{tagFilter}</span>
      <span class="data count">{tagNotes.length} notes</span>
      <button class="clear data" onclick={onclear}>clear</button>
    </div>
    <ul>
      {#each tagNotes as n (n.path)}
        <li>
          <button onclick={() => onopen(n.path)}>
            <span class="title">{n.title}</span>
            <span class="data path">{n.path}</span>
          </button>
        </li>
      {:else}
        <li class="data quiet">no notes carry this tag</li>
      {/each}
    </ul>
  {:else}
    <div class="head">
      <span class="data label">search</span>
      <input
        bind:this={field}
        bind:value={query}
        placeholder="Search the vault…"
        spellcheck="false"
        autocomplete="off"
      />
      {#if query}
        <span class="data count">{searching ? "…" : hits.length + " hits"}</span>
        <button class="clear data" onclick={onclear}>clear</button>
      {/if}
    </div>

    {#if !query.trim()}
      <EmptyState
        title="Search the vault"
        description="Full text across every note, with the sentence around each match. Results appear as you type."
        hint="whole words and prefixes"
      />
    {:else if hits.length === 0 && !searching}
      <EmptyState
        title="No matches"
        description="Nothing in the vault contains that. Try fewer words — matching is on whole words and prefixes, not fragments."
      />
    {:else}
      <ul>
        {#each hits as h (h.path)}
          <li>
            <button onclick={() => onopen(h.path)}>
              <span class="title">{h.title}</span>
              <span class="snippet">
                {#each runs(h.snippet) as run}
                  {#if run.hit}<mark>{run.text}</mark>{:else}{run.text}{/if}
                {/each}
              </span>
              <span class="data path">{h.path}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</div>

<style>
  .search {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .head {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--arc-space-3);
    padding: var(--arc-space-3) var(--arc-space-5);
    border-bottom: 1px solid var(--arc-line);
  }
  .head input {
    flex: 1;
    min-width: 0;
    background: none;
    border: none;
    outline: none;
    font-size: var(--arc-text-lg);
    color: var(--arc-fg);
  }
  .head input::placeholder {
    color: var(--arc-fg-faint);
  }
  .label {
    flex: none;
    color: var(--arc-accent);
    border: 1px solid var(--arc-accent-dim);
    border-radius: var(--arc-radius-sm);
    padding: 1px var(--arc-space-2);
    letter-spacing: 0.06em;
  }
  .tag {
    color: var(--arc-fg);
  }
  .count {
    color: var(--arc-fg-faint);
  }
  .clear {
    color: var(--arc-fg-faint);
  }
  .clear:hover {
    color: var(--arc-fg-dim);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: var(--arc-space-2) var(--arc-space-3);
    overflow-y: auto;
  }
  li button {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    padding: var(--arc-space-3);
    border-radius: var(--arc-radius);
    text-align: left;
    color: var(--arc-fg-dim);
  }
  li button:hover {
    background: var(--arc-bg-2);
  }

  .title {
    color: var(--arc-fg);
    font-weight: 550;
  }
  .snippet {
    color: var(--arc-fg-dim);
    line-height: var(--arc-leading);
    max-width: var(--arc-measure);
  }
  mark {
    background: var(--arc-accent-wash);
    color: var(--arc-accent);
    border-radius: 2px;
    padding: 0 2px;
  }
  .path {
    color: var(--arc-fg-faint);
  }
  .quiet {
    padding: var(--arc-space-4);
    color: var(--arc-fg-faint);
  }
</style>
