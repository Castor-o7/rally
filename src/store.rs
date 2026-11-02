//! The append-only record log, in SQLite (RAL-3).
//!
//! Each row holds one record as JSON. A record is checked against the state replayed from the log
//! before it's stored, so a refused change leaves no trace. Triggers refuse any update or delete,
//! which catches mistakes. They don't stop someone with write access to the file; that's RAL-14.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, TransactionBehavior, params};

use crate::ledger::{Ledger, LedgerError};
use crate::record::{Record, RecordError};

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS records (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        record TEXT NOT NULL
    );
    CREATE TRIGGER IF NOT EXISTS records_no_update BEFORE UPDATE ON records
    BEGIN SELECT RAISE(ABORT, 'the ledger is append-only'); END;
    CREATE TRIGGER IF NOT EXISTS records_no_delete BEFORE DELETE ON records
    BEGIN SELECT RAISE(ABORT, 'the ledger is append-only'); END;
";

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
    /// A stored record is refused on replay, or the log has a gap.
    Inconsistent(LedgerError),
    Database(rusqlite::Error),
    Io(std::io::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(e) => write!(f, "refused: {e}"),
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
}

impl Store {
    /// Opens the ledger at `path`, creating it readable and writable only by this account.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        create_private(path).map_err(StoreError::Io)?;
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(SCHEMA)?;
        Ok(Self { connection })
    }

    /// Checks a record against the current state and appends it, returning its position.
    pub fn append(&mut self, record: &Record) -> Result<u64, StoreError> {
        // Take the write lock before reading, so two writers can't both check against old state.
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut ledger = replay(&read_records(&transaction, None)?)?;
        let seq = ledger.last_seq() + 1;
        ledger.apply(seq, record).map_err(StoreError::Refused)?;
        transaction.execute(
            "INSERT INTO records (seq, record) VALUES (?1, ?2)",
            params![seq as i64, record.to_json()],
        )?;
        transaction.commit()?;
        Ok(seq)
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

fn read_records(connection: &Connection, as_of: Option<u64>) -> Result<Vec<Record>, StoreError> {
    let limit = as_of.map_or(i64::MAX, |seq| i64::try_from(seq).unwrap_or(i64::MAX));
    let mut statement = connection
        .prepare_cached("SELECT seq, record FROM records WHERE seq <= ?1 ORDER BY seq")?;
    let rows = statement.query_map([limit], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut records = Vec::new();
    for (position, row) in rows.enumerate() {
        let (seq, text) = row?;
        let seq = seq as u64;
        // A removed row shows up here as a gap, rather than silently renumbering what follows.
        let expected = position as u64 + 1;
        if seq != expected {
            return Err(StoreError::Inconsistent(LedgerError::OutOfOrder {
                expected,
                found: seq,
            }));
        }
        let record = Record::parse(&text).map_err(|e: RecordError| StoreError::Corrupt {
            seq,
            reason: e.to_string(),
        })?;
        records.push(record);
    }
    Ok(records)
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
            Err(StoreError::Corrupt { seq: 1, .. })
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
