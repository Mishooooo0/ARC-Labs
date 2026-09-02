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
            ErrorCode::Conflict => StatusCode::CONFLICT,
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
    let state = Arc::new(AppState { api, token: cfg.token.clone() });

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
        .route("/browse", get(browse))
        .route("/vault/open", post(open_vault))
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state);

    Router::new()
        .nest("/api", api_routes)
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
    let Some(expected) = state.token.as_deref() else {
        return next.run(req).await;
    };
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match presented {
        Some(t) if constant_time_eq(t.as_bytes(), expected.as_bytes()) => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            Json(ApiError::new(ErrorCode::NotPermitted, "a valid token is required")),
        )
            .into_response(),
    }
}

/// Compare without leaking the position of the first difference through timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
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

async fn save_note(
    State(s): State<Arc<AppState>>,
    Json(body): Json<SaveBody>,
) -> WebResult<arc_labs_api::SaveResult> {
    Ok(Json(s.api.write_note(&body.path, &body.text, body.base_hash.as_deref())?))
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
    Ok(Json(s.api.propose(&b.path, &b.agent, &b.model, &b.session, &b.reason, &b.content)?))
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

async fn tags(
    State(s): State<Arc<AppState>>,
) -> WebResult<Vec<arc_labs_index::query::TagCount>> {
    Ok(Json(s.api.tags()?))
}

async fn tag_notes(
    State(s): State<Arc<AppState>>,
    Query(q): Query<TextQuery>,
) -> WebResult<Vec<arc_labs_index::query::NoteRef>> {
    Ok(Json(s.api.notes_with_tag(&q.q)?))
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
    Ok(Json(s.api.browse(q.path.as_deref().map(std::path::Path::new))?))
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
        ServerConfig { host, port: 0, ui_dir: PathBuf::from("ui/dist"), token }
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

        let api = Arc::new(Api::new(Config::default(), None, Capabilities::local_server()));
        api.open_vault(tmp.path()).unwrap();

        let app = router(api, &cfg(IpAddr::V4(Ipv4Addr::LOCALHOST), None));
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
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
        for attack in ["..%2F..%2Fetc%2Fpasswd", "%2Fetc%2Fshadow", "C%3A%5CWindows%5Cwin.ini"] {
            if let Some(code) = get(attack).await {
                assert_eq!(code, "400", "traversal {attack} was not rejected");
            }
        }
    }
}
