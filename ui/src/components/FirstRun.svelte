<script lang="ts">
  /**
   * First run.
   *
   * This screen *is* the onboarding. The spec bans a tour outright and sets the
   * bar instead: someone who has never seen the app gets from launch to a
   * rendered note in under sixty seconds, with no instructions. So there is one
   * primary action and nothing competing with it.
   *
   * It has to work in two worlds. On the desktop there is a native folder
   * dialog. In a browser there is not, and pretending otherwise would be a dead
   * button — so the browser gets a real directory picker built on the API's
   * `browse`, which lists directories only and is switched off entirely on a
   * server bound past loopback.
   *
   * "Create a vault" needs no write path: an empty folder *is* an empty vault,
   * and the native dialog can make one. That is why Phase 0 can ship this screen
   * while `arc-labs-core` still has no way to write to disk.
   */
  import { transport } from "../lib/transport";
  import type { DirListing, Status } from "../lib/types";
  import { TransportError } from "../lib/types";
  import ArcMark from "./ArcMark.svelte";

  let { status, onopened }: { status: Status; onopened: () => void } = $props();

  let error = $state<string | null>(null);
  let busy = $state(false);

  // Browser picker state.
  let listing = $state<DirListing | null>(null);
  let typed = $state("");

  /**
   * Step two: on disk only, or connected to a vault server.
   *
   * **After** the vault is chosen, not alongside it. Two reasons. The screen
   * above is the whole onboarding and the gate on it is launch-to-a-rendered-
   * note in sixty seconds with no instructions, so it gets one primary action
   * and nothing competing. And "sync this vault somewhere" is not a coherent
   * question until there is a vault to sync.
   *
   * On disk only is one click and the default, so the fast path stays fast.
   */
  let step = $state<"vault" | "sync">("vault");
  let hub = $state("");
  let tokenEnv = $state("ARC_LABS_SYNC_TOKEN");

  async function open(path: string) {
    if (!path.trim()) return;
    busy = true;
    error = null;
    try {
      await transport.openVault(path);
      step = "sync";
    } catch (e) {
      error = e instanceof TransportError ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  /** Finish, having chosen. `connect` false leaves the config exactly as it is. */
  async function finish(connect: boolean) {
    if (connect && !hub.trim()) return;
    busy = true;
    error = null;
    try {
      if (connect) {
        const config = await transport.config();
        config.sync = {
          ...config.sync,
          role: "client",
          hub: hub.trim().replace(/\/+$/, ""),
          tokenEnv: tokenEnv.trim(),
          utcOffsetMinutes: -new Date().getTimezoneOffset(),
        };
        await transport.saveConfig(config);
      }
      onopened();
    } catch (e) {
      // Never strands anyone on this screen. The vault is already open, and a
      // server that could not be saved is a thing to fix in Settings — not a
      // reason to refuse to show someone their notes.
      error = e instanceof TransportError ? e.message : String(e);
      onopened();
    } finally {
      busy = false;
    }
  }

  async function pickNatively() {
    error = null;
    const chosen = await transport.pickFolder();
    if (chosen) await open(chosen);
  }

  async function browse(path?: string) {
    error = null;
    try {
      listing = await transport.browse(path);
      typed = listing.path;
    } catch (e) {
      error = e instanceof TransportError ? e.message : String(e);
    }
  }

  // Open the browser picker straight away when there is no native dialog: one
  // fewer click on the path that has more friction to begin with.
  $effect(() => {
    if (!status.canPickFolder && status.canBrowse && listing === null) void browse();
  });
</script>

<div class="first-run">
  <div class="panel">
    <header>
      <ArcMark size={22} />
      <div>
        <h1>ARC-LABS</h1>
        <p class="tagline">A lab notebook your agents can hold.</p>
      </div>
    </header>

    {#if step === "sync"}
      <div class="lead">
        <p>Where does this vault live?</p>
      </div>

      <div class="choices">
        <button class="primary" onclick={() => finish(false)} disabled={busy}>
          On this machine only
        </button>
        <p class="data note">
          The default. Nothing leaves this computer, and you can connect a server later
          in Settings.
        </p>

        <div class="orsync">
          <span class="data or">or connect it to a vault server</span>
          <input
            class="data"
            bind:value={hub}
            placeholder="https://vault.example"
            spellcheck="false"
          />
          <input class="data" bind:value={tokenEnv} spellcheck="false" />
          <p class="data note">
            The second box is the <em>name</em> of an environment variable holding the
            vault's token — never the token itself.
          </p>
          <button class="secondary" onclick={() => finish(true)} disabled={busy || !hub.trim()}>
            Connect
          </button>
        </div>
      </div>
    {:else if status.canPickFolder}
      <div class="lead">
        <p>Open a folder of markdown notes. An existing Obsidian vault works unchanged.</p>
        <button class="primary" onclick={pickNatively} disabled={busy}>
          Choose a folder…
        </button>
        <p class="data note">An empty folder becomes a new vault.</p>
      </div>
    {:else if status.canBrowse}
      <div class="lead">
        <p>Choose a folder of markdown notes on this machine.</p>
      </div>

      <div class="picker">
        <div class="crumbs">
          <button
            class="up"
            onclick={() => browse(listing?.parent)}
            disabled={!listing?.parent}
            title="Parent folder"
          >
            ↰
          </button>
          <span class="data path">{listing?.path ?? "…"}</span>
        </div>

        <ul class="dirs">
          {#each listing?.entries ?? [] as entry (entry.path)}
            <li>
              <button class="dir" onclick={() => browse(entry.path)}>
                <span class="name">{entry.name}</span>
                {#if entry.isVault}<span class="data badge">vault</span>{/if}
              </button>
            </li>
          {:else}
            <li class="data none">no sub-folders here</li>
          {/each}
        </ul>

        <div class="confirm">
          <input
            class="data"
            bind:value={typed}
            placeholder="/path/to/vault"
            spellcheck="false"
            onkeydown={(e) => e.key === "Enter" && open(typed)}
          />
          <button class="primary" onclick={() => open(typed)} disabled={busy || !typed}>
            Open this folder
          </button>
        </div>
      </div>
    {:else}
      <!-- A server bound past loopback: the filesystem is deliberately invisible,
           so the vault must be chosen when the server starts. -->
      <div class="lead">
        <p>
          This server was started without a vault, and browsing the filesystem is
          switched off because it is reachable from outside this machine.
        </p>
        <p class="data note">
          Restart with: arc-labs serve --vault /path/to/notes
        </p>
      </div>
    {/if}

    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}
  </div>

  <footer class="data">ARC-LABS {status.version} · {status.shell}</footer>
</div>

<style>
  .first-run {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--arc-space-5);
    background: var(--arc-bg-0);
  }

  .panel {
    width: min(520px, calc(100vw - var(--arc-space-6)));
    background: var(--arc-bg-1);
    border: 1px solid var(--arc-line);
    border-radius: var(--arc-radius-lg);
    padding: var(--arc-space-6);
    display: flex;
    flex-direction: column;
    gap: var(--arc-space-5);
  }

  header {
    display: flex;
    align-items: center;
    gap: var(--arc-space-3);
    color: var(--arc-fg);
  }
  h1 {
    margin: 0;
    font-size: var(--arc-text-lg);
    font-weight: 600;
    letter-spacing: 0.08em;
  }
  .tagline {
    margin: 2px 0 0;
    color: var(--arc-fg-dim);
    font-size: var(--arc-text-sm);
  }

  .lead {
    display: flex;
    flex-direction: column;
    gap: var(--arc-space-3);
    align-items: flex-start;
  }
  .lead p {
    margin: 0;
    color: var(--arc-fg-dim);
  }

  .primary {
    padding: var(--arc-space-2) var(--arc-space-4);
    background: var(--arc-accent);
    color: var(--arc-fg-on-accent);
    border-radius: var(--arc-radius);
    font-weight: 550;
    transition: background var(--arc-dur-fast) var(--arc-ease);
  }
  .primary:hover:not(:disabled) {
    background: var(--arc-accent-dim);
  }
  .primary:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .note {
    color: var(--arc-fg-faint);
  }

  /* Step two. The secondary path is visibly secondary: connecting a server is
     a real choice, but "on this machine only" is the one most people want and
     the one that keeps the sixty-second gate honest. */
  .choices {
    display: flex;
    flex-direction: column;
    gap: var(--arc-space-3);
    align-items: flex-start;
  }
  .orsync {
    display: flex;
    flex-direction: column;
    gap: var(--arc-space-2);
    align-items: flex-start;
    width: 100%;
    padding-top: var(--arc-space-4);
    border-top: 1px solid var(--arc-line);
  }
  .or {
    color: var(--arc-fg-faint);
  }
  .orsync input {
    width: 100%;
    background: var(--arc-bg-2);
    border: 0;
    border-radius: var(--arc-radius-sm);
    color: var(--arc-fg);
    padding: var(--arc-space-2) var(--arc-space-3);
  }
  .orsync input:focus {
    outline: none;
  }
  .secondary {
    padding: var(--arc-space-2) var(--arc-space-4);
    background: var(--arc-bg-2);
    color: var(--arc-fg);
    border-radius: var(--arc-radius);
    transition: background var(--arc-dur-fast) var(--arc-ease);
  }
  .secondary:hover:not(:disabled) {
    background: var(--arc-bg-3);
  }
  .secondary:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .picker {
    display: flex;
    flex-direction: column;
    gap: var(--arc-space-2);
    border: 1px solid var(--arc-line);
    border-radius: var(--arc-radius);
    background: var(--arc-bg-2);
    overflow: hidden;
  }

  .crumbs {
    display: flex;
    align-items: center;
    gap: var(--arc-space-2);
    padding: var(--arc-space-2) var(--arc-space-3);
    border-bottom: 1px solid var(--arc-line);
  }
  .up {
    color: var(--arc-fg-dim);
    padding: 0 var(--arc-space-1);
  }
  .up:disabled {
    opacity: 0.3;
    cursor: default;
  }
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl; /* keep the tail of a long path visible */
    text-align: left;
  }

  .dirs {
    list-style: none;
    margin: 0;
    padding: var(--arc-space-1);
    max-height: 220px;
    overflow-y: auto;
  }
  .dir {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--arc-space-2);
    padding: var(--arc-space-2) var(--arc-space-3);
    border-radius: var(--arc-radius-sm);
    text-align: left;
  }
  .dir:hover {
    background: var(--arc-bg-3);
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    flex: none;
    color: var(--arc-accent);
    border: 1px solid var(--arc-accent-dim);
    border-radius: var(--arc-radius-lg);
    padding: 0 var(--arc-space-2);
  }
  .none {
    padding: var(--arc-space-3);
    color: var(--arc-fg-faint);
  }

  .confirm {
    display: flex;
    gap: var(--arc-space-2);
    padding: var(--arc-space-2);
    border-top: 1px solid var(--arc-line);
  }
  .confirm input {
    flex: 1;
    min-width: 0;
    background: var(--arc-bg-1);
    border: 1px solid var(--arc-line-strong);
    border-radius: var(--arc-radius-sm);
    padding: var(--arc-space-2);
    color: var(--arc-fg);
  }

  .error {
    margin: 0;
    color: var(--arc-danger);
    font-size: var(--arc-text-sm);
  }

  footer {
    color: var(--arc-fg-faint);
  }
</style>
