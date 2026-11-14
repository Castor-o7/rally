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
    /// Checks and reads of a past state, which read or replay the whole ledger, that a credential
    /// can make at once, and regains per second. They also count as reads.
    pub heavy_burst: f64,
    pub heavy_per_second: f64,
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
            // The viewer checks every 30 seconds.
            heavy_burst: 10.0,
            heavy_per_second: 0.2,
        }
    }
}

struct Service {
    // No lock of its own: appends queue on the writer's lock inside the store, and reads of
    // myko's copy need none, so a write stuck on Postgres doesn't hold up reads.
    store: Store,
    /// The operator's key, under `serve --secure` only (RAL-51).
    operator_key: Option<crate::designation::OperatorKey>,
    /// The times agreed with the operator for shelving and away hours (RAL-56).
    operator_times: Option<crate::store::OperatorTimes>,
    /// Checks submitted evidence in the background, once started (RAL-5).
    evidence: std::sync::OnceLock<crate::evidence::Checker>,
    /// Reads roadmaps in open pull requests with the evidence token, when there is one (RAL-68).
    roadmaps: std::sync::OnceLock<Arc<dyn crate::evidence::Roadmaps + Send + Sync>>,
    /// The owners `roadmaps` may read for (RAL-68).
    github_owners: std::sync::OnceLock<Vec<String>>,
    /// The credentials that may open or close a rally (RAL-77), besides a designated watchman's.
    /// `None` until the operator sets `--rally-openers`: then any paired credential may, and
    /// every answer says so.
    rally_openers: Option<Vec<String>>,
    state_dir: StateDir,
    limit: RateLimit,
    buckets: Mutex<HashMap<(String, Bucket), (f64, Instant)>>,
    /// Reads of the kept state running at once, and checks or past-state replays running at once.
    /// Each holds memory while it runs, so these bound the service's memory, not just its time.
    reads: Arc<tokio::sync::Semaphore>,
    heavy: Arc<tokio::sync::Semaphore>,
    /// Appends running at once. They queue on the writer's lock anyway; this makes the queue
    /// wait here, holding no thread, so that one write stuck on a hung Postgres leaves the rest
    /// told the service is busy rather than each holding a blocking thread.
    writes: Arc<tokio::sync::Semaphore>,
    /// Failed token attempts, by address.
    failures: Mutex<HashMap<IpAddr, (f64, Instant)>>,
}

/// Which limit a request counts against, for its credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Bucket {
    Request,
    Write,
    Heavy,
}

/// What a request does with the store, which decides how many can run at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Work {
    /// A read of the kept state.
    Read,
    /// A check, or a read of a past state: each reads or replays the whole ledger.
    Heavy,
    /// An append.
    Write,
}

/// Reads of the kept state running at once.
const READS_AT_ONCE: usize = 8;

/// Checks and past-state replays running at once. Checks also run one at a time in the store.
const HEAVY_AT_ONCE: usize = 1;

/// Appends running at once: one, as the writer's lock allows.
const WRITES_AT_ONCE: usize = 1;

/// How long a request waits for its turn before it's told the service is busy.
const QUEUE_SECONDS: u64 = 10;

/// How long a request's headers may take to arrive. A connection's own deadline is shorter if
/// it's set lower.
const HEADER_SECONDS: f64 = 10.0;

/// Starts the evidence checker, which appends each outcome as Rally's own observed record, and
/// queues every submission that has no check yet, such as ones a restart cut short (RAL-5), in
/// review or not (RAL-149).
fn start_evidence_checks(service: &Arc<Service>, settings: crate::evidence::Settings) {
    let github = settings
        .token
        .map(|token| Arc::new(crate::evidence::GitHub::new(token)));
    if let Some(github) = &github {
        let _ = service.roadmaps.set(github.clone() as _);
    }
    let _ = service.github_owners.set(settings.owners.clone());
    let forge: Option<Arc<dyn crate::evidence::Forge + Send + Sync>> = github.map(|github| {
        Arc::new(TokenWatch {
            github,
            service: Arc::downgrade(service),
            filing: std::sync::Mutex::new(()),
        }) as _
    });
    let weak = Arc::downgrade(service);
    let record = Arc::new(
        move |job: &crate::evidence::Job, outcome: &crate::evidence::Outcome| {
            let Some(service) = weak.upgrade() else {
                return;
            };
            let at_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as i64);
            let source = Source {
                credential: crate::store::RALLY_ITSELF.to_owned(),
                declared_name: None,
            };
            let body = json!({"change": "evidence_check", "story": job.story,
                              "submit": job.submit, "outcome": outcome.clone().for_job(job)});
            let appended = Record::new(Kind::Observed, source, at_ms, body)
                .map_err(|e| e.to_string())
                .and_then(|record| service.store.append(&record).map_err(|e| e.to_string()));
            if let Err(error) = appended {
                eprintln!(
                    "rally: couldn't record the evidence check of record {}: {error}",
                    job.submit
                );
            }
        },
    );
    let checker =
        crate::evidence::Checker::start(forge, crate::evidence::WORKERS, settings.timing, record);
    let unchecked = service
        .store
        .with_ledger(None, crate::evidence::unchecked)
        .unwrap_or_default();
    for job in unchecked {
        checker.submit(job);
    }
    let _ = service.evidence.set(checker);
}

/// The service's GitHub client, watched for what only the operator can fix (RAL-146): each check's
/// first read shows whether GitHub took the token and when it expires, and Rally opens or closes
/// its own items in the operator's inbox from that.
struct TokenWatch {
    github: Arc<crate::evidence::GitHub>,
    service: std::sync::Weak<Service>,
    /// Held while the ledger is read and Rally's items written, so two checks can't both file one.
    filing: std::sync::Mutex<()>,
}

impl TokenWatch {
    fn saw(&self, seen: &crate::inbox::TokenSeen) {
        let Some(service) = self.service.upgrade() else {
            return;
        };
        let _filing = self.filing.lock().unwrap_or_else(|e| e.into_inner());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as i64);
        let bodies = service
            .store
            .with_ledger(None, |ledger| {
                crate::inbox::token_records(ledger, seen, now)
            })
            .unwrap_or_default();
        for body in bodies {
            let source = Source {
                credential: crate::store::RALLY_ITSELF.to_owned(),
                declared_name: None,
            };
            let appended = Record::new(Kind::Observed, source, now, body)
                .map_err(|e| e.to_string())
                .and_then(|record| service.store.append(&record).map_err(|e| e.to_string()));
            if let Err(error) = appended {
                eprintln!("rally: couldn't file the token's item in the operator's inbox: {error}");
            }
        }
    }
}

impl crate::evidence::Forge for TokenWatch {
    fn commit_message(&self, repo: &str, commit: &str) -> Result<Option<String>, String> {
        let read = self.github.commit_message(repo, commit);
        match &read {
            Ok(_) => self.saw(&crate::inbox::TokenSeen::Accepted {
                expires: self.github.token_expiry(),
            }),
            Err(error) if crate::inbox::is_credential_refusal(error) => {
                self.saw(&crate::inbox::TokenSeen::Refused(error.clone()));
            }
            Err(_) => {}
        }
        read
    }
    fn repo_visible(&self, repo: &str) -> Result<bool, String> {
        self.github.repo_visible(repo)
    }
    fn pull_titles(&self, repo: &str, commit: &str) -> Result<Vec<String>, String> {
        self.github.pull_titles(repo, commit)
    }
    fn ci(&self, repo: &str, commit: &str) -> Result<crate::evidence::CiRead, String> {
        self.github.ci(repo, commit)
    }
}

/// What the operator sets on the service host for who may do what.
#[derive(Default)]
pub struct Rules {
    /// The operator's key, under `serve --secure` only (RAL-51).
    pub operator_key: Option<crate::designation::OperatorKey>,
    /// The times agreed with the operator for shelving and away hours (RAL-56).
    pub operator_times: Option<crate::store::OperatorTimes>,
    /// The credentials that may open or close a rally (RAL-77); `None` lets any.
    pub rally_openers: Option<Vec<String>>,
}

/// Runs the service on `listener` until the process ends.
pub async fn serve(
    listener: TcpListener,
    location: &Location,
    state_dir: StateDir,
    limit: RateLimit,
    rules: Rules,
    evidence: Option<crate::evidence::Settings>,
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
    if let Some(key) = &rules.operator_key {
        store.accept_designations(key.clone());
    }
    if let Some(times) = rules.operator_times {
        store.accept_shelves(times.window_ms);
    }
    let app = router(store, state_dir, limit, rules, evidence);
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

fn router(
    store: Store,
    state_dir: StateDir,
    limit: RateLimit,
    rules: Rules,
    evidence: Option<crate::evidence::Settings>,
) -> Router {
    let Rules {
        operator_key,
        operator_times,
        rally_openers,
    } = rules;
    let service = Arc::new(Service {
        store,
        operator_key,
        operator_times,
        evidence: std::sync::OnceLock::new(),
        roadmaps: std::sync::OnceLock::new(),
        github_owners: std::sync::OnceLock::new(),
        rally_openers,
        state_dir,
        limit,
        buckets: Mutex::new(HashMap::new()),
        reads: Arc::new(tokio::sync::Semaphore::new(READS_AT_ONCE)),
        heavy: Arc::new(tokio::sync::Semaphore::new(HEAVY_AT_ONCE)),
        writes: Arc::new(tokio::sync::Semaphore::new(WRITES_AT_ONCE)),
        failures: Mutex::new(HashMap::new()),
    });
    if let Some(settings) = evidence {
        start_evidence_checks(&service, settings);
    }
    Router::new()
        .route("/v1/records", post(write_record))
        .route("/v1/stories", get(list_stories))
        .route("/v1/stories/{id}", get(one_story))
        .route("/v1/teams", get(list_teams))
        .route("/v1/rallies", get(list_rallies))
        .route("/v1/standings", get(list_standings))
        .route("/v1/blockers", get(open_blockers))
        .route("/v1/track-records", get(list_track_records))
        .route("/v1/track-records/{name}", get(one_track_record))
        .route("/v1/blockers/all", get(all_blockers))
        .route("/v1/inbox", get(open_inbox))
        .route("/v1/inbox/all", get(all_inbox))
        .route("/v1/corrections", get(corrections))
        .route("/v1/teams/{name}", get(one_team))
        .route("/v1/teams/{name}/record", get(team_record))
        .route("/v1/check", get(check))
        .route("/v1/whoami", get(whoami))
        .route("/v1/designation", get(designation))
        .route("/v1/verifications", get(verifications))
        .route("/v1/handover", get(handover))
        .route("/v1/open-ids", get(open_ids))
        .route("/v1/brief", get(brief))
        .route("/v1/waiting", get(waiting))
        .route("/v1/export", get(export))
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
    if !service.allow(&identity.credential, Bucket::Request) {
        return failure(StatusCode::TOO_MANY_REQUESTS, "too many requests");
    }
    // Checked here rather than in each handler, so that a future route can't forget.
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        if identity.read_only {
            return failure(StatusCode::FORBIDDEN, "this credential is read-only");
        }
        if !service.allow(&identity.credential, Bucket::Write) {
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
    /// A token bucket per credential, with separate ones for its writes and its heavy reads.
    fn allow(&self, credential: &str, bucket: Bucket) -> bool {
        let (burst, per_second) = match bucket {
            Bucket::Request => (self.limit.burst, self.limit.per_second),
            Bucket::Write => (self.limit.write_burst, self.limit.write_per_second),
            Bucket::Heavy => (self.limit.heavy_burst, self.limit.heavy_per_second),
        };
        let now = Instant::now();
        let mut buckets = self.buckets.lock().expect("no panics hold this lock");
        let (level, last) = buckets
            .entry((credential.to_owned(), bucket))
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

    /// Runs `job` on the store in Tokio's blocking pool, so that neither Postgres nor a replay
    /// holds up the service's workers, once there's room for this kind of work. If there isn't
    /// within [`QUEUE_SECONDS`], the reply says the service is busy.
    async fn run<T: Send + 'static>(
        self: &Arc<Self>,
        work: Work,
        job: impl FnOnce(&Store) -> T + Send + 'static,
    ) -> Result<T, NotRun> {
        let turn = match work {
            Work::Read => &self.reads,
            Work::Heavy => &self.heavy,
            Work::Write => &self.writes,
        };
        let wait = Duration::from_secs(QUEUE_SECONDS);
        // Held until the work ends, even if the request has gone by then.
        let permit = match tokio::time::timeout(wait, turn.clone().acquire_owned()).await {
            Ok(Ok(permit)) => permit,
            _ => return Err(NotRun::Busy),
        };
        let service = self.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            job(&service.store)
        })
        .await
        .map_err(|_| {
            eprintln!("rally: a request's work on the store panicked");
            NotRun::Failed
        })
    }
}

/// Why a request's work on the store didn't finish.
#[derive(Debug)]
enum NotRun {
    /// There was no room for it in time.
    Busy,
    /// It panicked.
    Failed,
}

impl IntoResponse for NotRun {
    fn into_response(self) -> Response {
        match self {
            Self::Busy => failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "the service is busy; try again shortly",
            ),
            Self::Failed => failure(StatusCode::INTERNAL_SERVER_ERROR, "the request failed"),
        }
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
    let mut body = request.body;
    // A shelve is measured by the service's window, not one the writer chose (RAL-56).
    if body["change"] == "shelve"
        && body.get("window_ms").is_none()
        && let (Some(times), Some(fields)) = (service.operator_times, body.as_object_mut())
    {
        fields.insert("window_ms".into(), json!(times.window_ms));
    }
    // A designation bound to a credential must name one that's paired and can write (RAL-53), so
    // the watchman's reads can carry it.
    if body["change"] == "designate"
        && let Some(credential) = body["credential"].as_str()
    {
        match service.state_dir.paired(credential) {
            Ok(Some(false)) => {}
            Ok(Some(true)) => {
                return failure(
                    StatusCode::CONFLICT,
                    format!("{credential} is read-only, so the watchman couldn't verify with it"),
                );
            }
            Ok(None) => {
                return failure(
                    StatusCode::CONFLICT,
                    format!("{credential} isn't paired: run rally add-machine for it first"),
                );
            }
            Err(error) => return failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        }
    }
    // Only the operator or the watchman opens or closes a rally (RAL-77), adds or removes one
    // of its sessions (RAL-110) or projects (RAL-135), or assigns a story (RAL-130), backfilled or
    // not.
    let applied = applied_change(&body);
    let rally_change = applied == "open_rally"
        || applied == "close_rally"
        || applied == "rally_session"
        || applied == "rally_project"
        || applied == "assign";
    if rally_change && let Err(refused) = may_open_rallies(&service, &identity.credential).await {
        return *refused;
    }
    let submitted = (body["change"] == "submit")
        .then(|| {
            Some((
                body["story"].as_str()?.to_owned(),
                body["evidence"].as_str()?.to_owned(),
            ))
        })
        .flatten();
    let source = Source {
        credential: identity.credential,
        declared_name: Some(request.declared_name),
    };
    let at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64);
    let record = match Record::new(request.kind, source, at_ms, body) {
        Ok(record) => record,
        Err(error) => return failure(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let times = service.operator_times;
    let appended = service
        .run(Work::Write, move |store| {
            let seq = store.append(&record)?;
            // A shelve that completes a run of unanswered shelves puts the operator away. It's
            // checked after every write, so a failure here is tried again on the next one. The
            // write stands either way.
            if let Some(times) = times
                && let Err(error) = store.enter_away_if_due(times)
            {
                eprintln!("rally: couldn't record the operator as away: {error}");
            }
            Ok(seq)
        })
        .await;
    let appended = match appended {
        Ok(appended) => appended,
        Err(not_run) => return not_run.into_response(),
    };
    match appended {
        Ok(seq) => {
            // A submission's evidence is checked in the background, by Rally itself (RAL-5).
            if let (Some(checker), Some((story, evidence))) = (service.evidence.get(), submitted)
                && !checker.submit(crate::evidence::Job {
                    story,
                    submit: seq,
                    evidence,
                    submitted_at_ms: at_ms,
                    late: None,
                })
            {
                eprintln!(
                    "rally: the evidence queue is full; record {seq} is checked after a restart"
                );
            }
            let mut answer = json!({"recorded": seq});
            if rally_change && service.rally_openers.is_none() {
                answer["warning"] = json!(ANYONE_MAY_OPEN);
            }
            Json(answer).into_response()
        }
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
        // Every other error comes before the commit: a broken chain, a head file off the chain,
        // a difference from Postgres, or another writer.
        Err(error) => {
            eprintln!("rally: {error}");
            failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                "nothing was recorded: the ledger needs attention on its host, so run `rally check` \
                 there",
            )
        }
    }
}

/// What a read may ask for: a past state, and which page of a list or history.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadQuery {
    as_of: Option<u64>,
    after: Option<String>,
    before: Option<u64>,
    /// For blockers: the instant to judge questions to the operator at, so that a reader paging
    /// through gets every page at the first page's `now_ms` (RAL-56).
    at_ms: Option<i64>,
    /// For corrections: the past record whose later corrections to list (RAL-28).
    since: Option<u64>,
    /// For waiting: every project's closes due, not only the open rally's (RAL-148).
    all: Option<bool>,
}

/// The longest `after` accepted, well past any story ID or team name.
const MAX_AFTER_CHARS: usize = 256;

async fn view(
    service: Arc<Service>,
    identity: Identity,
    query: ReadQuery,
    read: impl FnOnce(&Ledger, &views::Page) -> Result<Value, String> + Send + 'static,
) -> Response {
    if query
        .after
        .as_ref()
        .is_some_and(|after| after.chars().count() > MAX_AFTER_CHARS)
    {
        return failure(StatusCode::BAD_REQUEST, "after is too long");
    }
    // A past state is replayed, which costs about as much as a check.
    let work = if query.as_of.is_some() {
        if !service.allow(&identity.credential, Bucket::Heavy) {
            return failure(
                StatusCode::TOO_MANY_REQUESTS,
                "too many checks and past-state reads",
            );
        }
        Work::Heavy
    } else {
        Work::Read
    };
    let page = views::Page {
        after: query.after,
        before: query.before,
    };
    let as_of = query.as_of;
    let result = service
        .run(work, move |store| {
            // Serialized here, still under the kept state's read lock but bounded by the page.
            store.with_ledger(as_of, |ledger| {
                read(ledger, &page).map(|value| value.to_string())
            })
        })
        .await;
    match result {
        Ok(Ok(Ok(body))) => ([(header::CONTENT_TYPE, "application/json")], body).into_response(),
        Ok(Ok(Err(message))) => failure(StatusCode::NOT_FOUND, message),
        Ok(Err(error)) => internal(&error),
        Err(not_run) => not_run.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expected {
    seq: Option<u64>,
    hash: Option<String>,
}

/// Checks the ledger. A failed check is still a successful reply: the problems are the answer.
async fn check(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    Query(expected): Query<Expected>,
) -> Response {
    let expected = match (expected.seq, expected.hash) {
        (Some(seq), Some(hash)) => Some(Head { seq, hash }),
        (None, None) => None,
        _ => return failure(StatusCode::BAD_REQUEST, "seq and hash go together"),
    };
    if !service.allow(&identity.credential, Bucket::Heavy) {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            "too many checks and past-state reads",
        );
    }
    let result = service
        .run(Work::Heavy, move |store| store.verify(expected.as_ref()))
        .await;
    match result {
        // Problems can quote a record, which an agent wrote (RAL-41).
        Ok(Ok(result)) => {
            let mut value = json!(result);
            value["untrusted_text"] = json!(views::UNTRUSTED_TEXT);
            Json(value).into_response()
        }
        Ok(Err(error)) => internal(&error),
        Err(not_run) => not_run.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportQuery {
    after: Option<u64>,
}

/// Records as stored, with their hashes, a page at a time, for a backup to recheck and keep
/// (RAL-15). It reads the kept copy without a replay, so it counts as an ordinary read.
async fn export(State(service): State<Arc<Service>>, Query(query): Query<ExportQuery>) -> Response {
    let after = query.after.unwrap_or(0);
    let result = service
        .run(Work::Read, move |store| store.export(after))
        .await;
    match result {
        Ok(Ok((records, head))) => {
            let mut value = json!({
                "untrusted_text": views::UNTRUSTED_TEXT, "head": head, "records": records,
            });
            if records_more(&value, &head) {
                value["next"] = value["records"]
                    .as_array()
                    .and_then(|records| records.last())
                    .map(|last| last["seq"].clone())
                    .unwrap_or_default();
            }
            Json(value).into_response()
        }
        Ok(Err(error)) => internal(&error),
        Err(not_run) => not_run.into_response(),
    }
}

/// Whether there are records after the last one in this page.
fn records_more(page: &Value, head: &Head) -> bool {
    page["records"]
        .as_array()
        .and_then(|records| records.last())
        .and_then(|last| last["seq"].as_u64())
        .is_some_and(|last| last < head.seq)
}

/// Who the watchman is, each designation checked again against the operator's key and the chain
/// (RAL-51). Heavy: without a writer's kept copy, it replays from Postgres.
async fn designation(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
) -> Response {
    if !service.allow(&identity.credential, Bucket::Heavy) {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            "too many checks and past-state reads",
        );
    }
    let key = service.operator_key.clone();
    let result = service
        .run(Work::Heavy, move |store| {
            store.designation_report(key.as_ref())
        })
        .await;
    match result {
        Ok(Ok(mut report)) => {
            // Whether the credential each watchman is bound to is still paired (RAL-53, RAL-63).
            let paired = |credential: &str| {
                json!(matches!(service.state_dir.paired(credential), Ok(Some(_))))
            };
            if let Some(credential) = report["watchman"]["credential"].as_str() {
                report["watchman"]["credential_paired"] = paired(credential);
            }
            if let Some(watchmen) = report["watchmen"].as_array_mut() {
                for watch in watchmen {
                    if let Some(credential) = watch["credential"].as_str() {
                        watch["credential_paired"] = paired(credential);
                    }
                }
            }
            Json(report).into_response()
        }
        Ok(Err(error)) => internal(&error),
        Err(not_run) => not_run.into_response(),
    }
}

/// Every verification labelled watchman, peer or other (RAL-51). Heavy, as `designation` is.
async fn verifications(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
) -> Response {
    if !service.allow(&identity.credential, Bucket::Heavy) {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            "too many checks and past-state reads",
        );
    }
    let key = service.operator_key.clone();
    let result = service
        .run(Work::Heavy, move |store| {
            store.verifications_report(key.as_ref())
        })
        .await;
    match result {
        Ok(Ok(report)) => Json(report).into_response(),
        Ok(Err(error)) => internal(&error),
        Err(not_run) => not_run.into_response(),
    }
}

/// One brief for a new watchman (RAL-22a), at the service's clock. Heavy, like a handover.
async fn brief(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
) -> Response {
    if !service.allow(&identity.credential, Bucket::Heavy) {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            "too many checks and past-state reads",
        );
    }
    let key = service.operator_key.clone();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64);
    let result = service
        .run(Work::Heavy, move |store| {
            store.brief_report(key.as_ref(), now_ms)
        })
        .await;
    match result {
        Ok(Ok(report)) => Json(report).into_response(),
        Ok(Err(error)) => internal(&error),
        Err(not_run) => not_run.into_response(),
    }
}

#[derive(Deserialize)]
struct OpenIdsQuery {
    repo: String,
    path: String,
    prefix: String,
}

/// The highest story number for a prefix written in the roadmap of any open pull request
/// (RAL-68), read from GitHub with the evidence token, read-only. Heavy: it's a GitHub read per
/// open pull request. Without a token it says it couldn't check.
async fn open_ids(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<OpenIdsQuery>,
) -> Response {
    if !service.allow(&identity.credential, Bucket::Heavy) {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            "too many checks and past-state reads",
        );
    }
    let Some(roadmaps) = service.roadmaps.get().cloned() else {
        return Json(json!({"checked": false, "reason": "the service has no GitHub token"}))
            .into_response();
    };
    let owners = service.github_owners.get().cloned().unwrap_or_default();
    let result = tokio::task::spawn_blocking(move || {
        crate::evidence::open_pull_ids(
            roadmaps.as_ref(),
            &owners,
            &query.repo,
            &query.path,
            &query.prefix,
        )
    })
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(_) => failure(StatusCode::INTERNAL_SERVER_ERROR, "the GitHub read stopped"),
    }
}

/// The peer-verified work a newly designated watchman re-reads (RAL-52). Heavy.
async fn handover(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
) -> Response {
    if !service.allow(&identity.credential, Bucket::Heavy) {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            "too many checks and past-state reads",
        );
    }
    let key = service.operator_key.clone();
    let result = service
        .run(Work::Heavy, move |store| {
            store.handover_report(key.as_ref())
        })
        .await;
    match result {
        Ok(Ok(report)) => Json(report).into_response(),
        Ok(Err(error)) => internal(&error),
        Err(not_run) => not_run.into_response(),
    }
}

/// Submissions waiting for a reader, oldest first (RAL-51).
async fn waiting(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<ReadQuery>,
) -> Response {
    // The suggestions are for now, so a past state isn't offered rather than quietly ignored.
    if query.as_of.is_some() {
        return failure(StatusCode::BAD_REQUEST, "waiting doesn't take as_of");
    }
    // The service's own store is the writer, which keeps the chain's hashes, so checking
    // designations costs no replay. A store without them replays from Postgres, which is as
    // costly as a check, so it's limited like one.
    let work = if service.store.keeps_chain() {
        Work::Read
    } else {
        if !service.allow(&identity.credential, Bucket::Heavy) {
            return failure(
                StatusCode::TOO_MANY_REQUESTS,
                "too many checks and past-state reads",
            );
        }
        Work::Heavy
    };
    let key = service.operator_key.clone();
    let all = query.all.unwrap_or(false);
    let result = service
        .run(work, move |store| store.waiting_report(key.as_ref(), all))
        .await;
    match result {
        Ok(Ok(report)) => Json(report).into_response(),
        Ok(Err(error)) => internal(&error),
        Err(not_run) => not_run.into_response(),
    }
}

/// Which credential this is, so that the viewer can warn if it could write.
async fn whoami(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
) -> Response {
    Json(json!({
        "credential": identity.credential, "read_only": identity.read_only,
        "operator_window_ms": service.operator_times.map(|t| t.window_ms),
        "operator_away_after": service.operator_times.map(|t| t.away_after),
        "operator_away_ms": service.operator_times.map(|t| t.away_ms),
    }))
    .into_response()
}

async fn list_stories(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<ReadQuery>,
) -> Response {
    view(service, identity, query, |ledger, page| {
        views::stories(ledger, None, page)
    })
    .await
}

async fn one_story(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    UrlPath(id): UrlPath<String>,
    Query(query): Query<ReadQuery>,
) -> Response {
    view(service, identity, query, move |ledger, page| {
        views::stories(ledger, Some(&id), page)
    })
    .await
}

async fn list_track_records(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<ReadQuery>,
) -> Response {
    view(service, identity, query, |ledger, page| {
        views::track_records(ledger, None, page)
    })
    .await
}

async fn one_track_record(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    UrlPath(name): UrlPath<String>,
    Query(query): Query<ReadQuery>,
) -> Response {
    view(service, identity, query, move |ledger, page| {
        views::track_records(ledger, Some(&name), page)
    })
    .await
}

async fn open_blockers(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<ReadQuery>,
) -> Response {
    let at_ms = query.at_ms;
    view(service, identity, query, move |ledger, page| match at_ms {
        Some(at_ms) => views::blockers_at(ledger, false, page, at_ms),
        None => views::blockers(ledger, false, page),
    })
    .await
}

/// Corrections made after a past record to records at or before it (RAL-28), read from the
/// ledger as it is now.
async fn corrections(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<ReadQuery>,
) -> Response {
    if query.as_of.is_some() {
        return failure(StatusCode::BAD_REQUEST, "corrections don't take as_of");
    }
    let Some(since) = query.since else {
        return failure(
            StatusCode::BAD_REQUEST,
            "corrections need since, a record number",
        );
    };
    view(service, identity, query, move |ledger, page| {
        views::corrections_since(ledger, since, page)
    })
    .await
}

async fn all_blockers(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<ReadQuery>,
) -> Response {
    let at_ms = query.at_ms;
    view(service, identity, query, move |ledger, page| match at_ms {
        Some(at_ms) => views::blockers_at(ledger, true, page, at_ms),
        None => views::blockers(ledger, true, page),
    })
    .await
}

/// The operator's inbox (RAL-146): open items, urgent first then oldest first, as of the
/// service's clock, or `at_ms` when a reader gives one.
async fn open_inbox(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<ReadQuery>,
) -> Response {
    let at_ms = query.at_ms.unwrap_or_else(wall_ms);
    view(service, identity, query, move |ledger, _| {
        Ok(crate::inbox::inbox(ledger, false, at_ms))
    })
    .await
}

/// The operator's inbox with closed items too (RAL-146).
async fn all_inbox(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<ReadQuery>,
) -> Response {
    let at_ms = query.at_ms.unwrap_or_else(wall_ms);
    view(service, identity, query, move |ledger, _| {
        Ok(crate::inbox::inbox(ledger, true, at_ms))
    })
    .await
}

/// Now, by the service's clock.
fn wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

async fn list_teams(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<ReadQuery>,
) -> Response {
    view(service, identity, query, |ledger, page| {
        views::teams(ledger, None, page)
    })
    .await
}

/// Every rally, and the open one (RAL-76), with who may open or close one (RAL-77).
async fn list_rallies(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<ReadQuery>,
) -> Response {
    let who = who_may_open_rallies(&service);
    view(service, identity, query, move |ledger, page| {
        views::rallies(ledger, page).map(|mut value| {
            value["who_may_open"] = who;
            value
        })
    })
    .await
}

/// The holds, conditions and grants in force (RAL-136), read light, for the viewer.
async fn list_standings(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    Query(query): Query<ReadQuery>,
) -> Response {
    view(service, identity, query, views::standings).await
}

/// The change a record's body applies: a backfill applies the entry inside it (RAL-30), so a
/// check on the kind of change looks through the wrapper (RAL-77). The ledger refuses a backfill
/// inside a backfill, but this unwraps every level regardless.
fn applied_change(body: &Value) -> &Value {
    let mut body = body;
    while body["change"] == "backfill" {
        body = &body["entry"];
    }
    &body["change"]
}

/// What every answer says while `--rally-openers` isn't set (RAL-77).
const ANYONE_MAY_OPEN: &str =
    "any paired credential may open or close a rally: set --rally-openers on the service";

/// Who may open or close a rally, as `rally rallies` shows it (RAL-77).
fn who_may_open_rallies(service: &Service) -> Value {
    match &service.rally_openers {
        None => json!({"anyone": true, "warning": ANYONE_MAY_OPEN}),
        Some(openers) => json!({
            "anyone": false,
            "credentials": openers,
            "designated_watchman": service.operator_key.is_some(),
        }),
    }
}

/// Whether `credential` may open or close a rally (RAL-77): any credential while the operator
/// hasn't set `--rally-openers`; then only those listed, or the credential a watchman in force is
/// bound to by a signed designation (RAL-53). `Err` is the refusal to send.
async fn may_open_rallies(service: &Arc<Service>, credential: &str) -> Result<(), Box<Response>> {
    let Some(openers) = &service.rally_openers else {
        return Ok(());
    };
    if openers.iter().any(|opener| opener == credential) {
        return Ok(());
    }
    if let Some(key) = service.operator_key.clone() {
        let watchmen = service
            .run(Work::Read, move |store| store.watchman_credentials(&key))
            .await;
        match watchmen {
            Ok(Ok(bound)) if bound.iter().any(|bound| bound == credential) => return Ok(()),
            Ok(Ok(_)) => {}
            Ok(Err(error)) => return Err(Box::new(internal(&error))),
            Err(not_run) => return Err(Box::new(not_run.into_response())),
        }
    }
    Err(Box::new(failure(
        StatusCode::FORBIDDEN,
        format!(
            "only the operator or the watchman opens or closes a rally, changes its sessions or \
             assigns a story: {credential} isn't in the service's --rally-openers, nor bound to a \
             designated watchman"
        ),
    )))
}

/// Everything recorded about one team, with annotations beside each record (RAL-19).
async fn team_record(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    UrlPath(name): UrlPath<String>,
    Query(query): Query<ReadQuery>,
) -> Response {
    view(service, identity, query, move |ledger, page| {
        views::team_record(ledger, &name, page)
    })
    .await
}

async fn one_team(
    State(service): State<Arc<Service>>,
    Extension(identity): Extension<Identity>,
    UrlPath(name): UrlPath<String>,
    Query(query): Query<ReadQuery>,
) -> Response {
    view(service, identity, query, move |ledger, page| {
        views::teams(ledger, Some(&name), page)
    })
    .await
}
