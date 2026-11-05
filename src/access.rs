//! Who may connect to the service (RAL-13): the service's certificate and machines' tokens.
//!
//! Tokens are stored only as SHA-256 hashes. They're long and random, so a plain hash is enough,
//! and they're Rally's own, separate from Muster's. A pair code holds a token, so treat it like a
//! password. See `docs/service.md`.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MACHINES_FILE: &str = "machines.json";
const CERT_FILE: &str = "cert.pem";
const KEY_FILE: &str = "key.pem";
const LOCK_FILE: &str = "machines.lock";
const PAIR_PREFIX: &str = "rally-pair:v1/";

/// A paired machine or read-only viewer, as stored on the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Machine {
    token_sha256: String,
    pub read_only: bool,
}

/// A connecting machine that presented a valid token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub credential: String,
    pub read_only: bool,
}

/// What a client needs to reach the service: its address, its pinned certificate and a token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pairing {
    pub address: String,
    pub fingerprint: String,
    pub token: String,
}

impl fmt::Debug for Pairing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pairing")
            .field("address", &self.address)
            .field("fingerprint", &self.fingerprint)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl Pairing {
    /// The code to run `rally pair` with on the machine being paired.
    pub fn code(&self) -> String {
        format!(
            "{PAIR_PREFIX}{}/{}/{}",
            self.address, self.fingerprint, self.token
        )
    }

    pub fn parse(code: &str) -> Result<Self, AccessError> {
        let bad = || AccessError::BadPairCode;
        let rest = code.trim().strip_prefix(PAIR_PREFIX).ok_or_else(bad)?;
        let mut parts = rest.split('/');
        let (Some(address), Some(fingerprint), Some(token), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(bad());
        };
        if address.is_empty() || fingerprint.len() != 64 || token.is_empty() {
            return Err(bad());
        }
        Ok(Self {
            address: address.to_owned(),
            fingerprint: fingerprint.to_owned(),
            token: token.to_owned(),
        })
    }
}

/// Why an access operation failed. No variant ever holds a token or key.
#[derive(Debug)]
pub enum AccessError {
    Io(PathBuf, std::io::Error),
    /// A file on the host can't be understood. Holds the path.
    BadFile(PathBuf),
    BadPairCode,
    AlreadyPaired(String),
    UnknownMachine(String),
    EmptyName,
    /// The name is one the ledger would refuse as a record's credential.
    BadName,
    /// An address is empty, or has a character a pair code can't carry.
    BadAddress,
    Certificate(String),
    NoRandomness,
}

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, e) => write!(f, "{}: {e}", path.display()),
            Self::BadFile(path) => write!(f, "{} can't be read", path.display()),
            Self::BadPairCode => write!(f, "that isn't a valid pair code"),
            Self::AlreadyPaired(name) => write!(f, "{name} is already paired"),
            Self::UnknownMachine(name) => write!(f, "{name} isn't paired"),
            Self::EmptyName => write!(f, "the machine's name is empty"),
            Self::BadName => write!(
                f,
                "the machine's name must be at most 64 characters, with no control, invisible or \
                 direction characters, because each record carries it"
            ),
            Self::BadAddress => write!(f, "the address must be a host and port"),
            Self::Certificate(reason) => write!(f, "certificate error: {reason}"),
            Self::NoRandomness => write!(f, "can't get secure random bytes"),
        }
    }
}

impl std::error::Error for AccessError {}

/// The service's private state directory.
#[derive(Clone, Debug)]
pub struct StateDir(PathBuf);

impl StateDir {
    /// Opens the directory, creating it private to this account.
    pub fn open(path: &Path) -> Result<Self, AccessError> {
        // Only a directory made here is made private, so a mistaken path can't change an existing
        // directory such as a home directory. The secrets inside are private either way.
        if !path.exists() {
            std::fs::create_dir_all(path).map_err(|e| AccessError::Io(path.to_owned(), e))?;
            set_private_dir(path).map_err(|e| AccessError::Io(path.to_owned(), e))?;
        }
        Ok(Self(path.to_owned()))
    }

    /// The certificate and key as PEM, created on first use, plus the certificate's fingerprint.
    pub fn certificate(&self) -> Result<(PathBuf, PathBuf, String), AccessError> {
        let _lock = self.lock()?;
        let cert = self.0.join(CERT_FILE);
        let key = self.0.join(KEY_FILE);
        // One without the other means something was lost. Replacing them would silently break every
        // machine's pin, so that's left for the operator to decide.
        if cert.exists() != key.exists() {
            return Err(AccessError::Certificate(format!(
                "only one of {CERT_FILE} and {KEY_FILE} exists in {}",
                self.0.display()
            )));
        }
        if !cert.exists() {
            let generated = rcgen::generate_simple_self_signed(vec!["rally".to_owned()])
                .map_err(|e| AccessError::Certificate(e.to_string()))?;
            write_private(&key, generated.signing_key.serialize_pem().as_bytes())?;
            write_private(&cert, generated.cert.pem().as_bytes())?;
        }
        let pem = std::fs::read_to_string(&cert).map_err(|e| AccessError::Io(cert.clone(), e))?;
        let fingerprint =
            fingerprint_of_pem(&pem).ok_or_else(|| AccessError::BadFile(cert.clone()))?;
        // An empty or damaged key would only fail when the service starts, after pair codes with
        // this fingerprint were handed out.
        let key_pem = std::fs::read(&key).map_err(|e| AccessError::Io(key.clone(), e))?;
        {
            use rustls::pki_types::PrivateKeyDer;
            use rustls::pki_types::pem::PemObject;
            let parsed = PrivateKeyDer::from_pem_slice(&key_pem)
                .map_err(|_| AccessError::BadFile(key.clone()))?;
            if parsed.secret_der().is_empty() {
                return Err(AccessError::BadFile(key.clone()));
            }
        }
        Ok((cert, key, fingerprint))
    }

    /// Adds a machine and returns its pair code's parts. The token isn't kept anywhere else.
    pub fn add_machine(
        &self,
        name: &str,
        address: &str,
        read_only: bool,
    ) -> Result<Pairing, AccessError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AccessError::EmptyName);
        }
        // The service puts this name on every record the machine writes, so a name the ledger
        // refuses would make every write fail.
        if !crate::record::is_clear_name(name) {
            return Err(AccessError::BadName);
        }
        if address.is_empty() || address.contains(|c: char| c == '/' || c.is_whitespace()) {
            return Err(AccessError::BadAddress);
        }
        let (_, _, fingerprint) = self.certificate()?;
        let _lock = self.lock()?;
        let mut machines = self.machines()?;
        if machines.contains_key(name) {
            return Err(AccessError::AlreadyPaired(name.to_owned()));
        }
        let token = random_token()?;
        machines.insert(
            name.to_owned(),
            Machine {
                token_sha256: sha256_hex(token.as_bytes()),
                read_only,
            },
        );
        self.save(&machines)?;
        Ok(Pairing {
            address: address.to_owned(),
            fingerprint,
            token,
        })
    }

    /// Removes a machine. The service reads this file for each request, so it applies at once.
    pub fn revoke(&self, name: &str) -> Result<(), AccessError> {
        let _lock = self.lock()?;
        let mut machines = self.machines()?;
        if machines.remove(name).is_none() {
            return Err(AccessError::UnknownMachine(name.to_owned()));
        }
        self.save(&machines)
    }

    /// Finds who a token belongs to, comparing against every machine's hash.
    pub fn identify(&self, token: &str) -> Result<Option<Identity>, AccessError> {
        let hash = sha256_hex(token.as_bytes());
        Ok(self
            .machines()?
            .into_iter()
            .find(|(_, machine)| machine.token_sha256 == hash)
            .map(|(credential, machine)| Identity {
                credential,
                read_only: machine.read_only,
            }))
    }

    /// Holds off other changes, across processes, until the returned file is dropped.
    fn lock(&self) -> Result<std::fs::File, AccessError> {
        let path = self.0.join(LOCK_FILE);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| AccessError::Io(path.clone(), e))?;
        file.lock().map_err(|e| AccessError::Io(path, e))?;
        Ok(file)
    }

    fn machines(&self) -> Result<BTreeMap<String, Machine>, AccessError> {
        let path = self.0.join(MACHINES_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|_| AccessError::BadFile(path)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(AccessError::Io(path, e)),
        }
    }

    fn save(&self, machines: &BTreeMap<String, Machine>) -> Result<(), AccessError> {
        let path = self.0.join(MACHINES_FILE);
        let temporary = self.0.join(format!("{MACHINES_FILE}.new"));
        let text = serde_json::to_string_pretty(machines).expect("machines have string keys");
        write_private(&temporary, text.as_bytes())?;
        std::fs::rename(&temporary, &path).map_err(|e| AccessError::Io(path.clone(), e))?;
        // Make the rename itself survive a crash.
        std::fs::File::open(&self.0)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| AccessError::Io(path, e))
    }
}

/// The SHA-256 of a certificate's DER bytes, in lower-case hex.
pub fn fingerprint_of_der(der: &[u8]) -> String {
    sha256_hex(der)
}

/// The fingerprint of the one certificate in `pem`, or `None` if it holds none. An empty file, or
/// text that isn't a certificate, has none, rather than the fingerprint of nothing.
fn fingerprint_of_pem(pem: &str) -> Option<String> {
    use rustls::pki_types::CertificateDer;
    use rustls::pki_types::pem::PemObject;
    let der = CertificateDer::from_pem_slice(pem.as_bytes()).ok()?;
    // A certificate is a DER sequence, so it starts with 0x30.
    if der.first() != Some(&0x30) {
        return None;
    }
    Some(fingerprint_of_der(&der))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn random_token() -> Result<String, AccessError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| AccessError::NoRandomness)?;
    Ok(hex::encode(bytes))
}

/// Writes a file readable and writable only by this account, replacing any contents.
pub fn write_private(path: &Path, contents: &[u8]) -> Result<(), AccessError> {
    let io = |e| AccessError::Io(path.to_owned(), e);
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(io)?;
    }
    file.write_all(contents).map_err(io)?;
    file.sync_all().map_err(io)
}

#[cfg(unix)]
fn set_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_dir(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> (tempfile::TempDir, StateDir) {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::open(&dir.path().join("state")).unwrap();
        (dir, state)
    }

    #[test]
    fn a_token_identifies_its_machine_and_only_a_hash_is_stored() {
        let (dir, state) = state();
        let pairing = state.add_machine("a1", "10.0.0.1:6300", false).unwrap();
        let stored = std::fs::read_to_string(dir.path().join("state/machines.json")).unwrap();
        assert!(!stored.contains(&pairing.token));
        assert_eq!(
            state.identify(&pairing.token).unwrap(),
            Some(Identity {
                credential: "a1".into(),
                read_only: false
            })
        );
        assert_eq!(state.identify("wrong").unwrap(), None);
    }

    #[test]
    fn revoking_a_machine_stops_its_token() {
        let (_dir, state) = state();
        let pairing = state.add_machine("a1", "x:1", false).unwrap();
        state.revoke("a1").unwrap();
        assert_eq!(state.identify(&pairing.token).unwrap(), None);
        assert!(matches!(
            state.revoke("a1"),
            Err(AccessError::UnknownMachine(_))
        ));
    }

    #[test]
    fn refuses_a_duplicate_or_empty_name() {
        let (_dir, state) = state();
        state.add_machine("a1", "x:1", false).unwrap();
        assert!(matches!(
            state.add_machine("a1", "x:1", false),
            Err(AccessError::AlreadyPaired(_))
        ));
        assert!(matches!(
            state.add_machine(" ", "x:1", false),
            Err(AccessError::EmptyName)
        ));
    }

    #[test]
    fn refuses_a_name_the_ledger_would_refuse_as_a_credential() {
        let (_dir, state) = state();
        for name in [
            "x".repeat(65),
            "a\tb".to_owned(),
            "a\u{85}b".to_owned(),
            "a\u{200b}b".to_owned(),
            "a\u{202e}b".to_owned(),
        ] {
            assert!(
                matches!(
                    state.add_machine(&name, "x:1", false),
                    Err(AccessError::BadName)
                ),
                "{name:?} was accepted"
            );
        }
        assert!(state.add_machine(&"x".repeat(64), "x:1", false).is_ok());
    }

    #[test]
    fn an_empty_or_damaged_certificate_or_key_is_refused() {
        let real = {
            let (_dir, state) = state();
            state.certificate().unwrap();
            (
                std::fs::read_to_string(state.0.join(CERT_FILE)).unwrap(),
                std::fs::read_to_string(state.0.join(KEY_FILE)).unwrap(),
            )
        };
        let not_der = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
        for (cert, key) in [
            ("", real.1.as_str()),
            (not_der, real.1.as_str()),
            ("just text\n", real.1.as_str()),
            (real.0.as_str(), ""),
            (
                real.0.as_str(),
                "-----BEGIN PRIVATE KEY-----\n-----END PRIVATE KEY-----\n",
            ),
        ] {
            let (_dir, state) = state();
            write_private(&state.0.join(CERT_FILE), cert.as_bytes()).unwrap();
            write_private(&state.0.join(KEY_FILE), key.as_bytes()).unwrap();
            assert!(
                state.certificate().is_err(),
                "cert {:?}, key {:?} were accepted",
                cert.get(..20),
                key.get(..20)
            );
        }
    }

    #[test]
    fn the_certificate_stays_the_same_across_uses() {
        let (_dir, state) = state();
        let (_, _, first) = state.certificate().unwrap();
        let (_, _, second) = state.certificate().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn a_pair_code_round_trips_and_bad_ones_are_refused() {
        let pairing = Pairing {
            address: "192.0.2.1:6300".into(),
            fingerprint: "a".repeat(64),
            token: "secret".into(),
        };
        assert_eq!(Pairing::parse(&pairing.code()).unwrap(), pairing);
        for bad in [
            "",
            "nonsense",
            "rally-pair:v1/a/b",
            "rally-pair:v1/a/short/token",
            "rally-pair:v2/a/b/c",
        ] {
            assert!(matches!(Pairing::parse(bad), Err(AccessError::BadPairCode)));
        }
    }

    #[cfg(unix)]
    #[test]
    fn secrets_and_the_state_directory_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, state) = state();
        state.add_machine("a1", "x:1", false).unwrap();
        let mode = |name: &str| {
            let path = dir.path().join("state").join(name);
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777
        };
        assert_eq!(mode(""), 0o700);
        assert_eq!(mode("key.pem"), 0o600);
        assert_eq!(mode("machines.json"), 0o600);
    }

    #[test]
    fn losing_the_key_is_an_error_rather_than_a_silent_new_certificate() {
        let (dir, state) = state();
        let (_, _, first) = state.certificate().unwrap();
        std::fs::remove_file(dir.path().join("state/key.pem")).unwrap();
        assert!(matches!(
            state.certificate(),
            Err(AccessError::Certificate(_))
        ));
        let cert = std::fs::read_to_string(dir.path().join("state/cert.pem")).unwrap();
        assert_eq!(fingerprint_of_pem(&cert).unwrap(), first);
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_directory_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing");
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        StateDir::open(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn concurrent_changes_to_machines_are_not_lost() {
        let (dir, state) = state();
        state.certificate().unwrap();
        let path = dir.path().join("state");
        let workers: Vec<_> = (0..12)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let state = StateDir::open(&path).unwrap();
                    state.add_machine(&format!("m{i}"), "x:1", false).unwrap()
                })
            })
            .collect();
        let pairings: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        for pairing in pairings {
            assert!(state.identify(&pairing.token).unwrap().is_some());
        }
    }

    #[test]
    fn a_revoke_survives_a_concurrent_add() {
        let (dir, state) = state();
        state.certificate().unwrap();
        let path = dir.path().join("state");
        for round in 0..20 {
            let victim = format!("victim{round}");
            let pairing = state.add_machine(&victim, "x:1", false).unwrap();
            let adder = {
                let path = path.clone();
                std::thread::spawn(move || {
                    let state = StateDir::open(&path).unwrap();
                    state
                        .add_machine(&format!("new{round}"), "x:1", false)
                        .unwrap();
                })
            };
            state.revoke(&victim).unwrap();
            adder.join().unwrap();
            assert_eq!(
                state.identify(&pairing.token).unwrap(),
                None,
                "round {round}"
            );
        }
    }

    #[test]
    fn refuses_an_address_a_pair_code_cannot_carry() {
        let (_dir, state) = state();
        for address in ["", " ", "a/b:1", "a b:1"] {
            assert!(
                matches!(
                    state.add_machine("a1", address, false),
                    Err(AccessError::BadAddress)
                ),
                "{address:?}"
            );
        }
        assert_eq!(state.identify("x").unwrap(), None);
    }

    #[test]
    fn debug_output_never_shows_a_token() {
        let pairing = Pairing {
            address: "x:1".into(),
            fingerprint: "a".repeat(64),
            token: "supersecret".into(),
        };
        assert!(!format!("{pairing:?}").contains("supersecret"));
    }

    #[test]
    fn errors_never_show_a_token() {
        let code = "rally-pair:v1/a/too-short/supersecret";
        let error = Pairing::parse(code).unwrap_err().to_string();
        assert!(!error.contains("supersecret"));
    }
}
