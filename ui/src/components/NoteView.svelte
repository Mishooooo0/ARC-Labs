<script lang="ts">
  /**
   * A rendered note.
   *
   * The HTML comes from the Rust renderer, which escapes rather than emits raw
   * HTML — so `{@html}` here is safe by construction, not by trust. That
   * property lives in `arc-labs-core::markdown`, is tested there, and is the
   * reason this component can exist at all in a build that serves vaults to
   * browsers.
   *
   * Wikilinks and tags arrive as `arc://` hrefs. They are intercepted here
   * rather than followed: one delegated listener on the container, so a note
   * with three thousand links costs one listener.
   */
  import type { NoteView } from "../lib/types";

  let {
    note,
    onnavigate,
  }: {
    note: NoteView;
    /** A wikilink was clicked. Resolution to a real path arrives in Phase 2. */
    onnavigate: (target: string, kind: "note" | "tag" | "embed") => void;
  } = $props();

  let showFrontmatter = $state(false);

  function intercept(event: MouseEvent) {
    const el = (event.target as HTMLElement | null)?.closest("a");
    if (!(el instanceof HTMLAnchorElement)) return;

    const href = el.getAttribute("href") ?? "";
    if (!href.startsWith("arc://")) {
      // An outbound link. Constraint 3: nothing leaves this machine because the
      // page felt like it. The CSP blocks the fetch; this blocks the navigation.
      event.preventDefault();
      return;
    }

    event.preventDefault();
    const rest = href.slice("arc://".length);
    const slash = rest.indexOf("/");
    if (slash < 0) return;
    const kind = rest.slice(0, slash) as "note" | "tag" | "embed";
    const target = decodeURIComponent(rest.slice(slash + 1).split("#")[0] ?? "");
    if (target) onnavigate(target, kind);
  }
</script>

<article class="note">
  <header>
    <h1 class="title">{note.name}</h1>
    <!-- The detail strip. Monospace because every value in it is data. -->
    <div class="meta data">
      <span class="path">{note.path}</span>
      <span>{note.size.toLocaleString()} B</span>
      <span>{note.lineEnding}</span>
      {#if note.links.length}<span>{note.links.length} links</span>{/if}
      {#if note.tags.length}<span>{note.tags.length} tags</span>{/if}
      {#if note.frontmatter}
        <button class="fm-toggle" onclick={() => (showFrontmatter = !showFrontmatter)}>
          frontmatter {showFrontmatter ? "−" : "+"}
        </button>
      {/if}
    </div>
  </header>

  {#if note.frontmatter && showFrontmatter}
    <!-- Shown exactly as written. It is never parsed and re-serialised, so what
         is on screen is what is on disk, byte for byte. -->
    <pre class="frontmatter data">{note.frontmatter}</pre>
  {/if}

  {#if note.html.trim()}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="prose" onclick={intercept}>
      {@html note.html}
    </div>
  {:else}
    <p class="blank">This note is empty.</p>
  {/if}
</article>

<style>
  .note {
    height: 100%;
    overflow-y: auto;
    padding: var(--arc-space-6) var(--arc-space-7);
  }

  header {
    margin-bottom: var(--arc-space-5);
    padding-bottom: var(--arc-space-3);
    border-bottom: 1px solid var(--arc-line);
    max-width: var(--arc-measure);
  }

  .title {
    margin: 0 0 var(--arc-space-2);
    font-size: var(--arc-text-xl);
    font-weight: 600;
    letter-spacing: -0.005em;
    line-height: var(--arc-leading-tight);
  }

  .meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--arc-space-3);
    color: var(--arc-fg-faint);
  }
  .meta > span + span::before {
    content: "· ";
    margin-right: var(--arc-space-2);
    color: var(--arc-line-strong);
  }
  .path {
    color: var(--arc-fg-dim);
  }

  .fm-toggle {
    color: var(--arc-fg-faint);
    font: inherit;
    text-decoration: underline dotted;
    text-underline-offset: 2px;
  }
  .fm-toggle:hover {
    color: var(--arc-fg-dim);
  }

  .frontmatter {
    max-width: var(--arc-measure);
    margin: 0 0 var(--arc-space-5);
    padding: var(--arc-space-3);
    background: var(--arc-bg-1);
    border: 1px solid var(--arc-line);
    border-left: 2px solid var(--arc-line-strong);
    border-radius: var(--arc-radius-sm);
    white-space: pre-wrap;
    color: var(--arc-fg-dim);
    overflow-x: auto;
  }

  .blank {
    color: var(--arc-fg-faint);
    font-style: italic;
  }
</style>
