//! Watchman designation (RAL-51): only the operator names the watchman.
//!
//! A designation is a ledger record carrying the operator's SSH signature, in the
//! `rally-designation` namespace, over [`designation_statement`]. The operator signs with a key
//! that asks for Touch ID on each use (Secretive's Secure Enclave keys), so an agent can ask for a
//! signature but can't make one. The service holds the operator's public key in a file only root
//! can change, and runs from a binary only root can change (`serve --secure`).
//!
//! The ledger's database can be written past the service, so a designation is checked again here
//! every time it's reported, not only when it's written: its signature against the operator's key,
//! and its place, that the record it names is the one just before it in the chain. A designation
//! that fails either is listed but not in force. What this can't see is a later designation
//! deleted from the database, which would revive an earlier one; the chain and `rally check
//! --expect` against a head kept elsewhere are what find that.

use std::path::Path;

use serde_json::{Value, json};
use ssh_key::{HashAlg, PublicKey, SshSig};

use crate::ledger::{Designation, Ledger, designation_statement};
use crate::store::CHAIN_START;

/// The SSH signature namespace for designations, as `ssh-keygen -Y sign -n` takes it.
pub const NAMESPACE: &str = "rally-designation";

/// The operator's public key, read from an OpenSSH public key file.
#[derive(Clone, Debug)]
pub struct OperatorKey {
    key: PublicKey,
}

impl OperatorKey {
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("the operator key {} can't be read: {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let line = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with('#'))
            .ok_or("the operator key file holds no key")?;
        let key = PublicKey::from_openssh(line)
            .map_err(|e| format!("the operator key isn't an OpenSSH public key: {e}"))?;
        Ok(Self { key })
    }

    /// As `ssh-keygen -l` prints it: `SHA256:` and base64.
    pub fn fingerprint(&self) -> String {
        self.key.fingerprint(HashAlg::Sha256).to_string()
    }

    /// Whether `signature` is this key's, in the designation namespace, over `statement`.
    pub fn verifies(&self, statement: &str, signature: &str) -> Result<(), String> {
        let signature =
            SshSig::from_pem(signature).map_err(|_| "the signature can't be read".to_string())?;
        if signature.public_key() != self.key.key_data() {
            return Err("it's signed by another key".into());
        }
        self.key
            .verify(NAMESPACE, statement.as_bytes(), &signature)
            .map_err(|_| "the signature doesn't verify".to_string())
    }
}

/// Checks a designation about to be written as record `seq`, after `previous` (its number and
/// chain hash): it must name that record and carry the operator's signature.
pub fn check_new(
    key: &OperatorKey,
    seq: u64,
    previous: (u64, &str),
    (watchman, credential): (Option<&str>, Option<&str>),
    after: (u64, &str),
    key_fingerprint: &str,
    signature: &str,
) -> Result<(), String> {
    if after != previous || seq != previous.0 + 1 {
        return Err(format!(
            "it names record {} but the ledger is at record {}; read the head again and sign again",
            after.0, previous.0
        ));
    }
    if key_fingerprint != key.fingerprint() {
        return Err(format!(
            "it names key {key_fingerprint}, not the operator's {}",
            key.fingerprint()
        ));
    }
    key.verifies(
        &designation_statement(watchman, credential, after.0, after.1),
        signature,
    )
}

/// Why one designation record isn't in force, or `None` if it holds.
fn fault(designation: &Designation, hashes: &[String], key: &OperatorKey) -> Option<String> {
    let named = match designation.after_seq {
        0 => Some(CHAIN_START.to_string()),
        n => hashes.get(n as usize - 1).cloned(),
    };
    if designation.seq != designation.after_seq + 1
        || named.as_deref() != Some(&designation.after_hash)
    {
        return Some("out of place: it doesn't name the record just before it".into());
    }
    if designation.key_fingerprint != key.fingerprint() {
        return Some("signed with a key that isn't the operator's".into());
    }
    key.verifies(&designation.statement(), &designation.signature)
        .err()
}

/// Who the watchman is, from `ledger` and its chain `hashes` (record `n` at `n - 1`), checked
/// against the operator's `key`. Without a key, nothing is in force and the report says why.
pub fn report(ledger: &Ledger, hashes: &[String], key: Option<&OperatorKey>) -> Value {
    let mut in_force: Option<&Designation> = None;
    let mut ended = false;
    let mut records = Vec::new();
    for designation in ledger.designations() {
        let fault = match key {
            Some(key) => fault(designation, hashes, key),
            None => Some("no operator key: the service wasn't started with --operator-key".into()),
        };
        if fault.is_none() {
            in_force = designation.watchman.is_some().then_some(designation);
            ended = designation.watchman.is_none();
        }
        records.push(json!({
            "seq": designation.seq,
            "watchman": designation.watchman,
            "credential": designation.credential,
            "at_ms": designation.at_ms,
            "key_fingerprint": designation.key_fingerprint,
            "status": match &fault {
                None => "valid".to_string(),
                Some(why) => format!("not in force: {why}"),
            },
        }));
    }
    json!({
        "watchman": in_force.map(|d| json!({
            "name": d.watchman,
            "credential": d.credential,
            "seq": d.seq,
            "at_ms": d.at_ms,
            "key_fingerprint": d.key_fingerprint,
        })),
        "ended": ended && in_force.is_none(),
        "operator_key": key.map(OperatorKey::fingerprint),
        "records": records,
        "limit": "A later designation deleted from the database would revive an earlier one. Rally \
                  doesn't record whether `rally check --expect` last passed, so run it against a \
                  head saved elsewhere to find a removal.",
    })
}

/// From which record each valid designation held: `(seq, watchman)`, oldest first, with `None`
/// after an `--end`. Without a key, there are none.
pub fn eras(
    ledger: &Ledger,
    hashes: &[String],
    key: Option<&OperatorKey>,
) -> Vec<(u64, Option<Watch>)> {
    let Some(key) = key else {
        return Vec::new();
    };
    ledger
        .designations()
        .iter()
        .filter(|d| fault(d, hashes, key).is_none())
        .map(|d| {
            let watch = d.watchman.clone().map(|name| Watch {
                name,
                credential: d.credential.clone(),
            });
            (d.seq, watch)
        })
        .collect()
}

/// A designated watchman, and under v2, the credential it's bound to (RAL-53).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watch {
    pub name: String,
    pub credential: Option<String>,
}

impl Watch {
    /// Whether a record from `credential`, declaring `declared`, is the watchman's. Under v2 only
    /// the bound credential counts, which the service sets from the connection, and the name it
    /// declares doesn't matter. Under v1, as before RAL-53, a matching declared name counts.
    pub fn wrote(&self, credential: &str, declared: Option<&str>) -> bool {
        match &self.credential {
            Some(bound) => credential == bound,
            None => declared.is_some_and(|declared| crate::ledger::same_name(declared, &self.name)),
        }
    }
}

/// The watchman in force when record `seq` was written, if any.
fn watchman_at(eras: &[(u64, Option<Watch>)], seq: u64) -> Option<&Watch> {
    eras.iter()
        .rev()
        .find(|(from, _)| *from < seq)
        .and_then(|(_, watch)| watch.as_ref())
}

/// Every verification and send-back, including by correction, newest first, each labelled by who
/// read while what was designated (RAL-51): `watchman` if the designated watchman wrote it,
/// `peer` if no watchman was designated, and `other` if one was but someone else wrote it. Names
/// are declared, so `watchman` means the record's declared name matched.
pub fn verifications(ledger: &Ledger, hashes: &[String], key: Option<&OperatorKey>) -> Value {
    use crate::ledger::{Change, State, same_name};
    let eras = eras(ledger, hashes, key);
    let mut rows: Vec<(u64, Value)> = Vec::new();
    for (id, story) in ledger.stories() {
        for entry in &story.history {
            let (verdict, commit) = match &entry.change {
                Change::Verify { commit, .. } => ("verified", commit),
                Change::SendBack { commit, .. } => ("sent_back", commit),
                Change::Correct {
                    state: Some(State::Verified),
                    commit,
                    ..
                } => ("verified", commit),
                Change::Correct {
                    state: Some(State::SentBack),
                    commit,
                    ..
                } => ("sent_back", commit),
                _ => continue,
            };
            let by = entry
                .backfilled
                .as_ref()
                .map(|b| b.entered_by.clone())
                .or_else(|| entry.source.declared_name.clone());
            let designated = watchman_at(&eras, entry.seq);
            let credential = entry.source.credential.as_str();
            let reader = match designated {
                // Without the operator's key, no designation can be checked, so who read can't be
                // said either; calling it a peer read would be a guess.
                _ if key.is_none() => "unknown",
                None => "peer",
                Some(watch) if watch.wrote(credential, by.as_deref()) => "watchman",
                Some(_) => "other",
            };
            let mut row = json!({
                "story": id, "seq": entry.seq, "at_ms": entry.at_ms, "verdict": verdict,
                "commit": commit, "by": by, "credential": credential, "reader": reader,
                "watchman_designated": designated.map(|watch| &watch.name),
            });
            if let Some(watch) = designated {
                row["matched_by"] = json!(match watch.credential {
                    Some(_) => "credential",
                    None => "declared name",
                });
                // The watchman's name, declared from another credential, under a bound designation.
                if reader == "other"
                    && watch.credential.is_some()
                    && by.as_deref().is_some_and(|by| same_name(by, &watch.name))
                {
                    row["note"] = json!(format!(
                        "declared the watchman's name from credential {credential}, not the \
                         watchman's"
                    ));
                }
            }
            rows.push((entry.seq, row));
        }
    }
    rows.sort_by_key(|(seq, _)| std::cmp::Reverse(*seq));
    let total = rows.len();
    json!({
        "verifications": rows.into_iter().take(crate::views::LIST_LIMIT).map(|(_, r)| r).collect::<Vec<_>>(),
        "total": total,
        "operator_key": key.map(OperatorKey::fingerprint),
        "readers_are": "watchman: the designated watchman wrote it, by the bound credential under a \
                        v2 designation, or by declared name under a v1 one (matched_by says which); \
                        peer: no watchman was designated; other: one was, and someone else wrote it; \
                        unknown: there's no operator key to check designations with. A declared name \
                        catches mistakes only. A credential is only as private as its pairing file: \
                        see docs/designation.md.",
    })
}

/// What a newly designated watchman re-reads (RAL-52): every story verified by a peer since the
/// designation before the current one, each with the commit read, who verified it and whether and
/// where it merged. Each stays "awaiting the watchman's re-read" until the watchman, by declared
/// name, records its own verification, send-back or correction of that story after it was
/// designated; the peer's record is never changed. With no watchman designated, the list is what
/// the next one would get. Nothing stands on a peer's verification alone.
pub fn handover(ledger: &Ledger, hashes: &[String], key: Option<&OperatorKey>) -> Value {
    use crate::ledger::{Change, State};
    let Some(key) = key else {
        return json!({
            "watchman": null, "operator_key": null, "stories": [],
            "note": "no operator key: designations can't be checked, so there's no handover list",
        });
    };
    let eras = eras(ledger, hashes, Some(key));
    // The current designation, if a watchman is in force, and where the window opens: just after
    // the valid designation before it, or the ledger's start.
    let (current, opens) = match eras.last() {
        Some((seq, Some(watch))) => (
            Some((*seq, watch.clone())),
            eras.iter().rev().nth(1).map_or(0, |(seq, _)| *seq),
        ),
        Some((seq, None)) => (None, *seq),
        None => (None, 0),
    };
    let closes = current.as_ref().map_or(u64::MAX, |(seq, _)| *seq);
    let mut stories = Vec::new();
    for (id, story) in ledger.stories() {
        // The newest peer verification in the window: made while nobody was designated.
        let peer = story.history.iter().rev().find(|entry| {
            entry.seq > opens
                && entry.seq < closes
                && watchman_at(&eras, entry.seq).is_none()
                && matches!(
                    entry.change,
                    Change::Verify { .. }
                        | Change::Correct {
                            state: Some(State::Verified),
                            ..
                        }
                )
        });
        let Some(peer) = peer else { continue };
        let commit = match &peer.change {
            Change::Verify { commit, .. } | Change::Correct { commit, .. } => commit.clone(),
            _ => None,
        };
        let by = peer
            .backfilled
            .as_ref()
            .map(|b| b.entered_by.clone())
            .or_else(|| peer.source.declared_name.clone());
        let merged = story
            .history
            .iter()
            .rev()
            .find_map(|entry| match &entry.change {
                Change::Merge { commit, .. } => Some(json!({"seq": entry.seq, "commit": commit})),
                _ => None,
            });
        let reread = current.as_ref().and_then(|(designated, watch)| {
            story.history.iter().find(|entry| {
                entry.seq > *designated
                    && watch.wrote(
                        &entry.source.credential,
                        entry.source.declared_name.as_deref(),
                    )
                    && matches!(
                        entry.change,
                        Change::Verify { .. } | Change::SendBack { .. } | Change::Correct { .. }
                    )
            })
        });
        let status = match (&current, reread) {
            (None, _) => "awaiting a watchman".to_string(),
            (Some(_), None) => "awaiting the watchman's re-read".to_string(),
            (Some(_), Some(entry)) => format!("re-read by the watchman at record {}", entry.seq),
        };
        stories.push(json!({
            "story": id, "verified_seq": peer.seq, "verified_at_ms": peer.at_ms,
            "commit": commit, "verified_by": by, "merged": merged, "state": story.state,
            "status": status,
        }));
    }
    let awaiting = stories
        .iter()
        .filter(|s| {
            s["status"]
                .as_str()
                .is_some_and(|t| t.starts_with("awaiting"))
        })
        .count();
    json!({
        "watchman": current.as_ref().map(|(seq, watch)| {
            json!({"name": watch.name, "credential": watch.credential, "seq": seq})
        }),
        "operator_key": key.fingerprint(),
        "since_record": opens,
        "stories": stories,
        "awaiting": awaiting,
        "rule": "Nothing stands on a peer's verification alone: the watchman re-reads each, merged or \
                 not, and keeps it, sends it back or asks for fixes. The peer's record stays.",
    })
}

/// What `serve --secure` requires of `path`: it, and every directory above it, owned by `uid`
/// (root, outside tests) and writable by no one else. Stops above `stop`, for tests only.
pub fn check_owned(path: &Path, uid: u32, stop: Option<&Path>) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let path = path
        .canonicalize()
        .map_err(|e| format!("{} can't be resolved: {e}", path.display()))?;
    for step in path.ancestors() {
        let meta = std::fs::metadata(step)
            .map_err(|e| format!("{} can't be read: {e}", step.display()))?;
        if meta.uid() != uid {
            return Err(format!(
                "{} is owned by uid {}, not {}",
                step.display(),
                meta.uid(),
                if uid == 0 {
                    "root".to_string()
                } else {
                    uid.to_string()
                }
            ));
        }
        if meta.mode() & 0o022 != 0 {
            return Err(format!(
                "{} can be written by its group or others (mode {:o})",
                step.display(),
                meta.mode() & 0o7777
            ));
        }
        if stop.is_some_and(|stop| step == stop) {
            break;
        }
    }
    Ok(())
}

/// The commands for the operator to make the binary and key root-owned, for `serve --secure`'s
/// refusal.
pub fn secure_steps(exe: &Path, key: &Path) -> String {
    format!(
        "As the operator, on this host:\n  \
         sudo install -o root -g root -m 0755 {exe} /usr/local/bin/rally\n  \
         sudo install -d -o root -g root -m 0755 /etc/rally\n  \
         sudo install -o root -g root -m 0644 {key} /etc/rally/operator.pub\n\
         then start the service as /usr/local/bin/rally serve --secure --operator-key \
         /etc/rally/operator.pub (see docs/designation.md).",
        exe = exe.display(),
        key = key.display()
    )
}

/// Keys and signatures made by the real `ssh-keygen`, for tests here and in the store.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::record::{Kind, Record, Source};
    use crate::store::chain_hash;
    use std::process::Command;

    /// An ed25519 key made by ssh-keygen in `dir`, and its public key.
    pub fn keypair(dir: &Path, name: &str) -> (std::path::PathBuf, OperatorKey) {
        let private = dir.join(name);
        let status = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "test", "-f"])
            .arg(&private)
            .status()
            .expect("ssh-keygen runs");
        assert!(status.success());
        let public = OperatorKey::read(&private.with_extension("pub")).unwrap();
        (private, public)
    }

    /// Builds a ledger from bodies, numbering from 1, with the chain hashes.
    pub struct Chain {
        pub records: Vec<Record>,
        pub hashes: Vec<String>,
    }

    impl Chain {
        pub fn new() -> Self {
            Self {
                records: Vec::new(),
                hashes: Vec::new(),
            }
        }

        pub fn head(&self) -> (u64, String) {
            (
                self.records.len() as u64,
                self.hashes
                    .last()
                    .cloned()
                    .unwrap_or_else(|| CHAIN_START.to_string()),
            )
        }

        pub fn push(&mut self, body: Value) {
            self.push_as("operator", body);
        }

        pub fn push_as(&mut self, name: &str, body: Value) {
            self.push_from("a1", name, body);
        }

        /// A record from `credential`, as the service would set it from the connection.
        pub fn push_from(&mut self, credential: &str, name: &str, body: Value) {
            let source = Source {
                credential: credential.into(),
                declared_name: Some(name.into()),
            };
            // A verifier's record is of the verified kind, as the ledger wants.
            let kind = match body["change"].as_str() {
                Some("verify" | "send_back") => Kind::Verified,
                _ => Kind::Declared,
            };
            let record = Record::new(kind, source, 1_000, body).unwrap();
            let (seq, previous) = self.head();
            self.hashes
                .push(chain_hash(&previous, seq + 1, &record.to_json()));
            self.records.push(record);
        }

        pub fn designate(&mut self, private: &Path, key: &OperatorKey, watchman: Option<&str>) {
            let (seq, hash) = self.head();
            let statement = designation_statement(watchman, None, seq, &hash);
            self.push(json!({
                "change": "designate", "watchman": watchman, "after_seq": seq, "after_hash": hash,
                "key_fingerprint": key.fingerprint(), "signature": sign(private, &statement),
            }));
        }

        /// A v2 designation, bound to `credential` (RAL-53).
        pub fn designate_bound(
            &mut self,
            private: &Path,
            key: &OperatorKey,
            watchman: &str,
            credential: &str,
        ) {
            let (seq, hash) = self.head();
            let statement = designation_statement(Some(watchman), Some(credential), seq, &hash);
            self.push(json!({
                "change": "designate", "watchman": watchman, "credential": credential,
                "after_seq": seq, "after_hash": hash,
                "key_fingerprint": key.fingerprint(), "signature": sign(private, &statement),
            }));
        }

        pub fn report(&self, key: Option<&OperatorKey>) -> Value {
            report(&Ledger::replay(&self.records).unwrap(), &self.hashes, key)
        }
    }

    /// `ssh-keygen -Y sign` over `statement`, as the operator would.
    pub fn sign(private: &Path, statement: &str) -> String {
        use std::io::Write;
        let mut child = Command::new("ssh-keygen")
            .args(["-q", "-Y", "sign", "-n", NAMESPACE, "-f"])
            .arg(private)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(statement.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Chain, keypair, sign};
    use super::*;
    use crate::record::{Kind, Record, Source};

    /// Submits RAL-1 for review, by team rally, from a1.
    fn submitted_one(chain: &mut Chain) {
        chain.push_as("r", json!({"change": "ready", "story": "RAL-1"}));
        chain.push_as(
            "r",
            json!({"change": "claim", "story": "RAL-1", "team": "rally"}),
        );
        chain.push_as(
            "r",
            json!({"change": "submit", "story": "RAL-1", "team": "rally", "evidence": "e"}),
        );
    }

    fn read(by_credential: &str, name: &str) -> (String, String, Value) {
        (
            by_credential.into(),
            name.into(),
            json!({"change": "verify", "story": "RAL-1", "note": "read it",
                   "commit": "a".repeat(40)}),
        )
    }

    #[test]
    fn under_a_bound_designation_only_the_watchmans_credential_reads_as_the_watchman() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        submitted_one(&mut chain);
        chain.designate_bound(&private, &key, "grand-photon", "a1-watchman");
        let report = chain.report(Some(&key));
        assert_eq!(report["watchman"]["credential"], "a1-watchman");
        assert_eq!(report["records"][0]["status"], "valid");

        // Another session on a1 declaring the watchman's name.
        let (credential, name, body) = read("a1", "grand-photon");
        let mut impostor = Chain {
            records: chain.records.clone(),
            hashes: chain.hashes.clone(),
        };
        impostor.push_from(&credential, &name, body);
        let ledger = Ledger::replay(&impostor.records).unwrap();
        let rows = verifications(&ledger, &impostor.hashes, Some(&key));
        let row = &rows["verifications"][0];
        assert_eq!(row["reader"], "other", "{row}");
        assert_eq!(row["matched_by"], "credential");
        assert!(row["note"].as_str().unwrap().contains("from credential a1"));

        // The watchman's own credential, whatever name it declares.
        let (credential, name, body) = read("a1-watchman", "gp");
        chain.push_from(&credential, &name, body);
        let ledger = Ledger::replay(&chain.records).unwrap();
        let rows = verifications(&ledger, &chain.hashes, Some(&key));
        assert_eq!(rows["verifications"][0]["reader"], "watchman");
        assert_eq!(rows["verifications"][0]["credential"], "a1-watchman");
    }

    #[test]
    fn a_v1_designation_still_matches_by_declared_name() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        submitted_one(&mut chain);
        chain.designate(&private, &key, Some("grand-photon"));
        let (credential, name, body) = read("a1", "grand-photon");
        chain.push_from(&credential, &name, body);
        let ledger = Ledger::replay(&chain.records).unwrap();
        let rows = verifications(&ledger, &chain.hashes, Some(&key));
        assert_eq!(rows["verifications"][0]["reader"], "watchman");
        assert_eq!(rows["verifications"][0]["matched_by"], "declared name");
        assert!(rows["verifications"][0].get("note").is_none());
    }

    #[test]
    fn a_bound_designation_signs_its_credential_and_handover_counts_only_it() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        // The statement names the credential, so a signature of v1 text doesn't hold for v2.
        let hash = "a".repeat(64);
        assert_eq!(
            designation_statement(Some("w"), Some("a1-w"), 3, &hash),
            format!("rally-designation v2\nwatchman: w\ncredential: a1-w\nafter: 3:{hash}\n")
        );
        let v1 = sign(&private, &designation_statement(Some("w"), None, 3, &hash));
        assert!(
            check_new(
                &key,
                4,
                (3, &hash),
                (Some("w"), Some("a1-w")),
                (3, &hash),
                &key.fingerprint(),
                &v1
            )
            .is_err()
        );

        // A peer verifies with no watchman; then a bound watchman is designated.
        let mut chain = Chain::new();
        submitted_one(&mut chain);
        let (credential, name, body) = read("muster-box", "peer");
        chain.push_from(&credential, &name, body);
        chain.designate_bound(&private, &key, "grand-photon", "a1-watchman");
        // A re-read declared as the watchman from a1 doesn't count.
        chain.push_from(
            "a1",
            "grand-photon",
            json!({"change": "correct", "story": "RAL-1", "corrects": 4, "note": "re-read"}),
        );
        let ledger = Ledger::replay(&chain.records).unwrap();
        let list = handover(&ledger, &chain.hashes, Some(&key));
        assert_eq!(
            list["stories"][0]["status"], "awaiting the watchman's re-read",
            "{list}"
        );
        // From the bound credential, it does.
        chain.push_from(
            "a1-watchman",
            "grand-photon",
            json!({"change": "correct", "story": "RAL-1", "corrects": 4, "note": "re-read, kept"}),
        );
        let ledger = Ledger::replay(&chain.records).unwrap();
        let list = handover(&ledger, &chain.hashes, Some(&key));
        assert!(
            list["stories"][0]["status"]
                .as_str()
                .unwrap()
                .starts_with("re-read by the watchman"),
            "{list}"
        );
        assert_eq!(list["watchman"]["credential"], "a1-watchman");
    }

    #[test]
    fn an_end_binds_no_credential_and_a_credential_is_plain_text() {
        let hash = "a".repeat(64);
        for (watchman, credential) in [(None, "a1-w"), (Some("w"), "a1 w"), (Some("w"), "")] {
            let body = json!({
                "change": "designate", "watchman": watchman, "credential": credential,
                "after_seq": 0, "after_hash": hash, "key_fingerprint": "SHA256:abc",
                "signature": "-----BEGIN SSH SIGNATURE-----\nx\n-----END SSH SIGNATURE-----\n",
            });
            let source = Source {
                credential: "a1".into(),
                declared_name: Some("operator".into()),
            };
            let record = Record::new(Kind::Declared, source, 1_000, body).unwrap();
            assert!(
                Ledger::replay(&[record]).is_err(),
                "{watchman:?} {credential:?}"
            );
        }
    }

    #[test]
    fn a_signed_designation_is_in_force_and_an_end_ends_it() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        chain.push(json!({"change": "ready", "story": "RAL-1"}));
        chain.designate(&private, &key, Some("grand-photon"));
        let report = chain.report(Some(&key));
        assert_eq!(report["watchman"]["name"], "grand-photon");
        assert_eq!(report["watchman"]["seq"], 2);
        assert_eq!(report["operator_key"], key.fingerprint());
        assert_eq!(report["records"][0]["status"], "valid");

        chain.designate(&private, &key, None);
        let report = chain.report(Some(&key));
        assert!(report["watchman"].is_null());
        assert_eq!(report["ended"], true);
        assert!(report["limit"].as_str().unwrap().contains("check --expect"));
    }

    #[test]
    fn another_key_or_no_key_puts_nothing_in_force() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let (stranger, stranger_key) = keypair(dir.path(), "stranger");
        let mut chain = Chain::new();
        chain.designate(&stranger, &stranger_key, Some("impostor"));
        let report = chain.report(Some(&key));
        assert!(report["watchman"].is_null());
        assert!(
            report["records"][0]["status"]
                .as_str()
                .unwrap()
                .contains("isn't the operator's")
        );

        // A stranger's signature under the operator's fingerprint doesn't verify either.
        let (seq, hash) = chain.head();
        let statement = designation_statement(Some("impostor"), None, seq, &hash);
        chain.push(json!({
            "change": "designate", "watchman": "impostor", "after_seq": seq, "after_hash": hash,
            "key_fingerprint": key.fingerprint(), "signature": sign(&stranger, &statement),
        }));
        let report = chain.report(Some(&key));
        assert!(report["watchman"].is_null());
        assert!(
            report["records"][1]["status"]
                .as_str()
                .unwrap()
                .contains("another key")
        );

        chain.designate(&private, &key, Some("real"));
        assert!(chain.report(None)["watchman"].is_null());
        assert!(
            chain.report(None)["records"][2]["status"]
                .as_str()
                .unwrap()
                .contains("no operator key")
        );
        assert_eq!(chain.report(Some(&key))["watchman"]["name"], "real");
    }

    #[test]
    fn a_copied_designation_placed_later_is_out_of_place() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        chain.designate(&private, &key, Some("first"));
        chain.designate(&private, &key, None);
        // A byte-for-byte copy of record 1's body can't even be applied later: the ledger wants
        // it to name the record just before.
        let copy = chain.records[0].body().clone();
        let source = Source {
            credential: "a1".into(),
            declared_name: Some("operator".into()),
        };
        let mut records = chain.records.clone();
        records.push(Record::new(Kind::Declared, source, 1_000, copy).unwrap());
        assert!(Ledger::replay(&records).is_err());

        // Written straight into the database with its numbers adjusted, the signature no longer
        // covers it; with the right numbers but the wrong hash, it's out of place.
        let (seq, _) = chain.head();
        let first = &Ledger::replay(&chain.records).unwrap().designations()[0].clone();
        chain.push(json!({
            "change": "designate", "watchman": "first", "after_seq": seq,
            "after_hash": first.after_hash, "key_fingerprint": first.key_fingerprint,
            "signature": first.signature,
        }));
        let report = chain.report(Some(&key));
        assert!(report["watchman"].is_null(), "{report}");
        assert!(
            report["records"][2]["status"]
                .as_str()
                .unwrap()
                .contains("out of place")
        );
    }

    /// Readies, claims and submits `story` as team `t`, for someone else to read.
    fn submitted(chain: &mut Chain, story: &str) {
        chain.push_as("t", json!({"change": "ready", "story": story}));
        chain.push_as("t", json!({"change": "claim", "story": story, "team": "t"}));
        chain.push_as(
            "t",
            json!({"change": "submit", "story": story, "team": "t", "evidence": "repo@abc"}),
        );
    }

    fn verify(chain: &mut Chain, by: &str, story: &str) {
        chain.push_as(
            by,
            json!({"change": "verify", "story": story, "note": "read it", "commit": "c".repeat(40)}),
        );
    }

    #[test]
    fn each_verification_is_labelled_by_the_designation_when_it_was_written() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        submitted(&mut chain, "RAL-1");
        verify(&mut chain, "peer-a", "RAL-1");
        chain.designate(&private, &key, Some("Grand-Photon"));
        submitted(&mut chain, "RAL-2");
        verify(&mut chain, "grand-photon", "RAL-2");
        submitted(&mut chain, "RAL-3");
        chain.push_as(
            "someone",
            json!({"change": "send_back", "story": "RAL-3", "note": "no tests"}),
        );
        chain.designate(&private, &key, None);
        submitted(&mut chain, "RAL-4");
        verify(&mut chain, "peer-b", "RAL-4");

        let ledger = Ledger::replay(&chain.records).unwrap();
        let report = verifications(&ledger, &chain.hashes, Some(&key));
        let rows = report["verifications"].as_array().unwrap();
        let by_story = |story: &str| rows.iter().find(|r| r["story"] == story).unwrap().clone();
        assert_eq!(by_story("RAL-1")["reader"], "peer");
        assert_eq!(
            by_story("RAL-2")["reader"],
            "watchman",
            "names compare case-blind"
        );
        assert_eq!(by_story("RAL-2")["watchman_designated"], "Grand-Photon");
        assert_eq!(by_story("RAL-3")["reader"], "other");
        assert_eq!(by_story("RAL-3")["verdict"], "sent_back");
        assert_eq!(by_story("RAL-4")["reader"], "peer", "after --end");
        assert_eq!(report["total"], 4);
        assert_eq!(rows[0]["story"], "RAL-4", "newest first");

        // Without the key, who read can't be said.
        let blind = verifications(&ledger, &chain.hashes, None);
        assert!(
            blind["verifications"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["reader"] == "unknown")
        );

        // A designation that doesn't verify doesn't make anyone the watchman.
        let (_, stranger) = keypair(dir.path(), "stranger");
        let wrong = verifications(&ledger, &chain.hashes, Some(&stranger));
        let rows = wrong["verifications"].as_array().unwrap();
        assert!(rows.iter().all(|r| r["reader"] == "peer"));
    }

    #[test]
    fn a_new_watchman_gets_the_peer_verified_work_until_it_rereads_each() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        // Under a first watchman: not handed over.
        chain.designate(&private, &key, Some("first"));
        submitted(&mut chain, "RAL-1");
        verify(&mut chain, "first", "RAL-1");
        chain.designate(&private, &key, None);
        // With no watchman: two peer verifications, one merged.
        submitted(&mut chain, "RAL-2");
        verify(&mut chain, "peer-a", "RAL-2");
        let merge = "e".repeat(40);
        chain.push_as(
            "t",
            json!({"change": "merge", "story": "RAL-2", "commit": merge}),
        );
        submitted(&mut chain, "RAL-3");
        verify(&mut chain, "peer-b", "RAL-3");
        let pending = handover(
            &Ledger::replay(&chain.records).unwrap(),
            &chain.hashes,
            Some(&key),
        );
        assert!(pending["watchman"].is_null());
        assert_eq!(pending["stories"][0]["status"], "awaiting a watchman");

        // A watchman is designated partway through.
        chain.designate(&private, &key, Some("second"));
        let ledger = Ledger::replay(&chain.records).unwrap();
        let out = handover(&ledger, &chain.hashes, Some(&key));
        assert_eq!(out["watchman"]["name"], "second");
        let stories: Vec<&str> = out["stories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["story"].as_str().unwrap())
            .collect();
        assert_eq!(
            stories,
            ["RAL-2", "RAL-3"],
            "not RAL-1, read under a watchman"
        );
        let row = |out: &Value, story: &str| {
            out["stories"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["story"] == story)
                .unwrap()
                .clone()
        };
        assert_eq!(row(&out, "RAL-2")["merged"]["commit"], merge.as_str());
        assert_eq!(row(&out, "RAL-2")["verified_by"], "peer-a");
        assert_eq!(row(&out, "RAL-2")["commit"], "c".repeat(40));
        assert!(row(&out, "RAL-3")["merged"].is_null());
        assert_eq!(
            row(&out, "RAL-3")["status"],
            "awaiting the watchman's re-read"
        );
        assert_eq!(out["awaiting"], 2);

        // The watchman re-reads RAL-2 with a correction, and it leaves the list's awaiting count.
        let verified_seq = row(&out, "RAL-2")["verified_seq"].as_u64().unwrap();
        chain.push_as(
            "Second",
            json!({"change": "correct", "story": "RAL-2", "corrects": verified_seq,
                   "note": "re-read at the merged commit: keeps"}),
        );
        // Someone else's correction of RAL-3 isn't the watchman's re-read.
        let verified_3 = row(&out, "RAL-3")["verified_seq"].as_u64().unwrap();
        chain.push_as(
            "peer-a",
            json!({"change": "correct", "story": "RAL-3", "corrects": verified_3, "note": "fine"}),
        );
        let ledger = Ledger::replay(&chain.records).unwrap();
        let out = handover(&ledger, &chain.hashes, Some(&key));
        assert!(
            row(&out, "RAL-2")["status"]
                .as_str()
                .unwrap()
                .starts_with("re-read by the watchman")
        );
        assert_eq!(
            row(&out, "RAL-3")["status"],
            "awaiting the watchman's re-read"
        );
        assert_eq!(out["awaiting"], 1);
        // The peer's verification is still the story's record.
        assert_eq!(
            ledger.story("RAL-2").unwrap().state,
            crate::ledger::State::Verified
        );

        // Without the key there's no list.
        assert_eq!(
            handover(&ledger, &chain.hashes, None)["stories"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn a_tampered_hash_list_makes_a_designation_out_of_place() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        chain.push(json!({"change": "ready", "story": "RAL-1"}));
        chain.designate(&private, &key, Some("w"));
        let ledger = Ledger::replay(&chain.records).unwrap();
        let mut hashes = chain.hashes.clone();
        hashes[0] = "f".repeat(64);
        let report = report(&ledger, &hashes, Some(&key));
        assert!(report["watchman"].is_null());
        assert!(
            report["records"][0]["status"]
                .as_str()
                .unwrap()
                .contains("out of place")
        );
    }

    #[test]
    fn check_new_wants_the_current_head_and_the_operators_signature() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let (stranger, _) = keypair(dir.path(), "stranger");
        let hash = "a".repeat(64);
        let statement = designation_statement(Some("w"), None, 4, &hash);
        let good = sign(&private, &statement);
        let fp = key.fingerprint();
        assert!(
            check_new(
                &key,
                5,
                (4, &hash),
                (Some("w"), None),
                (4, &hash),
                &fp,
                &good
            )
            .is_ok()
        );
        let stale = check_new(
            &key,
            6,
            (5, &"b".repeat(64)),
            (Some("w"), None),
            (4, &hash),
            &fp,
            &good,
        );
        assert!(stale.unwrap_err().contains("read the head again"));
        let other = check_new(
            &key,
            5,
            (4, &hash),
            (Some("x"), None),
            (4, &hash),
            &fp,
            &good,
        );
        assert!(other.unwrap_err().contains("doesn't verify"));
        let theirs = sign(&stranger, &statement);
        let theirs = check_new(
            &key,
            5,
            (4, &hash),
            (Some("w"), None),
            (4, &hash),
            &fp,
            &theirs,
        );
        assert!(theirs.unwrap_err().contains("another key"));
        let wrong_fp = check_new(
            &key,
            5,
            (4, &hash),
            (Some("w"), None),
            (4, &hash),
            "SHA256:x",
            &good,
        );
        assert!(wrong_fp.unwrap_err().contains("not the operator's"));
    }

    #[test]
    fn a_secret_like_check_doesnt_refuse_a_signature() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        chain.designate(&private, &key, Some("w"));
        let ledger = Ledger::replay(&[]).unwrap();
        ledger.check(1, &chain.records[0]).unwrap();
    }

    #[test]
    fn check_owned_wants_the_owner_and_no_group_or_other_write() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = root.join("operator.pub");
        std::fs::write(&file, "key").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let me = std::fs::metadata(&file).unwrap().uid();
        assert!(check_owned(&file, me, Some(&root)).is_ok());
        let not_me = check_owned(&file, me + 1, Some(&root)).unwrap_err();
        assert!(not_me.contains("is owned by uid"), "{not_me}");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o664)).unwrap();
        let writable = check_owned(&file, me, Some(&root)).unwrap_err();
        assert!(writable.contains("group or others"), "{writable}");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();
        let dir_writable = check_owned(&file, me, Some(&root)).unwrap_err();
        assert!(
            dir_writable.contains(&root.display().to_string()),
            "{dir_writable}"
        );
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Without a stop, the check goes up to /, which a test's user doesn't own.
        assert!(check_owned(&file, me, None).is_err());
    }

    #[test]
    fn a_name_that_could_change_the_statement_is_refused() {
        let hash = "a".repeat(64);
        let long = "x".repeat(65);
        // Look-alikes of a real name too (RAL-41): a zero-width space, a right-to-left override, a
        // word joiner, and a Cyrillic а. The name is ASCII only, so the signed statement shows the
        // operator exactly the name a verification is matched against.
        let names = [
            "a\nafter: 9:x",
            "none",
            "",
            "x y",
            &long,
            "grand\u{200b}photon",
            "\u{202e}grand-photon",
            "grand-photon\u{2060}",
            "gr\u{430}nd-photon",
        ];
        for name in names {
            let body = json!({
                "change": "designate", "watchman": name, "after_seq": 0, "after_hash": hash,
                "key_fingerprint": "SHA256:abc",
                "signature": "-----BEGIN SSH SIGNATURE-----\nx\n-----END SSH SIGNATURE-----\n",
            });
            let source = Source {
                credential: "a1".into(),
                declared_name: Some("operator".into()),
            };
            let record = Record::new(Kind::Declared, source, 1_000, body).unwrap();
            assert!(Ledger::replay(&[record]).is_err(), "{name:?}");
        }
    }
}
