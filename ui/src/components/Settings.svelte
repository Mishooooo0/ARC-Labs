<script lang="ts">
  /**
   * Settings.
   *
   * Two rules this panel follows that most do not:
   *
   * **It renders what came back, not what was sent.** The server clamps the
   * Weave budget, refuses to move the vault, and bounds the motion multiplier.
   * A panel that showed the value you typed would quietly disagree with the
   * value in force — so every save replaces the form state with the server's
   * answer, and if it differs, the panel says so rather than hiding it.
   *
   * **It says which settings this deployment cannot honour.** A browser talking
   * to a headless server cannot open a folder picker, and a build with no index
   * has no Weave. The handshake already knows; showing a control that will not
   * work is worse than not showing it.
   */
  import { untrack } from "svelte";
  import type { ApiVersion, Config } from "../lib/types";

  interface Props {
    config: Config;
    version: ApiVersion | null;
    can: (capability: string) => boolean;
    saving?: boolean;
    /** What the server actually stored, when it differs from what was sent. */
    adjusted?: string | null;
    onsave: (config: Config) => void;
    onclose: () => void;
  }

  let {
    config,
    version,
    can,
    saving = false,
    adjusted = null,
    onsave,
    onclose,
  }: Props = $props();

  // A working copy, so nothing is committed until Save. Settings that apply
  // themselves as you drag a slider make it impossible to change your mind.
  // `untrack` because this is the *initial* value on purpose: the re-sync below
  // owns every later update, and without it Svelte rightly warns that this line
  // reads reactive state it will not react to.
  let draft = $state<Config>(untrack(() => structuredClone($state.snapshot(config))));

  // Re-sync whenever the server hands back a new config — after a save, or on
  // reopen. Without this the form keeps showing what you typed even after the
  // server clamped it, which is the exact disagreement this panel exists to
  // avoid.
  $effect(() => {
    draft = structuredClone($state.snapshot(config));
  });

  let dirty = $derived(JSON.stringify(draft) !== JSON.stringify(config));

  const THEMES = ["arc-dark", "arc-light", "arc-terminal"];
  const ACCESS: Array<[Config["model"]["access"], string, string]> = [
    ["local-only", "Local only", "Nothing leaves this machine."],
    ["trusted-endpoint", "Trusted endpoint", "The configured endpoint may be remote. Every run is ledgered."],
    ["ask-each-run", "Ask each run", "Confirm before anything leaves."],
  ];
</script>

<section class="settings">
  <header>
    <div>
      <h1>Settings</h1>
      {#if version}
        <p class="lede data">
          {version.shell} · API v{version.apiMajor}.{version.apiMinor} · build {version.server}
        </p>
      {/if}
    </div>
    <button class="close" onclick={onclose} aria-label="Close settings">×</button>
  </header>

  {#if adjusted}
    <!-- The server changed something. Saying so beats a panel that shows one
         value while another is in force. -->
    <p class="adjusted" role="status">{adjusted}</p>
  {/if}

  <!-- Appearance -->
  <section class="group">
    <h2>Appearance</h2>

    <label class="row">
      <span class="label">Theme</span>
      <select bind:value={draft.ui.theme}>
        {#each THEMES as t (t)}
          <option value={t}>{t.replace("arc-", "")}</option>
        {/each}
      </select>
    </label>

    <label class="row">
      <span class="label">Density</span>
      <select bind:value={draft.ui.density}>
        <option value="comfortable">comfortable</option>
        <option value="compact">compact</option>
      </select>
    </label>

    <label class="row">
      <span class="label">Motion</span>
      <span class="control">
        <input type="range" min="0" max="2" step="0.1" bind:value={draft.ui.motion} />
        <span class="data value">{draft.ui.motion.toFixed(1)}×</span>
      </span>
      <span class="hint">
        Multiplies every duration. <strong>0 disables animation</strong> — both an
        accessibility setting and a way to prove the app never relies on motion to
        explain itself.
      </span>
    </label>
  </section>

  <!-- Identity -->
  <section class="group">
    <h2>Identity</h2>
    <label class="row">
      <span class="label">Your name in the ledger</span>
      <input
        class="text"
        bind:value={draft.actorId}
        placeholder="falls back to the OS user, then “unknown”"
        spellcheck="false"
      />
      <span class="hint">
        Stamped on every change you make. On a server this is what tells your
        edits apart from anyone else's — a vault where everyone is
        <code>unknown</code> cannot answer who changed a note.
      </span>
    </label>
  </section>

  <!-- Models -->
  <section class="group">
    <h2>Models</h2>

    <label class="row">
      <span class="label">Endpoint</span>
      <input class="text" bind:value={draft.model.endpoint} spellcheck="false" />
    </label>
    <label class="row">
      <span class="label">Instruct model</span>
      <input class="text" bind:value={draft.model.instruct} spellcheck="false" />
    </label>
    <label class="row">
      <span class="label">Embedding model</span>
      <input class="text" bind:value={draft.model.embed} spellcheck="false" />
      <span class="hint">
        Changing this invalidates every stored vector — they are not comparable
        across models, and the next Weave pass re-embeds the vault from scratch.
      </span>
    </label>

    <fieldset class="row">
      <legend class="label">Where vault bytes may go</legend>
      {#each ACCESS as [value, title, description] (value)}
        <label class="choice">
          <input type="radio" bind:group={draft.model.access} {value} />
          <span>
            <strong>{title}</strong>
            <span class="hint inline">{description}</span>
          </span>
        </label>
      {/each}
    </fieldset>
  </section>

  <!-- Weave -->
  <section class="group" class:unavailable={!can("weave")}>
    <h2>Weave</h2>
    {#if !can("weave")}
      <p class="hint">
        Unavailable here — Weave needs an index, and this deployment has not
        built one yet. The settings below are still saved.
      </p>
    {/if}

    <label class="row">
      <span class="label">Run in the background</span>
      <input type="checkbox" bind:checked={draft.weave.enabled} />
      <span class="hint">
        Off by default. Turning it on embeds every note in the vault, which is
        real work on your machine — a thing to opt into, not to discover.
      </span>
    </label>

    <label class="row">
      <span class="label">Suggest above</span>
      <span class="control">
        <input type="range" min="0.5" max="0.98" step="0.01" bind:value={draft.weave.threshold} />
        <span class="data value">{draft.weave.threshold.toFixed(2)}</span>
      </span>
      <span class="hint">
        Cosine similarity. Higher means fewer, better suggestions; an inbox full
        of weak ones is an inbox you stop reading.
      </span>
    </label>

    <label class="row">
      <span class="label">CPU ceiling</span>
      <span class="control">
        <input type="range" min="0.01" max="0.15" step="0.01" bind:value={draft.weave.cpuFraction} />
        <span class="data value">{Math.round(draft.weave.cpuFraction * 100)}%</span>
      </span>
      <span class="hint">
        Of one core, averaged over a minute. <strong>15% is the ceiling and
        cannot be raised</strong> — typing outranks Weave, and a setting that
        could overrule that would make it negotiable.
      </span>
    </label>

    <label class="row">
      <span class="label">Pass interval</span>
      <span class="control">
        <input type="number" min="5" max="3600" bind:value={draft.weave.intervalSecs} />
        <span class="data value">seconds</span>
      </span>
    </label>
  </section>

  <footer>
    <button class="save" onclick={() => onsave($state.snapshot(draft))} disabled={!dirty || saving}>
      {saving ? "saving…" : dirty ? "Save" : "Saved"}
    </button>
    <button
      class="revert"
      onclick={() => (draft = structuredClone($state.snapshot(config)))}
      disabled={!dirty || saving}>Revert</button>
    <span class="hint footnote">
      Written to <code>config.toml</code> atomically, and parsed back before it
      lands — a half-written config is one that fails at next boot.
    </span>
  </footer>
</section>

<style>
  .settings {
    /* Each view owns its own scroll — `.pane` is a flex column holding the
       banner as well, so scrolling the pane would carry the banner away with
       it. Without this the document itself grows and the app chrome scrolls
       off the top, which is what a long settings panel did. */
    height: 100%;
    overflow-y: auto;
    padding: var(--arc-space-5);
    max-width: 760px;
    margin: 0 auto;
    width: 100%;
  }

  header {
    display: flex;
    align-items: flex-start;
    gap: var(--arc-space-4);
    margin-bottom: var(--arc-space-5);
  }
  h1 {
    margin: 0;
    font-size: var(--arc-text-xl);
    font-weight: 500;
    letter-spacing: -0.01em;
  }
  .lede {
    margin: var(--arc-space-1) 0 0;
    color: var(--arc-fg-faint);
    font-size: var(--arc-text-xs);
  }
  .close {
    margin-left: auto;
    background: none;
    border: 0;
    color: var(--arc-fg-faint);
    font-size: var(--arc-text-xl);
    cursor: pointer;
    line-height: 1;
  }
  .close:hover {
    color: var(--arc-fg);
  }

  .adjusted {
    margin: 0 0 var(--arc-space-4);
    padding: var(--arc-space-2) var(--arc-space-3);
    border: 1px solid var(--arc-warn);
    border-radius: var(--arc-radius);
    color: var(--arc-warn);
    font-size: var(--arc-text-sm);
  }

  .group {
    border-top: 1px solid var(--arc-line);
    padding: var(--arc-space-4) 0;
  }
  .group.unavailable {
    opacity: 0.72;
  }
  h2 {
    margin: 0 0 var(--arc-space-3);
    font-size: var(--arc-text-sm);
    font-weight: 500;
    color: var(--arc-fg-dim);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }

  .row {
    display: grid;
    grid-template-columns: 210px 1fr;
    gap: var(--arc-space-2) var(--arc-space-4);
    align-items: center;
    padding: var(--arc-space-2) 0;
    border: 0;
    margin: 0;
  }
  .label {
    color: var(--arc-fg);
    font-size: var(--arc-text-md);
  }
  legend.label {
    padding: 0;
  }
  .hint {
    grid-column: 2;
    color: var(--arc-fg-faint);
    font-size: var(--arc-text-xs);
    line-height: var(--arc-leading);
    max-width: 60ch;
  }
  .hint strong {
    color: var(--arc-fg-dim);
    font-weight: 500;
  }
  .hint code {
    font-family: var(--arc-font-data);
  }
  .hint.inline {
    display: block;
    grid-column: auto;
  }

  .control {
    display: flex;
    align-items: center;
    gap: var(--arc-space-3);
  }
  .value {
    color: var(--arc-fg-dim);
    font-size: var(--arc-text-sm);
    min-width: 5ch;
  }

  select,
  input.text,
  input[type="number"] {
    background: var(--arc-bg-2);
    border: 1px solid var(--arc-line-strong);
    border-radius: var(--arc-radius);
    color: var(--arc-fg);
    font-family: var(--arc-font-data);
    font-size: var(--arc-text-sm);
    padding: var(--arc-space-2) var(--arc-space-3);
  }
  input.text {
    width: 100%;
    max-width: 42ch;
  }
  input[type="number"] {
    width: 10ch;
  }
  select:focus,
  input:focus {
    outline: none;
    border-color: var(--arc-accent-dim);
  }
  input[type="range"] {
    accent-color: var(--arc-accent);
    width: 220px;
  }
  input[type="checkbox"],
  input[type="radio"] {
    accent-color: var(--arc-accent);
  }

  .choice {
    grid-column: 2;
    display: flex;
    gap: var(--arc-space-2);
    align-items: flex-start;
    padding: var(--arc-space-1) 0;
  }
  .choice strong {
    display: block;
    color: var(--arc-fg);
    font-size: var(--arc-text-sm);
    font-weight: 400;
  }

  footer {
    border-top: 1px solid var(--arc-line);
    padding-top: var(--arc-space-4);
    display: flex;
    align-items: center;
    gap: var(--arc-space-3);
    flex-wrap: wrap;
  }
  .save,
  .revert {
    border-radius: var(--arc-radius);
    padding: var(--arc-space-2) var(--arc-space-4);
    font-size: var(--arc-text-sm);
    cursor: pointer;
    background: var(--arc-bg-2);
    border: 1px solid var(--arc-line-strong);
    color: var(--arc-fg);
  }
  .save {
    border-color: var(--arc-accent-dim);
    color: var(--arc-accent);
  }
  .save:disabled,
  .revert:disabled {
    opacity: 0.5;
    cursor: default;
    border-color: var(--arc-line);
    color: var(--arc-fg-faint);
  }
  .footnote {
    grid-column: auto;
    margin-left: auto;
    text-align: right;
    max-width: 42ch;
  }

  @media (max-width: 640px) {
    .row {
      grid-template-columns: 1fr;
    }
    .hint,
    .choice {
      grid-column: 1;
    }
  }
</style>
