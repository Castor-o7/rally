//! The Rally service (RAL-13): the single writer, reached over TLS with a token for each machine.
//!
//! Every route goes through [`guard`]. The service sets each record's credential and time itself,
//! so a client can only declare a name. See `docs/service.md` for what that does and doesn't prove.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{ConnectInfo, DefaultBodyLimit, Path as UrlPath, Query, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::access::{Identity, StateDir};
use crate::ledger::Ledger;
use crate::record::{Kind, Record, Source};
use crate::store::{Head, Location, SpaceLimits, Store, StoreError};
use crate::views;

/// The largest request body accepted.
pub const MAX_BODY_BYTES: usize = 16 * 1024;

/// Limits on requests from each credential, and on the ledger. Writes have their own, much lower
/// limit, because each one stays on disk for good.
#[derive(Clone, Copy, Debug)]
pub struct RateLimit {
    pub burst: f64,
    pub per_second: f64,
    pub write_burst: f64,
    pub write_per_second: f64,
    pub max_ledger_bytes: u64,
    /// Rally shares its host with Muster's daemon, so it stops writing well before the disk fills.
    pub min_free_bytes: u64,
    /// Connections open at once, from anyone, and from any one address.
    pub max_connections: usize,
    pub max_connections_per_address: usize,
    /// How long a connection may last, from its first byte, whether or not it has authenticated.
    pub connection_seconds: f64,
    /// Requests with a missing or wrong token that one address can make at once, and regains per
    /// second. Past that, its requests are refused before their token is looked at.
    pub failed_token_burst: f64,
    pub failed_token_per_second: f64,
}

impl Default for RateLimit {
    fn default() -> Self {
        Self {
            burst: 120.0,
            per_second: 5.0,
            write_burst: 30.0,
            write_per_second: 0.2,
            // The size of the ledger's table in Postgres, with its indexes. The service holds the
            // whole ledger in memory, and a check replays it, on a host shared with Muster: see
            // `docs/service.md` for what this costs. Raise this only after RAL-35.
            max_ledger_bytes: 64 * 1024 * 1024,
            min_free_bytes: 5 * 1024 * 1024 * 1024,
            max_connections: 64,
            max_connections_per_address: 16,
            // Longer than a write can wait on Postgres (its statement and lock timeouts), so that a
            // slow write still gets its answer.
            connection_seconds: 60.0,
            failed_token_burst: 20.0,
            failed_token_per_second: 0.2,
        }
    }
}

struct Service {
    // No lock of its own: appends queue on the writer's lock inside the store, and reads of
    // myko's copy need none, so a write stuck on Postgres doesn't hold up reads.
    store: Store,
    state_dir: StateDir,
    limit: RateLimit,
    buckets: Mutex<HashMap<(String, bool), (f64, Instant)>>,
    /// Failed token attempts, by address.
    failures: Mutex<HashMap<IpAddr, (f64, Instant)>>,
}

/// How long a request's headers may take to arrive. A connection's own deadline is shorter if
/// it's set lower.
const HEADER_SECONDS: f64 = 10.0;

/// Runs the service on `listener` until the process ends.
pub async fn serve(
    listener: TcpListener,
    location: &Location,
    state_dir: StateDir,
    limit: RateLimit,
) -> Result<(), Box<dyn std::error::Error>> {
    // Both ring and any other provider could be linked; the choice has to be explicit.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (cert, key, _) = state_dir.certificate()?;
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
    // Opening replays the ledger through myko, which never listens: this service is the only way
    // in. It also takes the writer lock, so a second writer can't fork the chain.
    let store = tokio::task::block_in_place(|| Store::open(location))?;
    store.limit_space(SpaceLimits {
        max_ledger_bytes: limit.max_ledger_bytes,
        min_free_bytes: limit.min_free_bytes,
    });
    let app = router(store, state_dir, limit);
    listener.set_nonblocking(true)?;
    let lifetime = Duration::from_secs_f64(limit.connection_seconds);
    let acceptor = Limited {
        inner: axum_server::tls_rustls::RustlsAcceptor::new(tls)
            .handshake_timeout(lifetime.min(Duration::from_secs(10))),
        slots: Arc::new(Slots {
            max: limit.max_connections,
            max_per_address: limit.max_connections_per_address,
            open: Mutex::new((0, HashMap::new())),
        }),
        lifetime,
    };
    let mut server = axum_server::from_tcp(listener)?
        .acceptor(acceptor)
        .http1_only();
    server
        .http_builder()
        .http1()
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(lifetime.min(Duration::from_secs_f64(HEADER_SECONDS)));
    server
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await?;
    Ok(())
}

fn router(store: Store, state_dir: StateDir, limit: RateLimit) -> Router {
    let service = Arc::new(Service {
        store,
        state_dir,
        limit,
        buckets: Mutex::new(HashMap::new()),
        failures: Mutex::new(HashMap::new()),
    });
    Router::new()
        .route("/v1/records", post(write_record))
        .route("/v1/stories", get(list_stories))
        .route("/v1/stories/{id}", get(one_story))
        .route("/v1/teams", get(list_teams))
        .route("/v1/teams/{name}", get(one_team))
        .route("/v1/check", get(check))
        .route("/v1/whoami", get(whoami))
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
    let address = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| address.ip());
    // Before the token is looked at, so that once an address has failed too often, a guess can't
    // tell a right token from a wrong one.
    if let Some(address) = address
        && !service.may_try_token(address)
    {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            "too many failed token attempts from this address",
        );
    }
    let Some(token) = bearer_token(request.headers()) else {
        service.token_failed(address);
        return failure(StatusCode::UNAUTHORIZED, "a token is needed");
    };
    let identity = match service.state_dir.identify(&token) {
        Ok(Some(identity)) => identity,
        Ok(None) => {
            service.token_failed(address);
            return failure(StatusCode::UNAUTHORIZED, "that token isn't valid");
        }
        Err(error) => {
            eprintln!("rally: can't check a token: {error}");
            return failure(StatusCode::INTERNAL_SERVER_ERROR, "can't check the token");
        }
    };
    if !service.allow(&identity.credential, false) {
        return failure(StatusCode::TOO_MANY_REQUESTS, "too many requests");
    }
    // Checked here rather than in each handler, so that a future route can't forget.
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        if identity.read_only {
            return failure(StatusCode::FORBIDDEN, "this credential is read-only");
        }
        if !service.allow(&identity.credential, true) {
            return failure(StatusCode::TOO_MANY_REQUESTS, "too many writes");
        }
    }
    request.extensions_mut().insert(identity);
    next.run(request).await
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    value.strip_prefix("Bearer ").map(str::to_owned)
}

impl Service {
    /// A token bucket per credential, with a separate one for its writes.
    fn allow(&self, credential: &str, write: bool) -> bool {
        let (burst, per_second) = if write {
            (self.limit.write_burst, self.limit.write_per_second)
        } else {
            (self.limit.burst, self.limit.per_second)
        };
        let now = Instant::now();
        let mut buckets = self.buckets.lock().expect("no panics hold this lock");
        let (level, last) = buckets
            .entry((credential.to_owned(), write))
            .or_insert((burst, now));
        let refill = now.duration_since(*last).as_secs_f64() * per_second;
        *level = (*level + refill).min(burst);
        *last = now;
        if *level >= 1.0 {
            *level -= 1.0;
            true
        } else {
            false
        }
    }

    /// Whether an address may present a token: it hasn't used up its failed attempts.
    fn may_try_token(&self, address: IpAddr) -> bool {
        let mut failures = self.failures.lock().expect("no panics hold this lock");
        match failures.get_mut(&address) {
            None => true,
            Some(bucket) => {
                refill(
                    bucket,
                    self.limit.failed_token_burst,
                    self.limit.failed_token_per_second,
                );
                bucket.0 >= 1.0
            }
        }
    }

    /// Spends one of an address's failed attempts.
    fn token_failed(&self, address: Option<IpAddr>) {
        let Some(address) = address else { return };
        let (burst, per_second) = (
            self.limit.failed_token_burst,
            self.limit.failed_token_per_second,
        );
        let mut failures = self.failures.lock().expect("no panics hold this lock");
        // Forget addresses that have recovered in full, so that many addresses can't grow this.
        if failures.len() >= 4096 {
            failures.retain(|_, bucket| {
                refill(bucket, burst, per_second);
                bucket.0 < burst
            });
        }
        let bucket = failures.entry(address).or_insert((burst, Instant::now()));
        refill(bucket, burst, per_second);
        bucket.0 = (bucket.0 - 1.0).max(0.0);
    }

    /// Runs `work` on the store. Postgres calls block, so they leave this worker free for others.
    fn with_store<T>(&self, work: impl FnOnce(&Store) -> T) -> T {
        tokio::task::block_in_place(|| work(&self.store))
    }

    /// Runs `read` on the ledger's state. The current state is the one the service keeps, read
    /// without a copy; a past state is replayed.
    fn with_ledger<T>(
        &self,
        as_of: Option<u64>,
        read: impl FnOnce(&Ledger) -> T,
    ) -> Result<T, StoreError> {
        self.with_store(|store| store.with_ledger(as_of, read))
    }
}

/// Tops up a token bucket for the time since it was last used.
fn refill(bucket: &mut (f64, Instant), burst: f64, per_second: f64) {
    let now = Instant::now();
    bucket.0 = (bucket.0 + now.duration_since(bucket.1).as_secs_f64() * per_second).min(burst);
    bucket.1 = now;
}

/// Places for open connections, overall and by address.
struct Slots {
    max: usize,
    max_per_address: usize,
    open: Mutex<(usize, HashMap<IpAddr, usize>)>,
}

/// One open connection's place, given back when the connection closes.
struct Slot {
    slots: Arc<Slots>,
    address: IpAddr,
}

impl Slots {
    fn take(self: &Arc<Self>, address: IpAddr) -> Option<Slot> {
        let mut open = self.open.lock().expect("no panics hold this lock");
        let (total, by_address) = &mut *open;
        let mine = by_address.entry(address).or_default();
        if *total >= self.max || *mine >= self.max_per_address {
            if *mine == 0 {
                by_address.remove(&address);
            }
            return None;
        }
        *mine += 1;
        *total += 1;
        Some(Slot {
            slots: self.clone(),
            address,
        })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut open = self
            .slots
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (total, by_address) = &mut *open;
        *total -= 1;
        if let Some(mine) = by_address.get_mut(&self.address) {
            *mine -= 1;
            if *mine == 0 {
                by_address.remove(&self.address);
            }
        }
    }
}

/// Takes a place for each connection before its TLS handshake, closing it at once if there's
/// none, and gives it a deadline that covers the handshake, the headers, the body and the reply.
#[derive(Clone)]
struct Limited<A> {
    inner: A,
    slots: Arc<Slots>,
    lifetime: Duration,
}

type Accepting<T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send>>;

impl<A, S> axum_server::accept::Accept<tokio::net::TcpStream, S> for Limited<A>
where
    A: axum_server::accept::Accept<Bounded, S>,
    A::Future: Send + 'static,
    A::Stream: Send + 'static,
    A::Service: Send + 'static,
{
    type Stream = A::Stream;
    type Service = A::Service;
    type Future = Accepting<(A::Stream, A::Service)>;

    fn accept(&self, stream: tokio::net::TcpStream, service: S) -> Self::Future {
        let slot = stream
            .peer_addr()
            .ok()
            .and_then(|peer| self.slots.take(peer.ip()));
        let Some(slot) = slot else {
            // Dropping the stream closes it.
            return Box::pin(std::future::ready(Err(io::Error::other(
                "too many connections",
            ))));
        };
        let bounded = Bounded {
            stream,
            deadline: Box::pin(tokio::time::sleep(self.lifetime)),
            _slot: slot,
        };
        Box::pin(self.inner.accept(bounded, service))
    }
}

/// A connection that fails every read and write once its deadline passes, which ends it, and
/// that holds its place until then.
struct Bounded {
    stream: tokio::net::TcpStream,
    deadline: Pin<Box<tokio::time::Sleep>>,
    _slot: Slot,
}

impl Bounded {
    fn expired(&mut self, cx: &mut Context<'_>) -> bool {
        self.deadline.as_mut().poll(cx).is_ready()
    }
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "the connection's time is up")
}

impl AsyncRead for Bounded {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.expired(cx) {
            return Poll::Ready(Err(timed_out()));
        }
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Bounded {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.expired(cx) {
            return Poll::Ready(Err(timed_out()));
        }
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.expired(cx) {
            return Poll::Ready(Err(timed_out()));
        }
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
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
    service.with_store(|store| match store.append(&record) {
        Ok(seq) => Json(json!({"recorded": seq})).into_response(),
        Err(StoreError::Refused(error)) => failure(StatusCode::CONFLICT, error.to_string()),
        Err(error @ StoreError::Database(_)) => {
            eprintln!("rally: {error}");
            failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "the ledger's database can't be reached, so nothing was recorded",
            )
        }
        Err(StoreError::Full(reason)) => failure(StatusCode::INSUFFICIENT_STORAGE, reason),
        Err(error @ StoreError::SpaceUnchecked(_)) => {
            failure(StatusCode::SERVICE_UNAVAILABLE, error.to_string())
        }
        Err(error @ StoreError::Unloaded { .. }) => {
            eprintln!("rally: {error}");
            failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                "it was recorded, but the service couldn't load it; it must be restarted \
                     before writing again",
            )
        }
        Err(error @ StoreError::Unknown(_)) => {
            eprintln!("rally: {error}");
            failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "the outcome is unknown; it may have been recorded, so check before retrying",
            )
        }
        Err(error) => internal(&error),
    })
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
    match service.with_ledger(as_of.as_of, read) {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(message)) => failure(StatusCode::NOT_FOUND, message),
        Err(error) => internal(&error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expected {
    seq: Option<u64>,
    hash: Option<String>,
}

/// Checks the ledger. A failed check is still a successful reply: the problems are the answer.
async fn check(State(service): State<Arc<Service>>, Query(expected): Query<Expected>) -> Response {
    let expected = match (expected.seq, expected.hash) {
        (Some(seq), Some(hash)) => Some(Head { seq, hash }),
        (None, None) => None,
        _ => return failure(StatusCode::BAD_REQUEST, "seq and hash go together"),
    };
    match service.with_store(|store| store.verify(expected.as_ref())) {
        Ok(result) => Json(result).into_response(),
        Err(error) => internal(&error),
    }
}

/// Which credential this is, so that the viewer can warn if it could write.
async fn whoami(Extension(identity): Extension<Identity>) -> Response {
    Json(json!({"credential": identity.credential, "read_only": identity.read_only}))
        .into_response()
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
