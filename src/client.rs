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
    parse_reply(&raw)
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
    Ok(Reply {
        status,
        body: String::from_utf8_lossy(body).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
