//! The Rally service (RAL-13): the single writer, reached over TLS with a token for each machine.
//!
//! Every route goes through [`guard`]. The service sets each record's credential and time itself,
//! so a client can only declare a name. See `docs/service.md` for what that does and doesn't prove.

use std::collections::HashMap;
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::access::{Identity, StateDir};
use crate::ledger::Ledger;
use crate::record::{Kind, Record, Source};
use crate::store::{Store, StoreError};
use crate::views;

/// The largest request body accepted.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

/// Limits on requests from each credential.
#[derive(Clone, Copy, Debug)]
pub struct RateLimit {
    pub burst: f64,
    pub per_second: f64,
}

impl Default for RateLimit {
    fn default() -> Self {
        Self {
            burst: 120.0,
            per_second: 5.0,
        }
    }
}

struct Service {
    store: Mutex<Store>,
    state_dir: StateDir,
    limit: RateLimit,
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}

/// Runs the service on `listener` until the process ends.
pub async fn serve(
    listener: TcpListener,
    db: &Path,
    state_dir: StateDir,
    limit: RateLimit,
) -> Result<(), Box<dyn std::error::Error>> {
    // Both ring and any other provider could be linked; the choice has to be explicit.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (cert, key, _) = state_dir.certificate()?;
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
    let app = router(Store::open(db)?, state_dir, limit);
    listener.set_nonblocking(true)?;
    axum_server::from_tcp_rustls(listener, tls)?
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

fn router(store: Store, state_dir: StateDir, limit: RateLimit) -> Router {
    let service = Arc::new(Service {
        store: Mutex::new(store),
        state_dir,
        limit,
        buckets: Mutex::new(HashMap::new()),
    });
    Router::new()
        .route("/v1/records", post(write_record))
        .route("/v1/stories", get(list_stories))
        .route("/v1/stories/{id}", get(one_story))
        .route("/v1/teams", get(list_teams))
        .route("/v1/teams/{name}", get(one_team))
        .fallback(|| async { failure(StatusCode::NOT_FOUND, "no such route") })
        .method_not_allowed_fallback(|| async {
            failure(StatusCode::METHOD_NOT_ALLOWED, "method not allowed")
        })
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn_with_state(service.clone(), guard))
        .with_state(service)
}

fn failure(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"error": message.into()}))).into_response()
}

/// Refuses anything from a web page or without a valid token, and applies the rate limit. A browser
/// always sends `Origin` on the requests that matter here, and nothing legitimate does.
async fn guard(State(service): State<Arc<Service>>, mut request: Request, next: Next) -> Response {
    if request.headers().contains_key(header::ORIGIN) {
        return failure(StatusCode::FORBIDDEN, "web pages can't use this service");
    }
    let Some(token) = bearer_token(request.headers()) else {
        return failure(StatusCode::UNAUTHORIZED, "a token is needed");
    };
    let identity = match service.state_dir.identify(&token) {
        Ok(Some(identity)) => identity,
        Ok(None) => return failure(StatusCode::UNAUTHORIZED, "that token isn't valid"),
        Err(error) => {
            eprintln!("rally: can't check a token: {error}");
            return failure(StatusCode::INTERNAL_SERVER_ERROR, "can't check the token");
        }
    };
    if !service.allow(&identity.credential) {
        return failure(StatusCode::TOO_MANY_REQUESTS, "too many requests");
    }
    request.extensions_mut().insert(identity);
    next.run(request).await
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    value.strip_prefix("Bearer ").map(str::to_owned)
}

impl Service {
    /// A token bucket per credential.
    fn allow(&self, credential: &str) -> bool {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().expect("no panics hold this lock");
        let (level, last) = buckets
            .entry(credential.to_owned())
            .or_insert((self.limit.burst, now));
        let refill = now.duration_since(*last).as_secs_f64() * self.limit.per_second;
        *level = (*level + refill).min(self.limit.burst);
        *last = now;
        if *level >= 1.0 {
            *level -= 1.0;
            true
        } else {
            false
        }
    }

    fn ledger(&self, as_of: Option<u64>) -> Result<Ledger, StoreError> {
        let store = self.store.lock().expect("no panics hold this lock");
        store.ledger(as_of)
    }
}

/// Logs the detail on the host and tells the client only that something's wrong.
fn internal(error: &StoreError) -> Response {
    eprintln!("rally: {error}");
    failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        "the ledger can't be read",
    )
}

/// What a client sends to write. It has no credential or time, because the service sets those.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteRequest {
    kind: Kind,
    declared_name: String,
    body: Value,
}

async fn write_record(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    text: String,
) -> Response {
    if identity.read_only {
        return failure(StatusCode::FORBIDDEN, "this credential is read-only");
    }
    let request: WriteRequest = match serde_json::from_str(&text) {
        Ok(request) => request,
        Err(error) => return failure(StatusCode::BAD_REQUEST, error.to_string()),
    };
    if request.kind == Kind::Observed {
        return failure(
            StatusCode::FORBIDDEN,
            "only Rally itself records observed facts",
        );
    }
    let source = Source {
        credential: identity.credential,
        declared_name: Some(request.declared_name),
    };
    let at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64);
    let record = match Record::new(request.kind, source, at_ms, request.body) {
        Ok(record) => record,
        Err(error) => return failure(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let appended = service
        .store
        .lock()
        .expect("no panics hold this lock")
        .append(&record);
    match appended {
        Ok(seq) => Json(json!({"recorded": seq})).into_response(),
        Err(StoreError::Refused(error)) => failure(StatusCode::CONFLICT, error.to_string()),
        Err(error) => internal(&error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AsOf {
    as_of: Option<u64>,
}

fn view(
    service: &Service,
    as_of: AsOf,
    read: impl FnOnce(&Ledger) -> Result<Value, String>,
) -> Response {
    let ledger = match service.ledger(as_of.as_of) {
        Ok(ledger) => ledger,
        Err(error) => return internal(&error),
    };
    match read(&ledger) {
        Ok(value) => Json(value).into_response(),
        Err(message) => failure(StatusCode::NOT_FOUND, message),
    }
}

async fn list_stories(State(service): State<Arc<Service>>, Query(as_of): Query<AsOf>) -> Response {
    view(&service, as_of, |ledger| views::stories(ledger, None))
}

async fn one_story(
    State(service): State<Arc<Service>>,
    UrlPath(id): UrlPath<String>,
    Query(as_of): Query<AsOf>,
) -> Response {
    view(&service, as_of, |ledger| views::stories(ledger, Some(&id)))
}

async fn list_teams(State(service): State<Arc<Service>>, Query(as_of): Query<AsOf>) -> Response {
    view(&service, as_of, |ledger| views::teams(ledger, None))
}

async fn one_team(
    State(service): State<Arc<Service>>,
    UrlPath(name): UrlPath<String>,
    Query(as_of): Query<AsOf>,
) -> Response {
    view(&service, as_of, |ledger| views::teams(ledger, Some(&name)))
}
