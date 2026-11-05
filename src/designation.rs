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

#[cfg(test)]
use crate::ledger::designation_statement;
use crate::ledger::{DesignateAction, Designation, Ledger, Terms, same_name};
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
/// chain hash): it must name that record and carry the operator's signature over its terms.
pub fn check_new(
    key: &OperatorKey,
    seq: u64,
    previous: (u64, &str),
    terms: Terms<'_>,
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
    key.verifies(&terms.statement(after.0, after.1), signature)
}

/// Checks a v3 designation about to be written against the watchmen in force (RAL-63): an add
/// can't bind a credential another watchman in force is bound to, so a read's label is never
/// ambiguous, and an end must name a watchman in force. v1 and v2 replace the whole set, so they
/// pass.
pub fn check_against_set(
    ledger: &Ledger,
    hashes: &[String],
    key: &OperatorKey,
    terms: Terms<'_>,
) -> Result<(), String> {
    let set = in_force(ledger, hashes, Some(key));
    change_set(&set, terms).map(|_| ())
}

/// The set after a valid designation with `terms`, from `set`, and the names of any watchmen it
/// ended as a side effect, or why it can't apply.
fn change_set(set: &[Watch], terms: Terms<'_>) -> Result<(Vec<Watch>, Vec<String>), String> {
    let Some(action) = terms.action else {
        // v1 and v2: the designation replaces every watchman, or ends them all.
        return Ok((
            terms
                .watchman
                .map(|name| {
                    vec![Watch {
                        name: name.to_owned(),
                        credential: terms.credential.map(str::to_owned),
                        scope: Scope::All,
                        seq: 0,
                    }]
                })
                .unwrap_or_default(),
            Vec::new(),
        ));
    };
    let name = terms.watchman.unwrap_or_default();
    let mut next: Vec<Watch> = set.to_vec();
    let mut dropped = Vec::new();
    match action {
        DesignateAction::Add => {
            let credential = terms.credential.unwrap_or_default();
            if let Some(other) = set
                .iter()
                .find(|w| !same_name(&w.name, name) && w.credential.as_deref() == Some(credential))
            {
                return Err(format!(
                    "credential {credential} is already bound to {}, another watchman in force",
                    other.name
                ));
            }
            next.retain(|w| !same_name(&w.name, name));
            // A watchman with no credential, from a v1 designation, is matched by declared name,
            // which any session can declare. Beside a bound one it would let any session label
            // its reads as that watchman's, so an add ends it, and the report says so.
            next.retain(|w| {
                if w.credential.is_none() {
                    dropped.push(w.name.clone());
                    return false;
                }
                true
            });
            next.push(Watch {
                name: name.to_owned(),
                credential: Some(credential.to_owned()),
                scope: Scope::parse(terms.scope.unwrap_or("all")).unwrap_or(Scope::All),
                seq: 0,
            });
        }
        DesignateAction::End => {
            if !set.iter().any(|w| same_name(&w.name, name)) {
                return Err(format!(
                    "{name} isn't a watchman in force, so there's nothing to end"
                ));
            }
            next.retain(|w| !same_name(&w.name, name));
        }
    }
    Ok((next, dropped))
}

/// Why one designation record isn't in force by its signature and place, or `None` if it holds.
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

/// One designation as replayed: the record, why it isn't in force (if it isn't), the watchmen in
/// force after it, and any it ended as a side effect.
type Step<'a> = (&'a Designation, Option<String>, Vec<Watch>, Vec<String>);

/// Each designation record, oldest first, with why it isn't in force if it isn't, and the set of
/// watchmen in force after it. Without a key, none is.
fn walk<'a>(ledger: &'a Ledger, hashes: &[String], key: Option<&OperatorKey>) -> Vec<Step<'a>> {
    let mut set: Vec<Watch> = Vec::new();
    let mut steps = Vec::new();
    for designation in ledger.designations() {
        let fault = match key {
            Some(key) => fault(designation, hashes, key),
            None => Some("no operator key: the service wasn't started with --operator-key".into()),
        };
        let mut dropped = Vec::new();
        let fault = match fault {
            Some(why) => Some(why),
            None => match change_set(&set, designation.terms()) {
                Ok((mut next, ended)) => {
                    dropped = ended;
                    // The one this record names, added here.
                    for watch in &mut next {
                        if watch.seq == 0 {
                            watch.seq = designation.seq;
                        }
                    }
                    set = next;
                    None
                }
                Err(why) => Some(why),
            },
        };
        steps.push((designation, fault, set.clone(), dropped));
    }
    steps
}

/// The watchmen in force now, oldest designation first.
pub fn in_force(ledger: &Ledger, hashes: &[String], key: Option<&OperatorKey>) -> Vec<Watch> {
    walk(ledger, hashes, key)
        .pop()
        .map(|(_, _, set, _)| set)
        .unwrap_or_default()
}

/// Who the watchmen are, from `ledger` and its chain `hashes` (record `n` at `n - 1`), checked
/// against the operator's `key`. Without a key, nothing is in force and the report says why.
/// `watchmen` lists every watchman in force with its scope (RAL-63); `watchman` is the one in
/// force when there's exactly one, as before.
pub fn report(ledger: &Ledger, hashes: &[String], key: Option<&OperatorKey>) -> Value {
    let steps = walk(ledger, hashes, key);
    let mut records = Vec::new();
    let mut any_valid = false;
    for (designation, fault, _, dropped) in &steps {
        any_valid |= fault.is_none();
        let mut row = json!({
            "seq": designation.seq,
            "watchman": designation.watchman,
            "credential": designation.credential,
            "action": designation.action,
            "scope": designation.scope,
            "at_ms": designation.at_ms,
            "key_fingerprint": designation.key_fingerprint,
            "status": match fault {
                None => "valid".to_string(),
                Some(why) => format!("not in force: {why}"),
            },
        });
        if !dropped.is_empty() {
            row["ended"] = json!(dropped);
            row["note"] = json!(format!(
                "this add ended {}, designated without a credential, so a declared name can't \
                 pass for a watchman beside a bound one",
                dropped.join(", ")
            ));
        }
        records.push(row);
    }
    let set = steps
        .last()
        .map(|(_, _, set, _)| set.clone())
        .unwrap_or_default();
    let shown: Vec<Value> = set
        .iter()
        .map(|watch| {
            let designation = ledger.designations().iter().find(|d| d.seq == watch.seq);
            json!({
                "name": watch.name,
                "credential": watch.credential,
                "scope": watch.scope.as_text(),
                "seq": watch.seq,
                "at_ms": designation.map(|d| d.at_ms),
                "key_fingerprint": designation.map(|d| &d.key_fingerprint),
            })
        })
        .collect();
    json!({
        "watchman": if shown.len() == 1 { shown[0].clone() } else { Value::Null },
        "watchmen": shown,
        "ended": any_valid && set.is_empty(),
        "operator_key": key.map(OperatorKey::fingerprint),
        "records": records,
        "scope_is": "A scope only decides whose read a merge waits for: all, team:<team> for \
                     stories that team holds, or machine:<credential> for submissions made with \
                     that pairing credential. A watchman's read outside its scope is still the \
                     watchman's.",
        "limit": "A later designation deleted from the database would revive an earlier one. Rally \
                  doesn't record whether `rally check --expect` last passed, so run it against a \
                  head saved elsewhere to find a removal.",
    })
}

/// From which record each set of watchmen held: `(seq, watchmen)`, oldest first, one for each
/// valid designation, with an empty set after an end of every watchman. Without a key, there are
/// none.
pub fn eras(
    ledger: &Ledger,
    hashes: &[String],
    key: Option<&OperatorKey>,
) -> Vec<(u64, Vec<Watch>)> {
    if key.is_none() {
        return Vec::new();
    }
    walk(ledger, hashes, key)
        .into_iter()
        .filter(|(_, fault, _, _)| fault.is_none())
        .map(|(designation, _, set, _)| (designation.seq, set))
        .collect()
}

/// Whose reads a merge waits for (RAL-63).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every story.
    All,
    /// Stories this team holds.
    Team(String),
    /// Submissions made with this pairing credential, which names a machine.
    Machine(String),
}

impl Scope {
    pub fn parse(text: &str) -> Option<Self> {
        if !crate::ledger::is_scope(text) {
            return None;
        }
        Some(match text.split_once(':') {
            None => Self::All,
            Some(("team", name)) => Self::Team(name.to_owned()),
            Some((_, name)) => Self::Machine(name.to_owned()),
        })
    }

    pub fn as_text(&self) -> String {
        match self {
            Self::All => "all".into(),
            Self::Team(team) => format!("team:{team}"),
            Self::Machine(credential) => format!("machine:{credential}"),
        }
    }

    /// Whether it covers a story held by `team`, submitted with `credential`.
    pub fn covers(&self, team: Option<&str>, credential: Option<&str>) -> bool {
        match self {
            Self::All => true,
            Self::Team(scope) => team.is_some_and(|team| same_name(team, scope)),
            Self::Machine(scope) => credential == Some(scope.as_str()),
        }
    }
}

/// A designated watchman: under v2 and v3, the credential it's bound to (RAL-53), and under v3,
/// its scope (RAL-63). `seq` is the designation that put it in force.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watch {
    pub name: String,
    pub credential: Option<String>,
    pub scope: Scope,
    pub seq: u64,
}

impl Watch {
    /// Whether a record from `credential`, declaring `declared`, is the watchman's. Under v2 and
    /// v3 only the bound credential counts, which the service sets from the connection, and the
    /// name it declares doesn't matter. Under v1, as before RAL-53, a matching declared name counts.
    pub fn wrote(&self, credential: &str, declared: Option<&str>) -> bool {
        match &self.credential {
            Some(bound) => credential == bound,
            None => declared.is_some_and(|declared| same_name(declared, &self.name)),
        }
    }
}

/// The watchmen in force when record `seq` was written.
fn watchmen_at(eras: &[(u64, Vec<Watch>)], seq: u64) -> &[Watch] {
    eras.iter()
        .rev()
        .find(|(from, _)| *from < seq)
        .map_or(&[], |(_, set)| set.as_slice())
}

/// The team holding `story` and the credential its latest submission came from, which scopes are
/// matched against.
fn coverage(story: &crate::ledger::Story) -> (Option<&str>, Option<&str>) {
    let submitted = story
        .history
        .iter()
        .rev()
        .find(|entry| matches!(entry.change, crate::ledger::Change::Submit { .. }))
        .map(|entry| entry.source.credential.as_str());
    (story.team.as_deref(), submitted)
}

/// Every verification and send-back, including by correction, newest first, each labelled by who
/// read while whom was designated (RAL-51, RAL-63): `watchman` if any watchman in force made it,
/// by its bound credential (or under v1 its declared name), with `watchman_name` saying which;
/// `peer` if no watchman was designated; and `other` if one was but someone else wrote it. Scope
/// doesn't change the label: it only decides whose read a merge waits for.
pub fn verifications(ledger: &Ledger, hashes: &[String], key: Option<&OperatorKey>) -> Value {
    use crate::ledger::{Change, State};
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
            let designated = watchmen_at(&eras, entry.seq);
            let credential = entry.source.credential.as_str();
            let reading = designated
                .iter()
                .find(|watch| watch.wrote(credential, by.as_deref()));
            let reader = match (designated.is_empty(), reading) {
                // Without the operator's key, no designation can be checked, so who read can't be
                // said either; calling it a peer read would be a guess.
                _ if key.is_none() => "unknown",
                (true, _) => "peer",
                (false, Some(_)) => "watchman",
                (false, None) => "other",
            };
            let mut row = json!({
                "story": id, "seq": entry.seq, "at_ms": entry.at_ms, "verdict": verdict,
                "commit": commit, "by": by, "credential": credential, "reader": reader,
                "watchmen_designated": designated.iter().map(|w| &w.name).collect::<Vec<_>>(),
            });
            // How the read was matched: by the reading watchman's binding, or for another's read,
            // by how the watchmen in force are matched.
            let bound = |watch: &Watch| watch.credential.is_some();
            let matched_by = match reading {
                Some(watch) if bound(watch) => Some("credential"),
                Some(_) => Some("declared name"),
                None if designated.is_empty() => None,
                None if designated.iter().all(bound) => Some("credential"),
                None if !designated.iter().any(bound) => Some("declared name"),
                None => Some("credential or declared name"),
            };
            if let Some(matched_by) = matched_by {
                row["matched_by"] = json!(matched_by);
            }
            if let Some(watch) = reading {
                row["watchman_name"] = json!(watch.name);
            }
            // A watchman's name, declared from another credential, under a bound designation.
            if reader == "other"
                && let Some(watch) = designated.iter().find(|watch| {
                    watch.credential.is_some()
                        && by.as_deref().is_some_and(|by| same_name(by, &watch.name))
                })
            {
                row["note"] = json!(format!(
                    "declared the name of watchman {} from credential {credential}, not the \
                     watchman's",
                    watch.name
                ));
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
        "readers_are": "watchman: a watchman in force wrote it (watchman_name says which), by its \
                        bound credential under a v2 or v3 designation, or by declared name under a \
                        v1 one (matched_by says which), inside its scope or not; peer: no watchman \
                        was designated; other: one was, and someone else wrote it; unknown: \
                        there's no operator key to check designations with. A declared name \
                        catches mistakes only. A credential is only as private as its pairing \
                        file: see docs/designation.md.",
    })
}

/// What each watchman in force re-reads (RAL-52, RAL-63): every story in its scope verified by a
/// peer, while no watchman in force covered that story, between the valid designation before the
/// watchman's and its own. Each stays "awaiting the watchman's re-read" until that watchman
/// records its own verification, send-back or correction of the story after it was designated;
/// the peer's record is never changed. With no watchman in force, the list is what the next one
/// would get. Nothing stands on a peer's verification alone.
pub fn handover(ledger: &Ledger, hashes: &[String], key: Option<&OperatorKey>) -> Value {
    use crate::ledger::{Change, State};
    let Some(key) = key else {
        return json!({
            "watchman": null, "watchmen": [], "operator_key": null, "stories": [],
            "note": "no operator key: designations can't be checked, so there's no handover list",
        });
    };
    let eras = eras(ledger, hashes, Some(key));
    let current: Vec<Watch> = eras.last().map(|(_, set)| set.clone()).unwrap_or_default();
    // Each watchman's window closes at its own designation; with none in force, there's one
    // window for whoever comes next. Where it opens depends on the story: at the start of the
    // last era before it closes in which some watchman covered that story. So a peer
    // verification made while no one covered the story goes to the first watchman whose scope
    // covers it, whatever other watchmen were added in between.
    let windows: Vec<(Option<&Watch>, u64)> = if current.is_empty() {
        vec![(None, u64::MAX)]
    } else {
        current
            .iter()
            .map(|watch| (Some(watch), watch.seq))
            .collect()
    };
    let opens_for = |closes: u64, team: Option<&str>, submitted: Option<&str>| {
        eras.iter()
            .rev()
            .filter(|(seq, _)| *seq < closes)
            .find(|(_, set)| set.iter().any(|w| w.scope.covers(team, submitted)))
            .map_or(0, |(seq, _)| *seq)
    };
    let mut since_record: Option<u64> = None;
    let mut stories = Vec::new();
    for (id, story) in ledger.stories() {
        let (team, submitted) = coverage(story);
        for (watch, closes) in &windows {
            if watch.is_some_and(|watch| !watch.scope.covers(team, submitted)) {
                continue;
            }
            let opens = &opens_for(*closes, team, submitted);
            since_record = Some(since_record.map_or(*opens, |since| since.min(*opens)));
            // The newest peer verification in the window: made while no watchman in force
            // covered this story.
            let peer = story.history.iter().rev().find(|entry| {
                entry.seq > *opens
                    && entry.seq < *closes
                    && !watchmen_at(&eras, entry.seq)
                        .iter()
                        .any(|w| w.scope.covers(team, submitted))
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
                    Change::Merge { commit, .. } => {
                        Some(json!({"seq": entry.seq, "commit": commit}))
                    }
                    _ => None,
                });
            let reread = watch.and_then(|watch| {
                story.history.iter().find(|entry| {
                    entry.seq > watch.seq
                        && watch.wrote(
                            &entry.source.credential,
                            entry.source.declared_name.as_deref(),
                        )
                        && matches!(
                            entry.change,
                            Change::Verify { .. }
                                | Change::SendBack { .. }
                                | Change::Correct { .. }
                        )
                })
            });
            let status = match (watch, reread) {
                (None, _) => "awaiting a watchman".to_string(),
                (Some(_), None) => "awaiting the watchman's re-read".to_string(),
                (Some(_), Some(entry)) => {
                    format!("re-read by the watchman at record {}", entry.seq)
                }
            };
            stories.push(json!({
                "story": id, "verified_seq": peer.seq, "verified_at_ms": peer.at_ms,
                "commit": commit, "verified_by": by, "merged": merged, "state": story.state,
                "status": status, "watchman": watch.map(|w| &w.name),
            }));
        }
    }
    let awaiting = stories
        .iter()
        .filter(|s| {
            s["status"]
                .as_str()
                .is_some_and(|t| t.starts_with("awaiting"))
        })
        .count();
    let shown: Vec<Value> = current
        .iter()
        .map(|watch| {
            json!({"name": watch.name, "credential": watch.credential,
                   "scope": watch.scope.as_text(), "seq": watch.seq})
        })
        .collect();
    json!({
        "watchman": if shown.len() == 1 { shown[0].clone() } else { Value::Null },
        "watchmen": shown,
        "operator_key": key.fingerprint(),
        "since_record": since_record.unwrap_or(0),
        "stories": stories,
        "awaiting": awaiting,
        "rule": "Nothing stands on a peer's verification alone: the watchman whose scope covers a \
                 story re-reads it, merged or not, and keeps it, sends it back or asks for fixes. \
                 The peer's record stays.",
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

    /// A v1 or v2 designation's terms: a watchman and maybe a credential, replacing every one.
    pub fn v12<'a>(watchman: Option<&'a str>, credential: Option<&'a str>) -> Terms<'a> {
        Terms {
            watchman,
            credential,
            action: None,
            scope: None,
        }
    }

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

        /// A v3 designation (RAL-63), signed with `private`, which may not be the operator's.
        pub fn designate_v3(
            &mut self,
            private: &Path,
            key: &OperatorKey,
            action: DesignateAction,
            watchman: &str,
            bound: Option<(&str, &str)>,
        ) {
            let (seq, hash) = self.head();
            let terms = Terms {
                watchman: Some(watchman),
                credential: bound.map(|(credential, _)| credential),
                action: Some(action),
                scope: bound.map(|(_, scope)| scope),
            };
            let statement = terms.statement(seq, &hash);
            let mut body = json!({
                "change": "designate", "watchman": watchman, "action": action,
                "after_seq": seq, "after_hash": hash,
                "key_fingerprint": key.fingerprint(), "signature": sign(private, &statement),
            });
            if let Some((credential, scope)) = bound {
                body["credential"] = json!(credential);
                body["scope"] = json!(scope);
            }
            self.push(body);
        }

        /// Adds `watchman`, bound to `credential`, with `scope` (RAL-63).
        pub fn add(
            &mut self,
            private: &Path,
            key: &OperatorKey,
            watchman: &str,
            credential: &str,
            scope: &str,
        ) {
            self.designate_v3(
                private,
                key,
                DesignateAction::Add,
                watchman,
                Some((credential, scope)),
            );
        }

        /// Ends `watchman`'s designation alone (RAL-63).
        pub fn end_one(&mut self, private: &Path, key: &OperatorKey, watchman: &str) {
            self.designate_v3(private, key, DesignateAction::End, watchman, None);
        }

        pub fn report(&self, key: Option<&OperatorKey>) -> Value {
            report(&Ledger::replay(&self.records).unwrap(), &self.hashes, key)
        }

        pub fn ledger(&self) -> Ledger {
            Ledger::replay(&self.records).unwrap()
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
                testing::v12(Some("w"), Some("a1-w")),
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
        assert_eq!(
            by_story("RAL-2")["watchmen_designated"],
            json!(["Grand-Photon"])
        );
        assert_eq!(by_story("RAL-2")["watchman_name"], "Grand-Photon");
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
                testing::v12(Some("w"), None),
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
            testing::v12(Some("w"), None),
            (4, &hash),
            &fp,
            &good,
        );
        assert!(stale.unwrap_err().contains("read the head again"));
        let other = check_new(
            &key,
            5,
            (4, &hash),
            testing::v12(Some("x"), None),
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
            testing::v12(Some("w"), None),
            (4, &hash),
            &fp,
            &theirs,
        );
        assert!(theirs.unwrap_err().contains("another key"));
        let wrong_fp = check_new(
            &key,
            5,
            (4, &hash),
            testing::v12(Some("w"), None),
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

    // RAL-63: more than one watchman at once.

    fn read_of(by_credential: &str, name: &str, story: &str) -> (String, String, Value) {
        let (credential, name, mut body) = read(by_credential, name);
        body["story"] = json!(story);
        (credential, name, body)
    }

    /// Two watchmen, one per machine: A on a1 and B on Murph, each bound to its own credential.
    fn two_watchmen(chain: &mut Chain, private: &Path, key: &OperatorKey) {
        chain.add(private, key, "watch-a1", "a1-watchman", "machine:a1");
        chain.add(
            private,
            key,
            "watch-murph",
            "murph-watchman",
            "machine:murph",
        );
    }

    fn names(report: &Value) -> Vec<String> {
        report["watchmen"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["name"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn two_watchmen_are_in_force_and_each_read_is_labelled_with_its_own_watchman() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        for story in ["RAL-1", "RAL-2", "RAL-3"] {
            submitted(&mut chain, story);
        }
        two_watchmen(&mut chain, &private, &key);
        let report = chain.report(Some(&key));
        assert_eq!(names(&report), ["watch-a1", "watch-murph"], "{report}");
        assert_eq!(report["watchmen"][0]["scope"], "machine:a1");
        assert_eq!(report["watchmen"][1]["credential"], "murph-watchman");
        assert!(
            report["watchman"].is_null(),
            "more than one, so no single watchman"
        );
        assert!(
            report["records"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["status"] == "valid")
        );

        // Each reads a story submitted from a1: Murph's watchman reads outside its scope,
        // and that's still its read.
        let (credential, name, body) = read_of("a1-watchman", "watch-a1", "RAL-1");
        chain.push_from(&credential, &name, body);
        let (credential, name, body) = read_of("murph-watchman", "anything", "RAL-2");
        chain.push_from(&credential, &name, body);
        // A third session declaring watchman A's name from the shared a1 credential.
        let (credential, name, body) = read_of("a1", "watch-a1", "RAL-3");
        chain.push_from(&credential, &name, body);
        let rows = verifications(&chain.ledger(), &chain.hashes, Some(&key));
        let rows = rows["verifications"].as_array().unwrap();
        let (impostor, murph, a1) = (&rows[0], &rows[1], &rows[2]);
        assert_eq!(a1["reader"], "watchman");
        assert_eq!(a1["watchman_name"], "watch-a1");
        assert_eq!(murph["reader"], "watchman");
        assert_eq!(murph["watchman_name"], "watch-murph");
        assert_eq!(
            murph["watchmen_designated"],
            json!(["watch-a1", "watch-murph"])
        );
        assert_eq!(impostor["reader"], "other", "{impostor}");
        assert!(impostor.get("watchman_name").is_none());
        assert!(
            impostor["note"]
                .as_str()
                .unwrap()
                .contains("watchman watch-a1 from credential a1"),
            "{impostor}"
        );
    }

    #[test]
    fn ending_one_watchman_leaves_the_other_in_force_and_an_end_with_no_name_ends_both() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        submitted_one(&mut chain);
        two_watchmen(&mut chain, &private, &key);
        chain.end_one(&private, &key, "watch-a1");
        let report = chain.report(Some(&key));
        assert_eq!(names(&report), ["watch-murph"]);
        assert_eq!(
            report["watchman"]["name"], "watch-murph",
            "one left, so it's the watchman"
        );
        assert_eq!(report["ended"], false);

        // A1's credential no longer reads as a watchman's; Murph's still does.
        let (credential, name, body) = read("a1-watchman", "watch-a1");
        chain.push_from(&credential, &name, body);
        let rows = verifications(&chain.ledger(), &chain.hashes, Some(&key));
        assert_eq!(rows["verifications"][0]["reader"], "other");

        // Re-adding replaces by name rather than adding a second entry.
        chain.add(
            &private,
            &key,
            "watch-murph",
            "murph-watchman",
            "team:rally",
        );
        let report = chain.report(Some(&key));
        assert_eq!(names(&report), ["watch-murph"]);
        assert_eq!(report["watchmen"][0]["scope"], "team:rally");

        // A v1 --end ends every watchman.
        chain.add(&private, &key, "watch-a1", "a1-watchman", "machine:a1");
        chain.designate(&private, &key, None);
        let report = chain.report(Some(&key));
        assert!(names(&report).is_empty());
        assert_eq!(report["ended"], true);
    }

    #[test]
    fn adding_or_ending_a_watchman_is_refused_from_anyone_but_the_operator() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let (stranger, _) = keypair(dir.path(), "stranger");
        let mut chain = Chain::new();
        submitted_one(&mut chain);
        chain.add(&private, &key, "watch-a1", "a1-watchman", "machine:a1");

        // A session's own key, naming the operator's fingerprint, can neither add nor end.
        chain.add(&stranger, &key, "mallory", "a1", "all");
        chain.end_one(&stranger, &key, "watch-a1");
        let report = chain.report(Some(&key));
        assert_eq!(names(&report), ["watch-a1"], "{report}");
        let records = report["records"].as_array().unwrap();
        for record in &records[1..] {
            assert!(
                record["status"].as_str().unwrap().contains("another key"),
                "{record}"
            );
        }

        // And when written, check_new refuses both.
        let (seq, hash) = chain.head();
        for (action, bound) in [
            (DesignateAction::Add, Some(("a1", "all"))),
            (DesignateAction::End, None),
        ] {
            let terms = Terms {
                watchman: Some("watch-a1"),
                credential: bound.map(|b| b.0),
                action: Some(action),
                scope: bound.map(|b| b.1),
            };
            let signature = sign(&stranger, &terms.statement(seq, &hash));
            let refused = check_new(
                &key,
                seq + 1,
                (seq, &hash),
                terms,
                (seq, &hash),
                &key.fingerprint(),
                &signature,
            );
            assert!(refused.is_err(), "{action:?}");
        }
    }

    #[test]
    fn a_credential_bound_to_one_watchman_in_force_cant_be_bound_to_another() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        chain.add(&private, &key, "watch-a1", "a1-watchman", "machine:a1");
        let ledger = chain.ledger();
        let terms = Terms {
            watchman: Some("watch-two"),
            credential: Some("a1-watchman"),
            action: Some(DesignateAction::Add),
            scope: Some("all"),
        };
        let refused = check_against_set(&ledger, &chain.hashes, &key, terms).unwrap_err();
        assert!(refused.contains("already bound to watch-a1"), "{refused}");
        // The same watchman may be designated again with its own credential.
        let again = Terms {
            watchman: Some("watch-a1"),
            ..terms
        };
        assert!(check_against_set(&ledger, &chain.hashes, &key, again).is_ok());

        // Written past the service, it isn't in force, and the first keeps its credential.
        chain.add(&private, &key, "watch-two", "a1-watchman", "all");
        let report = chain.report(Some(&key));
        assert_eq!(names(&report), ["watch-a1"]);
        assert!(
            report["records"][1]["status"]
                .as_str()
                .unwrap()
                .contains("already bound"),
            "{report}"
        );
    }

    #[test]
    fn ending_a_watchman_who_isnt_in_force_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        chain.add(&private, &key, "watch-a1", "a1-watchman", "machine:a1");
        let terms = Terms {
            watchman: Some("nobody"),
            credential: None,
            action: Some(DesignateAction::End),
            scope: None,
        };
        let refused = check_against_set(&chain.ledger(), &chain.hashes, &key, terms).unwrap_err();
        assert!(refused.contains("isn't a watchman in force"), "{refused}");
        chain.end_one(&private, &key, "nobody");
        let report = chain.report(Some(&key));
        assert_eq!(names(&report), ["watch-a1"]);
        assert!(
            report["records"][1]["status"]
                .as_str()
                .unwrap()
                .contains("isn't a watchman")
        );
    }

    #[test]
    fn a_v3_signature_doesnt_verify_as_v2_nor_a_v2_one_as_v3() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let hash = "b".repeat(64);
        let v3 = Terms {
            watchman: Some("w"),
            credential: Some("a1-w"),
            action: Some(DesignateAction::Add),
            scope: Some("all"),
        };
        assert_eq!(
            v3.statement(3, &hash),
            format!(
                "rally-designation v3\naction: add\nwatchman: w\ncredential: a1-w\nscope: all\n\
                 after: 3:{hash}\n"
            )
        );
        let v2 = testing::v12(Some("w"), Some("a1-w"));
        let v3_signature = sign(&private, &v3.statement(3, &hash));
        let v2_signature = sign(&private, &v2.statement(3, &hash));
        let check = |terms, signature: &str| {
            check_new(
                &key,
                4,
                (3, &hash),
                terms,
                (3, &hash),
                &key.fingerprint(),
                signature,
            )
        };
        assert!(check(v3, &v3_signature).is_ok());
        assert!(check(v2, &v2_signature).is_ok());
        assert!(check(v2, &v3_signature).is_err());
        assert!(check(v3, &v2_signature).is_err());
        // The scope is signed too.
        let wider = Terms {
            scope: Some("team:rally"),
            ..v3
        };
        assert!(check(wider, &v3_signature).is_err());
    }

    #[test]
    fn scopes_say_whose_read_a_merge_waits_for_in_waiting_and_handover() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        // RAL-1, from team rally on a1, is verified by a peer before any watchman.
        submitted_one(&mut chain);
        chain.push_from(
            "murph",
            "peer",
            json!({"change": "verify", "story": "RAL-1",
                                                "note": "ok", "commit": "a".repeat(40)}),
        );
        // RAL-2, from team catapp on Murph, waits.
        chain.push_from("murph", "c", json!({"change": "ready", "story": "RAL-2"}));
        chain.push_from(
            "murph",
            "c",
            json!({"change": "claim", "story": "RAL-2", "team": "catapp"}),
        );
        chain.push_from(
            "murph",
            "c",
            json!({"change": "submit", "story": "RAL-2",
                                             "team": "catapp", "evidence": "e"}),
        );
        two_watchmen(&mut chain, &private, &key);
        let ledger = chain.ledger();
        let set = in_force(&ledger, &chain.hashes, Some(&key));
        let waiting = crate::views::waiting_with(&ledger, &crate::views::Readers::Watchmen(set));
        let row = &waiting["waiting"][0];
        assert_eq!(row["story"], "RAL-2");
        assert_eq!(row["awaits"], json!(["watch-murph"]), "{row}");
        assert!(row.get("suggested_reader").is_none());

        // A team scope covers the team's story wherever it was submitted from; no scope covering
        // a story leaves it to any watchman, and says so.
        let mut team = Chain::new();
        team.records = chain.records[..chain.records.len() - 2].to_vec();
        team.hashes = chain.hashes[..chain.hashes.len() - 2].to_vec();
        team.add(&private, &key, "watch-rally", "a1-watchman", "team:rally");
        let ledger = team.ledger();
        let set = in_force(&ledger, &team.hashes, Some(&key));
        let waiting = crate::views::waiting_with(&ledger, &crate::views::Readers::Watchmen(set));
        let row = &waiting["waiting"][0];
        assert_eq!(row["awaits"], json!(["watch-rally"]));
        assert!(
            row["awaits_note"]
                .as_str()
                .unwrap()
                .contains("any watchman")
        );

        // Handover: RAL-1's peer verification is in watch-a1's scope (submitted from a1), not
        // watch-murph's.
        let handover = handover(&chain.ledger(), &chain.hashes, Some(&key));
        let stories = handover["stories"].as_array().unwrap();
        assert_eq!(stories.len(), 1, "{handover}");
        assert_eq!(stories[0]["story"], "RAL-1");
        assert_eq!(stories[0]["watchman"], "watch-a1");
        assert_eq!(stories[0]["status"], "awaiting the watchman's re-read");
        assert_eq!(handover["watchmen"].as_array().unwrap().len(), 2);
        // Re-read by watch-a1's credential, as a correction of the peer's verification.
        chain.push_from(
            "a1-watchman",
            "watch-a1",
            json!({"change": "correct", "story": "RAL-1", "corrects": 4, "note": "re-read"}),
        );
        let handover = super::handover(&chain.ledger(), &chain.hashes, Some(&key));
        assert!(
            handover["stories"][0]["status"]
                .as_str()
                .unwrap()
                .starts_with("re-read")
        );
    }

    #[test]
    fn adding_a_bound_watchman_ends_one_with_no_credential() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        // A v1 watchman W, matched by declared name, then a v3 add of V.
        chain.designate(&private, &key, Some("W"));
        chain.add(&private, &key, "V", "v-cred", "all");
        let report = chain.report(Some(&key));
        let names: Vec<&str> = report["watchmen"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["V"], "{report}");
        let add = &report["records"][1];
        assert_eq!(add["status"], "valid");
        assert_eq!(add["ended"], json!(["W"]));
        assert!(
            add["note"]
                .as_str()
                .unwrap()
                .contains("without a credential")
        );

        // A session declaring W's name is now labelled other, and V's credential is the watchman.
        submitted(&mut chain, "RAL-1");
        chain.push_from(
            "a1",
            "W",
            json!({"change": "verify", "story": "RAL-1", "note": "n", "commit": "c".repeat(40)}),
        );
        submitted(&mut chain, "RAL-2");
        chain.push_from(
            "v-cred",
            "anyone",
            json!({"change": "verify", "story": "RAL-2", "note": "n", "commit": "c".repeat(40)}),
        );
        let ledger = chain.ledger();
        let rows = verifications(&ledger, &chain.hashes, Some(&key));
        let reader = |story: &str| {
            rows["verifications"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["story"] == story)
                .unwrap()["reader"]
                .clone()
        };
        assert_eq!(reader("RAL-1"), "other");
        assert_eq!(reader("RAL-2"), "watchman");
    }

    #[test]
    fn a_peer_read_goes_to_the_first_watchman_whose_scope_covers_it() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        // Everyone ended, then a peer verifies a team-x story, then C (team:y) and A (team:x)
        // are added in turn.
        chain.designate(&private, &key, Some("old"));
        chain.designate(&private, &key, None);
        chain.push_as("x", json!({"change": "ready", "story": "X-1"}));
        chain.push_as("x", json!({"change": "claim", "story": "X-1", "team": "x"}));
        chain.push_as(
            "x",
            json!({"change": "submit", "story": "X-1", "team": "x", "evidence": "repo@abc"}),
        );
        verify(&mut chain, "peer", "X-1");
        let peer_seq = chain.records.len() as u64;
        chain.add(&private, &key, "C", "c-cred", "team:y");
        chain.add(&private, &key, "A", "a-cred", "team:x");
        let ledger = chain.ledger();
        let out = handover(&ledger, &chain.hashes, Some(&key));
        let rows = out["stories"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{out}");
        assert_eq!(rows[0]["story"], "X-1");
        assert_eq!(rows[0]["watchman"], "A");
        assert_eq!(rows[0]["verified_seq"], peer_seq);
        assert_eq!(rows[0]["status"], "awaiting the watchman's re-read");

        // And once A covers x, a later all-scope watchman doesn't get it again.
        chain.add(&private, &key, "B", "b-cred", "all");
        let ledger = chain.ledger();
        let out = handover(&ledger, &chain.hashes, Some(&key));
        let for_x: Vec<&Value> = out["stories"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["story"] == "X-1")
            .collect();
        assert_eq!(for_x.len(), 1, "{out}");
        assert_eq!(for_x[0]["watchman"], "A");
    }
}
