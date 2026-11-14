//! The client's side of the service: TLS pinned to one certificate, and one small HTTP request.
//!
//! The server's certificate is self-signed, so trust comes only from matching the fingerprint in the
//! pair code. The handshake's signatures are still checked, so that a certificate copied from the
//! real service doesn't let someone else stand in for it.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};

use crate::access::{Pairing, fingerprint_of_der};

const TIMEOUT: Duration = Duration::from_secs(15);

/// The most a whole request may take, from connecting to the end of the reply. Longer than the
/// service's own 60-second limit on a connection, so a slow write still gets its answer.
const TOTAL: Duration = Duration::from_secs(75);

/// The largest reply read. The service's replies are bounded by its pages (RAL-35), and are far
/// smaller than this.
const MAX_REPLY_BYTES: usize = 16 * 1024 * 1024;

/// How long a request may take and how large its reply may be.
#[derive(Clone, Copy, Debug)]
struct Limits {
    total: Duration,
    max_reply_bytes: usize,
}

const LIMITS: Limits = Limits {
    total: TOTAL,
    max_reply_bytes: MAX_REPLY_BYTES,
};

/// Gives the next read on `socket` at most `left`. macOS refuses the setting with "invalid argument"
/// on a socket the other end has already closed, as the service does once it has replied, so a
/// refusal is ignored: the read that follows then sees the close, or falls back on the previous
/// timeout, which is never more than [`TIMEOUT`], and the deadline is checked again after it.
fn shorten_reads(socket: &TcpStream, left: Duration) {
    let _ = socket.set_read_timeout(Some(left));
}

/// The time left before `deadline`, or an error if there's none.
fn left(deadline: Instant) -> Result<Duration, Box<dyn std::error::Error>> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err("the service took too long to reply".into());
    }
    Ok(left.min(TIMEOUT))
}

#[derive(Debug)]
struct PinnedCertificate {
    fingerprint: String,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if fingerprint_of_der(end_entity) == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "the service's certificate doesn't match its pairing".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = &self.provider.signature_verification_algorithms;
        verify_tls12_signature(message, cert, signature, algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = &self.provider.signature_verification_algorithms;
        verify_tls13_signature(message, cert, signature, algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// A response from the service.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    /// How long the service asked the client to wait, from a `Retry-After` of whole seconds
    /// (RAL-86). An HTTP date isn't read.
    pub retry_after_ms: Option<u64>,
    /// When the service has no such route at all: that it's older than the story that added the
    /// route, said in words (RAL-156).
    pub older_than: Option<String>,
}

/// The story that added each of the service's routes, by its path with `{…}` for a part that
/// varies, so a reply that the route doesn't exist can say the service is older than that story
/// (RAL-156). `None` for the routes every service has had. A test checks every route the service
/// serves is here. A feature added as a query on an existing route, such as the watchman's brief
/// (`/v1/brief?watchman=true`, RAL-157), can't be told from a 404, since an older service ignores
/// the query: its reader checks the answer's shape instead (RAL-166).
pub const ROUTE_STORIES: &[(&str, Option<&str>)] = &[
    ("/v1/records", None),
    ("/v1/stories", None),
    ("/v1/stories/{id}", None),
    ("/v1/teams", None),
    ("/v1/teams/{name}", None),
    ("/v1/check", None),
    ("/v1/whoami", None),
    ("/v1/export", Some("RAL-15")),
    ("/v1/teams/{name}/record", Some("RAL-19")),
    ("/v1/blockers", Some("RAL-20")),
    ("/v1/blockers/all", Some("RAL-20")),
    ("/v1/track-records", Some("RAL-21")),
    ("/v1/track-records/{name}", Some("RAL-21")),
    ("/v1/brief", Some("RAL-22a")),
    ("/v1/corrections", Some("RAL-28")),
    ("/v1/designation", Some("RAL-51")),
    ("/v1/verifications", Some("RAL-51")),
    ("/v1/waiting", Some("RAL-51")),
    ("/v1/handover", Some("RAL-52")),
    ("/v1/open-ids", Some("RAL-68")),
    ("/v1/rallies", Some("RAL-76")),
    ("/v1/standings", Some("RAL-136")),
    ("/v1/inbox", Some("RAL-146")),
    ("/v1/inbox/all", Some("RAL-146")),
];

/// The story that added the route `path` asks for, its query left aside (RAL-156).
pub fn route_story(path: &str) -> Option<&'static str> {
    let path = path.split('?').next().unwrap_or(path);
    let asked: Vec<&str> = path.split('/').collect();
    ROUTE_STORIES
        .iter()
        .find(|(route, _)| {
            let parts: Vec<&str> = route.split('/').collect();
            parts.len() == asked.len()
                && parts
                    .iter()
                    .zip(&asked)
                    .all(|(part, asked)| part.starts_with('{') || part == asked)
        })
        .and_then(|(_, story)| *story)
}

/// What a client says when the service answers that it has no route for `path` (RAL-156): that
/// the service is older than the story that added it, as the viewer says, rather than a bare 404.
pub fn older_service(path: &str) -> String {
    let route = path.split('?').next().unwrap_or(path);
    match route_story(path) {
        Some(story) => format!(
            "the service is older than {story}, which added {route}: it needs updating to the \
             current Rally"
        ),
        None => format!("the service has no {route}: it may be older or newer than this rally"),
    }
}

/// Whether a reply is the service's answer for a route it doesn't have.
fn no_such_route(status: u16, body: &str) -> bool {
    status == 404
        && serde_json::from_str::<serde_json::Value>(body)
            .is_ok_and(|value| value["error"] == "no such route")
}

/// A TLS connection to the service, pinned to its certificate, with the handshake done.
pub type Connection = StreamOwned<ClientConnection, TcpStream>;

/// Connects to the service and completes the TLS handshake, sending no request. [`request`] uses
/// it; tests use it to hold a connection open.
pub fn connect(pairing: &Pairing) -> Result<Connection, Box<dyn std::error::Error>> {
    connect_by(pairing, Instant::now() + TOTAL)
}

fn connect_by(
    pairing: &Pairing,
    deadline: Instant,
) -> Result<Connection, Box<dyn std::error::Error>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinnedCertificate {
        fingerprint: pairing.fingerprint.clone(),
        provider: provider.clone(),
    });
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    let connection =
        ClientConnection::new(Arc::new(config), ServerName::try_from("rally")?.to_owned())?;

    let address = pairing
        .address
        .to_socket_addrs()?
        .next()
        .ok_or("the service's address doesn't resolve")?;
    let socket = TcpStream::connect_timeout(&address, left(deadline)?)?;
    socket.set_read_timeout(Some(TIMEOUT))?;
    socket.set_write_timeout(Some(TIMEOUT))?;
    let mut stream = StreamOwned::new(connection, socket);
    while stream.conn.is_handshaking() {
        // Each step gets only the time left, so a server that drips the handshake can't stretch it.
        shorten_reads(&stream.sock, left(deadline)?);
        stream.conn.complete_io(&mut stream.sock)?;
    }
    Ok(stream)
}

/// Sends one request. `extra_headers` lets tests send what a real client wouldn't.
pub fn request(
    pairing: &Pairing,
    method: &str,
    path: &str,
    body: Option<&str>,
    extra_headers: &[(&str, &str)],
) -> Result<Reply, Box<dyn std::error::Error>> {
    request_within(pairing, method, path, body, extra_headers, LIMITS)
}

/// [`request`], but given up after `total` overall rather than the usual deadline, for callers
/// that mustn't wait long, such as the session-start hook (RAL-50).
pub fn request_for(
    pairing: &Pairing,
    method: &str,
    path: &str,
    total: Duration,
) -> Result<Reply, Box<dyn std::error::Error>> {
    request_within(
        pairing,
        method,
        path,
        None,
        &[],
        Limits {
            total: total.min(TOTAL),
            max_reply_bytes: MAX_REPLY_BYTES,
        },
    )
}

fn request_within(
    pairing: &Pairing,
    method: &str,
    path: &str,
    body: Option<&str>,
    extra_headers: &[(&str, &str)],
    limits: Limits,
) -> Result<Reply, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + limits.total;
    let mut stream = connect_by(pairing, deadline)?;
    let body = body.unwrap_or_default();
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: rally\r\nConnection: close\r\n\
         Authorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        pairing.token,
        body.len()
    );
    for (name, value) in extra_headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()?;

    // Read in pieces, so that neither a reply that never ends nor one that arrives a byte at a
    // time can hold this past its deadline or its size.
    let mut raw = Vec::new();
    let mut piece = [0u8; 16 * 1024];
    loop {
        shorten_reads(&stream.sock, left(deadline)?);
        match stream.read(&mut piece) {
            Ok(0) => break,
            Ok(read) => {
                if raw.len() + read > limits.max_reply_bytes {
                    return Err("the service's reply was too large".into());
                }
                raw.extend_from_slice(&piece[..read]);
            }
            // The server may close without a TLS close message once it has replied in full.
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof && !raw.is_empty() => {
                break;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) && Instant::now() >= deadline =>
            {
                return Err("the service took too long to reply".into());
            }
            Err(error) => return Err(error.into()),
        }
    }
    let mut reply = parse_reply(&raw)?;
    reply.older_than = no_such_route(reply.status, &reply.body).then(|| older_service(path));
    Ok(reply)
}

/// Reads a reply, refusing one shorter than it says it is: someone on the network can't forge a
/// reply, but can cut one off.
fn parse_reply(raw: &[u8]) -> Result<Reply, Box<dyn std::error::Error>> {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("the service sent an incomplete reply")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    let body = &raw[split + 4..];
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .ok_or("the service sent a reply with no status")?;
    let length: usize = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .ok_or("the service sent a reply with no length")?;
    if body.len() != length {
        return Err("the service's reply was cut off".into());
    }
    let retry_after_ms = head
        .lines()
        .find_map(|line| line.strip_prefix("retry-after:"))
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|seconds| seconds.saturating_mul(1000));
    Ok(Reply {
        status,
        body: String::from_utf8_lossy(body).into_owned(),
        retry_after_ms,
        older_than: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RAL-156: every route the service serves says which story added it, so a client meeting a
    /// service without it can say what the service is older than. A new route without an entry
    /// fails here.
    #[test]
    fn every_route_says_which_story_added_it() {
        let server = include_str!("server.rs");
        let routes: Vec<&str> = server
            .match_indices(".route(\"")
            .map(|(at, open)| {
                let rest = &server[at + open.len()..];
                &rest[..rest.find('"').unwrap()]
            })
            .collect();
        assert!(routes.len() > 20, "{routes:?}");
        for route in routes {
            assert!(
                ROUTE_STORIES.iter().any(|(listed, _)| *listed == route),
                "{route} isn't in ROUTE_STORIES"
            );
        }
    }

    #[test]
    fn a_missing_route_names_the_story_that_added_it() {
        assert_eq!(route_story("/v1/inbox"), Some("RAL-146"));
        assert_eq!(route_story("/v1/inbox/all?as_of=3"), Some("RAL-146"));
        assert_eq!(
            route_story("/v1/teams/rally/record?after=4"),
            Some("RAL-19")
        );
        assert_eq!(route_story("/v1/track-records/rally"), Some("RAL-21"));
        assert_eq!(route_story("/v1/stories/RAL-1"), None);
        assert_eq!(route_story("/v1/nope"), None);
        assert_eq!(
            older_service("/v1/inbox"),
            "the service is older than RAL-146, which added /v1/inbox: it needs updating to the \
             current Rally"
        );
        assert!(older_service("/v1/nope?x=1").starts_with("the service has no /v1/nope:"));
        assert!(no_such_route(404, r#"{"error":"no such route"}"#));
        assert!(!no_such_route(404, r#"{"error":"no story RAL-9"}"#));
        assert!(!no_such_route(405, r#"{"error":"no such route"}"#));
    }

    /// RAL-156: such a reply reads as what it is, not as a refusal.
    #[test]
    fn an_older_service_is_said_plainly() {
        let reply = Reply {
            status: 404,
            body: r#"{"error":"no such route"}"#.into(),
            retry_after_ms: None,
            older_than: Some(older_service("/v1/inbox")),
        };
        let said = crate::retry::Refused::of(&reply).to_string();
        assert!(
            said.starts_with("the service is older than RAL-146"),
            "{said}"
        );
        assert!(!said.contains("refused"), "{said}");
    }
    use rustls::pki_types::PrivatePkcs8KeyDer;
    use rustls::{ServerConfig, ServerConnection};

    /// A TLS server on loopback that reads one request, then sends `reply` with `pause` between
    /// pieces, `times` times or until the client goes. Returns a pairing for it.
    fn fake_service(reply: &'static [u8], pause: Duration, times: usize) -> Pairing {
        let generated = rcgen::generate_simple_self_signed(vec!["rally".to_owned()]).unwrap();
        let der = generated.cert.der().clone();
        let key = PrivatePkcs8KeyDer::from(generated.signing_key.serialize_der());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![der.clone()], key.into())
            .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            let connection = ServerConnection::new(Arc::new(config)).unwrap();
            let mut stream = StreamOwned::new(connection, socket);
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                request.push(byte[0]);
            }
            for _ in 0..times {
                if stream
                    .write_all(reply)
                    .and_then(|()| stream.flush())
                    .is_err()
                {
                    return;
                }
                std::thread::sleep(pause);
            }
        });
        Pairing {
            address,
            fingerprint: crate::access::fingerprint_of_der(&der),
            token: "t".into(),
        }
    }

    /// RAL-86: a fake service that refuses with 429 and `Retry-After`, then one that answers. The
    /// loop logs the refusal, waits as asked, and carries on to the answer.
    #[test]
    fn a_429_with_retry_after_is_waited_out_and_the_next_pass_succeeds() {
        use crate::retry::{Refused, run};
        let body = r#"{"error":"too many checks and past-state reads"}"#;
        let refusal = format!(
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        let busy = fake_service(
            Box::leak(refusal.into_bytes().into_boxed_slice()),
            Duration::ZERO,
            1,
        );
        let fine = fake_service(
            b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}",
            Duration::ZERO,
            1,
        );
        let services = std::cell::RefCell::new(vec![busy, fine]);
        let lines = std::cell::RefCell::new(Vec::new());
        let waits = std::cell::RefCell::new(Vec::new());
        let error = run(
            30_000,
            || Ok(()),
            || {
                if services.borrow().is_empty() {
                    return Err(Box::new(crate::retry::Fatal("done".into())));
                }
                let pairing = services.borrow_mut().remove(0);
                let reply = request(&pairing, "GET", "/v1/export", None, &[])?;
                if reply.status == 200 {
                    Ok(reply.body)
                } else {
                    Err(Box::new(Refused::of(&reply)))
                }
            },
            |line| lines.borrow_mut().push(line.to_owned()),
            |ms| waits.borrow_mut().push(ms),
        );
        assert_eq!(error.to_string(), "done");
        assert_eq!(
            *waits.borrow(),
            [30_000, 30_000],
            "Retry-After 7s is below the backoff"
        );
        let failed: serde_json::Value = serde_json::from_str(&lines.borrow()[0]).unwrap();
        assert_eq!(failed["pass"], "failed");
        assert_eq!(failed["retry_after_ms"], 7_000);
        assert!(failed["error"].as_str().unwrap().contains("refused (429)"));
        assert_eq!(lines.borrow()[1], "{}");
    }

    #[test]
    fn a_reply_that_drips_is_cut_off_at_the_deadline() {
        let pairing = fake_service(b"x", Duration::from_millis(100), 1_000);
        let limits = Limits {
            total: Duration::from_secs(2),
            max_reply_bytes: MAX_REPLY_BYTES,
        };
        let started = Instant::now();
        let result = request_within(&pairing, "GET", "/", None, &[], limits);
        let error = result.unwrap_err().to_string();
        assert!(error.contains("too long"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn request_for_gives_up_at_its_own_deadline() {
        let pairing = fake_service(b"x", Duration::from_millis(100), 1_000);
        let started = Instant::now();
        let result = request_for(&pairing, "GET", "/", Duration::from_millis(800));
        assert!(result.is_err());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_reply_that_never_ends_is_cut_off_at_its_size() {
        static PIECE: [u8; 64 * 1024] = [b'x'; 64 * 1024];
        let pairing = fake_service(&PIECE, Duration::ZERO, 1_000);
        let limits = Limits {
            total: TOTAL,
            max_reply_bytes: 1024 * 1024,
        };
        let error = request_within(&pairing, "GET", "/", None, &[], limits)
            .unwrap_err()
            .to_string();
        assert!(error.contains("too large"), "{error}");
    }

    #[test]
    fn a_whole_reply_within_the_limits_is_read() {
        let pairing = fake_service(
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
            Duration::ZERO,
            1,
        );
        let reply = request_within(&pairing, "GET", "/", None, &[], LIMITS).unwrap();
        assert_eq!((reply.status, reply.body.as_str()), (200, "{}"));
    }

    #[test]
    fn a_complete_reply_is_read() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        let reply = parse_reply(raw).unwrap();
        assert_eq!((reply.status, reply.body.as_str()), (200, "{}"));
    }

    #[test]
    fn a_truncated_reply_is_refused() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n{\"as";
        assert!(parse_reply(raw).is_err());
    }

    #[test]
    fn a_reply_without_a_length_is_refused() {
        let raw = b"HTTP/1.1 200 OK\r\n\r\n{}";
        assert!(parse_reply(raw).is_err());
    }

    #[test]
    fn a_reply_cut_off_in_its_headers_is_refused() {
        assert!(parse_reply(b"HTTP/1.1 200 OK\r\nContent-Le").is_err());
    }
}
