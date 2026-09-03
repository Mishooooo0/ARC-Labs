<script lang="ts">
  /**
   * Settings.
   *
   * A modal over a scrim with a category rail, rather than a page that takes the
   * main pane: the note you were reading stays behind it, and the rail scales as
   * categories arrive instead of becoming one very long scroll.
   *
   * Three rules this panel follows that most do not:
   *
   * **It renders what came back, not what was sent.** The server clamps the
   * Weave budget, refuses to move the vault, and bounds the motion multiplier.
   * A panel showing the value you typed would quietly disagree with the value in
   * force — so a save replaces the form with the server's answer, and says so
   * when the two differ.
   *
   * **It says which settings this deployment cannot honour.** The handshake
   * already knows; a control that will not work is worse than an absent one.
   *
   * **The MCP pane is a client, not a description.** It asks the server for its
   * own tool list over the same JSON-RPC path an external agent uses, so what it
   * shows cannot drift from what agents actually get.
   */
  import { untrack } from "svelte";
  import type { ApiVersion, Config, McpTool } from "../lib/types";
  import { transport } from "../lib/transport";

  interface Props {
    config: Config;
    version: ApiVersion | null;
    can: (capability: string) => boolean;
    saving?: boolean;
    adjusted?: string | null;
    onsave: (config: Config) => void;
    onclose: () => void;
  }

  let { config, version, can, saving = false, adjusted = null, onsave, onclose }: Props =
    $props();

  type Category = "appearance" | "identity" | "vault" | "models" | "weave" | "mcp" | "sync";
  let category = $state<Category>("appearance");

  // `untrack` because this is the initial value on purpose: the re-sync below
  // owns every later update.
  let draft = $state<Config>(untrack(() => structuredClone($state.snapshot(config))));

  // Re-sync when the server hands back a new config — after a save, or on
  // reopen. Without it the form keeps showing what you typed even after the
  // server clamped it, which is the exact disagreement this panel avoids.
  $effect(() => {
    draft = structuredClone($state.snapshot(config));
  });

  let dirty = $derived(JSON.stringify(draft) !== JSON.stringify(config));

  // ── MCP: ask the server, do not describe it ───────────────────────────────
  let tools = $state<McpTool[] | null>(null);
  let mcpError = $state<string | null>(null);

  $effect(() => {
    if (category !== "mcp" || tools || mcpError) return;
    void (async () => {
      try {
        const res = (await transport.mcp({
          jsonrpc: "2.0",
          id: 1,
          method: "tools/list",
        })) as { result?: { tools?: McpTool[] } } | null;
        tools = res?.result?.tools ?? [];
      } catch (e) {
        mcpError = e instanceof Error ? e.message : String(e);
      }
    })();
  });

  const THEMES = ["arc-dark", "arc-light", "arc-terminal"];
  const ACCESS: Array<[Config["model"]["access"], string, string]> = [
    ["local-only", "Local only", "Nothing leaves this machine."],
    [
      "trusted-endpoint",
      "Trusted endpoint",
      "The configured endpoint may be remote. Every run is ledgered.",
    ],
    ["ask-each-run", "Ask each run", "Confirm before anything leaves."],
  ];

  const CATEGORIES: Array<[Category, string]> = [
    ["appearance", "Appearance"],
    ["identity", "Identity"],
    ["vault", "Vault"],
    ["models", "Models"],
    ["weave", "Weave"],
    ["mcp", "MCP"],
    ["sync", "Sync"],
  ];

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onclose();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<!-- Click-outside closes. A modal whose only exit is a keyboard shortcut is the
     same problem this panel was built to fix. -->
<div
  class="scrim arc-fade"
  role="presentation"
  onclick={(e) => {
    if (e.target === e.currentTarget) onclose();
  }}
>
  <div class="modal arc-rise" role="dialog" aria-modal="true" aria-label="Settings">
    <nav class="rail" aria-label="Settings categories">
      <div class="data railhead">SETTINGS</div>
      {#each CATEGORIES as [id, label] (id)}
        <button
          class="cat"
          class:on={category === id}
          class:dim={id === "sync"}
          onclick={() => (category = id)}
        >
          {label}
          {#if id === "sync"}
            <!-- Named and marked rather than hidden: it is coming, and a
                 category that silently does not exist is its own confusion. -->
            <span class="data soon">soon</span>
          {/if}
        </button>
      {/each}

      <div class="railfoot data">
        {#if version}
          {version.shell} · API v{version.apiMajor}.{version.apiMinor}<br />build {version.server}
        {/if}
      </div>
    </nav>

    <div class="pane">
      <header>
        <h1>{CATEGORIES.find(([id]) => id === category)?.[1]}</h1>
        <button class="close" onclick={onclose} aria-label="Close settings">×</button>
      </header>

      <div class="body">
        {#if adjusted}
          <p class="adjusted" role="status">{adjusted}</p>
        {/if}

        {#if category === "appearance"}
          <div class="row">
            <span class="label">Theme</span>
            <div class="pills">
              {#each THEMES as t (t)}
                <button class="pill" class:on={draft.ui.theme === t} onclick={() => (draft.ui.theme = t)}>
                  {t.replace("arc-", "")}
                </button>
              {/each}
            </div>
          </div>

          <div class="row">
            <span class="label">Density</span>
            <div class="pills">
              <button
                class="pill"
                class:on={draft.ui.density === "comfortable"}
                onclick={() => (draft.ui.density = "comfortable")}>comfortable</button>
              <button
                class="pill"
                class:on={draft.ui.density === "compact"}
                onclick={() => (draft.ui.density = "compact")}>compact</button>
            </div>
            <p class="hint">
              Compact steps the type ramp down one rung and tightens the space scale, the
              topbar and the sidebar — noticeably more vault on screen.
              <strong>The tradeoff is reading comfort:</strong> 12px body over a long note
              is tighter than the 13px this app was designed around.
            </p>
          </div>

          <div class="row">
            <span class="label">Motion</span>
            <span class="control">
              <input type="range" min="0" max="2" step="0.1" bind:value={draft.ui.motion} />
              <span class="data value">{draft.ui.motion.toFixed(1)}×</span>
            </span>
            <p class="hint">
              Multiplies every duration. <strong>0 disables animation</strong> — both an
              accessibility setting and a way to prove the app never relies on motion to
              explain itself.
            </p>
          </div>
        {:else if category === "identity"}
          <div class="row">
            <span class="label">Your name in the ledger</span>
            <input
              class="text"
              bind:value={draft.actorId}
              placeholder="falls back to $USER, then “unknown”"
              spellcheck="false"
            />
            <p class="hint">
              Stamped on every change you make. On a server this is what tells your edits
              apart from anyone else's — a vault where everyone is <code>unknown</code>
              cannot answer who changed a note.
            </p>
          </div>
        {:else if category === "vault"}
          <div class="row">
            <span class="label">Keep deleted notes for</span>
            <div class="days">
              <input
                class="text num"
                type="number"
                min="0"
                max="3650"
                bind:value={draft.trash.retentionDays}
              />
              <span class="data unit">
                {draft.trash.retentionDays === 0 ? "for ever" : "days"}
              </span>
            </div>
            <p class="hint">
              A deleted note leaves a copy in the vault's trash. After this many days
              that copy is removed; <code>0</code> keeps it for ever.
            </p>
            <p class="hint">
              This is not how long a note stays recoverable. Restore replays content
              from the ledger, which keeps it for ever — what expires here is the
              second copy, the one that exists in case the ledger is the thing that
              went wrong.
            </p>
          </div>
        {:else if category === "models"}
          <div class="row">
            <span class="label">Endpoint</span>
            <input class="text" bind:value={draft.model.endpoint} spellcheck="false" />
          </div>
          <div class="row">
            <span class="label">Instruct model</span>
            <input class="text" bind:value={draft.model.instruct} spellcheck="false" />
          </div>
          <div class="row">
            <span class="label">Embedding model</span>
            <input class="text" bind:value={draft.model.embed} spellcheck="false" />
            <p class="hint">
              Changing this invalidates every stored vector — they are not comparable
              across models, and the next Weave pass re-embeds the vault from scratch.
            </p>
          </div>
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
        {:else if category === "weave"}
          {#if !can("weave")}
            <p class="hint standalone">
              Unavailable here — Weave needs an index, and this deployment has not built
              one yet. These settings are still saved.
            </p>
          {/if}
          <div class="row">
            <span class="label">Run in the background</span>
            <input type="checkbox" bind:checked={draft.weave.enabled} />
            <p class="hint">
              Off by default. Turning it on embeds every note in the vault, which is real
              work on your machine — a thing to opt into, not to discover.
            </p>
          </div>
          <div class="row">
            <span class="label">Suggest above</span>
            <span class="control">
              <input type="range" min="0.5" max="0.98" step="0.01" bind:value={draft.weave.threshold} />
              <span class="data value">{draft.weave.threshold.toFixed(2)}</span>
            </span>
            <p class="hint">
              Cosine similarity. Higher means fewer, better suggestions; an inbox full of
              weak ones is an inbox you stop reading.
            </p>
          </div>
          <div class="row">
            <span class="label">CPU ceiling</span>
            <span class="control">
              <input type="range" min="0.01" max="0.15" step="0.01" bind:value={draft.weave.cpuFraction} />
              <span class="data value">{Math.round(draft.weave.cpuFraction * 100)}%</span>
            </span>
            <p class="hint">
              Of one core, averaged over a minute. <strong>15% is the ceiling and cannot be
              raised</strong> — typing outranks Weave, and a setting that could overrule
              that would make it negotiable.
            </p>
          </div>
          <div class="row">
            <span class="label">Pass interval</span>
            <span class="control">
              <input type="number" min="5" max="3600" bind:value={draft.weave.intervalSecs} />
              <span class="data value">seconds</span>
            </span>
          </div>
        {:else if category === "mcp"}
          <p class="hint standalone">
            Other agents reach this vault through one surface. The list below is asked of
            the server itself over the same JSON-RPC path an external client uses — it is
            what agents actually get, not a description of it.
          </p>

          <div class="guarantee">
            <strong>No tool here writes to a file.</strong>
            <span>
              An agent proposes; a person accepts. <code>note_propose</code>, never
              <code>note_write</code> — the write operation does not exist, so it cannot be
              reached by mistake.
            </span>
          </div>

          {#if mcpError}
            <p class="hint standalone">Could not reach the MCP server: {mcpError}</p>
          {:else if tools === null}
            <p class="hint standalone">Asking the server…</p>
          {:else}
            <ul class="tools arc-stagger">
              {#each tools as t (t.name)}
                <li>
                  <span class="data toolname">{t.name}</span>
                  <span class="tooldesc">{t.description}</span>
                </li>
              {/each}
            </ul>
          {/if}

          <div class="row">
            <span class="label">Connect a client</span>
            <div class="connect">
              <div>
                <span class="data connlabel">stdio</span>
                <code class="data">arc-labs mcp</code>
              </div>
              <div>
                <span class="data connlabel">http</span>
                <code class="data">POST /api/v1/mcp</code>
              </div>
            </div>
            <p class="hint">
              A server bound past loopback needs its bearer token on the HTTP path, the
              same as every other route.
            </p>
          </div>
        {:else if category === "sync"}
          <p class="hint standalone">
            Not built yet. Scheduled sync to an always-on node — daily, weekly or monthly
            at an hour you choose — is the next piece of work.
          </p>
          <p class="hint standalone">
            To be clear about today: a vault on this machine and a vault on another are
            unrelated directories with nothing between them. What exists now keeps two
            <em>surfaces</em> in step — a desktop window and a browser sharing one engine —
            and never crossed machines.
          </p>
        {/if}
      </div>

      <footer>
        <button class="save" onclick={() => onsave($state.snapshot(draft))} disabled={!dirty || saving}>
          {saving ? "saving…" : dirty ? "Save" : "Saved"}
        </button>
        <button
          class="revert"
          onclick={() => (draft = structuredClone($state.snapshot(config)))}
          disabled={!dirty || saving}>Revert</button>
        <span class="hint footnote">
          Written to <code>config.toml</code> atomically, and parsed back before it lands.
        </span>
      </footer>
    </div>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: var(--arc-z-modal);
    background: var(--arc-scrim);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--arc-space-6);
  }

  .modal {
    width: min(940px, 100%);
    height: min(620px, 100%);
    display: flex;
    background: var(--arc-bg-1);
    border-radius: var(--arc-radius-lg);
    overflow: hidden;
    box-shadow: 0 24px 64px var(--arc-scrim);
  }

  .rail {
    width: 216px;
    flex: none;
    background: var(--arc-bg-0);
    padding: var(--arc-space-4) var(--arc-space-2);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .railhead {
    font-size: var(--arc-text-xs);
    color: var(--arc-fg-faint);
    letter-spacing: 0.06em;
    padding: 0 var(--arc-space-3) var(--arc-space-3);
  }
  .cat {
    display: flex;
    align-items: center;
    gap: var(--arc-space-2);
    padding: var(--arc-space-2) var(--arc-space-3);
    border: 0;
    background: none;
    border-radius: var(--arc-radius-sm);
    font-family: var(--arc-font-ui);
    font-size: var(--arc-text-md);
    color: var(--arc-fg-dim);
    text-align: left;
    cursor: pointer;
    transition:
      background var(--arc-dur) var(--arc-ease),
      color var(--arc-dur) var(--arc-ease);
  }
  .cat:hover {
    background: var(--arc-bg-2);
    color: var(--arc-fg);
  }
  .cat.on {
    background: var(--arc-bg-3);
    color: var(--arc-fg);
  }
  .cat.dim {
    opacity: 0.62;
  }
  .soon {
    margin-left: auto;
    font-size: var(--arc-text-xs);
    color: var(--arc-warn);
    background: var(--arc-human-wash);
    padding: 1px var(--arc-space-2);
    border-radius: var(--arc-radius-pill);
  }
  .railfoot {
    margin-top: auto;
    font-size: var(--arc-text-xs);
    color: var(--arc-fg-faint);
    padding: 0 var(--arc-space-3);
    line-height: 1.7;
  }

  .pane {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  header {
    display: flex;
    align-items: center;
    padding: var(--arc-space-4) var(--arc-space-5) 0;
  }
  h1 {
    margin: 0;
    font-size: var(--arc-text-xl);
    font-weight: 500;
    letter-spacing: -0.01em;
  }
  .close {
    margin-left: auto;
    background: none;
    border: 0;
    color: var(--arc-fg-faint);
    font-size: var(--arc-text-xl);
    line-height: 1;
    cursor: pointer;
    transition: color var(--arc-dur-fast) var(--arc-ease);
  }
  .close:hover {
    color: var(--arc-fg);
  }
  .body {
    flex: 1;
    overflow-y: auto;
    padding: var(--arc-space-4) var(--arc-space-5);
  }
  footer {
    padding: var(--arc-space-3) var(--arc-space-5);
    background: var(--arc-bg-0);
    display: flex;
    align-items: center;
    gap: var(--arc-space-3);
    flex-wrap: wrap;
  }

  .adjusted {
    margin: 0 0 var(--arc-space-4);
    padding: var(--arc-space-2) var(--arc-space-3);
    border-radius: var(--arc-radius);
    background: var(--arc-human-wash);
    color: var(--arc-warn);
    font-size: var(--arc-text-sm);
  }

  .row {
    display: grid;
    grid-template-columns: 190px minmax(0, 1fr);
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
    margin: var(--arc-space-1) 0 0;
    color: var(--arc-fg-faint);
    font-size: var(--arc-text-xs);
    line-height: var(--arc-leading);
    max-width: 58ch;
  }
  .hint.standalone {
    grid-column: auto;
    margin: 0 0 var(--arc-space-4);
  }
  .hint strong {
    color: var(--arc-fg-dim);
    font-weight: 500;
  }
  .hint.inline {
    display: block;
    grid-column: auto;
  }
  code {
    font-family: var(--arc-font-data);
  }

  .pills {
    display: flex;
    gap: var(--arc-space-2);
    flex-wrap: wrap;
  }
  .pill {
    font-family: var(--arc-font-data);
    font-size: var(--arc-text-sm);
    padding: var(--arc-space-2) var(--arc-space-3);
    border: 0;
    border-radius: var(--arc-radius-pill);
    background: var(--arc-bg-2);
    color: var(--arc-fg-faint);
    cursor: pointer;
    transition:
      background var(--arc-dur) var(--arc-ease),
      color var(--arc-dur) var(--arc-ease);
  }
  .pill:hover {
    background: var(--arc-bg-3);
    color: var(--arc-fg-dim);
  }
  .pill.on {
    background: var(--arc-accent-wash);
    color: var(--arc-accent);
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

  input.text,
  input[type="number"] {
    background: var(--arc-bg-2);
    border: 0;
    border-radius: var(--arc-radius-sm);
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

  .guarantee {
    border-radius: var(--arc-radius);
    background: var(--arc-agent-wash);
    padding: var(--arc-space-3) var(--arc-space-4);
    margin-bottom: var(--arc-space-4);
    display: flex;
    flex-direction: column;
    gap: var(--arc-space-1);
  }
  .guarantee strong {
    color: var(--arc-agent);
    font-size: var(--arc-text-md);
    font-weight: 500;
  }
  .guarantee span {
    color: var(--arc-fg-dim);
    font-size: var(--arc-text-xs);
    line-height: var(--arc-leading);
  }
  .days {
    display: flex;
    align-items: center;
    gap: var(--arc-space-3);
  }
  .num {
    width: 6rem;
    flex: none;
  }
  .unit {
    font-size: var(--arc-text-sm);
    color: var(--arc-fg-faint);
  }

  .tools {
    list-style: none;
    margin: 0 0 var(--arc-space-4);
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--arc-space-1);
  }
  .tools li {
    display: grid;
    grid-template-columns: 150px minmax(0, 1fr);
    gap: var(--arc-space-3);
    align-items: baseline;
    padding: var(--arc-space-2) var(--arc-space-3);
    background: var(--arc-bg-2);
    border-radius: var(--arc-radius-sm);
  }
  .toolname {
    font-size: var(--arc-text-sm);
    color: var(--arc-accent);
  }
  .tooldesc {
    font-size: var(--arc-text-xs);
    color: var(--arc-fg-dim);
    line-height: var(--arc-leading);
  }
  .connect {
    display: flex;
    flex-direction: column;
    gap: var(--arc-space-2);
  }
  .connect div {
    display: flex;
    align-items: center;
    gap: var(--arc-space-3);
  }
  .connlabel {
    font-size: var(--arc-text-xs);
    color: var(--arc-fg-faint);
    min-width: 5ch;
  }
  .connect code {
    font-size: var(--arc-text-sm);
    color: var(--arc-fg);
    background: var(--arc-bg-2);
    padding: var(--arc-space-1) var(--arc-space-3);
    border-radius: var(--arc-radius-sm);
  }

  .save,
  .revert {
    border-radius: var(--arc-radius-pill);
    padding: var(--arc-space-2) var(--arc-space-4);
    font-size: var(--arc-text-sm);
    cursor: pointer;
    background: var(--arc-bg-2);
    border: 0;
    color: var(--arc-fg);
    transition: background var(--arc-dur) var(--arc-ease);
  }
  .save {
    background: var(--arc-accent-wash);
    color: var(--arc-accent);
  }
  .save:disabled,
  .revert:disabled {
    opacity: 0.5;
    cursor: default;
    background: var(--arc-bg-2);
    color: var(--arc-fg-faint);
  }
  .footnote {
    grid-column: auto;
    margin: 0 0 0 auto;
    text-align: right;
    max-width: 40ch;
  }

  @media (max-width: 760px) {
    .modal {
      flex-direction: column;
    }
    .rail {
      width: auto;
      flex-direction: row;
      overflow-x: auto;
    }
    .railfoot {
      display: none;
    }
    .row {
      grid-template-columns: 1fr;
    }
    .hint,
    .choice {
      grid-column: 1;
    }
  }
</style>
