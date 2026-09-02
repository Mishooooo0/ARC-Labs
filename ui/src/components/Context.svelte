<script lang="ts">
  /**
   * The context pane: what links here, and what this links to.
   *
   * ## Constraint 7 is the whole design of this component
   *
   * Everything shown here is **observed** — a link the user actually typed.
   * Nothing is inferred, suggested or scored. Phase 6 adds inferred links, and
   * they will arrive in their own section with a visibly different register
   * (dashed, dimmer, carrying their source and score), because a user must never
   * have to click something to find out whether a connection is real.
   *
   * The one place a judgement appears is whether an outgoing link *resolves*.
   * That is computed, not guessed: an unresolved link is drawn with a dashed
   * underline and says "no note yet" in words. It is never silently rendered as
   * if it worked.
   */
  import type { Backlink, OutgoingLink } from "../lib/types";
  import EmptyState from "./EmptyState.svelte";

  let {
    path,
    backlinks,
    outgoing,
    loading,
    onopen,
  }: {
    path: string;
    backlinks: Backlink[];
    outgoing: OutgoingLink[];
    loading: boolean;
    onopen: (path: string) => void;
  } = $props();

  let resolved = $derived(outgoing.filter((l) => l.resolvedPath));
  let unresolved = $derived(outgoing.filter((l) => !l.resolvedPath));
</script>

<aside class="context">
  <section>
    <h2 class="data">
      Backlinks <span class="count">{backlinks.length}</span>
    </h2>
    {#if loading}
      <p class="data quiet">…</p>
    {:else if backlinks.length}
      <ul>
        {#each backlinks as b (b.path + b.target)}
          <li>
            <button class="row" onclick={() => onopen(b.path)} title={b.path}>
              <span class="title">{b.title}</span>
              <span class="data sub">
                {#if b.isEmbed}embeds{:else}links{/if} as {b.target}
              </span>
            </button>
          </li>
        {/each}
      </ul>
    {:else}
      <!-- A zero-backlink note is one of the five empty states the spec names
           as its own design, not a generic placeholder. -->
      <EmptyState
        title="Nothing links here yet"
        description="When another note writes [[{path.split('/').pop()?.replace(/\.md$/, '')}]], it will appear here."
      />
    {/if}
  </section>

  <section>
    <h2 class="data">
      Links from this note <span class="count">{outgoing.length}</span>
    </h2>
    {#if resolved.length}
      <ul>
        {#each resolved as l (l.target + (l.anchor ?? ""))}
          <li>
            <button class="row" onclick={() => onopen(l.resolvedPath!)} title={l.resolvedPath}>
              <span class="title">{l.alias ?? l.target}</span>
              <span class="data sub">
                {l.resolvedPath}{#if l.anchor}#{l.anchor}{/if}
              </span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}

    {#if unresolved.length}
      <h3 class="data sub-head">not written yet</h3>
      <ul>
        {#each unresolved as l (l.target)}
          <li>
            <!-- Deliberately not a button: there is nothing to open. Making it
                 look clickable would be a promise the vault cannot keep. -->
            <div class="row dead" title="No note matches “{l.target}”">
              <span class="title">{l.target}</span>
              <span class="data sub">no note yet</span>
            </div>
          </li>
        {/each}
      </ul>
    {/if}

    {#if !outgoing.length && !loading}
      <p class="data quiet">This note links to nothing.</p>
    {/if}
  </section>
</aside>

<style>
  .context {
    height: 100%;
    overflow-y: auto;
    padding: var(--arc-space-4) 0;
    display: flex;
    flex-direction: column;
    gap: var(--arc-space-5);
  }

  h2 {
    margin: 0 0 var(--arc-space-2);
    padding: 0 var(--arc-space-4);
    font-size: var(--arc-text-xs);
    font-weight: 500;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--arc-fg-faint);
    display: flex;
    align-items: baseline;
    gap: var(--arc-space-2);
  }
  .count {
    color: var(--arc-fg-dim);
  }

  .sub-head {
    margin: var(--arc-space-3) 0 var(--arc-space-1);
    padding: 0 var(--arc-space-4);
    font-size: var(--arc-text-xs);
    font-weight: 400;
    letter-spacing: 0.06em;
    color: var(--arc-fg-faint);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0 var(--arc-space-2);
  }

  .row {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
    padding: var(--arc-space-2);
    border-radius: var(--arc-radius-sm);
    text-align: left;
    color: var(--arc-fg-dim);
    transition: background var(--arc-dur-fast) var(--arc-ease);
  }
  button.row:hover {
    background: var(--arc-bg-2);
    color: var(--arc-fg);
  }

  /* An unresolved link. Dashed and dimmed, and it does not respond to hover:
     the visual register says "this is not a destination" before the words do. */
  .dead {
    cursor: default;
    color: var(--arc-fg-faint);
  }
  .dead .title {
    text-decoration: underline dashed;
    text-underline-offset: 3px;
    text-decoration-color: var(--arc-line-strong);
  }

  .title {
    width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub {
    width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--arc-fg-faint);
  }

  .quiet {
    margin: 0;
    padding: 0 var(--arc-space-4);
    color: var(--arc-fg-faint);
  }
</style>
