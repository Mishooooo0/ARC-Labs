/**
 * The transport boundary.
 *
 * This is the load-bearing decision of the whole build. ARC-LABS runs as a Tauri
 * desktop app on Windows and Linux, as an HTTP server for a browser, and inside
 * Docker. That is four shells — and exactly one UI, because everything above
 * this file talks to the `Transport` interface and never learns which one it is
 * in.
 *
 * **No component may import `@tauri-apps/api`.** The moment one does, the
 * browser and Docker builds break in a way that only shows up at runtime, in the
 * one environment nobody is testing in. Everything Tauri-specific is here.
 *
 * Adding an operation means adding it to the interface and to both
 * implementations, which is deliberate friction: it is the compiler asking
 * whether the browser shell can do this too.
 */

import type {
  Backlink, CanvasRunnability, CanvasView, DirListing, EntryDiff, GraphData, IndexStats,
  ApiVersion, Config, Deleted, LinkSuggestion, NodeGeometry, VaultEvent, NoteRef, NoteView, OutgoingLink, PassReport, Proposal, RunStatus,
  SaveResult, SearchHit, Status, TagCount, TimelineEntry, TreeView, UnresolvedLink, VaultInfo,
  WeaveStatus,
} from "./types";
import { CLIENT_API_MAJOR, TransportError } from "./types";

export interface Transport {
  readonly kind: "desktop" | "server";

  /**
   * The handshake. Cached after the first call.
   *
   * Everything else on this interface assumes the two ends agree about what the
   * wire means; this is where that is established rather than assumed.
   */
  version(): Promise<ApiVersion>;
  /**
   * A stable id for this client, stamped on the changes it causes so it can
   * ignore its own events. Without it, the surface that just saved a note
   * immediately reloads because of its own save, and the editor fights whoever
   * is typing into it.
   */
  readonly clientId: string;
  /**
   * Listen for vault changes. Returns an unsubscribe function.
   *
   * WebSocket in the browser, Tauri events on the desktop — the same split that
   * already makes four shells cost one UI, extended from request/response to
   * push. Callers never learn which one they got.
   */
  subscribe(handler: (event: VaultEvent) => void): () => void;
  /**
   * Whether this deployment can do a thing — `"index"`, `"weave"`, `"mcp"`,
   * `"events"`, `"browse"`. False before the handshake completes, so callers
   * must treat it as "not yet" rather than "never".
   *
   * Branch on this rather than on version numbers: an older server and a server
   * built without a feature look the same to the UI, and both should simply not
   * show it.
   */
  can(capability: string): boolean;
  status(): Promise<Status>;
  tree(): Promise<TreeView>;
  note(path: string): Promise<NoteView>;
  /** Same as `note`, plus the raw markdown for the editor. */
  noteForEdit(path: string): Promise<NoteView>;
  /**
   * Save. `baseHash` is the hash the editor started from; the server refuses
   * rather than overwriting if the file changed underneath.
   */
  saveNote(path: string, text: string, baseHash: string): Promise<SaveResult>;

  // ── Note lifecycle ────────────────────────────────────────────────────
  /** Create a note and return it open. Refuses to overwrite. */
  createNote(path: string, text?: string): Promise<NoteView>;
  /** Move a note. Its history goes with it. */
  renameNote(from: string, to: string): Promise<NoteView>;
  /** Delete to the vault trash. History is kept. */
  deleteNote(path: string): Promise<Deleted>;
  /** A free path near `desired`, so a collision becomes "Untitled 2". */
  uniquePath(desired: string): Promise<string>;
  // ── Runtime (Phase 5) ─────────────────────────────────────────────────────
  runnability(path: string): Promise<CanvasRunnability>;
  startRun(path: string, node: string, approveEgress?: boolean): Promise<string>;
  runStatus(id: string): Promise<RunStatus>;
  cancelRun(id: string): Promise<void>;

  // ── Canvas (Phase 4) ──────────────────────────────────────────────────────
  canvas(path: string): Promise<CanvasView>;
  moveCanvasNodes(path: string, moves: NodeGeometry[]): Promise<SaveResult>;

  // ── Ledger (Phase 3) ──────────────────────────────────────────────────────
  timeline(path: string): Promise<TimelineEntry[]>;
  proposals(path: string): Promise<Proposal[]>;
  entryDiff(path: string, index: number): Promise<EntryDiff>;
  restore(path: string, index: number): Promise<SaveResult>;
  accept(path: string, index: number): Promise<SaveResult>;
  reject(path: string, index: number): Promise<void>;
  propose(
    path: string, agent: string, model: string, session: string, reason: string, content: string,
  ): Promise<Proposal>;

  // ── Index-backed (Phase 2) ────────────────────────────────────────────────
  search(q: string, limit?: number): Promise<SearchHit[]>;
  quickOpen(q: string, limit?: number): Promise<NoteRef[]>;
  recent(limit?: number): Promise<NoteRef[]>;
  backlinks(path: string): Promise<Backlink[]>;
  outgoing(path: string): Promise<OutgoingLink[]>;
  unresolved(limit?: number): Promise<UnresolvedLink[]>;
  tags(): Promise<TagCount[]>;
  tagNotes(tag: string): Promise<NoteRef[]>;
  graph(): Promise<GraphData>;
  indexStats(): Promise<IndexStats>;

  // ── Weave (Phase 6) ───────────────────────────────────────────────────────
  /** Inferred links awaiting a person. Never mixed with `outgoing`. */
  suggestions(limit?: number): Promise<LinkSuggestion[]>;
  /** Write the link into the source note. Two ledger entries: proposed, accepted. */
  acceptSuggestion(id: number): Promise<SaveResult>;
  dismissSuggestion(id: number): Promise<void>;
  weaveStatus(): Promise<WeaveStatus>;
  /** Run one bounded pass now. */
  weavePass(): Promise<PassReport>;
  /**
   * Tell the backend the user is typing, so Weave stands down.
   *
   * Fire-and-forget: nothing waits on it, and a failure is silent, because a
   * keystroke must never be delayed by bookkeeping about a background task.
   */
  userActive(): void;

  // ── Settings ─────────────────────────────────────────────────
  config(): Promise<Config>;
  /** Returns what was actually stored, which may differ from what was sent. */
  saveConfig(config: Config): Promise<Config>;

  browse(path?: string): Promise<DirListing>;
  openVault(path: string): Promise<VaultInfo>;
  /** Native folder picker. `null` in a browser, which has none. */
  pickFolder(): Promise<string | null>;
}

/**
 * A per-tab, per-window id. Not persisted: two tabs on the same machine are two
 * clients and must not swallow each other's events.
 */
function newClientId(): string {
  try {
    return crypto.randomUUID();
  } catch {
    return `c${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
  }
}

/**
 * Tauri 2 installs `__TAURI_INTERNALS__` before any app script runs. Checked
 * once at module load rather than per call, so behaviour cannot change halfway
 * through a session.
 */
function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// ── Server transport ────────────────────────────────────────────────────────

/**
 * Bearer token for a server bound past loopback. It arrives once as `?token=`,
 * is moved straight into sessionStorage, and is stripped from the URL — so it
 * does not sit in the address bar, in history, or in a `Referer` header.
 *
 * sessionStorage rather than localStorage: the grant should end with the tab.
 */
function takeToken(): string | null {
  if (typeof window === "undefined") return null;
  const key = "arc-labs-token";
  try {
    const url = new URL(window.location.href);
    const fromUrl = url.searchParams.get("token");
    if (fromUrl) {
      sessionStorage.setItem(key, fromUrl);
      url.searchParams.delete("token");
      window.history.replaceState({}, "", url.toString());
      return fromUrl;
    }
    return sessionStorage.getItem(key);
  } catch {
    // Private mode, or storage disabled. The app still works on loopback, where
    // no token is needed at all.
    return null;
  }
}

/**
 * Check a handshake and complain loudly about a major we cannot speak.
 *
 * A mismatched *major* means a field somewhere changed meaning. Carrying on and
 * hoping is how a client writes a payload the server will misread, so this
 * throws. A mismatched *minor* is fine in both directions and says nothing.
 */
function checkMajor(v: ApiVersion): ApiVersion {
  if (v.apiMajor !== CLIENT_API_MAJOR) {
    throw new TransportError({
      code: "config",
      message:
        `this ARC-LABS client speaks API v${CLIENT_API_MAJOR} and the server speaks ` +
        `v${v.apiMajor} (server build ${v.server}). Update whichever is older.`,
    });
  }
  return v;
}

class ServerTransport implements Transport {
  readonly kind = "server" as const;
  readonly clientId = newClientId();
  #token = takeToken();
  #version: ApiVersion | null = null;
  #versionInFlight: Promise<ApiVersion> | null = null;
  #socket: WebSocket | null = null;
  #listeners = new Set<(e: VaultEvent) => void>();
  #retry = 0;
  #closing = false;

  async #call<T>(path: string, init?: RequestInit): Promise<T> {
    const headers = new Headers(init?.headers);
    if (this.#token) headers.set("Authorization", `Bearer ${this.#token}`);
    // So the events caused by this request come back tagged as ours.
    headers.set("X-Arc-Client", this.clientId);
    if (init?.body) headers.set("Content-Type", "application/json");

    let res: Response;
    try {
      res = await fetch(`api/${path}`, { ...init, headers });
    } catch {
      // A dead socket is not an API error, and must not be reported as one —
      // "the server went away" is a different problem from "no such note".
      throw new TransportError({
        code: "io",
        message: "cannot reach the ARC-LABS server",
      });
    }

    if (!res.ok) {
      let body: unknown = null;
      try {
        body = await res.json();
      } catch {
        /* a non-JSON error body is still an error */
      }
      if (body && typeof body === "object" && "code" in body) {
        throw new TransportError(body as never);
      }
      throw new TransportError({ code: "io", message: `request failed (${res.status})` });
    }
    return (await res.json()) as T;
  }

  async version() {
    if (this.#version) return this.#version;
    // One request even if twenty callers ask at once during boot.
    this.#versionInFlight ??= this.#call<ApiVersion>("version").then((v) => {
      this.#version = checkMajor(v);
      this.#versionInFlight = null;
      return this.#version;
    });
    return this.#versionInFlight;
  }

  can(capability: string) {
    return this.#version?.capabilities.includes(capability) ?? false;
  }

  subscribe(handler: (event: VaultEvent) => void) {
    this.#listeners.add(handler);
    this.#connect();
    return () => {
      this.#listeners.delete(handler);
      if (this.#listeners.size === 0) {
        this.#closing = true;
        this.#socket?.close();
        this.#socket = null;
      }
    };
  }

  #connect() {
    if (this.#socket || this.#listeners.size === 0) return;
    this.#closing = false;

    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const base = new URL("api/v1/events", location.href).pathname;
    // The token rides in the query string because a browser cannot set headers
    // on a WebSocket handshake. It never reaches a server log we do not own —
    // this URL is always same-origin — and the server accepts it only here.
    const q = this.#token ? `?token=${encodeURIComponent(this.#token)}` : "";
    const socket = new WebSocket(`${proto}//${location.host}${base}${q}`);
    this.#socket = socket;

    socket.onopen = () => {
      this.#retry = 0;
    };
    socket.onmessage = (m) => {
      try {
        const event = JSON.parse(m.data as string) as VaultEvent;
        for (const l of this.#listeners) l(event);
      } catch {
        /* a malformed frame is not worth tearing the connection down for */
      }
    };
    socket.onclose = () => {
      this.#socket = null;
      if (this.#closing || this.#listeners.size === 0) return;
      // Backoff, capped. A server restart should not need a page reload, and a
      // server that is down should not be hammered while it comes back.
      const wait = Math.min(30_000, 500 * 2 ** this.#retry++);
      setTimeout(() => this.#connect(), wait);
    };
    socket.onerror = () => socket.close();
  }

  status() {
    return this.#call<Status>("status");
  }
  tree() {
    return this.#call<TreeView>("tree");
  }
  note(path: string) {
    return this.#call<NoteView>(`note?path=${encodeURIComponent(path)}`);
  }
  noteForEdit(path: string) {
    return this.#call<NoteView>(`note/edit?path=${encodeURIComponent(path)}`);
  }
  saveNote(path: string, text: string, baseHash: string) {
    return this.#call<SaveResult>("note/save", {
      method: "POST",
      body: JSON.stringify({ path, text, base_hash: baseHash }),
    });
  }
  runnability(path: string) {
    return this.#call<CanvasRunnability>(`canvas/runnable?path=${encodeURIComponent(path)}`);
  }
  startRun(path: string, node: string, approveEgress = false) {
    return this.#call<string>("run", {
      method: "POST",
      body: JSON.stringify({ path, node, approve_egress: approveEgress }),
    });
  }
  runStatus(id: string) {
    return this.#call<RunStatus>(`run/status?id=${encodeURIComponent(id)}`);
  }
  cancelRun(id: string) {
    return this.#call<void>("run/cancel", { method: "POST", body: JSON.stringify({ id }) });
  }
  canvas(path: string) {
    return this.#call<CanvasView>(`canvas?path=${encodeURIComponent(path)}`);
  }
  moveCanvasNodes(path: string, moves: NodeGeometry[]) {
    return this.#call<SaveResult>("canvas/move", {
      method: "POST", body: JSON.stringify({ path, moves }),
    });
  }
  timeline(path: string) {
    return this.#call<TimelineEntry[]>(`timeline?path=${encodeURIComponent(path)}`);
  }
  proposals(path: string) {
    return this.#call<Proposal[]>(`proposals?path=${encodeURIComponent(path)}`);
  }
  entryDiff(path: string, index: number) {
    return this.#call<EntryDiff>(
      `entry-diff?path=${encodeURIComponent(path)}&index=${index}`,
    );
  }
  restore(path: string, index: number) {
    return this.#call<SaveResult>("restore", {
      method: "POST", body: JSON.stringify({ path, index }),
    });
  }
  accept(path: string, index: number) {
    return this.#call<SaveResult>("accept", {
      method: "POST", body: JSON.stringify({ path, index }),
    });
  }
  reject(path: string, index: number) {
    return this.#call<void>("reject", {
      method: "POST", body: JSON.stringify({ path, index }),
    });
  }
  propose(path: string, agent: string, model: string, session: string, reason: string, content: string) {
    return this.#call<Proposal>("propose", {
      method: "POST",
      body: JSON.stringify({ path, agent, model, session, reason, content }),
    });
  }

  search(q: string, limit = 50) {
    return this.#call<SearchHit[]>(`search?q=${encodeURIComponent(q)}&limit=${limit}`);
  }
  quickOpen(q: string, limit = 50) {
    return this.#call<NoteRef[]>(`quick-open?q=${encodeURIComponent(q)}&limit=${limit}`);
  }
  recent(limit = 20) {
    return this.#call<NoteRef[]>(`recent?limit=${limit}`);
  }
  backlinks(path: string) {
    return this.#call<Backlink[]>(`backlinks?path=${encodeURIComponent(path)}`);
  }
  outgoing(path: string) {
    return this.#call<OutgoingLink[]>(`outgoing?path=${encodeURIComponent(path)}`);
  }
  unresolved(limit = 100) {
    return this.#call<UnresolvedLink[]>(`unresolved?limit=${limit}`);
  }
  tags() {
    return this.#call<TagCount[]>("tags");
  }
  tagNotes(tag: string) {
    return this.#call<NoteRef[]>(`tag?q=${encodeURIComponent(tag)}`);
  }
  graph() {
    return this.#call<GraphData>("graph");
  }
  indexStats() {
    return this.#call<IndexStats>("index-stats");
  }

  createNote(path: string, text = "") {
    return this.#call<NoteView>("note/create", {
      method: "POST",
      body: JSON.stringify({ path, text }),
    });
  }
  renameNote(from: string, to: string) {
    return this.#call<NoteView>("note/rename", {
      method: "POST",
      body: JSON.stringify({ from, to }),
    });
  }
  deleteNote(path: string) {
    return this.#call<Deleted>("note/delete", {
      method: "POST",
      body: JSON.stringify({ path }),
    });
  }
  uniquePath(desired: string) {
    return this.#call<string>(`note/unique-path?q=${encodeURIComponent(desired)}`);
  }

  suggestions(limit = 50) {
    return this.#call<LinkSuggestion[]>(`suggestions?limit=${limit}`);
  }
  acceptSuggestion(id: number) {
    return this.#call<SaveResult>("suggestion/accept", {
      method: "POST",
      body: JSON.stringify({ id }),
    });
  }
  dismissSuggestion(id: number) {
    return this.#call<void>("suggestion/dismiss", {
      method: "POST",
      body: JSON.stringify({ id }),
    });
  }
  weaveStatus() {
    return this.#call<WeaveStatus>("weave/status");
  }
  weavePass() {
    return this.#call<PassReport>("weave/pass", { method: "POST" });
  }
  userActive() {
    // Not sent. A fetch per keystroke would cost more than the daemon it is
    // trying to quiet — and the server owns the daemon, so it can see its own
    // save traffic. The desktop shell, where the call is one IPC hop and Weave
    // runs in-process, does send it.
  }

  config() {
    return this.#call<Config>("config");
  }
  saveConfig(config: Config) {
    return this.#call<Config>("config", {
      method: "POST",
      body: JSON.stringify(config),
    });
  }

  browse(path?: string) {
    const q = path ? `?path=${encodeURIComponent(path)}` : "";
    return this.#call<DirListing>(`browse${q}`);
  }
  openVault(path: string) {
    return this.#call<VaultInfo>("vault/open", {
      method: "POST",
      body: JSON.stringify({ path }),
    });
  }
  async pickFolder() {
    // A browser cannot open a native folder dialog, and must not pretend to.
    // The first-run surface uses `browse()` to build a picker instead.
    return null;
  }
}

// ── Desktop transport ───────────────────────────────────────────────────────

class DesktopTransport implements Transport {
  readonly kind = "desktop" as const;
  readonly clientId = newClientId();
  #version: ApiVersion | null = null;

  async #invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    // Imported lazily and by name so the browser bundle never resolves it.
    const { invoke } = await import("@tauri-apps/api/core");
    try {
      return (await invoke<T>(cmd, args)) as T;
    } catch (e) {
      // Tauri rejects with whatever the command returned — our ApiError shape.
      if (e && typeof e === "object" && "code" in e) throw new TransportError(e as never);
      throw new TransportError({ code: "io", message: String(e) });
    }
  }

  async version() {
    // In-process, so the majors cannot disagree — the check runs anyway, because
    // a guarantee that is only true by construction stops being true the moment
    // the construction changes.
    this.#version ??= checkMajor(await this.#invoke<ApiVersion>("api_version"));
    return this.#version;
  }

  can(capability: string) {
    return this.#version?.capabilities.includes(capability) ?? false;
  }

  subscribe(handler: (event: VaultEvent) => void) {
    // Tauri's listen() is async but the caller needs an unsubscribe now, so the
    // teardown is deferred behind the same handle rather than made async.
    let stop: (() => void) | null = null;
    let cancelled = false;

    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const un = await listen<VaultEvent>("arc:vault", (e) => handler(e.payload));
      if (cancelled) un();
      else stop = un;
    })();

    return () => {
      cancelled = true;
      stop?.();
    };
  }

  status() {
    return this.#invoke<Status>("status");
  }
  tree() {
    return this.#invoke<TreeView>("tree");
  }
  note(path: string) {
    return this.#invoke<NoteView>("note", { path });
  }
  noteForEdit(path: string) {
    return this.#invoke<NoteView>("note_for_edit", { path });
  }
  saveNote(path: string, text: string, baseHash: string) {
    return this.#invoke<SaveResult>("save_note", { path, text, baseHash });
  }
  runnability(path: string) {
    return this.#invoke<CanvasRunnability>("runnability", { path });
  }
  startRun(path: string, node: string, approveEgress = false) {
    return this.#invoke<string>("start_run", { path, node, approveEgress });
  }
  runStatus(id: string) {
    return this.#invoke<RunStatus>("run_status", { id });
  }
  cancelRun(id: string) {
    return this.#invoke<void>("cancel_run", { id });
  }
  canvas(path: string) {
    return this.#invoke<CanvasView>("canvas", { path });
  }
  moveCanvasNodes(path: string, moves: NodeGeometry[]) {
    return this.#invoke<SaveResult>("move_canvas", { path, moves });
  }
  timeline(path: string) {
    return this.#invoke<TimelineEntry[]>("timeline", { path });
  }
  proposals(path: string) {
    return this.#invoke<Proposal[]>("proposals", { path });
  }
  entryDiff(path: string, index: number) {
    return this.#invoke<EntryDiff>("entry_diff", { path, index });
  }
  restore(path: string, index: number) {
    return this.#invoke<SaveResult>("restore", { path, index });
  }
  accept(path: string, index: number) {
    return this.#invoke<SaveResult>("accept", { path, index });
  }
  reject(path: string, index: number) {
    return this.#invoke<void>("reject", { path, index });
  }
  propose(path: string, agent: string, model: string, session: string, reason: string, content: string) {
    return this.#invoke<Proposal>("propose", { path, agent, model, session, reason, content });
  }

  search(q: string, limit = 50) {
    return this.#invoke<SearchHit[]>("search", { q, limit });
  }
  quickOpen(q: string, limit = 50) {
    return this.#invoke<NoteRef[]>("quick_open", { q, limit });
  }
  recent(limit = 20) {
    return this.#invoke<NoteRef[]>("recent", { limit });
  }
  backlinks(path: string) {
    return this.#invoke<Backlink[]>("backlinks", { path });
  }
  outgoing(path: string) {
    return this.#invoke<OutgoingLink[]>("outgoing", { path });
  }
  unresolved(limit = 100) {
    return this.#invoke<UnresolvedLink[]>("unresolved", { limit });
  }
  tags() {
    return this.#invoke<TagCount[]>("tags");
  }
  tagNotes(tag: string) {
    return this.#invoke<NoteRef[]>("tag_notes", { q: tag });
  }
  graph() {
    return this.#invoke<GraphData>("graph");
  }
  indexStats() {
    return this.#invoke<IndexStats>("index_stats");
  }

  createNote(path: string, text = "") {
    return this.#invoke<NoteView>("create_note", { path, text });
  }
  renameNote(from: string, to: string) {
    return this.#invoke<NoteView>("rename_note", { from, to });
  }
  deleteNote(path: string) {
    return this.#invoke<Deleted>("delete_note", { path });
  }
  uniquePath(desired: string) {
    return this.#invoke<string>("unique_path", { q: desired });
  }

  suggestions(limit = 50) {
    return this.#invoke<LinkSuggestion[]>("suggestions", { limit });
  }
  acceptSuggestion(id: number) {
    return this.#invoke<SaveResult>("accept_suggestion", { id });
  }
  dismissSuggestion(id: number) {
    return this.#invoke<void>("dismiss_suggestion", { id });
  }
  weaveStatus() {
    return this.#invoke<WeaveStatus>("weave_status");
  }
  weavePass() {
    return this.#invoke<PassReport>("weave_pass");
  }
  userActive() {
    // Deliberately not awaited and deliberately swallowing errors: this runs on
    // the keystroke path, where the only acceptable cost is the one IPC hop.
    void this.#invoke<void>("user_active").catch(() => {});
  }

  config() {
    return this.#invoke<Config>("get_config");
  }
  saveConfig(config: Config) {
    return this.#invoke<Config>("set_config", { config });
  }

  browse(path?: string) {
    return this.#invoke<DirListing>("browse", { path: path ?? null });
  }
  openVault(path: string) {
    return this.#invoke<VaultInfo>("open_vault", { path });
  }
  pickFolder() {
    return this.#invoke<string | null>("pick_folder");
  }
}

/** Chosen once, at module load. */
export const transport: Transport = inTauri() ? new DesktopTransport() : new ServerTransport();
