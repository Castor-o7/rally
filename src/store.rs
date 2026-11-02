//! The append-only record log, in SQLite (RAL-3).
//!
//! Each row holds one record as JSON. A record is checked against the state replayed from the log
//! before it's stored, so a refused change leaves no trace. Triggers refuse any update or delete,
//! which catches mistakes.
//!
//! Tampering is detected rather than prevented (RAL-14). Each row's `chain` is a hash over the row
//! and the previous row's hash, so altering, removing or reordering a row breaks reads that reach it.
//! Removing the newest rows leaves a shorter chain that's still valid, so the head is also kept in a
//! file beside the ledger. A write refuses to go ahead unless that file is on the current chain, or
//! the write would overwrite the evidence. A head saved elsewhere can be compared too. See
//! `docs/service.md` for what this can't catch.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rusqlite::{Connection, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::access::write_private;
use crate::ledger::{Ledger, LedgerError};
use crate::record::{Record, RecordError};

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS records (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        record TEXT NOT NULL,
        chain TEXT NOT NULL
    );
    CREATE TRIGGER IF NOT EXISTS records_no_update BEFORE UPDATE ON records
    BEGIN SELECT RAISE(ABORT, 'the ledger is append-only'); END;
    CREATE TRIGGER IF NOT EXISTS records_no_delete BEFORE DELETE ON records
    BEGIN SELECT RAISE(ABORT, 'the ledger is append-only'); END;
";

/// The chain's hash before the first record.
const CHAIN_START: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// The newest record's position and chain hash. A copy kept elsewhere can show later that nothing
/// up to that point was changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    pub seq: u64,
    pub hash: String,
}

/// The result of checking the ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Verification {
    pub ok: bool,
    pub head: Head,
    pub problems: Vec<String>,
}

fn chain_hash(previous: &str, seq: u64, record: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(previous.as_bytes());
    hash.update(b"\n");
    hash.update(seq.to_string().as_bytes());
    hash.update(b"\n");
    hash.update(record.as_bytes());
    hex::encode(hash.finalize())
}

/// Why the store refused or failed an operation.
#[derive(Debug)]
pub enum StoreError {
    /// The change was refused. The log is unchanged.
    Refused(LedgerError),
    /// A stored record can't be read, which means the log has been damaged. Holds its position.
    Corrupt {
        seq: u64,
        reason: String,
    },
    /// A row's hash doesn't match, so the row, or one before it, was altered or moved.
    Tampered {
        seq: u64,
    },
    /// The head file isn't on the current chain, so writes stop rather than overwrite the evidence.
    HeadMismatch(String),
    /// A stored record is refused on replay, or the log has a gap.
    Inconsistent(LedgerError),
    Database(rusqlite::Error),
    Io(std::io::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(e) => write!(f, "refused: {e}"),
            Self::Tampered { seq } => write!(f, "record {seq} doesn't match its chain hash"),
            Self::HeadMismatch(reason) => write!(f, "writes refused: {reason}"),
            Self::Corrupt { seq, reason } => write!(f, "record {seq} is corrupt: {reason}"),
            Self::Inconsistent(e) => write!(f, "the ledger doesn't replay: {e}"),
            Self::Database(e) => write!(f, "database error: {e}"),
            Self::Io(e) => write!(f, "can't open the ledger: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

/// An open ledger file.
pub struct Store {
    connection: Connection,
    head_file: PathBuf,
}

impl Store {
    /// Opens the ledger at `path`, creating it readable and writable only by this account.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        create_private(path).map_err(StoreError::Io)?;
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(SCHEMA)?;
        let mut head_file = path.as_os_str().to_owned();
        head_file.push(".head");
        let store = Self {
            connection,
            head_file: head_file.into(),
        };
        // From here on a head file always exists, so a missing one is never just a new ledger.
        let empty: bool =
            store
                .connection
                .query_row("SELECT NOT EXISTS (SELECT 1 FROM records)", [], |row| {
                    row.get(0)
                })?;
        if empty && !store.head_file.exists() {
            let start = Head {
                seq: 0,
                hash: CHAIN_START.to_owned(),
            };
            store.save_head(&start).map_err(StoreError::HeadMismatch)?;
        }
        Ok(store)
    }

    /// Checks a record against the current state and appends it, returning its position.
    pub fn append(&mut self, record: &Record) -> Result<u64, StoreError> {
        let head_file = self.head_file.clone();
        // Take the write lock before reading, so two writers can't both check against old state.
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut ledger = replay(&read_records(&transaction, None)?)?;
        Self::check_head_on_chain(&head_file, &transaction)?;
        let seq = ledger.last_seq() + 1;
        ledger.apply(seq, record).map_err(StoreError::Refused)?;
        let text = record.to_json();
        let previous: String = transaction
            .query_row(
                "SELECT chain FROM records ORDER BY seq DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(CHAIN_START.to_owned()),
                other => Err(other),
            })?;
        let hash = chain_hash(&previous, seq, &text);
        transaction.execute(
            "INSERT INTO records (seq, record, chain) VALUES (?1, ?2, ?3)",
            params![seq as i64, text, hash],
        )?;
        transaction.commit()?;
        // If this fails or the process dies here, the file is merely behind, which `verify` reports.
        if let Err(error) = self.save_head(&Head { seq, hash }) {
            eprintln!("rally: warning: the head file wasn't updated: {error}");
        }
        Ok(seq)
    }

    /// Checks the whole ledger, against the head file and optionally a head saved elsewhere.
    pub fn verify(&self, expected: Option<&Head>) -> Result<Verification, StoreError> {
        let mut problems = Vec::new();
        let mut rows = Vec::new();
        // Read the head first: a write in between then leaves it behind, which is allowed, rather
        // than ahead, which looks like records were removed.
        let saved_head = self.read_head();
        match read_chain(&self.connection) {
            Ok(read) => rows = read,
            Err(error @ (StoreError::Tampered { .. } | StoreError::Inconsistent(_))) => {
                problems.push(error.to_string());
            }
            Err(error) => return Err(error),
        }
        if problems.is_empty() {
            let mut replayed = Ledger::default();
            for (index, (_, text)) in rows.iter().enumerate() {
                let seq = index as u64 + 1;
                let result = Record::parse(text)
                    .map_err(|e| e.to_string())
                    .and_then(|record| replayed.apply(seq, &record).map_err(|e| e.to_string()));
                if let Err(reason) = result {
                    problems.push(format!("record {seq} doesn't replay: {reason}"));
                    break;
                }
            }
        }
        let head = Head {
            seq: rows.len() as u64,
            hash: rows
                .last()
                .map_or_else(|| CHAIN_START.to_owned(), |(hash, _)| hash.clone()),
        };
        if problems.is_empty() {
            match saved_head {
                Ok(Some(saved)) => compare_head(&rows, &saved, "the head file", &mut problems),
                Ok(None) if rows.is_empty() => {}
                Ok(None) => problems.push("the head file is missing".to_owned()),
                Err(reason) => problems.push(reason),
            }
            if let Some(expected) = expected {
                compare_head(&rows, expected, "the expected head", &mut problems);
            }
        }
        Ok(Verification {
            ok: problems.is_empty(),
            head,
            problems,
        })
    }

    /// After a lost or damaged head file, makes the current head the saved one, for the operator to
    /// run on the host. Refuses if the chain itself is broken. This also accepts any removal of the
    /// newest records that `check` was reporting, so a head saved elsewhere should be checked first.
    pub fn adopt_head(&self) -> Result<Head, StoreError> {
        let rows = read_chain(&self.connection)?;
        let head = Head {
            seq: rows.len() as u64,
            hash: rows
                .last()
                .map_or_else(|| CHAIN_START.to_owned(), |(hash, _)| hash.clone()),
        };
        self.save_head(&head).map_err(StoreError::HeadMismatch)?;
        Ok(head)
    }

    /// Refuses unless the head file names a point on the current chain.
    fn check_head_on_chain(head_file: &Path, connection: &Connection) -> Result<(), StoreError> {
        let head = read_head_file(head_file)
            .map_err(StoreError::HeadMismatch)?
            .ok_or_else(|| StoreError::HeadMismatch("the head file is missing".to_owned()))?;
        let hash: Option<String> = if head.seq == 0 {
            Some(CHAIN_START.to_owned())
        } else {
            connection
                .query_row(
                    "SELECT chain FROM records WHERE seq = ?1",
                    [i64::try_from(head.seq).unwrap_or(i64::MAX)],
                    |row| row.get(0),
                )
                .or_else(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?
        };
        match hash {
            Some(hash) if hash.eq_ignore_ascii_case(&head.hash) => Ok(()),
            _ => Err(StoreError::HeadMismatch(format!(
                "the head file at record {} isn't on the ledger's chain; run `rally check`",
                head.seq
            ))),
        }
    }

    fn save_head(&self, head: &Head) -> Result<(), String> {
        // A name of its own, so that writers can't truncate each other's file before a rename.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut temporary = self.head_file.as_os_str().to_owned();
        temporary.push(format!(
            ".{}.{}.new",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let temporary = PathBuf::from(temporary);
        let text = serde_json::to_string(head).map_err(|e| e.to_string())?;
        write_private(&temporary, text.as_bytes()).map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, &self.head_file).map_err(|e| e.to_string())?;
        // Make the rename survive a crash. Otherwise a new ledger could end up with records and no
        // head file, which refuses writes.
        let directory = self.head_file.parent().unwrap_or_else(|| Path::new("."));
        let directory = if directory.as_os_str().is_empty() {
            Path::new(".")
        } else {
            directory
        };
        std::fs::File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| e.to_string())
    }

    fn read_head(&self) -> Result<Option<Head>, String> {
        read_head_file(&self.head_file)
    }

    /// Every record in order, or those up to and including `as_of`.
    pub fn records(&self, as_of: Option<u64>) -> Result<Vec<Record>, StoreError> {
        read_records(&self.connection, as_of)
    }

    /// The state as of the latest record, or as of `as_of`.
    pub fn ledger(&self, as_of: Option<u64>) -> Result<Ledger, StoreError> {
        replay(&self.records(as_of)?)
    }
}

fn read_head_file(path: &Path) -> Result<Option<Head>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|_| "the head file can't be read".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("the head file can't be read: {error}")),
    }
}

/// Every row's hash and text, checking there are no gaps and that the chain holds.
fn read_chain_to(connection: &Connection, limit: i64) -> Result<Vec<(String, String)>, StoreError> {
    let mut statement = connection
        .prepare_cached("SELECT seq, record, chain FROM records WHERE seq <= ?1 ORDER BY seq")?;
    let rows = statement.query_map([limit], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut chain = Vec::new();
    let mut previous = CHAIN_START.to_owned();
    for (position, row) in rows.enumerate() {
        let (seq, text, hash) = row?;
        let seq = seq as u64;
        // A removed row shows up here as a gap, rather than silently renumbering what follows.
        let expected = position as u64 + 1;
        if seq != expected {
            return Err(StoreError::Inconsistent(LedgerError::OutOfOrder {
                expected,
                found: seq,
            }));
        }
        if chain_hash(&previous, seq, &text) != hash {
            return Err(StoreError::Tampered { seq });
        }
        previous.clone_from(&hash);
        chain.push((hash, text));
    }
    Ok(chain)
}

fn read_chain(connection: &Connection) -> Result<Vec<(String, String)>, StoreError> {
    read_chain_to(connection, i64::MAX)
}

fn read_records(connection: &Connection, as_of: Option<u64>) -> Result<Vec<Record>, StoreError> {
    let limit = as_of.map_or(i64::MAX, |seq| i64::try_from(seq).unwrap_or(i64::MAX));
    let mut records = Vec::new();
    for (index, (_, text)) in read_chain_to(connection, limit)?.iter().enumerate() {
        let record = Record::parse(text).map_err(|e: RecordError| StoreError::Corrupt {
            seq: index as u64 + 1,
            reason: e.to_string(),
        })?;
        records.push(record);
    }
    Ok(records)
}

/// Adds a problem if `saved` isn't a point on this chain.
fn compare_head(rows: &[(String, String)], saved: &Head, name: &str, problems: &mut Vec<String>) {
    if saved.seq == 0 {
        if !saved.hash.eq_ignore_ascii_case(CHAIN_START) {
            problems.push(format!("{name} at record 0 isn't the start of a chain"));
        }
        return;
    }
    let row = usize::try_from(saved.seq - 1)
        .ok()
        .and_then(|i| rows.get(i));
    match row {
        None => problems.push(format!(
            "{name} is at record {} but the ledger ends at {}: records were removed",
            saved.seq,
            rows.len()
        )),
        Some((hash, _)) if !hash.eq_ignore_ascii_case(&saved.hash) => problems.push(format!(
            "{name} doesn't match record {}: the ledger was rewritten",
            saved.seq
        )),
        Some(_) => {}
    }
}

fn replay(records: &[Record]) -> Result<Ledger, StoreError> {
    Ledger::replay(records).map_err(StoreError::Inconsistent)
}

#[cfg(unix)]
fn create_private(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .map(|_| ())
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::State;
    use crate::record::{Kind, Source};
    use serde_json::{Value, json};

    fn temp_store() -> (tempfile::TempDir, std::path::PathBuf, Store) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        let store = Store::open(&path).unwrap();
        (dir, path, store)
    }

    fn team(body: Value) -> Record {
        let source = Source {
            credential: "a1".into(),
            declared_name: Some("alice".into()),
        };
        Record::new(Kind::Declared, source, 1_000, body).unwrap()
    }

    fn ready() -> Record {
        team(json!({"change": "ready", "story": "RAL-3"}))
    }

    fn claim() -> Record {
        team(json!({"change": "claim", "story": "RAL-3", "team": "rally"}))
    }

    #[test]
    fn appended_records_survive_reopening() {
        let (_dir, path, mut store) = temp_store();
        assert_eq!(store.append(&ready()).unwrap(), 1);
        assert_eq!(store.append(&claim()).unwrap(), 2);
        drop(store);

        let store = Store::open(&path).unwrap();
        assert_eq!(store.records(None).unwrap(), vec![ready(), claim()]);
        let ledger = store.ledger(None).unwrap();
        assert_eq!(ledger.story("RAL-3").unwrap().state, State::Claimed);
    }

    #[test]
    fn a_refused_change_leaves_no_record() {
        let (_dir, _path, mut store) = temp_store();
        assert!(matches!(
            store.append(&claim()),
            Err(StoreError::Refused(LedgerError::UnknownStory(_)))
        ));
        assert!(store.records(None).unwrap().is_empty());
    }

    #[test]
    fn can_read_the_state_as_of_a_past_record() {
        let (_dir, _path, mut store) = temp_store();
        store.append(&ready()).unwrap();
        store.append(&claim()).unwrap();
        let then = store.ledger(Some(1)).unwrap();
        assert_eq!(then.story("RAL-3").unwrap().state, State::Ready);
        assert_eq!(then.last_seq(), 1);
    }

    #[test]
    fn the_log_refuses_updates_and_deletes() {
        let (_dir, _path, mut store) = temp_store();
        store.append(&ready()).unwrap();
        for sql in [
            "UPDATE records SET record = '{}' WHERE seq = 1",
            "DELETE FROM records WHERE seq = 1",
        ] {
            let error = store.connection.execute(sql, []).unwrap_err();
            assert!(error.to_string().contains("append-only"), "{error}");
        }
        assert_eq!(store.records(None).unwrap(), vec![ready()]);
    }

    fn altered_record(store: &Store) {
        store
            .connection
            .execute_batch(
                "DROP TRIGGER IF EXISTS records_no_update;
                 UPDATE records SET record = replace(record, 'alice', 'mallory') WHERE seq = 1;",
            )
            .unwrap();
    }

    fn remove(store: &Store, condition: &str) {
        store
            .connection
            .execute_batch(&format!(
                "DROP TRIGGER IF EXISTS records_no_delete;
                 DELETE FROM records WHERE {condition};"
            ))
            .unwrap();
    }

    fn two_records() -> (tempfile::TempDir, std::path::PathBuf, Store) {
        let (dir, path, mut store) = temp_store();
        store.append(&ready()).unwrap();
        store.append(&claim()).unwrap();
        (dir, path, store)
    }

    #[test]
    fn an_untouched_ledger_verifies() {
        let (_dir, _path, store) = two_records();
        let result = store.verify(None).unwrap();
        assert!(result.ok, "{:?}", result.problems);
        assert_eq!(result.head.seq, 2);
    }

    #[test]
    fn an_empty_ledger_verifies() {
        let (_dir, _path, store) = temp_store();
        assert!(store.verify(None).unwrap().ok);
    }

    #[test]
    fn an_altered_record_is_detected_and_not_read() {
        let (_dir, _path, store) = two_records();
        altered_record(&store);
        assert!(matches!(
            store.ledger(None),
            Err(StoreError::Tampered { seq: 1 })
        ));
        let result = store.verify(None).unwrap();
        assert!(!result.ok);
        assert!(
            result.problems[0].contains("record 1"),
            "{:?}",
            result.problems
        );
    }

    #[test]
    fn a_removed_record_is_detected() {
        let (_dir, _path, store) = two_records();
        remove(&store, "seq = 1");
        assert!(!store.verify(None).unwrap().ok);
    }

    #[test]
    fn reordered_records_are_detected() {
        let (_dir, _path, store) = two_records();
        store
            .connection
            .execute_batch(
                "DROP TRIGGER IF EXISTS records_no_update;
                 UPDATE records SET seq = 3 WHERE seq = 1;
                 UPDATE records SET seq = 1 WHERE seq = 2;
                 UPDATE records SET seq = 2 WHERE seq = 3;",
            )
            .unwrap();
        assert!(!store.verify(None).unwrap().ok);
    }

    #[test]
    fn removing_the_newest_records_is_detected_by_the_head_file() {
        let (_dir, _path, store) = two_records();
        remove(&store, "seq = 2");
        let result = store.verify(None).unwrap();
        assert!(!result.ok);
        assert!(
            result.problems[0].contains("removed"),
            "{:?}",
            result.problems
        );
        // The shorter chain is valid by itself, which is why the file is needed.
        assert_eq!(store.ledger(None).unwrap().last_seq(), 1);
    }

    #[test]
    fn a_saved_head_catches_what_the_head_file_cannot() {
        let (_dir, path, store) = two_records();
        let saved = store.verify(None).unwrap().head;
        remove(&store, "seq = 2");
        let mut head_file = path.into_os_string();
        head_file.push(".head");
        std::fs::remove_file(&head_file).unwrap();
        let stale = Head {
            seq: 1,
            hash: store.verify(None).unwrap().head.hash,
        };
        std::fs::write(&head_file, serde_json::to_string(&stale).unwrap()).unwrap();

        assert!(store.verify(None).unwrap().ok, "the file alone is fooled");
        let result = store.verify(Some(&saved)).unwrap();
        assert!(!result.ok);
        assert!(
            result.problems[0].contains("removed"),
            "{:?}",
            result.problems
        );
    }

    #[test]
    fn a_rewritten_ledger_fails_against_a_saved_head() {
        let (_dir, _path, store) = two_records();
        let mut saved = store.verify(None).unwrap().head;
        saved.hash = "f".repeat(64);
        let result = store.verify(Some(&saved)).unwrap();
        assert!(!result.ok);
        assert!(
            result.problems[0].contains("rewritten"),
            "{:?}",
            result.problems
        );
    }

    #[test]
    fn a_head_file_behind_the_ledger_after_a_crash_still_verifies() {
        let (_dir, path, mut store) = two_records();
        let mut head_file = path.into_os_string();
        head_file.push(".head");
        let behind = std::fs::read_to_string(&head_file).unwrap();
        store
            .append(&team(
                json!({"change": "release", "story": "RAL-3", "team": "rally"}),
            ))
            .unwrap();
        std::fs::write(&head_file, behind).unwrap();
        assert!(store.verify(None).unwrap().ok);
    }

    #[test]
    fn a_missing_head_file_is_reported() {
        let (_dir, path, store) = two_records();
        let mut head_file = path.into_os_string();
        head_file.push(".head");
        std::fs::remove_file(head_file).unwrap();
        assert!(!store.verify(None).unwrap().ok);
    }

    fn release() -> Record {
        team(json!({"change": "release", "story": "RAL-3", "team": "rally"}))
    }

    #[test]
    fn removing_the_newest_records_cannot_heal_on_the_next_write() {
        let (_dir, _path, mut store) = two_records();
        remove(&store, "seq = 2");
        let appended = store.append(&claim());
        assert!(appended.is_err(), "the next write should be refused");
        let result = store.verify(None).unwrap();
        assert!(!result.ok, "the removal must still show");
    }

    #[test]
    fn a_missing_head_file_refuses_writes() {
        let (_dir, path, mut store) = two_records();
        let mut head_file = path.into_os_string();
        head_file.push(".head");
        std::fs::remove_file(head_file).unwrap();
        assert!(store.append(&release()).is_err());
    }

    #[test]
    fn a_new_ledger_can_survive_a_crash_before_its_first_head_save() {
        let (_dir, path, mut store) = temp_store();
        let mut head_file = path.clone().into_os_string();
        head_file.push(".head");
        assert!(std::path::Path::new(&head_file).exists());
        store.append(&ready()).unwrap();
        assert!(store.verify(None).unwrap().ok);
    }

    #[test]
    fn recomputing_one_rows_hash_fails_at_the_next_row() {
        let (_dir, _path, store) = two_records();
        let text = claim().to_json();
        let forged = ready().to_json().replace("alice", "mallory");
        let hash = chain_hash(CHAIN_START, 1, &forged);
        assert_ne!(text, forged);
        store
            .connection
            .execute_batch("DROP TRIGGER IF EXISTS records_no_update;")
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE records SET record = ?1, chain = ?2 WHERE seq = 1",
                params![forged, hash],
            )
            .unwrap();
        assert!(matches!(
            store.ledger(None),
            Err(StoreError::Tampered { seq: 2 })
        ));
        assert!(!store.verify(None).unwrap().ok);
    }

    #[test]
    fn the_hash_covers_the_previous_hash_the_position_and_the_record() {
        let base = chain_hash(CHAIN_START, 1, "x");
        assert_ne!(base, chain_hash(&"f".repeat(64), 1, "x"));
        assert_ne!(base, chain_hash(CHAIN_START, 2, "x"));
        assert_ne!(base, chain_hash(CHAIN_START, 1, "y"));
    }

    #[test]
    fn a_head_at_zero_must_be_the_real_start_of_the_chain() {
        let (_dir, path, store) = two_records();
        let junk = Head {
            seq: 0,
            hash: "junk".into(),
        };
        assert!(!store.verify(Some(&junk)).unwrap().ok);
        let start = Head {
            seq: 0,
            hash: CHAIN_START.into(),
        };
        assert!(store.verify(Some(&start)).unwrap().ok);

        remove(&store, "seq = 2");
        let mut head_file = path.into_os_string();
        head_file.push(".head");
        std::fs::write(&head_file, serde_json::to_string(&junk).unwrap()).unwrap();
        assert!(!store.verify(None).unwrap().ok);
    }

    #[test]
    fn a_saved_head_in_upper_case_still_matches() {
        let (_dir, _path, store) = two_records();
        let mut head = store.verify(None).unwrap().head;
        head.hash = head.hash.to_uppercase();
        assert!(store.verify(Some(&head)).unwrap().ok);
    }

    #[test]
    fn adopting_the_head_recovers_from_a_lost_head_file() {
        let (_dir, path, mut store) = two_records();
        let mut head_file = path.into_os_string();
        head_file.push(".head");
        std::fs::remove_file(&head_file).unwrap();
        assert!(store.append(&release()).is_err());

        let head = store.adopt_head().unwrap();
        assert_eq!(head.seq, 2);
        assert!(store.verify(None).unwrap().ok);
        store.append(&release()).unwrap();
    }

    #[test]
    fn adopting_the_head_refuses_a_broken_chain() {
        let (_dir, _path, store) = two_records();
        altered_record(&store);
        assert!(matches!(
            store.adopt_head(),
            Err(StoreError::Tampered { seq: 1 })
        ));
    }

    #[test]
    fn no_temporary_head_files_are_left_behind() {
        let (dir, _path, _store) = two_records();
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(
            !names.iter().any(|name| name.ends_with(".new")),
            "{names:?}"
        );
    }

    #[test]
    fn a_damaged_log_is_reported_not_read() {
        let (_dir, _path, mut store) = temp_store();
        store.append(&ready()).unwrap();
        store
            .connection
            .execute_batch(
                "DROP TRIGGER records_no_update;
                 UPDATE records SET record = 'garbage' WHERE seq = 1;",
            )
            .unwrap();
        assert!(matches!(
            store.ledger(None),
            Err(StoreError::Tampered { seq: 1 })
        ));
    }

    #[test]
    fn a_gap_in_the_log_is_reported() {
        let (_dir, _path, mut store) = temp_store();
        store.append(&ready()).unwrap();
        store.append(&claim()).unwrap();
        store
            .connection
            .execute_batch(
                "DROP TRIGGER records_no_delete;
                 DELETE FROM records WHERE seq = 1;",
            )
            .unwrap();
        assert!(matches!(
            store.ledger(None),
            Err(StoreError::Inconsistent(LedgerError::OutOfOrder { .. }))
        ));
    }

    #[test]
    fn concurrent_writers_keep_one_order() {
        let (_dir, path, mut store) = temp_store();
        store.append(&ready()).unwrap();
        store.append(&claim()).unwrap();
        let writers: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut store = Store::open(&path).unwrap();
                    let note = team(json!({
                        "change": "correct", "story": "RAL-3", "corrects": 2, "note": "again"
                    }));
                    store.append(&note).unwrap()
                })
            })
            .collect();
        let mut positions: Vec<u64> = writers.into_iter().map(|w| w.join().unwrap()).collect();
        positions.sort_unstable();
        assert_eq!(positions, (3..=10).collect::<Vec<u64>>());
        assert_eq!(store.ledger(None).unwrap().last_seq(), 10);
    }

    #[cfg(unix)]
    #[test]
    fn the_ledger_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, path, _store) = temp_store();
        let mode = std::fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
