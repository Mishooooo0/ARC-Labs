<script lang="ts">
  /**
   * The home surface, shown when nothing is open.
   *
   * Everything here is a **count of something observed** — notes that exist,
   * links that were written, tags that were typed. There is deliberately no
   * discovery feed and no "notes you might like": the spec bans one outright,
   * and the reason is constraint 7. The first screen is the worst possible place
   * to put an inference, because whatever sits there gets read as fact.
   *
   * Inferred connections arrive in Phase 6, in the Weave inbox, where the
   * register rules apply and the user has gone looking for suggestions.
   */
  import type { IndexStats, NoteRef, TagCount, UnresolvedLink } from "../lib/types";

  let {
    vaultName,
    stats,
    recent,
    tags,
    unresolved,
    onopen,
    ontag,
  }: {
    vaultName: string;
    stats: IndexStats | null;
    recent: NoteRef[];
    tags: TagCount[];
    unresolved: UnresolvedLink[];
    onopen: (path: string) => void;
    ontag: (tag: string) => void;
  } = $props();
</script>

<div class="home">
  <header>
    <h1>{vaultName}</h1>
    {#if stats}
      <div class="figures data">
        <span><b>{stats.notes.toLocaleString()}</b> notes</span>
        {#if stats.canvases}<span><b>{stats.canvases}</b> canvases</span>{/if}
        <span><b>{stats.resolvedLinks.toLocaleString()}</b> links</span>
        <span><b>{stats.distinctTags.toLocaleString()}</b> tags</span>
        {#if stats.orphans}<span><b>{stats.orphans.toLocaleString()}</b> unlinked</span>{/if}
      </div>
    {/if}
  </header>

  <div class="cols">
    <section>
      <h2 class="data">Recent</h2>
      <ul>
        {#each recent.slice(0, 12) as n (n.path)}
          <li>
            <button onclick={() => onopen(n.path)} title={n.path}>
              <span class="t">{n.title}</span>
              <span class="data p">{n.path}</span>
            </button>
          </li>
        {:else}
          <li class="data quiet">no notes yet</li>
        {/each}
      </ul>
    </section>

    <section>
      <h2 class="data">Tags</h2>
      <div class="tags">
        {#each tags.slice(0, 24) as t (t.name)}
          <button class="tag data" onclick={() => ontag(t.name)}>
            #{t.name}<span class="n">{t.count}</span>
          </button>
        {:else}
          <span class="data quiet">no tags yet</span>
        {/each}
      </div>
    </section>

    <section>
      <!-- Not a suggestion: these are links the user wrote to notes that do not
           exist. Observed, not inferred — which is why it can sit on Home. -->
      <h2 class="data">Written about, not yet written</h2>
      <ul>
        {#each unresolved.slice(0, 10) as u (u.target)}
          <li>
            <div class="dead">
              <span class="t">{u.target}</span>
              <span class="data p">
                {u.count}
                {u.count === 1 ? "note wants it" : "notes want it"}
              </span>
            </div>
          </li>
        {:else}
          <li class="data quiet">every link resolves</li>
        {/each}
      </ul>
    </section>
  </div>
</div>

<style>
  .home {
    height: 100%;
    overflow-y: auto;
    padding: var(--arc-space-8) var(--arc-space-7);
  }
  header {
    max-width: 76ch;
    margin: 0 auto var(--arc-space-7);
  }
  h1 {
    margin: 0 0 var(--arc-space-3);
    font-size: var(--arc-text-2xl);
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .figures {
    display: flex;
    flex-wrap: wrap;
    gap: var(--arc-space-4);
    color: var(--arc-fg-faint);
  }
  .figures b {
    color: var(--arc-fg-dim);
    font-weight: 500;
  }

  .cols {
    max-width: 76ch;
    margin: 0 auto;
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: var(--arc-space-6);
  }

  h2 {
    margin: 0 0 var(--arc-space-3);
    font-size: var(--arc-text-xs);
    font-weight: 500;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--arc-fg-faint);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  li button,
  .dead {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
    padding: var(--arc-space-2);
    margin-left: calc(var(--arc-space-2) * -1);
    border-radius: var(--arc-radius-sm);
    text-align: left;
    color: var(--arc-fg-dim);
  }
  li button:hover {
    background: var(--arc-bg-2);
    color: var(--arc-fg);
  }
  .dead {
    color: var(--arc-fg-faint);
  }
  .dead .t {
    text-decoration: underline dashed;
    text-underline-offset: 3px;
    text-decoration-color: var(--arc-line-strong);
  }

  .t,
  .p {
    width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .p {
    color: var(--arc-fg-faint);
  }

  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: var(--arc-space-2);
  }
  .tag {
    display: inline-flex;
    align-items: baseline;
    gap: var(--arc-space-2);
    padding: 2px var(--arc-space-2);
    border: 1px solid var(--arc-line);
    border-radius: var(--arc-radius-lg);
    color: var(--arc-fg-dim);
    transition: border-color var(--arc-dur-fast) var(--arc-ease);
  }
  .tag:hover {
    border-color: var(--arc-accent-dim);
    color: var(--arc-fg);
  }
  .tag .n {
    color: var(--arc-fg-faint);
  }

  .quiet {
    color: var(--arc-fg-faint);
  }
</style>
