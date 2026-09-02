//! The HTTP shell: headless Linux, any browser, and the Docker image.
//!
//! Every handler here is a thin wrapper over [`arc_labs_api::Api`]. If a handler
//! grows logic, that logic is in the wrong crate — it would exist for browser
//! users and not for desktop users, which is precisely the class of bug this
//! architecture exists to make impossible.
//!
//! # Binding, and why the default is not configurable away by accident
//!
//! Loopback by default. Serving your own UI to your own browser on `127.0.0.1`
//! is a user-initiated local action, so constraint 3 holds unchanged.
//!
//! Binding anywhere else requires **both** an explicit `--host` and a token, and
//! the token is generated rather than chosen — there is no "no auth on 0.0.0.0"
//! configuration to fall into. Beyond loopback the API also runs with
//! [`Capabilities::remote_server`], so the filesystem cannot be browsed and the
//! vault's absolute path is never sent.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use arc_labs_api::{Api, ApiError, Capabilities, ErrorCode};
use arc_labs_core::VaultPath;
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

/// Content-Security-Policy for the browser shell.
///
/// The mirror of the Tauri shell's CSP, and it does the same job: constraint 3
/// stops being a promise the code makes and becomes something the browser
/// refuses. `connect-src 'self'` means the page cannot reach any other origin,
/// so even a compromised bundle cannot exfiltrate a vault. Phase 5 widens this
/// by exactly one configured Ollama origin, and that widening is the thing the
/// egress ledger records.
const CSP: &str = "default-src 'self'; \
     script-src 'self'; \
     style-src 'self' 'unsafe-inline'; \
     img-src 'self' data: blob:; \
     font-src 'self' data:; \
     connect-src 'self'; \
     object-src 'none'; \
     base-uri 'none'; \
     form-action 'none'; \
     frame-ancestors 'none'";

pub struct ServerConfig {
    pub host: IpAddr,
    pub port: u16,
    /// Directory holding the built UI (`ui/dist`).
    pub ui_dir: PathBuf,
    /// Required in `Authorization: Bearer …` when bound past loopback.
    pub token: Option<String>,
}

impl ServerConfig {
    pub fn is_loopback(&self) -> bool {
        self.host.is_loopback()
    }

    /// The capability set this binding earns.
    pub fn capabilities(&self) -> Capabilities {
        if self.is_loopback() {
            Capabilities::local_server()
        } else {
            Capabilities::remote_server()
        }
    }

    pub fn addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }
}

/// A token with enough entropy to be worth having, from OS randomness.
///
/// Hand-rolled rather than pulling in `rand`: this is the only random value in
/// the product, `getrandom` is already in the tree via blake3, and a dependency
/// whose job is one call is a dependency to audit forever.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).expect("OS randomness unavailable");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

struct AppState {
    api: Arc<Api>,
    token: Option<String>,
}

/// Errors cross the wire as the same JSON shape the Tauri shell returns, so the
/// UI has one error path rather than two.
struct WebError(ApiError);

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let status = match self.0.code {
            ErrorCode::NoVault | ErrorCode::NoteNotFound | ErrorCode::VaultNotFound => {
                StatusCode::NOT_FOUND
            }
            ErrorCode::InvalidPath | ErrorCode::NotADirectory | ErrorCode::NotUtf8 => {
                StatusCode::BAD_REQUEST
            }
            ErrorCode::NotPermitted => StatusCode::FORBIDDEN,
            // Both mean "the thing you asked for clashes with what is there".
            ErrorCode::Conflict | ErrorCode::AlreadyExists => StatusCode::CONFLICT,
            ErrorCode::Config | ErrorCode::Io => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(self.0)).into_response()
    }
}

impl From<ApiError> for WebError {
    fn from(e: ApiError) -> Self {
        WebError(e)
    }
}

type WebResult<T> = Result<Json<T>, WebError>;

pub fn router(api: Arc<Api>, cfg: &ServerConfig) -> Router {
    let state = Arc::new(AppState {
        api,
        token: cfg.token.clone(),
    });

    let index = cfg.ui_dir.join("index.html");
    // SPA fallback: unknown paths serve index.html so client-side routing works,
    // while /api/* is matched first and never falls through to it.
    let static_files = ServeDir::new(&cfg.ui_dir).fallback(ServeFile::new(index));

    let api_routes = Router::new()
        .route("/status", get(status))
        .route("/tree", get(tree))
        .route("/note", get(note))
        .route("/note/edit", get(note_for_edit))
        .route("/note/save", post(save_note))
        .route("/note/create", post(create_note))
        .route("/note/rename", post(rename_note))
        .route("/note/delete", post(delete_note))
        .route("/note/unique-path", get(unique_path))
        .route("/canvas", get(canvas))
        .route("/canvas/runnable", get(runnability))
        .route("/run", post(start_run))
        .route("/run/status", get(run_status))
        .route("/run/cancel", post(cancel_run))
        .route("/runs", get(list_runs))
        .route("/canvas/move", post(move_canvas))
        .route("/timeline", get(timeline))
        .route("/entry-diff", get(entry_diff))
        .route("/proposals", get(proposals))
        .route("/restore", post(restore))
        .route("/propose", post(propose))
        .route("/accept", post(accept))
        .route("/reject", post(reject))
        .route("/search", get(search))
        .route("/quick-open", get(quick_open))
        .route("/backlinks", get(backlinks))
        .route("/outgoing", get(outgoing))
        .route("/unresolved", get(unresolved))
        .route("/tags", get(tags))
        .route("/tag", get(tag_notes))
        .route("/graph", get(graph))
        .route("/index-stats", get(index_stats))
        .route("/recent", get(recent))
        .route("/suggestions", get(suggestions))
        .route("/suggestion/accept", post(accept_suggestion))
        .route("/suggestion/dismiss", post(dismiss_suggestion))
        .route("/weave/status", get(weave_status))
        .route("/weave/pass", post(weave_pass))
        .route("/mcp", post(mcp))
        .route("/events", get(events))
        .route("/browse", get(browse))
        .route("/vault/open", post(open_vault))
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state.clone());

    // The handshake lives outside the *version* prefix, because a client cannot
    // ask which version to speak from behind a version-specific path. It stays
    // behind the token like everything else: on a non-loopback bind, even
    // "which build are you and what can you do" is more than an unauthenticated
    // caller needs.
    let handshake = Router::new()
        .route("/version", get(api_version))
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state.clone());

    Router::new()
        // Liveness, deliberately outside the auth layer.
        //
        // A probe runs inside the container and has no way to learn the token,
        // so a health endpoint behind auth is one that always fails: every
        // container reports unhealthy for ever and an orchestrator restarts it
        // in a loop. It was doing exactly that until a container was actually
        // run and inspected.
        //
        // Unauthenticated means it must give nothing away. It answers "this
        // process is serving" and not one word about the vault — no name, no
        // counts, no version, no whether a vault is even open.
        .route("/healthz", get(|| async { "ok" }))
        // Canonical, versioned. New clients use this.
        .nest("/api/v1", api_routes.clone())
        // The unversioned alias, kept pointing at the current major so every
        // client written before versioning existed keeps working. It is an
        // alias, not a second API: when the major changes, this follows it, and
        // clients that care about stability say /v1 explicitly.
        .nest("/api", api_routes)
        // The handshake answers on both mounts. A client that only knows
        // `/api/v1` must still be able to ask what version to speak, and a
        // client that predates versioning must still find it at `/api`.
        .nest("/api/v1", handshake.clone())
        .nest("/api", handshake)
        .fallback_service(static_files)
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        ))
        // Defence in depth for the rest of what a browser will happily do.
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(tower_http::trace::TraceLayer::new_for_http())
}

/// Bearer check. A no-op when no token is set, which is only ever the case on a
/// loopback bind — [`ServerConfig`] is what enforces that pairing.
async fn auth(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    // Which client is asking, so the events this request causes come back
    // tagged and that client can ignore its own. Untrusted and only ever
    // compared for equality by the client that sent it, so a forged value can
    // at worst make someone miss their own echo.
    state.api.set_origin(
        req.headers()
            .get("x-arc-client")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
    );

    let Some(expected) = state.token.as_deref() else {
        return next.run(req).await;
    };

    let from_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string);

    // A browser cannot set headers on a WebSocket handshake, so the token may
    // also arrive as a query parameter. Accepted only for the events upgrade:
    // everywhere else a query token would end up in logs and history for no
    // reason, and this URL is always same-origin.
    let from_query = if req.uri().path().ends_with("/events") {
        req.uri().query().and_then(|q| {
            q.split('&')
                .filter_map(|kv| kv.split_once('='))
                .find(|(k, _)| *k == "token")
                .map(|(_, v)| percent_decode(v))
        })
    } else {
        None
    };

    let presented = from_header.or(from_query);

    match presented.as_deref() {
        Some(t) if constant_time_eq(t.as_bytes(), expected.as_bytes()) => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            Json(ApiError::new(
                ErrorCode::NotPermitted,
                "a valid token is required",
            )),
        )
            .into_response(),
    }
}

/// Compare without leaking the position of the first difference through timing.
/// Just enough percent-decoding for a token in a query string. Tokens are
/// generated from an alphanumeric alphabet, so this only ever has to survive a
/// client that encoded them anyway.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `GET /api/v1/events` — the push channel.
///
/// Upgrades to a WebSocket and forwards every [`VaultEvent`] until the client
/// goes away. It sits behind the same token as everything else, because a
/// stream of "which note just changed" is a description of the vault.
async fn events(
    ws: axum::extract::ws::WebSocketUpgrade,
    State(s): State<Arc<AppState>>,
) -> Response {
    let mut rx = s.api.subscribe();
    ws.on_upgrade(move |mut socket| async move {
        use axum::extract::ws::Message;
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let Ok(text) = serde_json::to_string(&event) else {
                        continue;
                    };
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        // The client went away mid-send. Normal; a tab closed.
                        break;
                    }
                }
                // This connection fell behind and the channel dropped events for
                // it. Say so rather than pretending it saw everything: the
                // client's answer is to refetch, and it can only do that if it
                // knows it has a hole.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    let notice = serde_json::json!({ "kind": "lagged", "missed": n });
                    if socket
                        .send(Message::Text(notice.to_string().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                // The Api is gone; the process is shutting down.
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

/// `GET /api/version` — the one payload whose shape never changes.
async fn api_version(State(s): State<Arc<AppState>>) -> Json<arc_labs_api::ApiVersion> {
    Json(s.api.api_version())
}

async fn status(State(s): State<Arc<AppState>>) -> Json<arc_labs_api::Status> {
    Json(s.api.status())
}

async fn tree(State(s): State<Arc<AppState>>) -> WebResult<arc_labs_api::TreeView> {
    Ok(Json(s.api.tree()?))
}

#[derive(Deserialize)]
struct NoteQuery {
    /// Deserialising into `VaultPath` runs the full containment validation, so a
    /// traversal in the query string is rejected before any handler code runs.
    path: VaultPath,
}

async fn note(
    State(s): State<Arc<AppState>>,
    Query(q): Query<NoteQuery>,
) -> WebResult<arc_labs_api::NoteView> {
    Ok(Json(s.api.read_note(&q.path)?))
}

async fn note_for_edit(
    State(s): State<Arc<AppState>>,
    Query(q): Query<NoteQuery>,
) -> WebResult<arc_labs_api::NoteView> {
    Ok(Json(s.api.read_note_for_edit(&q.path)?))
}

#[derive(Deserialize)]
struct SaveBody {
    path: VaultPath,
    text: String,
    /// The hash the editor started from. Omitting it skips the conflict check,
    /// which only a caller that genuinely means to overwrite should do.
    #[serde(default)]
    base_hash: Option<String>,
}

/// Save a note.
///
/// On a blocking thread, unlike most handlers here, because this one is on the
/// typing path and it is the one that actually went wrong: a save that stalls
/// blocks an async worker, and enough stalled saves take the whole server down
/// rather than just the editor. The stall it hit is fixed — Weave no longer
/// holds the index across an embedding call — but a synchronous fsync belongs
/// off the runtime's workers regardless.
async fn save_note(
    State(s): State<Arc<AppState>>,
    Json(body): Json<SaveBody>,
) -> WebResult<arc_labs_api::SaveResult> {
    let api = s.api.clone();
    let saved = tokio::task::spawn_blocking(move || {
        api.write_note(&body.path, &body.text, body.base_hash.as_deref())
    })
    .await
    .map_err(|e| ApiError::new(ErrorCode::Io, e.to_string()))??;
    Ok(Json(saved))
}

#[derive(Deserialize)]
struct CreateBody {
    path: VaultPath,
    #[serde(default)]
    text: String,
}

/// Create a note. On a blocking thread with the other write paths.
async fn create_note(
    State(s): State<Arc<AppState>>,
    Json(body): Json<CreateBody>,
) -> WebResult<arc_labs_api::NoteView> {
    let api = s.api.clone();
    let view = tokio::task::spawn_blocking(move || api.create_note(&body.path, &body.text))
        .await
        .map_err(|e| ApiError::new(ErrorCode::Io, e.to_string()))??;
    Ok(Json(view))
}

#[derive(Deserialize)]
struct RenameBody {
    from: VaultPath,
    to: VaultPath,
}

async fn rename_note(
    State(s): State<Arc<AppState>>,
    Json(body): Json<RenameBody>,
) -> WebResult<arc_labs_api::NoteView> {
    let api = s.api.clone();
    let view = tokio::task::spawn_blocking(move || api.rename_note(&body.from, &body.to))
        .await
        .map_err(|e| ApiError::new(ErrorCode::Io, e.to_string()))??;
    Ok(Json(view))
}

async fn delete_note(
    State(s): State<Arc<AppState>>,
    Json(body): Json<NoteQuery>,
) -> WebResult<arc_labs_api::Deleted> {
    let api = s.api.clone();
    let out = tokio::task::spawn_blocking(move || api.delete_note(&body.path))
        .await
        .map_err(|e| ApiError::new(ErrorCode::Io, e.to_string()))??;
    Ok(Json(out))
}

/// A free path near a desired name, so the UI never has to show a collision.
async fn unique_path(
    State(s): State<Arc<AppState>>,
    Query(q): Query<TextQuery>,
) -> WebResult<VaultPath> {
    Ok(Json(s.api.unique_note_path(&q.q)?))
}

async fn runnability(
    State(s): State<Arc<AppState>>,
    Query(q): Query<NoteQuery>,
) -> WebResult<arc_labs_api::CanvasRunnability> {
    Ok(Json(s.api.canvas_runnability(&q.path)?))
}

#[derive(Deserialize)]
struct StartRunBody {
    path: VaultPath,
    node: String,
    #[serde(default)]
    approve_egress: bool,
}

async fn start_run(
    State(s): State<Arc<AppState>>,
    Json(b): Json<StartRunBody>,
) -> WebResult<String> {
    Ok(Json(s.api.start_run(&b.path, &b.node, b.approve_egress)?))
}

#[derive(Deserialize)]
struct RunQuery {
    id: String,
}

async fn run_status(
    State(s): State<Arc<AppState>>,
    Query(q): Query<RunQuery>,
) -> WebResult<arc_labs_api::RunStatus> {
    Ok(Json(s.api.run_status(&q.id)?))
}

async fn cancel_run(State(s): State<Arc<AppState>>, Json(q): Json<RunQuery>) -> WebResult<()> {
    Ok(Json(s.api.cancel_run(&q.id)?))
}

async fn list_runs(State(s): State<Arc<AppState>>) -> Json<Vec<arc_labs_api::RunStatus>> {
    Json(s.api.runs())
}

async fn canvas(
    State(s): State<Arc<AppState>>,
    Query(q): Query<NoteQuery>,
) -> WebResult<arc_labs_api::CanvasView> {
    Ok(Json(s.api.read_canvas(&q.path)?))
}

#[derive(Deserialize)]
struct MoveBody {
    path: VaultPath,
    moves: Vec<arc_labs_api::NodeGeometry>,
}

async fn move_canvas(
    State(s): State<Arc<AppState>>,
    Json(b): Json<MoveBody>,
) -> WebResult<arc_labs_api::SaveResult> {
    Ok(Json(s.api.move_canvas_nodes(&b.path, &b.moves)?))
}

async fn timeline(
    State(s): State<Arc<AppState>>,
    Query(q): Query<NoteQuery>,
) -> WebResult<Vec<arc_labs_api::TimelineEntry>> {
    Ok(Json(s.api.timeline(&q.path)?))
}

async fn proposals(
    State(s): State<Arc<AppState>>,
    Query(q): Query<NoteQuery>,
) -> WebResult<Vec<arc_labs_api::Proposal>> {
    Ok(Json(s.api.proposals(&q.path)?))
}

#[derive(Deserialize)]
struct EntryQuery {
    path: VaultPath,
    index: usize,
}

async fn entry_diff(
    State(s): State<Arc<AppState>>,
    Query(q): Query<EntryQuery>,
) -> WebResult<arc_labs_api::EntryDiff> {
    Ok(Json(s.api.entry_diff(&q.path, q.index)?))
}

async fn restore(
    State(s): State<Arc<AppState>>,
    Json(b): Json<EntryQuery>,
) -> WebResult<arc_labs_api::SaveResult> {
    Ok(Json(s.api.restore(&b.path, b.index)?))
}

async fn accept(
    State(s): State<Arc<AppState>>,
    Json(b): Json<EntryQuery>,
) -> WebResult<arc_labs_api::SaveResult> {
    Ok(Json(s.api.accept(&b.path, b.index)?))
}

async fn reject(State(s): State<Arc<AppState>>, Json(b): Json<EntryQuery>) -> WebResult<()> {
    Ok(Json(s.api.reject(&b.path, b.index)?))
}

#[derive(Deserialize)]
struct ProposeBody {
    path: VaultPath,
    agent: String,
    model: String,
    session: String,
    reason: String,
    content: String,
}

async fn propose(
    State(s): State<Arc<AppState>>,
    Json(b): Json<ProposeBody>,
) -> WebResult<arc_labs_api::Proposal> {
    Ok(Json(s.api.propose(
        &b.path, &b.agent, &b.model, &b.session, &b.reason, &b.content,
    )?))
}

#[derive(Deserialize)]
struct TextQuery {
    #[serde(default)]
    q: String,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    50
}

async fn search(
    State(s): State<Arc<AppState>>,
    Query(q): Query<TextQuery>,
) -> WebResult<Vec<arc_labs_index::query::SearchHit>> {
    Ok(Json(s.api.search(&q.q, q.limit)?))
}

async fn quick_open(
    State(s): State<Arc<AppState>>,
    Query(q): Query<TextQuery>,
) -> WebResult<Vec<arc_labs_index::query::NoteRef>> {
    Ok(Json(s.api.quick_open(&q.q, q.limit)?))
}

async fn recent(
    State(s): State<Arc<AppState>>,
    Query(q): Query<TextQuery>,
) -> WebResult<Vec<arc_labs_index::query::NoteRef>> {
    Ok(Json(s.api.recent(q.limit)?))
}

async fn backlinks(
    State(s): State<Arc<AppState>>,
    Query(q): Query<NoteQuery>,
) -> WebResult<Vec<arc_labs_index::query::Backlink>> {
    Ok(Json(s.api.backlinks(&q.path)?))
}

async fn outgoing(
    State(s): State<Arc<AppState>>,
    Query(q): Query<NoteQuery>,
) -> WebResult<Vec<arc_labs_index::query::OutgoingLink>> {
    Ok(Json(s.api.outgoing(&q.path)?))
}

async fn unresolved(
    State(s): State<Arc<AppState>>,
    Query(q): Query<TextQuery>,
) -> WebResult<Vec<arc_labs_index::query::UnresolvedLink>> {
    Ok(Json(s.api.unresolved(q.limit)?))
}

async fn tags(State(s): State<Arc<AppState>>) -> WebResult<Vec<arc_labs_index::query::TagCount>> {
    Ok(Json(s.api.tags()?))
}

async fn tag_notes(
    State(s): State<Arc<AppState>>,
    Query(q): Query<TextQuery>,
) -> WebResult<Vec<arc_labs_index::query::NoteRef>> {
    Ok(Json(s.api.notes_with_tag(&q.q)?))
}

// ---------------------------------------------------------------------------
// Phase 6 — the inbox, and MCP over HTTP
// ---------------------------------------------------------------------------

async fn suggestions(
    State(s): State<Arc<AppState>>,
    Query(q): Query<TextQuery>,
) -> WebResult<Vec<arc_labs_api::LinkSuggestion>> {
    Ok(Json(s.api.suggestions(q.limit)?))
}

#[derive(Deserialize)]
struct SuggestionQuery {
    id: i64,
}

async fn accept_suggestion(
    State(s): State<Arc<AppState>>,
    Json(b): Json<SuggestionQuery>,
) -> WebResult<arc_labs_api::SaveResult> {
    Ok(Json(s.api.accept_suggestion(b.id)?))
}

async fn dismiss_suggestion(
    State(s): State<Arc<AppState>>,
    Json(b): Json<SuggestionQuery>,
) -> WebResult<()> {
    Ok(Json(s.api.dismiss_suggestion(b.id)?))
}

async fn weave_status(State(s): State<Arc<AppState>>) -> WebResult<arc_labs_api::WeaveStatus> {
    Ok(Json(s.api.weave_status()?))
}

/// Run one bounded pass, on request.
///
/// The browser shell has no daemon of its own — the server owns that — so this
/// is how a user in a browser says "look now" without waiting out an interval.
async fn weave_pass(State(s): State<Arc<AppState>>) -> WebResult<arc_labs_weave::PassReport> {
    let api = s.api.clone();
    // A pass is bounded but not instant, and it is synchronous. Handing it to a
    // blocking thread keeps it off the async runtime's worker, where it would
    // stall every other request served by that worker.
    let report = tokio::task::spawn_blocking(move || api.weave_pass())
        .await
        .map_err(|e| ApiError::new(ErrorCode::Io, e.to_string()))??;
    Ok(Json(report))
}

/// MCP over HTTP: one JSON-RPC message per request.
///
/// The transport is different from stdio; the tools are identical, because both
/// call the same [`arc_labs_mcp::handle`]. That is the whole reason the Docker
/// container can serve agents the desktop app serves — there is no second
/// implementation to drift.
///
/// It sits behind the same bearer-token middleware as everything else under
/// `/api`, so a non-loopback bind does not hand an anonymous agent the vault.
async fn mcp(
    State(s): State<Arc<AppState>>,
    body: String,
) -> Result<axum::response::Response, WebError> {
    use axum::response::IntoResponse;
    let api = s.api.clone();
    let reply = tokio::task::spawn_blocking(move || arc_labs_mcp::handle(&api, &body))
        .await
        .map_err(|e| ApiError::new(ErrorCode::Io, e.to_string()))?;

    Ok(match reply {
        Some(text) => ([(header::CONTENT_TYPE, "application/json")], text).into_response(),
        // A notification. By the spec it gets no body.
        None => StatusCode::ACCEPTED.into_response(),
    })
}

async fn graph(State(s): State<Arc<AppState>>) -> WebResult<arc_labs_index::query::Graph> {
    Ok(Json(s.api.graph()?))
}

async fn index_stats(
    State(s): State<Arc<AppState>>,
) -> WebResult<arc_labs_index::query::IndexStats> {
    Ok(Json(s.api.index_stats()?))
}

#[derive(Deserialize)]
struct BrowseQuery {
    path: Option<String>,
}

async fn browse(
    State(s): State<Arc<AppState>>,
    Query(q): Query<BrowseQuery>,
) -> WebResult<arc_labs_api::DirListing> {
    Ok(Json(
        s.api.browse(q.path.as_deref().map(std::path::Path::new))?,
    ))
}

#[derive(Deserialize)]
struct OpenVaultBody {
    path: String,
}

async fn open_vault(
    State(s): State<Arc<AppState>>,
    Json(body): Json<OpenVaultBody>,
) -> WebResult<arc_labs_api::VaultInfo> {
    Ok(Json(s.api.open_vault(std::path::Path::new(&body.path))?))
}

/// Serve until the process is asked to stop.
pub async fn serve(api: Arc<Api>, cfg: ServerConfig) -> anyhow::Result<()> {
    let app = router(api, &cfg);
    let listener = tokio::net::TcpListener::bind(cfg.addr()).await?;
    let addr = listener.local_addr()?;

    tracing::info!(%addr, loopback = cfg.is_loopback(), "arc-labs serving");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_labs_core::Config;
    use std::net::Ipv4Addr;

    fn cfg(host: IpAddr, token: Option<String>) -> ServerConfig {
        ServerConfig {
            host,
            port: 0,
            ui_dir: PathBuf::from("ui/dist"),
            token,
        }
    }

    #[test]
    fn loopback_and_remote_binds_earn_different_capabilities() {
        let local = cfg(IpAddr::V4(Ipv4Addr::LOCALHOST), None);
        assert!(local.is_loopback());
        assert!(local.capabilities().browse_filesystem);
        assert!(local.capabilities().expose_paths);

        let remote = cfg(IpAddr::V4(Ipv4Addr::UNSPECIFIED), Some("t".into()));
        assert!(!remote.is_loopback());
        // A remote client can neither list the filesystem nor learn where the
        // vault lives on the host.
        assert!(!remote.capabilities().browse_filesystem);
        assert!(!remote.capabilities().expose_paths);
    }

    #[test]
    fn tokens_are_long_random_and_distinct() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 48);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn constant_time_compare_is_still_correct() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    #[tokio::test]
    async fn a_traversal_in_the_query_string_is_rejected_before_any_handler_runs() {
        // The end-to-end version of the VaultPath deserialisation guarantee:
        // this must fail at extraction, not reach the filesystem.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("ok.md"), b"# ok\n").unwrap();

        let api = Arc::new(Api::new(
            Config::default(),
            None,
            Capabilities::local_server(),
        ));
        api.open_vault(tmp.path()).unwrap();

        let app = router(api, &cfg(IpAddr::V4(Ipv4Addr::LOCALHOST), None));
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });

        let get = |q: &str| {
            let url = format!("http://{addr}/api/note?path={q}");
            async move {
                let out = tokio::process::Command::new("curl")
                    .args(["-s", "-o", "/dev/null", "-w", "%{http_code}", &url])
                    .output()
                    .await
                    .ok()?;
                String::from_utf8(out.stdout).ok()
            }
        };

        if let Some(code) = get("ok.md").await {
            if code == "000" {
                eprintln!("skipping: curl unavailable");
                return;
            }
            assert_eq!(code, "200", "a legitimate note should be served");
        }
        for attack in [
            "..%2F..%2Fetc%2Fpasswd",
            "%2Fetc%2Fshadow",
            "C%3A%5CWindows%5Cwin.ini",
        ] {
            if let Some(code) = get(attack).await {
                assert_eq!(code, "400", "traversal {attack} was not rejected");
            }
        }
    }
}
