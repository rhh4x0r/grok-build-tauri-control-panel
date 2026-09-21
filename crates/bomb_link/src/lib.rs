//! How a Mac and a server recognise each other.
//!
//! There is no certificate authority and no account service. Each side has a
//! self-signed [`Identity`]. The server's fingerprint travels inside the
//! [`PairingLink`], so the client pins it; the server learns a device's
//! fingerprint when that device presents a valid pairing secret. After that,
//! TLS 1.3 with client certificates proves both ends on every connection.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring, verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{ClientConfig, DigitallySignedStruct, DistinguishedName, ServerConfig, SignatureScheme};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// A self-signed certificate and its private key. The key never leaves the machine that made it.
#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    pub cert_pem: String,
    pub key_pem: String,
}

impl Identity {
    pub fn generate(name: &str) -> Result<Self, String> {
        let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
        let params = rcgen::CertificateParams::new(vec![name.to_string()]).map_err(|e| e.to_string())?;
        let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
        Ok(Self { cert_pem: cert.pem(), key_pem: key.serialize_pem() })
    }

    fn cert_der(&self) -> Result<CertificateDer<'static>, String> {
        let (label, der) = pem_decode(&self.cert_pem)?;
        if label != "CERTIFICATE" { return Err("not a certificate".into()); }
        Ok(CertificateDer::from(der))
    }

    fn key_der(&self) -> Result<PrivateKeyDer<'static>, String> {
        let (label, der) = pem_decode(&self.key_pem)?;
        if label != "PRIVATE KEY" { return Err("not a PKCS#8 private key".into()); }
        Ok(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(der)))
    }

    /// What the other side pins or registers: SHA-256 of the certificate, hex.
    pub fn fingerprint(&self) -> Result<String, String> {
        Ok(fingerprint(&self.cert_der()?))
    }
}

pub fn fingerprint(cert: &CertificateDer<'_>) -> String {
    hex::encode(Sha256::digest(cert.as_ref()))
}

fn pem_decode(pem: &str) -> Result<(String, Vec<u8>), String> {
    let mut lines = pem.lines().map(str::trim).filter(|l| !l.is_empty());
    let label = lines.next().and_then(|l| l.strip_prefix("-----BEGIN ")).and_then(|l| l.strip_suffix("-----")).ok_or("bad PEM header")?.to_string();
    let body: String = lines.take_while(|l| !l.starts_with("-----END")).collect();
    Ok((label, base64_decode(&body)?))
}

fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u8);
    for byte in text.bytes().filter(|b| *b != b'=') {
        let value = TABLE.iter().position(|c| *c == byte).ok_or("bad base64")? as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

fn algorithms() -> WebPkiSupportedAlgorithms {
    ring::default_provider().signature_verification_algorithms
}

/// Accepts exactly one server certificate: the one named in the pairing link.
#[derive(Debug)]
struct PinnedServer {
    fingerprint: Vec<u8>,
}

impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        let seen = Sha256::digest(end_entity.as_ref());
        if bool::from(seen.as_slice().ct_eq(&self.fingerprint)) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("this is not the server this Mac was paired with".into()))
        }
    }
    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.3 only".into()))
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &algorithms())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        algorithms().supported_schemes()
    }
}

/// Lets any device finish the handshake, but only if it proves it holds its certificate's key.
/// Who the device is, and whether it may do anything, is decided afterwards from its fingerprint.
#[derive(Debug)]
struct AnyDevice;

impl ClientCertVerifier for AnyDevice {
    fn root_hint_subjects(&self) -> &[DistinguishedName] { &[] }
    fn verify_client_cert(&self, _: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: UnixTime) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.3 only".into()))
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &algorithms())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        algorithms().supported_schemes()
    }
}

pub fn server_config(identity: &Identity) -> Result<Arc<ServerConfig>, String> {
    let mut config = ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .with_client_cert_verifier(Arc::new(AnyDevice))
        .with_single_cert(vec![identity.cert_der()?], identity.key_der()?)
        .map_err(|e| e.to_string())?;
    // No resumption: a revoked device must not slip back in on a cached session.
    config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    config.send_tls13_tickets = 0;
    Ok(Arc::new(config))
}

pub fn client_config(identity: &Identity, server_fingerprint: &str) -> Result<Arc<ClientConfig>, String> {
    let fingerprint = hex::decode(server_fingerprint).map_err(|_| "The server fingerprint in the pairing link is not valid.".to_string())?;
    if fingerprint.len() != 32 { return Err("The server fingerprint in the pairing link is not valid.".into()); }
    let mut config = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedServer { fingerprint }))
        .with_client_auth_cert(vec![identity.cert_der()?], identity.key_der()?)
        .map_err(|e| e.to_string())?;
    config.resumption = rustls::client::Resumption::disabled();
    Ok(Arc::new(config))
}

pub type ClientStream = tokio_rustls::client::TlsStream<tokio::net::TcpStream>;

/// Open an authenticated connection to a paired server.
pub async fn connect(host: &str, server_fingerprint: &str, identity: &Identity) -> Result<ClientStream, String> {
    let config = client_config(identity, server_fingerprint)?;
    let tcp = tokio::time::timeout(std::time::Duration::from_secs(10), tokio::net::TcpStream::connect(host))
        .await
        .map_err(|_| format!("Could not reach {host} (timed out)."))?
        .map_err(|e| format!("Could not reach {host}: {e}"))?;
    let _ = tcp.set_nodelay(true);
    // The name is not used for trust; the pinned fingerprint is.
    let name = ServerName::try_from("bombd").map_err(|e| e.to_string())?;
    tokio_rustls::TlsConnector::from(config).connect(name, tcp).await.map_err(|e| format!("Could not set up a secure connection: {e}"))
}

/// The fingerprint of the device on the other end of an accepted connection.
pub fn peer_fingerprint<S>(stream: &tokio_rustls::server::TlsStream<S>) -> Option<String> {
    stream.get_ref().1.peer_certificates().and_then(|certs| certs.first()).map(fingerprint)
}

/// What a person pastes into a new Mac to pair it: where the server is, how to recognise it, and a one-time secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingLink {
    pub host: String,
    pub fingerprint: String,
    pub secret: String,
}

impl PairingLink {
    pub fn new(host: &str, fingerprint: &str) -> Self {
        let secret: [u8; 16] = rand::random();
        Self { host: host.into(), fingerprint: fingerprint.into(), secret: hex::encode(secret) }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let bad = || "That doesn’t look like a Bomb Code pairing link.".to_string();
        let query = text.trim().strip_prefix("bomb://pair?").ok_or_else(bad)?;
        let (mut host, mut fingerprint, mut secret) = (None, None, None);
        for pair in query.split('&') {
            match pair.split_once('=') {
                Some(("h", v)) => host = Some(v.to_string()),
                Some(("fp", v)) => fingerprint = Some(v.to_string()),
                Some(("s", v)) => secret = Some(v.to_string()),
                _ => {}
            }
        }
        let link = Self { host: host.ok_or_else(bad)?, fingerprint: fingerprint.ok_or_else(bad)?, secret: secret.ok_or_else(bad)? };
        let hex_of = |s: &str, bytes: usize| s.len() == bytes * 2 && s.bytes().all(|b| b.is_ascii_hexdigit());
        if !hex_of(&link.fingerprint, 32) || !hex_of(&link.secret, 16) || !link.host.contains(':') { return Err(bad()); }
        Ok(link)
    }

    /// Stored instead of the secret itself.
    pub fn secret_hash(secret: &str) -> String {
        hex::encode(Sha256::digest(secret.as_bytes()))
    }
}

impl std::fmt::Display for PairingLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bomb://pair?h={}&fp={}&s={}", self.host, self.fingerprint, self.secret)
    }
}

/// Compare two hex digests without leaking where they differ.
pub fn same_digest(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn pairing_links_round_trip_and_reject_anything_else() {
        let link = PairingLink::new("203.0.113.7:7443", &"ab".repeat(32));
        assert_eq!(PairingLink::parse(&link.to_string()).unwrap(), link);
        assert_eq!(link.secret.len(), 32);
        assert_ne!(PairingLink::new("h:1", &"ab".repeat(32)).secret, link.secret);
        for bad in ["", "https://example.com", "bomb://pair?h=host:1&fp=zz&s=00", "bomb://pair?h=nohostport&fp=", &format!("bomb://pair?h=h:1&fp={}&s=short", "ab".repeat(32))] {
            assert!(PairingLink::parse(bad).is_err(), "{bad}");
        }
        assert!(same_digest(&PairingLink::secret_hash("a"), &PairingLink::secret_hash("a")));
        assert!(!same_digest(&PairingLink::secret_hash("a"), &PairingLink::secret_hash("b")));
    }

    #[tokio::test]
    async fn only_the_pinned_server_is_trusted_and_the_server_learns_the_device() {
        let server = Identity::generate("bombd").unwrap();
        let impostor = Identity::generate("bombd").unwrap();
        let device = Identity::generate("mac").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let acceptor = tokio_rustls::TlsAcceptor::from(server_config(&server).unwrap());
        let expected = device.fingerprint().unwrap();
        tokio::spawn(async move {
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                let expected = expected.clone();
                tokio::spawn(async move {
                    if let Ok(mut tls) = acceptor.accept(tcp).await {
                        assert_eq!(peer_fingerprint(&tls).as_deref(), Some(expected.as_str()));
                        let _ = tls.write_all(b"ok").await;
                        let _ = tls.shutdown().await;
                    }
                });
            }
        });
        let mut good = connect(&address, &server.fingerprint().unwrap(), &device).await.unwrap();
        let mut reply = Vec::new();
        good.read_to_end(&mut reply).await.unwrap();
        assert_eq!(reply, b"ok");
        // Same address, but the Mac was paired with a different server identity.
        let error = connect(&address, &impostor.fingerprint().unwrap(), &device).await.unwrap_err();
        assert!(error.contains("secure connection"), "{error}");
    }
}
