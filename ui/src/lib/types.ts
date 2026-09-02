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
