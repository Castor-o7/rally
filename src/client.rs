//! The client's side of the service: TLS pinned to one certificate, and one small HTTP request.
//!
//! The server's certificate is self-signed, so trust comes only from matching the fingerprint in the
//! pair code. The handshake's signatures are still checked, so that a certificate copied from the
//! real service doesn't let someone else stand in for it.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};

use crate::access::{Pairing, fingerprint_of_der};

const TIMEOUT: Duration = Duration::from_secs(15);

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
    let socket = TcpStream::connect_timeout(&address, TIMEOUT)?;
    socket.set_read_timeout(Some(TIMEOUT))?;
    socket.set_write_timeout(Some(TIMEOUT))?;
    let mut stream = StreamOwned::new(connection, socket);
    while stream.conn.is_handshaking() {
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
    let mut stream = connect(pairing)?;
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

    let mut raw = Vec::new();
    if let Err(error) = stream.read_to_end(&mut raw) {
        // The server may close without a TLS close message once it has replied in full.
        if raw.is_empty() || error.kind() != std::io::ErrorKind::UnexpectedEof {
            return Err(error.into());
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
