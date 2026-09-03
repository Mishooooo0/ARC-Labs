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
/// The token for a non-loopback bind: `ARC_LABS_TOKEN` if set, else a fresh one.
///
/// The environment variable exists because a generated token changes on every
/// restart, and an always-on node restarts — on a host reboot, on a `docker
/// restart`, on an image update. Every one of those silently invalidates the URL
/// anyone had saved, and the symptom is an app that will not load rather than a
/// message saying the token expired. A server you actually deploy wants a token
/// that outlives its container.
///
/// Refuses a short one. A token is the only thing between the port and the
/// vault, and "it was just for testing" is how a two-character token ends up on
/// a machine that is reachable.
pub fn generate_token() -> String {
    if let Ok(fixed) = std::env::var("ARC_LABS_TOKEN") {
        let fixed = fixed.trim().to_string();
        if fixed.len() >= 16 {
            return fixed;
        }
        tracing::warn!(
            "ARC_LABS_TOKEN is shorter than 16 characters and was ignored;              generating one instead"
        );
    }
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

    // Vite content-hashes every asset filename, so an asset URL never changes
    // meaning and can be cached for a year.
    let assets = Router::new()
        .fallback_service(ServeDir::new(cfg.ui_dir.join("assets")))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        ));

    // SPA fallback: unknown paths serve index.html so client-side routing works,
    // while /api/* is matched first and never falls through to it.
    //
    // `no-cache` on this half is load-bearing, and it is the opposite of the
    // rule above. `index.html` has a fixed URL and its body names which hashed
    // bundle to load, so with no cache directive a browser applies heuristic
    // freshness and keeps serving the previous UI after the server is upgraded
    // — which looks exactly like a feature that was never shipped rather than
    // like a caching bug. Observed during this build: a rebuilt UI kept loading
    // the old bundle until a hard refresh. `no-cache` means revalidate, not
    // "do not store": the ETag above still makes that a 304 on the common path.
    let static_files = Router::new()
        .fallback_service(ServeDir::new(&cfg.ui_dir).fallback(ServeFile::new(index)))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ));

    let api_routes = Router::new()
        .route("/status", get(status))
        .route("/tree", get(tree))
        .route("/note", get(note))
        .route("/note/edit", get(note_for_edit))
        .route("/note/save", post(save_note))
        .route("/note/create", post(create_note))
        .route("/folder/create", post(create_folder))
        .route("/canvas/create", post(create_canvas))
        .route("/templates", get(templates))
        .route("/template/save", post(save_template))
        .route("/template/draft", post(draft_template))
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
        .route("/config", get(get_config).post(set_config))
        .route("/browse", get(browse))
        .route("/vault/open", post(open_vault))
        // ── The hub half of sync ────────────────────────────────────────────
        //
        // Present only when this instance is a hub, so a vault someone chose to
        // keep on disk does not quietly answer sync requests just because it
        // happens to be serving a browser. See `hub_only`.
        .route("/hub/manifest", get(hub_manifest))
        .route("/hub/file", get(hub_read).post(hub_write))
        .route("/hub/delete", post(hub_delete))
        .route("/hub/objects/missing", post(hub_missing_objects))
        .route("/hub/object", get(hub_read_object).post(hub_write_object))
        .route("/hub/ledger/keys", get(hub_ledger_keys))
        .route("/hub/ledger", get(hub_read_ledger).post(hub_merge_ledger))
        // Anything else under /api is a route this build does not have.
        //
        // Without this it falls through to the SPA and a client asking an older
        // server for an endpoint it lacks gets `200 OK` and a page of HTML,
        // which parses as neither an answer nor an error — the worst possible
        // reply to "do you support this?". The versioning contract says a
        // client degrades gracefully against an older server; it can only do
        // that if the server says no in a language the client speaks.
        //
        // Inside the nested router and before the auth layer, so it is behind
        // the token too: an unauthenticated caller cannot map which endpoints
        // exist by watching which ones 404.
        .fallback(unknown_endpoint)
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
        .nest("/assets", assets)
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

async fn create_folder(State(s): State<Arc<AppState>>, Json(b): Json<CreateBody>) -> WebResult<()> {
    Ok(Json(s.api.create_folder(&b.path)?))
}

async fn create_canvas(State(s): State<Arc<AppState>>, Json(b): Json<CreateBody>) -> WebResult<()> {
    Ok(Json(s.api.create_canvas(&b.path)?))
}

async fn templates(State(s): State<Arc<AppState>>) -> WebResult<Vec<arc_labs_api::Template>> {
    Ok(Json(s.api.templates()?))
}

#[derive(Deserialize)]
struct SaveTemplateBody {
    name: String,
    body: String,
    /// Absent from an older client, which means "not drafted" — the honest
    /// reading, since a client that predates drafting cannot have drafted it.
    #[serde(default)]
    drafted: bool,
}

async fn save_template(
    State(s): State<Arc<AppState>>,
    Json(b): Json<SaveTemplateBody>,
) -> WebResult<arc_labs_api::Template> {
    Ok(Json(s.api.save_template(&b.name, &b.body, b.drafted)?))
}

#[derive(Deserialize)]
struct DraftBody {
    description: String,
}

/// Drafting waits on a model, so it goes to a blocking thread rather than
/// occupying an async worker for the length of a generation.
async fn draft_template(
    State(s): State<Arc<AppState>>,
    Json(b): Json<DraftBody>,
) -> WebResult<String> {
    let api = s.api.clone();
    let text = tokio::task::spawn_blocking(move || api.draft_template(&b.description))
        .await
        .map_err(|e| ApiError::new(ErrorCode::Io, e.to_string()))??;
    Ok(Json(text))
}

async fn get_config(State(s): State<Arc<AppState>>) -> Json<arc_labs_api::Settings> {
    Json(s.api.settings())
}

async fn set_config(
    State(s): State<Arc<AppState>>,
    Json(body): Json<arc_labs_api::Settings>,
) -> WebResult<arc_labs_api::Settings> {
    Ok(Json(s.api.update_settings(&body)?))
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
    /// Present when the note starts from a template. The two are exclusive:
    /// a template supplies the text, so sending both would be ambiguous about
    /// which one wins.
    #[serde(default)]
    template: Option<VaultPath>,
}

/// Create a note. On a blocking thread with the other write paths.
async fn create_note(
    State(s): State<Arc<AppState>>,
    Json(body): Json<CreateBody>,
) -> WebResult<arc_labs_api::NoteView> {
    let api = s.api.clone();
    let view = tokio::task::spawn_blocking(move || match &body.template {
        Some(t) => api.create_note_from_template(&body.path, t),
        None => api.create_note(&body.path, &body.text),
    })
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

/// Every `/api` path this build does not serve.
async fn unknown_endpoint() -> WebError {
    WebError(ApiError::new(
        ErrorCode::NoteNotFound,
        "this server does not have that endpoint; ask /api/version for what it supports",
    ))
}

// ── Hub ─────────────────────────────────────────────────────────────────────

/// Refuse unless this instance was told it is a vault server.
///
/// "On disk only" has to mean it. Without this, any ARC-LABS serving a browser
/// would also accept another machine's notes into its vault the moment someone
/// pointed a client at it — which is a surprising thing for a program to do on
/// the strength of a default.
fn hub_only(s: &AppState) -> Result<(), ApiError> {
    if s.api.config().resolved_role() == arc_labs_core::Role::Hub {
        return Ok(());
    }
    Err(ApiError::new(
        ErrorCode::NotPermitted,
        "this ARC-LABS is not a vault server. Set role = \"hub\" in [sync], or ARC_LABS_ROLE=hub, on the instance other machines sync to.",
    ))
}

#[derive(Deserialize)]
struct GenQuery {
    path: VaultPath,
    /// The generation the caller planned against. Absent is allowed; see
    /// `Api::check_generation`.
    #[serde(default)]
    generation: Option<String>,
}

#[derive(Deserialize)]
struct HashQuery {
    hash: String,
}

#[derive(Deserialize)]
struct KeyQuery {
    key: String,
}

async fn hub_manifest(State(s): State<Arc<AppState>>) -> WebResult<arc_labs_api::HubManifest> {
    hub_only(&s)?;
    Ok(Json(s.api.hub_manifest()?))
}

/// Raw bytes, not JSON. A vault holds images and PDFs, and base64 in a JSON
/// envelope would inflate every transfer by a third to carry them.
async fn hub_read(
    State(s): State<Arc<AppState>>,
    Query(q): Query<GenQuery>,
) -> Result<Response, WebError> {
    hub_only(&s)?;
    let bytes = s.api.hub_read(&q.path)?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}

async fn hub_write(
    State(s): State<Arc<AppState>>,
    Query(q): Query<GenQuery>,
    body: axum::body::Bytes,
) -> WebResult<()> {
    hub_only(&s)?;
    let api = Arc::clone(&s.api);
    let generation = q.generation.clone();
    tokio::task::spawn_blocking(move || api.hub_write(&q.path, &body, generation.as_deref()))
        .await
        .map_err(|e| ApiError::new(ErrorCode::Io, e.to_string()))??;
    Ok(Json(()))
}

async fn hub_delete(State(s): State<Arc<AppState>>, Json(q): Json<GenQuery>) -> WebResult<()> {
    hub_only(&s)?;
    let api = Arc::clone(&s.api);
    tokio::task::spawn_blocking(move || api.hub_delete(&q.path, q.generation.as_deref()))
        .await
        .map_err(|e| ApiError::new(ErrorCode::Io, e.to_string()))??;
    Ok(Json(()))
}

#[derive(Deserialize)]
struct HashesBody {
    hashes: Vec<String>,
}

async fn hub_missing_objects(
    State(s): State<Arc<AppState>>,
    Json(b): Json<HashesBody>,
) -> WebResult<Vec<String>> {
    hub_only(&s)?;
    Ok(Json(s.api.hub_missing_objects(&b.hashes)?))
}

async fn hub_read_object(
    State(s): State<Arc<AppState>>,
    Query(q): Query<HashQuery>,
) -> Result<Response, WebError> {
    hub_only(&s)?;
    Ok(s.api.hub_read_object(&q.hash)?.into_response())
}

async fn hub_write_object(State(s): State<Arc<AppState>>, body: String) -> WebResult<String> {
    hub_only(&s)?;
    Ok(Json(s.api.hub_write_object(&body)?))
}

async fn hub_ledger_keys(State(s): State<Arc<AppState>>) -> WebResult<Vec<String>> {
    hub_only(&s)?;
    Ok(Json(s.api.hub_ledger_keys()?))
}

async fn hub_read_ledger(
    State(s): State<Arc<AppState>>,
    Query(q): Query<KeyQuery>,
) -> Result<Response, WebError> {
    hub_only(&s)?;
    Ok(s.api.hub_read_ledger(&q.key)?.into_response())
}

async fn hub_merge_ledger(
    State(s): State<Arc<AppState>>,
    Query(q): Query<KeyQuery>,
    body: String,
) -> WebResult<usize> {
    hub_only(&s)?;
    Ok(Json(s.api.hub_merge_ledger(&q.key, &body)?))
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

    /// Both halves of token generation in one test, deliberately.
    ///
    /// `ARC_LABS_TOKEN` is process-global and Rust runs tests in parallel
    /// threads, so a separate test that set it raced this one and made it fail
    /// intermittently. One test, one sequence, no shared mutable state between
    /// tests to reason about.
    #[test]
    fn tokens_are_generated_well_and_can_be_pinned() {
        std::env::remove_var("ARC_LABS_TOKEN");

        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 48);
        assert_ne!(a, b, "two generated tokens must differ");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));

        // A generated token changes on every restart, which silently
        // invalidates every saved URL. An always-on node needs one that
        // outlives its container.
        let fixed = "0123456789abcdef0123";
        std::env::set_var("ARC_LABS_TOKEN", fixed);
        assert_eq!(generate_token(), fixed);

        // Surrounding whitespace is a copy-paste artefact, not part of a secret.
        std::env::set_var("ARC_LABS_TOKEN", "  0123456789abcdef0123  ");
        assert_eq!(generate_token(), fixed);

        // Too short to be the only thing between a port and a vault.
        std::env::set_var("ARC_LABS_TOKEN", "hunter2");
        let generated = generate_token();
        assert_ne!(generated, "hunter2");
        assert_eq!(generated.len(), 48);

        std::env::remove_var("ARC_LABS_TOKEN");
        assert_eq!(generate_token().len(), 48);
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

    /// Two answers a client depends on being able to tell apart.
    ///
    /// "On disk only" has to mean it, so a vault that was not told it is a hub
    /// refuses sync outright. And a route this build does not have must say so
    /// in JSON — it used to fall through to the SPA and return 200 with a page
    /// of HTML, which is neither an answer nor an error and is the worst
    /// possible reply to "do you support this?".
    #[tokio::test]
    async fn a_standalone_vault_refuses_sync_and_unknown_routes_say_so() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.md"), b"# a\n").unwrap();

        let api = Arc::new(Api::new(
            Config::default(),
            None,
            Capabilities::local_server(),
        ));
        api.open_vault(tmp.path()).unwrap();

        // A real UI directory, so the last assertion is testing the SPA
        // fallback rather than a missing fixture.
        let ui = tempfile::tempdir().unwrap();
        std::fs::write(ui.path().join("index.html"), b"<!doctype html>\n").unwrap();
        let mut c = cfg(IpAddr::V4(Ipv4Addr::LOCALHOST), None);
        c.ui_dir = ui.path().to_path_buf();

        let app = router(api, &c);
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });

        let code = |path: String| async move {
            let url = format!("http://{addr}{path}");
            let out = tokio::process::Command::new("curl")
                .args(["-s", "-o", "/dev/null", "-w", "%{http_code}", &url])
                .output()
                .await
                .ok()?;
            String::from_utf8(out.stdout).ok()
        };

        let Some(first) = code("/api/version".into()).await else {
            return;
        };
        if first == "000" {
            eprintln!("skipping: curl unavailable");
            return;
        }
        assert_eq!(first, "200", "the handshake must still resolve");

        assert_eq!(
            code("/api/v1/hub/manifest".into()).await.unwrap(),
            "403",
            "a vault nobody made a hub must not answer sync requests"
        );
        assert_eq!(
            code("/api/v1/no-such-endpoint".into()).await.unwrap(),
            "404",
            "an unknown API route must not return the app shell"
        );
        // The SPA fallback still works for everything that is not /api.
        assert_eq!(code("/some/client/route".into()).await.unwrap(), "200");
    }

    /// The upgrade path, as a test.
    ///
    /// A browser that keeps its cached `index.html` keeps loading the bundle
    /// that copy names, so a server upgraded in place serves a UI nobody
    /// shipped any more. It looks like a missing feature, not a caching bug,
    /// which is what makes it worth a test rather than a comment.
    #[tokio::test]
    async fn the_shell_revalidates_and_the_hashed_assets_do_not() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("index.html"),
            b"<!doctype html>
",
        )
        .unwrap();
        std::fs::create_dir(tmp.path().join("assets")).unwrap();
        std::fs::write(
            tmp.path().join("assets/index-abc123.js"),
            b"//
",
        )
        .unwrap();

        let vault = tempfile::tempdir().unwrap();
        let api = Arc::new(Api::new(
            Config::default(),
            None,
            Capabilities::local_server(),
        ));
        api.open_vault(vault.path()).unwrap();

        let mut c = cfg(IpAddr::V4(Ipv4Addr::LOCALHOST), None);
        c.ui_dir = tmp.path().to_path_buf();
        let app = router(api, &c);
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });

        let headers = |path: &str| {
            let url = format!("http://{addr}{path}");
            async move {
                let out = tokio::process::Command::new("curl")
                    .args(["-s", "-D", "-", "-o", "/dev/null", &url])
                    .output()
                    .await
                    .ok()?;
                Some(String::from_utf8_lossy(&out.stdout).to_lowercase())
            }
        };

        let Some(shell) = headers("/").await else {
            return;
        };
        if shell.is_empty() {
            eprintln!("skipping: curl unavailable");
            return;
        }
        assert!(
            shell.contains("cache-control: no-cache"),
            "index.html must revalidate, got: {shell}"
        );

        let asset = headers("/assets/index-abc123.js").await.unwrap();
        assert!(
            asset.contains("immutable"),
            "a content-hashed asset should be cacheable for a year, got: {asset}"
        );
        // The SPA fallback is the same document as `/`, so it inherits the
        // same rule — a deep link after an upgrade must not be the one route
        // that hands back yesterday's app.
        let deep = headers("/some/client/route").await.unwrap();
        assert!(deep.contains("cache-control: no-cache"), "got: {deep}");
    }
}
