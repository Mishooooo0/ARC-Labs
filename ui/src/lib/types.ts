/**
 * Wire types, mirroring `arc-labs-api::types`.
 *
 * Hand-written rather than generated. The surface is small, it changes with the
 * phases, and a generator would be a build step to maintain for a file this
 * size. If it grows past a screen or two, generate it — but not before.
 */

export type VaultStatus = "offline" | "scanning" | "online" | "indexing";
export type Shell = "desktop" | "server";

export interface VaultInfo {
  name: string;
  /** This process cannot write to the vault. Reading still works. */
  readOnly?: boolean;
  /** Absent when the server is serving a client that must not learn its layout. */
  path?: string;
  noteCount: number;
  canvasCount: number;
}

export interface Status {
  status: VaultStatus;
  shell: Shell;
  version: string;
  vault?: VaultInfo;
  canBrowse: boolean;
  canPickFolder: boolean;
}

export interface Link {
  target: string;
  anchor?: string;
  alias?: string;
  display: string;
  /**
   * Whether the link resolves to a real note. `undefined` until Phase 2 builds
   * the index — constraint 7: an unknown is never rendered as an answer.
   */
  resolved?: boolean;
}

export interface NoteView {
  path: string;
  name: string;
  html: string;
  frontmatter?: string;
  links: Link[];
  embeds: Link[];
  tags: string[];
  size: number;
  lineEnding: string;
  /** True when the file mixes CRLF and LF. */
  lineEndingsMixed: boolean;
  /** Raw markdown. Present only from `noteForEdit`. */
  text?: string;
  /** Content hash as read — sent back on save to detect an external write. */
  hash: string;
}

export interface SaveResult {
  /** False when the bytes already matched disk, so mtime is untouched. */
  written: boolean;
  bytes: number;
  /** The new base hash for the next save. */
  hash: string;
}

export interface TreeEntry {
  path: string;
  name: string;
  is_dir: boolean;
  parent: number | null;
}

export interface Skipped {
  path: string;
  reason: string;
}

export interface TreeView {
  entries: TreeEntry[];
  note_count: number;
  canvas_count: number;
  skipped: Skipped[];
}

export interface DirEntry {
  name: string;
  path: string;
  isVault: boolean;
}

export interface DirListing {
  path: string;
  parent?: string;
  entries: DirEntry[];
}

export type ErrorCode =
  | "no_vault"
  | "vault_not_found"
  | "not_a_directory"
  | "note_not_found"
  /** A create or rename would land on an existing file. The UI picks another name. */
  | "already_exists"
  | "invalid_path"
  | "not_utf8"
  | "config"
  | "not_permitted"
  | "conflict"
  | "io";

export interface ApiError {
  code: ErrorCode;
  message: string;
}

export class TransportError extends Error {
  code: ErrorCode;
  constructor(e: ApiError) {
    super(e.message);
    this.name = "TransportError";
    this.code = e.code;
  }
}

// ── Index-backed types (Phase 2) ─────────────────────────────────────────────

export interface NoteRef {
  path: string;
  title: string;
  isCanvas: boolean;
}

export interface SearchHit {
  path: string;
  title: string;
  /** Context around the match, matched terms wrapped in «…». */
  snippet: string;
}

export interface Backlink {
  path: string;
  title: string;
  target: string;
  alias?: string;
  isEmbed: boolean;
}

export interface OutgoingLink {
  target: string;
  anchor?: string;
  alias?: string;
  isEmbed: boolean;
  /** Absent when the link resolves to nothing. Never guessed. */
  resolvedPath?: string;
}

export interface UnresolvedLink {
  target: string;
  count: number;
  sources: string[];
}

export interface TagCount {
  name: string;
  count: number;
}

export interface IndexStats {
  notes: number;
  canvases: number;
  links: number;
  resolvedLinks: number;
  unresolvedLinks: number;
  tags: number;
  distinctTags: number;
  orphans: number;
}

export interface GraphNode {
  id: number;
  path: string;
  title: string;
  isCanvas: boolean;
  degree: number;
}

export interface GraphEdge {
  source: number;
  target: number;
}

export interface GraphData {
  nodes: GraphNode[];
  /** Observed links only. Phase 6 inferred edges arrive as a separate field. */
  edges: GraphEdge[];
}

// ── Ledger (Phase 3) ─────────────────────────────────────────────────────────

export interface TimelineEntry {
  index: number;
  ts: string;
  /**
   * "human" or "agent". The field the whole surface hangs off: it decides
   * whether an entry is drawn amber or blue, which is constraint 6.
   */
  actorKind: "human" | "agent";
  actorId: string;
  model?: string;
  session?: string;
  op: string;
  reason: string;
  /** Whether this operation changed the file. A proposal did not. */
  touchedFile: boolean;
  added: number;
  removed: number;
  fromPath?: string;
  destination?: string;
  bytes?: number;
}

export interface Proposal {
  index: number;
  ts: string;
  actorId: string;
  model?: string;
  reason: string;
  patch: string;
  added: number;
  removed: number;
}

export interface EntryDiff {
  index: number;
  patch: string;
  content?: string;
}

// ── Canvas (Phase 4) ─────────────────────────────────────────────────────────

export interface CanvasNode {
  id: string;
  kind: "text" | "file" | "link" | "group" | "unknown";
  /** prompt | query | transform, when this is an ARC node. */
  arcKind?: string;
  x: number;
  y: number;
  width: number;
  height: number;
  file?: string;
  text?: string;
  url?: string;
  color?: string;
  /**
   * "human" or "agent" — who last changed what this card shows. Absent when
   * there is no record either way, and deliberately not defaulted: a card with
   * no history gets no border rather than an invented one.
   */
  author?: "human" | "agent";
  authorModel?: string;
}

export interface CanvasEdge {
  id: string;
  fromNode: string;
  toNode: string;
  fromSide?: string;
  toSide?: string;
  label?: string;
  color?: string;
}

export interface CanvasView {
  path: string;
  name: string;
  nodes: CanvasNode[];
  edges: CanvasEdge[];
}

export interface NodeGeometry {
  id: string;
  x: number;
  y: number;
  width?: number;
  height?: number;
}

// ── Runtime (Phase 5) ────────────────────────────────────────────────────────

export type RunState =
  | "running"
  | "done"
  | "cancelled"
  | "failed"
  /** Waiting for a person to approve sending vault content off the machine. */
  | "needsEgressApproval";

export interface RunNodeState {
  id: string;
  kind: string;
  running: boolean;
  /** Grows while a prompt node streams. */
  output: string;
  tokens?: number;
  tokensPerSec?: number;
  elapsedMs?: number;
  peakRssBytes?: number;
  proposedTo?: string;
}

export interface RunStatus {
  id: string;
  canvas: string;
  target: string;
  state: RunState;
  nodes: RunNodeState[];
  totalTokens: number;
  elapsedMs: number;
  error?: string;
  /** Present while vault bytes are going somewhere. Drives the indicator. */
  egressTo?: string;
  egressBytes?: number;
}

export interface CanvasRunnability {
  /** Node ids caught in a cycle. Non-empty disables Run. */
  cycle: string[];
  runnable: string[];
}

// ── Phase 6 — inferred, and it says so ──────────────────────────────────────

/**
 * A link ARC-LABS *thinks* might belong.
 *
 * Deliberately not shaped like {@link Link}. Constraint 7 says an inference must
 * never be presented as an observation, and the cheapest way to hold that line
 * is for the two to be different types — a component that draws real links
 * cannot be handed one of these by accident, because it will not type-check.
 * `score` and `model` are required for the same reason: an inferred edge carries
 * its provenance everywhere it appears, or it does not appear.
 */
export interface LinkSuggestion {
  id: number;
  srcPath: string;
  srcTitle: string;
  dstPath: string;
  dstTitle: string;
  /** Cosine similarity, 0..1. */
  score: number;
  /** The embedding model that produced it. */
  model: string;
  createdAt: string;
  /** Always true. Present so this cannot be mistaken for an observed link. */
  inferred: true;
}

export interface PassReport {
  embedded: number;
  skippedUnchanged: number;
  suggested: number;
  /** Why the pass ended early: "UserActive", "QueueBacked", "Stopped". */
  stoppedBecause?: string;
  elapsedMs: number;
  cpuFraction: number;
  remaining: number;
  /** How long the daemon must idle before working again, to stay in budget. */
  owedMs: number;
}

export interface WeaveStatus {
  running: boolean;
  enabled: boolean;
  model: string;
  embedded: number;
  total: number;
  openSuggestions: number;
  cpuFraction: number;
  /** Seconds until the budget allows another pass. Zero means now. */
  coolingSecs: number;
  lastPass?: PassReport;
}

/** What a delete did. `recoverable` is always true — the surface says so. */
export interface Deleted {
  path: string;
  /** Where the bytes went. Absent when the client may not see host paths. */
  trashedTo?: string;
  recoverable: boolean;
}

// ── The version contract ────────────────────────────────────────────────

/**
 * What the server says it is, before anything else is asked of it.
 *
 * `apiMajor` changes only when something is removed or repurposed — a client
 * that does not know the major must refuse rather than guess. `apiMinor` only
 * ever grows, additively, so an older client is always safe against a newer
 * server. `capabilities` is what UI code should actually branch on: version
 * numbers describe the wire, capabilities describe what this deployment can do,
 * and a headless server missing a folder picker is not a version difference.
 */
export interface ApiVersion {
  apiMajor: number;
  apiMinor: number;
  /** The build string. For bug reports. Never branch on it. */
  server: string;
  shell: "desktop" | "server";
  capabilities: string[];
}

/** The major this UI was written against. */
export const CLIENT_API_MAJOR = 1;

// ── Change events ───────────────────────────────────────────────────────────

export type EventKind =
  | "created"
  | "edited"
  | "renamed"
  | "deleted"
  | "proposed"
  | "accepted"
  | "indexReady"
  | "suggested"
  /** Synthesised locally when the stream drops events or reconnects. */
  | "lagged";

/**
 * Something changed in the vault, pushed rather than polled.
 *
 * Carries what changed and who did it, never the content — pushing note bodies
 * to every listener would put the whole vault on the wire each time anyone
 * typed. A client that cares fetches the note it is actually showing.
 */
export interface VaultEvent {
  kind: EventKind;
  path?: string;
  /** Where a rename came from. */
  from?: string;
  /** Content hash after the change, so a client can tell if it already has it. */
  hash?: string;
  /** Which client caused it. Clients ignore their own. */
  origin?: string;
  actorKind: string;
  at: string;
  /** Present on a `lagged` notice: how many events this client missed. */
  missed?: number;
}

// ── Settings ───────────────────────────────────────────────────────

/**
 * Settings, in the API's shape.
 *
 * Mirrors `arc_labs_api::Settings`, **not** the config file. The file is
 * snake_case because a person edits it by hand; the wire is camelCase like
 * every other type here. Sending the file's shape produced a 422 on every save,
 * which is why the two are separate.
 *
 * The server clamps the Weave budget, bounds motion, and keeps `vault` as it
 * was, so this is a request rather than a command — always render what comes
 * *back* from a save.
 */
export interface Config {
  vault?: string;
  actorId?: string;
  ui: {
    theme: string;
    /** Multiplies every duration in the UI. 0 disables animation entirely. */
    motion: number;
    density: "compact" | "comfortable";
  };
  model: {
    endpoint: string;
    instruct: string;
    embed: string;
    access: "local-only" | "trusted-endpoint" | "ask-each-run";
  };
  weave: {
    enabled: boolean;
    threshold: number;
    /** Clamped server-side to the 0.15 ceiling. It is a gate, not a preference. */
    cpuFraction: number;
    intervalSecs: number;
  };
  trash: {
    /**
     * Days a deleted note's copy is kept on disk. `0` keeps it for ever.
     *
     * This is not how long a note is recoverable — the ledger keeps content
     * for ever and restore replays from it. What expires is the second copy.
     */
    retentionDays: number;
  };
}

/** One tool the MCP server exposes, as `tools/list` returns it. */
export interface McpTool {
  name: string;
  description: string;
}

/** A template file, as the creation window lists it. */
export interface Template {
  /** Vault path — a template is an ordinary note and can be opened as one. */
  path: string;
  /** The file stem, which is what the picker shows. */
  name: string;
}

/** What the creation window can make. */
export type CreateKind = "note" | "canvas" | "folder";
