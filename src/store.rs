//! The append-only record log: myko entities, kept durably in Postgres (RAL-3, RAL-39).
//!
//! Each record is a `LedgerRecord` entity in myko's events table. A writer runs myko in this
//! process without its listener, so nothing but Rally can reach it, and appends only through
//! [`AppendLedgerRecord`], which checks the record against the replayed state. The row is committed
//! to Postgres before the record enters myko's store or is acknowledged, so a refused or failed
//! append leaves no trace. One writer at a time holds a Postgres advisory lock, and the table
//! refuses updates, deletes and a second version of any record, which catches mistakes.
//!
//! Tampering is detected rather than prevented (RAL-14). Each record's `hash` covers the record and
//! the previous record's hash, so altering, removing or reordering a record breaks reads that reach
//! it. Removing the newest records leaves a shorter chain that's still valid, so the head is also
//! kept in a file outside Postgres. A write refuses to go ahead unless that file is on the current
//! chain, or the write would overwrite the evidence. A head saved elsewhere can be compared too.
//! `verify` reads Postgres directly, not myko, so the check doesn't depend on the store it checks.
//! See `docs/service.md` for what this can't catch.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use myko::command::{CommandContext, CommandError, CommandHandler};
use myko::request::RequestContext;
use myko::server::{BlackholePersister, MykoServerContext, Persister};
use myko_server::MykoServer;
use myko_server::postgres::PostgresConfig;
use postgres::{Client, NoTls};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::access::write_private;
use crate::entity::{
    AppendLedgerRecord, Appended, GetAllLedgerRecords, LedgerRecord, LedgerRecordId,
};
use crate::ledger::{Ledger, LedgerError};
use crate::record::{Record, RecordError};

/// myko's events table and notification channel for Rally.
pub const TABLE: &str = "rally_events";
const CHANNEL: &str = "rally_events_notify";
/// The entity type of a ledger record.
pub const ENTITY: &str = "LedgerRecord";
/// The advisory lock held by the one writer.
const WRITER_LOCK: i64 = 0x5241_4c4c_595f_3339;
/// How long opening a writer waits for another writer to finish.
pub const WRITER_WAIT: Duration = Duration::from_secs(5);
/// How long replay at startup may take.
const REPLAY_WAIT: Duration = Duration::from_secs(300);

/// The chain's hash before the first record.
const CHAIN_START: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// myko's own schema, created first so that Rally's protections can go on it. myko runs the same
/// statements, which do nothing once the table exists. Copied from myko-server 6.9.0's
/// `postgres.rs` `ensure_schema`.
const MYKO_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS rally_events (
        id BIGSERIAL PRIMARY KEY,
        item_type TEXT NOT NULL,
        item_id TEXT NOT NULL,
        change_type TEXT NOT NULL,
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        tx TEXT NOT NULL,
        source_id TEXT,
        event JSONB NOT NULL
    );
    CREATE INDEX IF NOT EXISTS rally_events_tx_idx ON rally_events (tx);
    CREATE INDEX IF NOT EXISTS rally_events_item_type_item_id_idx ON rally_events (item_type, item_id);
    CREATE INDEX IF NOT EXISTS rally_events_item_latest_idx ON rally_events (item_type, item_id, id DESC);
    CREATE INDEX IF NOT EXISTS rally_events_created_at_idx ON rally_events (created_at);
";

/// Rally's protections: a record has one version, and rows are never changed or removed. Someone
/// who can change the schema can remove these, which the chain and a saved head then have to catch.
const RALLY_SCHEMA: &str = "
    CREATE UNIQUE INDEX IF NOT EXISTS rally_records_once
        ON rally_events (item_id) WHERE item_type = 'LedgerRecord';
    CREATE OR REPLACE FUNCTION rally_append_only() RETURNS trigger AS $$
    BEGIN
        RAISE EXCEPTION 'the ledger is append-only';
    END;
    $$ LANGUAGE plpgsql;
    CREATE OR REPLACE FUNCTION rally_records_are_sets() RETURNS trigger AS $$
    BEGIN
        IF NEW.item_type = 'LedgerRecord' AND NEW.change_type <> 'SET' THEN
            RAISE EXCEPTION 'the ledger is append-only';
        END IF;
        RETURN NEW;
    END;
    $$ LANGUAGE plpgsql;
    DO $$ BEGIN
        IF NOT EXISTS (SELECT 1 FROM pg_trigger WHERE tgname = 'rally_no_update') THEN
            CREATE TRIGGER rally_no_update BEFORE UPDATE ON rally_events
                FOR EACH ROW EXECUTE FUNCTION rally_append_only();
        END IF;
        IF NOT EXISTS (SELECT 1 FROM pg_trigger WHERE tgname = 'rally_no_delete') THEN
            CREATE TRIGGER rally_no_delete BEFORE DELETE ON rally_events
                FOR EACH ROW EXECUTE FUNCTION rally_append_only();
        END IF;
        IF NOT EXISTS (SELECT 1 FROM pg_trigger WHERE tgname = 'rally_no_truncate') THEN
            CREATE TRIGGER rally_no_truncate BEFORE TRUNCATE ON rally_events
                FOR EACH STATEMENT EXECUTE FUNCTION rally_append_only();
        END IF;
        IF NOT EXISTS (SELECT 1 FROM pg_trigger WHERE tgname = 'rally_only_sets') THEN
            CREATE TRIGGER rally_only_sets BEFORE INSERT ON rally_events
                FOR EACH ROW EXECUTE FUNCTION rally_records_are_sets();
        END IF;
    END $$;
";

/// Where a ledger is: its Postgres database, and the head file kept beside the service, outside
/// Postgres.
#[derive(Clone, Debug)]
pub struct Location {
    pub url: String,
    pub head_file: PathBuf,
}

impl Location {
    /// The database URL without any password, for messages.
    pub fn redacted(&self) -> String {
        redact(&self.url)
    }
}

/// A URL with any password removed.
pub fn redact(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(scheme), Some(at)) if at > scheme => {
            let user = &url[scheme + 3..at];
            let user = user.split(':').next().unwrap_or_default();
            format!("{}{user}{}", &url[..scheme + 3], &url[at..])
        }
        _ => url.to_owned(),
    }
}

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
    /// Not problems, but worth knowing, such as a head file behind the ledger.
    pub notes: Vec<String>,
}

pub(crate) fn chain_hash(previous: &str, seq: u64, record: &str) -> String {
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
    /// A record's hash doesn't match, so it, or one before it, was altered or moved.
    Tampered {
        seq: u64,
    },
    /// The head file isn't on the current chain, so writes stop rather than overwrite the evidence.
    HeadMismatch(String),
    /// A stored record is refused on replay, or the log has a gap.
    Inconsistent(LedgerError),
    /// myko's copy and Postgres differ, so writes stop until a restart replays Postgres.
    Diverged(String),
    /// Another process is writing to this ledger.
    Busy,
    /// There's no ledger at that location.
    NoLedger(String),
    /// Postgres couldn't be reached or refused. A write that fails this way wasn't acknowledged.
    Database(String),
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
            Self::Diverged(reason) => write!(f, "writes refused: {reason}"),
            Self::Busy => write!(
                f,
                "another process is writing to this ledger, such as the service; \
                 write through it, or stop it first"
            ),
            Self::NoLedger(at) => write!(f, "no ledger at {at}"),
            Self::Database(e) => write!(f, "database error: {e}"),
            Self::Io(e) => write!(f, "can't open the ledger: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<postgres::Error> for StoreError {
    fn from(error: postgres::Error) -> Self {
        Self::Database(describe(&error))
    }
}

fn describe(error: &postgres::Error) -> String {
    match error.as_db_error() {
        Some(db) => format!("{error}: {}", db.message()),
        None => error.to_string(),
    }
}

/// One stored record as read back: its position, text and claimed hash.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    seq: u64,
    text: String,
    hash: String,
}

/// The writers in this process, by myko host id, so the command handler can find its own.
fn writers() -> &'static Mutex<HashMap<Uuid, Arc<Writer>>> {
    static WRITERS: OnceLock<Mutex<HashMap<Uuid, Arc<Writer>>>> = OnceLock::new();
    WRITERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The writing half: myko without a listener, and the connection holding the writer lock.
struct Writer {
    server: MykoServer,
    ctx: Arc<MykoServerContext>,
    location: Location,
    /// Holds the advisory lock for as long as it's open. Also the one writer lock in this process.
    sql: Mutex<Client>,
    /// The typed reason for the last failed append, which a `CommandError` can't carry.
    failure: Mutex<Option<StoreError>>,
}

/// An open ledger.
pub struct Store {
    location: Location,
    /// For reading: a read-only connection, or the writer's own.
    reader: Option<Mutex<Client>>,
    writer: Option<Arc<Writer>>,
}

impl Drop for Store {
    fn drop(&mut self) {
        if let Some(writer) = &self.writer {
            writers()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&writer.server.host_id);
        }
    }
}

fn config(url: &str, read_only: bool) -> Result<postgres::Config, StoreError> {
    let mut config: postgres::Config = url
        .parse()
        .map_err(|_| StoreError::Database(format!("{} isn't a Postgres URL", redact(url))))?;
    config.connect_timeout(Duration::from_secs(5));
    config.keepalives(true);
    config.keepalives_idle(Duration::from_secs(30));
    config.tcp_user_timeout(Duration::from_secs(15));
    let mut options = "-c statement_timeout=30000 -c lock_timeout=10000".to_owned();
    if read_only {
        options.push_str(" -c default_transaction_read_only=on");
    }
    config.options(&options);
    config.application_name("rally");
    Ok(config)
}

fn connect(location: &Location, read_only: bool) -> Result<Client, StoreError> {
    config(&location.url, read_only)?
        .connect(NoTls)
        .map_err(|error| match error.code() {
            Some(code) if *code == postgres::error::SqlState::INVALID_CATALOG_NAME => {
                StoreError::NoLedger(location.redacted())
            }
            _ => StoreError::Database(format!("{}: {}", location.redacted(), describe(&error))),
        })
}

fn has_table(client: &mut Client) -> Result<bool, StoreError> {
    let row = client.query_one("SELECT to_regclass('rally_events') IS NOT NULL", &[])?;
    Ok(row.get(0))
}

/// Takes the writer lock, waiting up to `wait` for another writer to let go.
fn take_writer_lock(client: &mut Client, wait: Duration) -> Result<(), StoreError> {
    let started = Instant::now();
    loop {
        let row = client.query_one("SELECT pg_try_advisory_lock($1)", &[&WRITER_LOCK])?;
        if row.get::<_, bool>(0) {
            return Ok(());
        }
        if started.elapsed() >= wait {
            return Err(StoreError::Busy);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

impl Store {
    /// Opens the ledger for writing, creating it if it doesn't exist. Waits up to [`WRITER_WAIT`]
    /// if another process is writing.
    pub fn open(location: &Location) -> Result<Self, StoreError> {
        Self::open_waiting(location, WRITER_WAIT)
    }

    /// Opens the ledger for writing, waiting up to `wait` for another writer.
    pub fn open_waiting(location: &Location, wait: Duration) -> Result<Self, StoreError> {
        let mut sql = connect(location, false)?;
        take_writer_lock(&mut sql, wait)?;
        sql.batch_execute(MYKO_SCHEMA)?;
        sql.batch_execute(RALLY_SCHEMA).map_err(|error| {
            match error.code() {
                // The one-version index can't be made over a record that has two.
                Some(code) if *code == postgres::error::SqlState::UNIQUE_VIOLATION => {
                    StoreError::Diverged(
                        "a record has more than one version in Postgres, so the ledger was \
                         changed past the append path; run `rally check`"
                            .to_owned(),
                    )
                }
                _ => StoreError::from(error),
            }
        })?;
        let (seq, _) = durable_tail(&mut sql)?;

        // Every myko event other than Rally's own appends is kept out of Postgres, and each run
        // gets a new host id: with a fixed one, replay skips this host's rows as its own.
        let blackhole: Arc<dyn Persister> = Arc::new(BlackholePersister);
        let server = MykoServer::builder()
            .with_postgres(PostgresConfig {
                url: location.url.clone(),
                table: TABLE.to_owned(),
                channel: CHANNEL.to_owned(),
            })
            .with_default_persister(blackhole)
            .build();
        server
            .init_postgres_and_wait(REPLAY_WAIT)
            .map_err(StoreError::Database)?;
        let ctx = Arc::new(server.ctx());
        let host_id = server.host_id;
        let writer = Arc::new(Writer {
            server,
            ctx,
            location: location.clone(),
            sql: Mutex::new(sql),
            failure: Mutex::new(None),
        });
        writers()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(host_id, writer.clone());
        let store = Self {
            location: location.clone(),
            reader: None,
            writer: Some(writer),
        };
        // From here on a head file always exists, so a missing one is never just a new ledger.
        if seq == 0 && !location.head_file.exists() {
            let start = Head {
                seq: 0,
                hash: CHAIN_START.to_owned(),
            };
            save_head(&location.head_file, &start).map_err(StoreError::HeadMismatch)?;
        }
        Ok(store)
    }

    /// Opens an existing ledger for reading only, straight from Postgres, creating nothing. It
    /// doesn't need the writer lock, so it works while the service runs.
    pub fn open_read_only(location: &Location) -> Result<Self, StoreError> {
        let mut sql = connect(location, true)?;
        if !has_table(&mut sql)? {
            return Err(StoreError::NoLedger(location.redacted()));
        }
        Ok(Self {
            location: location.clone(),
            reader: Some(Mutex::new(sql)),
            writer: None,
        })
    }

    /// The myko host id of this writer, which is new each time it opens.
    pub fn host_id(&self) -> Option<Uuid> {
        self.writer.as_ref().map(|writer| writer.server.host_id)
    }

    /// myko's context, for tests that tamper with the store from inside this process. Nothing
    /// outside this process can reach it.
    #[doc(hidden)]
    pub fn myko_context(&self) -> Option<Arc<MykoServerContext>> {
        self.writer.as_ref().map(|writer| writer.ctx.clone())
    }

    fn with_sql<T>(
        &self,
        read: impl FnOnce(&mut Client) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        if let Some(writer) = &self.writer {
            let mut client = writer
                .sql
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            writer.reconnect_if_needed(&mut client)?;
            return read(&mut client);
        }
        let reader = self
            .reader
            .as_ref()
            .expect("a store is a reader or a writer");
        let mut client = reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        read(&mut client)
    }

    /// Checks a record against the current state and appends it, returning its position. It
    /// returns only once Postgres has committed the record.
    pub fn append(&mut self, record: &Record) -> Result<u64, StoreError> {
        let Some(writer) = &self.writer else {
            return Err(StoreError::Database(
                "this ledger was opened read-only".to_owned(),
            ));
        };
        let request = Arc::new(RequestContext::internal(
            Arc::from(Uuid::new_v4().to_string()),
            writer.server.host_id,
            "rally",
        ));
        let command =
            CommandContext::new(Arc::from("AppendLedgerRecord"), request, writer.ctx.clone());
        let appended = AppendLedgerRecord {
            record: record.to_json(),
        }
        .execute(command);
        match appended {
            Ok(appended) => Ok(appended.seq),
            Err(error) => Err(writer
                .failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .unwrap_or(StoreError::Database(error.message))),
        }
    }

    /// The size of the ledger's table in Postgres, with its indexes.
    pub fn size_bytes(&self) -> Result<u64, StoreError> {
        self.with_sql(|sql| {
            let row = sql.query_one("SELECT pg_total_relation_size('rally_events')", &[])?;
            Ok(u64::try_from(row.get::<_, i64>(0)).unwrap_or(0))
        })
    }

    /// Where Postgres keeps its data, so the disk floor can be checked there.
    pub fn data_directory(&self) -> Result<PathBuf, StoreError> {
        self.with_sql(|sql| {
            let row = sql.query_one("SHOW data_directory", &[])?;
            Ok(PathBuf::from(row.get::<_, String>(0)))
        })
    }

    /// Checks the whole ledger, as Postgres holds it, against the head file and optionally a head
    /// saved elsewhere. A writer also compares myko's copy with Postgres.
    pub fn verify(&self, expected: Option<&Head>) -> Result<Verification, StoreError> {
        let mut problems = Vec::new();
        let mut chain = Vec::new();
        // Read the head first: a write in between then leaves it behind, which is allowed, rather
        // than ahead, which looks like records were removed.
        let saved_head = read_head_file(&self.location.head_file);
        let (rows, mut history) = self.with_sql(durable_rows)?;
        match check_chain(rows.clone(), u64::MAX) {
            Ok(read) => chain = read,
            Err(error @ (StoreError::Tampered { .. } | StoreError::Inconsistent(_))) => {
                problems.push(error.to_string());
            }
            Err(StoreError::Corrupt { seq, reason }) => {
                problems.push(format!("record {seq} is corrupt: {reason}"));
            }
            Err(error) => return Err(error),
        }
        if problems.is_empty() {
            let mut replayed = Ledger::default();
            for row in &chain {
                let result = Record::parse(&row.text)
                    .map_err(|e| e.to_string())
                    .and_then(|record| replayed.apply(row.seq, &record).map_err(|e| e.to_string()));
                if let Err(reason) = result {
                    problems.push(format!("record {} doesn't replay: {reason}", row.seq));
                    break;
                }
            }
        }
        // Not part of the chain's own story, so they don't stop the head comparisons below.
        if let Some(writer) = &self.writer
            && let Some(reason) = differs(&writer.memory_rows()?, &rows)
        {
            history.push(reason);
        }
        let head = head_of(&chain);
        let mut notes = Vec::new();
        if problems.is_empty() {
            match saved_head {
                Ok(Some(saved)) => {
                    compare_head(&chain, &saved, "the head file", &mut problems);
                    if saved.seq < head.seq {
                        notes.push(format!(
                            "the head file is behind the ledger, at record {} of {}",
                            saved.seq, head.seq
                        ));
                    }
                }
                Ok(None) if chain.is_empty() => {}
                Ok(None) => problems.push("the head file is missing".to_owned()),
                Err(reason) => problems.push(reason),
            }
            if let Some(expected) = expected {
                compare_head(&chain, expected, "the expected head", &mut problems);
            }
        }
        problems.extend(history);
        Ok(Verification {
            ok: problems.is_empty(),
            head,
            problems,
            notes,
        })
    }

    /// After a lost or damaged head file, makes the current head the saved one, for the operator to
    /// run on the host. Refuses if the chain itself is broken. This also accepts any removal of the
    /// newest records that `check` was reporting, so a head saved elsewhere should be checked first.
    pub fn adopt_head(&self) -> Result<Head, StoreError> {
        let (rows, history) = self.with_sql(durable_rows)?;
        if let Some(problem) = history.into_iter().next() {
            return Err(StoreError::Diverged(problem));
        }
        let head = head_of(&check_chain(rows, u64::MAX)?);
        save_head(&self.location.head_file, &head).map_err(StoreError::HeadMismatch)?;
        Ok(head)
    }

    /// Every record in order, or those up to and including `as_of`. A writer reads myko's store;
    /// a reader reads Postgres.
    pub fn records(&self, as_of: Option<u64>) -> Result<Vec<Record>, StoreError> {
        let rows = match &self.writer {
            Some(writer) => writer.memory_rows()?,
            None => self.with_sql(durable_rows)?.0,
        };
        parse_rows(&check_chain(rows, as_of.unwrap_or(u64::MAX))?)
    }

    /// The state as of the latest record, or as of `as_of`.
    pub fn ledger(&self, as_of: Option<u64>) -> Result<Ledger, StoreError> {
        replay(&self.records(as_of)?)
    }
}

impl Writer {
    /// Every record in myko's store, sorted by number, because the store keeps no order.
    fn memory_rows(&self) -> Result<Vec<Row>, StoreError> {
        let request = Arc::new(RequestContext::internal(
            Arc::from(Uuid::new_v4().to_string()),
            self.server.host_id,
            "rally",
        ));
        let items = self.ctx.query_snapshot(GetAllLedgerRecords {}, request);
        let mut rows = Vec::with_capacity(items.len());
        for item in items {
            let seq = parse_seq(&item.id.0)?;
            rows.push(Row {
                seq,
                text: item.record.clone(),
                hash: item.hash.clone(),
            });
        }
        rows.sort_by_key(|row| row.seq);
        Ok(rows)
    }

    /// The append itself, holding the writer lock throughout.
    fn append(&self, text: String) -> Result<Appended, StoreError> {
        let mut sql = self
            .sql
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let record = Record::parse(&text).map_err(|e| StoreError::Corrupt {
            seq: 0,
            reason: e.to_string(),
        })?;
        let chain = check_chain(self.memory_rows()?, u64::MAX)?;
        let mut ledger = replay(&parse_rows(&chain)?)?;
        check_head_on_chain(&self.location.head_file, &chain)?;
        let seq = ledger.last_seq() + 1;
        ledger.apply(seq, &record).map_err(StoreError::Refused)?;
        let previous = head_of(&chain);
        let hash = chain_hash(&previous.hash, seq, &text);

        // Postgres must still hold exactly what this writer replayed, or the chain would fork.
        self.reconnect_if_needed(&mut sql)?;
        let tail = durable_tail(&mut sql)?;
        if tail != (previous.seq, previous.hash.clone()) {
            return Err(StoreError::Diverged(format!(
                "Postgres ends at record {} but this service's copy ends at {}; restart it to replay",
                tail.0, previous.seq
            )));
        }
        let item = LedgerRecord {
            id: LedgerRecordId(Arc::from(seq.to_string())),
            record: text,
            hash: hash.clone(),
        };
        let event = myko::event::MEvent::from_item(
            &item,
            myko::event::MEventType::SET,
            &self.server.host_id.to_string(),
        );
        let event_json =
            serde_json::to_string(&event).map_err(|e| StoreError::Database(e.to_string()))?;
        let mut transaction = sql.transaction()?;
        transaction.execute(
            "INSERT INTO rally_events (item_type, item_id, change_type, created_at, tx, source_id, event)
             VALUES ($1, $2, 'SET', ($3::text)::timestamptz, $4, $5, ($6::text)::jsonb)",
            &[
                &ENTITY,
                &seq.to_string(),
                &&*event.created_at,
                &&*event.tx,
                &event.source_id.as_deref(),
                &event_json,
            ],
        )?;
        // If the connection fails during the commit, Postgres may or may not have the record. It
        // isn't acknowledged, and the next append's comparison refuses until a restart replays.
        transaction.commit()?;

        // Only now does myko's store hold it. Its persister is a blackhole, so this writes nothing
        // more to Postgres.
        self.ctx
            .set(&item)
            .map_err(|e| StoreError::Diverged(format!("myko refused a committed record: {e}")))?;
        // If this fails or the process dies here, the file is merely behind, which `verify` notes.
        let head = Head { seq, hash };
        if let Err(error) = save_head(&self.location.head_file, &head) {
            eprintln!("rally: warning: the head file wasn't updated: {error}");
        }
        Ok(Appended {
            seq,
            hash: head.hash,
        })
    }

    /// After Postgres restarts, the connection and its lock are gone. Takes them again, so that a
    /// second writer can't have slipped in unnoticed: the tail comparison then catches that.
    fn reconnect_if_needed(&self, sql: &mut Client) -> Result<(), StoreError> {
        if !sql.is_closed() && sql.simple_query("SELECT 1").is_ok() {
            return Ok(());
        }
        let mut fresh = connect(&self.location, false)?;
        take_writer_lock(&mut fresh, Duration::ZERO)?;
        *sql = fresh;
        Ok(())
    }
}

impl CommandHandler for AppendLedgerRecord {
    fn execute(self, ctx: CommandContext) -> Result<Self::Result, CommandError> {
        let writer = writers()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&ctx.req.host_id)
            .cloned();
        let fail = |message: String| {
            CommandError::new(ctx.req.tx.to_string(), ctx.command_id.to_string(), message)
        };
        let Some(writer) = writer else {
            return Err(fail("no writer is open in this process".to_owned()));
        };
        writer.append(self.record).map_err(|error| {
            let message = error.to_string();
            *writer
                .failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
            fail(message)
        })
    }
}

fn parse_seq(id: &str) -> Result<u64, StoreError> {
    // Only canonical decimal numbers, so that "01" and "1" can't both be record 1.
    match id.parse::<u64>() {
        Ok(seq) if seq > 0 && seq.to_string() == id => Ok(seq),
        _ => Err(StoreError::Corrupt {
            seq: 0,
            reason: format!("a record's id {id:?} isn't a sequence number"),
        }),
    }
}

/// The last row Postgres committed for the ledger: its number and hash, or the start of a chain.
fn durable_tail(sql: &mut Client) -> Result<(u64, String), StoreError> {
    let row = sql.query_opt(
        "SELECT item_id, event->'item'->>'hash' FROM rally_events
         WHERE item_type = 'LedgerRecord' ORDER BY id DESC LIMIT 1",
        &[],
    )?;
    match row {
        None => Ok((0, CHAIN_START.to_owned())),
        Some(row) => Ok((
            parse_seq(&row.get::<_, String>(0))?,
            row.get::<_, Option<String>>(1).unwrap_or_default(),
        )),
    }
}

/// The ledger as myko's replay would see it, read from Postgres without myko: the latest version of
/// each record, sorted by number. Also lists anything in the history that the protections should
/// have refused, such as a second version of a record or a deletion.
fn durable_rows(sql: &mut Client) -> Result<(Vec<Row>, Vec<String>), StoreError> {
    let mut transaction = sql
        .build_transaction()
        .read_only(true)
        .isolation_level(postgres::IsolationLevel::RepeatableRead)
        .start()?;
    let found = transaction.query(
        "SELECT item_id, change_type, event->'item'->>'record', event->'item'->>'hash',
                event->'item'->>'id'
         FROM rally_events WHERE item_type = 'LedgerRecord' ORDER BY id",
        &[],
    )?;
    transaction.commit()?;
    // myko picks each entity's latest row by this column, then keys it by the id inside the event.
    let mut latest: HashMap<String, Option<(String, String, String)>> = HashMap::new();
    let mut versions: HashMap<String, usize> = HashMap::new();
    for row in found {
        let id: String = row.get(0);
        let change: String = row.get(1);
        let value = (change == "SET").then(|| {
            (
                row.get::<_, Option<String>>(2).unwrap_or_default(),
                row.get::<_, Option<String>>(3).unwrap_or_default(),
                row.get::<_, Option<String>>(4).unwrap_or_default(),
            )
        });
        *versions.entry(id.clone()).or_default() += 1;
        latest.insert(id, value);
    }
    let mut history: Vec<(String, usize)> = versions
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .collect();
    history.sort();
    let mut history: Vec<String> = history
        .into_iter()
        .map(|(id, count)| {
            format!(
                "record {id} has {count} versions in Postgres, so it was rewritten or deleted \
                 past the append path"
            )
        })
        .collect();
    let mut rows = Vec::new();
    for (id, value) in latest {
        let Some((text, hash, own_id)) = value else {
            if !history
                .iter()
                .any(|problem| problem.starts_with(&format!("record {id} ")))
            {
                history.push(format!("record {id} was deleted in Postgres"));
            }
            continue;
        };
        if own_id != id {
            history.push(format!("record {id}'s row names itself {own_id:?}"));
        }
        rows.push(Row {
            seq: parse_seq(&own_id)?,
            text,
            hash,
        });
    }
    rows.sort_by_key(|row| row.seq);
    Ok((rows, history))
}

/// Says how myko's copy differs from Postgres, if it does.
fn differs(memory: &[Row], durable: &[Row]) -> Option<String> {
    if memory == durable {
        return None;
    }
    let first = memory
        .iter()
        .zip(durable)
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| memory.len().min(durable.len()));
    Some(format!(
        "the service's copy of the ledger differs from Postgres from position {} \
         ({} records in the copy, {} in Postgres)",
        first + 1,
        memory.len(),
        durable.len()
    ))
}

fn head_of(chain: &[Row]) -> Head {
    chain.last().map_or_else(
        || Head {
            seq: 0,
            hash: CHAIN_START.to_owned(),
        },
        |row| Head {
            seq: row.seq,
            hash: row.hash.clone(),
        },
    )
}

/// Checks there are no gaps and that the chain holds, for rows sorted by number, up to `limit`.
fn check_chain(rows: Vec<Row>, limit: u64) -> Result<Vec<Row>, StoreError> {
    let mut chain = Vec::new();
    let mut previous = CHAIN_START.to_owned();
    for (position, row) in rows.into_iter().enumerate() {
        if row.seq > limit {
            break;
        }
        // A removed record shows up here as a gap, rather than silently renumbering what follows.
        let expected = position as u64 + 1;
        if row.seq != expected {
            return Err(StoreError::Inconsistent(LedgerError::OutOfOrder {
                expected,
                found: row.seq,
            }));
        }
        if chain_hash(&previous, row.seq, &row.text) != row.hash {
            return Err(StoreError::Tampered { seq: row.seq });
        }
        previous.clone_from(&row.hash);
        chain.push(row);
    }
    Ok(chain)
}

fn parse_rows(chain: &[Row]) -> Result<Vec<Record>, StoreError> {
    chain
        .iter()
        .map(|row| {
            Record::parse(&row.text).map_err(|e: RecordError| StoreError::Corrupt {
                seq: row.seq,
                reason: e.to_string(),
            })
        })
        .collect()
}

/// Refuses unless the head file names a point on the current chain.
fn check_head_on_chain(head_file: &Path, chain: &[Row]) -> Result<(), StoreError> {
    let head = read_head_file(head_file)
        .map_err(StoreError::HeadMismatch)?
        .ok_or_else(|| StoreError::HeadMismatch("the head file is missing".to_owned()))?;
    let hash = if head.seq == 0 {
        Some(CHAIN_START)
    } else {
        usize::try_from(head.seq - 1)
            .ok()
            .and_then(|index| chain.get(index))
            .map(|row| row.hash.as_str())
    };
    match hash {
        Some(hash) if hash.eq_ignore_ascii_case(&head.hash) => Ok(()),
        _ => Err(StoreError::HeadMismatch(format!(
            "the head file at record {} isn't on the ledger's chain; run `rally check`",
            head.seq
        ))),
    }
}

fn save_head(head_file: &Path, head: &Head) -> Result<(), String> {
    // A name of its own, so that writers can't truncate each other's file before a rename.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut temporary = head_file.as_os_str().to_owned();
    temporary.push(format!(
        ".{}.{}.new",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let temporary = PathBuf::from(temporary);
    let text = serde_json::to_string(head).map_err(|e| e.to_string())?;
    write_private(&temporary, text.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::rename(&temporary, head_file).map_err(|e| e.to_string())?;
    // Make the rename survive a crash. Otherwise a new ledger could end up with records and no
    // head file, which refuses writes.
    let directory = head_file.parent().unwrap_or_else(|| Path::new("."));
    let directory = if directory.as_os_str().is_empty() {
        Path::new(".")
    } else {
        directory
    };
    std::fs::File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| e.to_string())
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

/// Adds a problem if `saved` isn't a point on this chain.
fn compare_head(chain: &[Row], saved: &Head, name: &str, problems: &mut Vec<String>) {
    if saved.seq == 0 {
        if !saved.hash.eq_ignore_ascii_case(CHAIN_START) {
            problems.push(format!("{name} at record 0 isn't the start of a chain"));
        }
        return;
    }
    let row = usize::try_from(saved.seq - 1)
        .ok()
        .and_then(|i| chain.get(i));
    match row {
        None => problems.push(format!(
            "{name} is at record {} but the ledger ends at {}: records were removed",
            saved.seq,
            chain.len()
        )),
        Some(row) if !row.hash.eq_ignore_ascii_case(&saved.hash) => problems.push(format!(
            "{name} doesn't match record {}: the ledger was rewritten",
            saved.seq
        )),
        Some(_) => {}
    }
}

fn replay(records: &[Record]) -> Result<Ledger, StoreError> {
    Ledger::replay(records).map_err(StoreError::Inconsistent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::DeleteLedgerRecord;
    use crate::ledger::State;
    use crate::record::{Kind, Source};
    use crate::scratch_pg::{self, Cluster, Database};
    use serde_json::{Value, json};

    struct Scratch {
        // Fields drop in order: the store before the database it uses.
        store: Option<Store>,
        location: Location,
        db: Database<'static>,
        dir: tempfile::TempDir,
    }

    impl Scratch {
        fn new() -> Self {
            let db = scratch_pg::shared().database();
            let dir = tempfile::tempdir().unwrap();
            let location = Location {
                url: db.url.clone(),
                head_file: dir.path().join("ledger.head"),
            };
            let store = Some(Store::open(&location).unwrap());
            Self {
                store,
                location,
                db,
                dir,
            }
        }

        fn store(&mut self) -> &mut Store {
            self.store.as_mut().unwrap()
        }

        /// A real restart of the writer: myko replays everything from Postgres.
        fn restart(&mut self) {
            self.store = None;
            self.store = Some(Store::open(&self.location).unwrap());
        }

        fn reader(&self) -> Store {
            Store::open_read_only(&self.location).unwrap()
        }

        fn sql(&self, statements: &str) {
            self.db.connect().batch_execute(statements).unwrap();
        }

        fn head_file(&self) -> PathBuf {
            self.location.head_file.clone()
        }
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

    fn release() -> Record {
        team(json!({"change": "release", "story": "RAL-3", "team": "rally"}))
    }

    fn note() -> Record {
        team(json!({"change": "correct", "story": "RAL-3", "corrects": 1, "note": "again"}))
    }

    fn two_records() -> Scratch {
        let mut scratch = Scratch::new();
        scratch.store().append(&ready()).unwrap();
        scratch.store().append(&claim()).unwrap();
        scratch
    }

    const ON_RECORD_1: &str = "item_type = 'LedgerRecord' AND item_id = '1'";

    fn altered_record(scratch: &Scratch) {
        scratch.sql(&format!(
            "ALTER TABLE rally_events DISABLE TRIGGER rally_no_update;
             UPDATE rally_events SET event = jsonb_set(event, '{{item,record}}',
                 to_jsonb(replace(event->'item'->>'record', 'alice', 'mallory')))
             WHERE {ON_RECORD_1};"
        ));
    }

    fn remove(scratch: &Scratch, ids: &str) {
        scratch.sql(&format!(
            "ALTER TABLE rally_events DISABLE TRIGGER rally_no_delete;
             DELETE FROM rally_events WHERE item_type = 'LedgerRecord' AND item_id IN ({ids});"
        ));
    }

    #[test]
    fn appended_records_survive_a_restart() {
        let mut scratch = Scratch::new();
        assert_eq!(scratch.store().append(&ready()).unwrap(), 1);
        assert_eq!(scratch.store().append(&claim()).unwrap(), 2);
        scratch.restart();
        assert_eq!(
            scratch.store().records(None).unwrap(),
            vec![ready(), claim()]
        );
        let ledger = scratch.store().ledger(None).unwrap();
        assert_eq!(ledger.story("RAL-3").unwrap().state, State::Claimed);
        assert_eq!(
            scratch.reader().records(None).unwrap(),
            vec![ready(), claim()]
        );
    }

    #[test]
    fn each_open_gets_a_new_host_id_and_replay_still_returns_every_record() {
        // A fixed host id made myko's replay skip every row as its own, in the spike.
        let mut scratch = two_records();
        let mut seen = vec![scratch.store().host_id().unwrap()];
        for _ in 0..3 {
            scratch.restart();
            let id = scratch.store().host_id().unwrap();
            assert!(!seen.contains(&id), "a host id was reused");
            seen.push(id);
            assert_eq!(
                scratch.store().records(None).unwrap(),
                vec![ready(), claim()]
            );
        }
    }

    #[test]
    fn records_are_sorted_by_number_and_numbers_are_never_reused() {
        let mut scratch = Scratch::new();
        scratch.store().append(&ready()).unwrap();
        for _ in 0..11 {
            scratch.store().append(&note()).unwrap();
        }
        scratch.restart();
        // Sorted as text, 10 would come before 2 and the chain wouldn't hold.
        assert_eq!(scratch.store().ledger(None).unwrap().last_seq(), 12);
        assert!(scratch.store().verify(None).unwrap().ok);
        assert_eq!(scratch.store().append(&note()).unwrap(), 13);
        scratch.restart();
        assert_eq!(scratch.store().append(&note()).unwrap(), 14);

        // Postgres itself refuses a second row for a number, however it's written.
        let error = scratch
            .db
            .connect()
            .batch_execute(
                "INSERT INTO rally_events (item_type, item_id, change_type, tx, event)
                 SELECT item_type, item_id, 'SET', tx, event FROM rally_events
                 WHERE item_type = 'LedgerRecord' AND item_id = '14'",
            )
            .unwrap_err();
        assert!(describe(&error).contains("rally_records_once"), "{error}");
    }

    #[test]
    fn a_refused_change_leaves_no_record() {
        let mut scratch = Scratch::new();
        assert!(matches!(
            scratch.store().append(&claim()),
            Err(StoreError::Refused(LedgerError::UnknownStory(_)))
        ));
        assert!(scratch.store().records(None).unwrap().is_empty());
        scratch.restart();
        assert!(scratch.store().records(None).unwrap().is_empty());
    }

    #[test]
    fn can_read_the_state_as_of_a_past_record() {
        let mut scratch = two_records();
        scratch.restart();
        for store in [scratch.reader(), scratch.store.take().unwrap()] {
            let then = store.ledger(Some(1)).unwrap();
            assert_eq!(then.story("RAL-3").unwrap().state, State::Ready);
            assert_eq!(then.last_seq(), 1);
        }
    }

    #[test]
    fn the_log_refuses_updates_deletes_and_deletion_events() {
        let scratch = two_records();
        let mut sql = scratch.db.connect();
        for statement in [
            format!("UPDATE rally_events SET event = '{{}}' WHERE {ON_RECORD_1}"),
            format!("DELETE FROM rally_events WHERE {ON_RECORD_1}"),
            "TRUNCATE rally_events".to_owned(),
            "INSERT INTO rally_events (item_type, item_id, change_type, tx, event)
             VALUES ('LedgerRecord', '3', 'DEL', 'x', '{}')"
                .to_owned(),
        ] {
            let error = sql.batch_execute(&statement).unwrap_err();
            assert!(
                describe(&error).contains("append-only"),
                "{statement}: {error}"
            );
        }
        assert_eq!(
            scratch.reader().records(None).unwrap(),
            vec![ready(), claim()]
        );
    }

    #[test]
    fn an_untouched_ledger_verifies() {
        let mut scratch = two_records();
        let result = scratch.store().verify(None).unwrap();
        assert!(result.ok, "{:?}", result.problems);
        assert_eq!(result.head.seq, 2);
        assert!(scratch.reader().verify(None).unwrap().ok);
    }

    #[test]
    fn an_empty_ledger_verifies() {
        let mut scratch = Scratch::new();
        assert!(scratch.store().verify(None).unwrap().ok);
    }

    #[test]
    fn an_altered_record_is_detected_and_not_read() {
        let mut scratch = two_records();
        altered_record(&scratch);
        // The running writer's check reads Postgres, so it sees the change at once.
        let result = scratch.store().verify(None).unwrap();
        assert!(!result.ok);
        assert!(
            result.problems[0].contains("record 1"),
            "{:?}",
            result.problems
        );
        assert!(matches!(
            scratch.reader().ledger(None),
            Err(StoreError::Tampered { seq: 1 })
        ));
        scratch.restart();
        assert!(matches!(
            scratch.store().ledger(None),
            Err(StoreError::Tampered { seq: 1 })
        ));
        assert!(scratch.store().append(&release()).is_err());
    }

    #[test]
    fn a_removed_record_is_detected() {
        let mut scratch = two_records();
        remove(&scratch, "'1'");
        assert!(!scratch.store().verify(None).unwrap().ok);
        assert!(!scratch.reader().verify(None).unwrap().ok);
        scratch.restart();
        assert!(!scratch.store().verify(None).unwrap().ok);
    }

    #[test]
    fn reordered_records_are_detected() {
        let mut scratch = two_records();
        scratch.sql(
            "ALTER TABLE rally_events DISABLE TRIGGER rally_no_update;
             UPDATE rally_events e SET event = jsonb_set(jsonb_set(e.event,
                 '{item,record}', o.event->'item'->'record'), '{item,hash}', o.event->'item'->'hash')
             FROM rally_events o
             WHERE e.item_type = 'LedgerRecord' AND o.item_type = 'LedgerRecord'
               AND ((e.item_id = '1' AND o.item_id = '2') OR (e.item_id = '2' AND o.item_id = '1'));",
        );
        assert!(!scratch.reader().verify(None).unwrap().ok);
        scratch.restart();
        assert!(!scratch.store().verify(None).unwrap().ok);
        assert!(scratch.store().ledger(None).is_err());
    }

    #[test]
    fn removing_the_newest_records_is_detected_by_the_head_file() {
        let mut scratch = two_records();
        remove(&scratch, "'2'");
        let result = scratch.reader().verify(None).unwrap();
        assert!(!result.ok);
        assert!(
            result.problems[0].contains("removed"),
            "{:?}",
            result.problems
        );
        // The shorter chain is valid by itself, which is why the file is needed.
        assert_eq!(scratch.reader().ledger(None).unwrap().last_seq(), 1);
        scratch.restart();
        assert!(!scratch.store().verify(None).unwrap().ok);
    }

    #[test]
    fn a_saved_head_catches_what_the_head_file_cannot() {
        let mut scratch = two_records();
        let saved = scratch.store().verify(None).unwrap().head;
        remove(&scratch, "'2'");
        let stale = Head {
            seq: 1,
            hash: scratch.reader().verify(None).unwrap().head.hash,
        };
        std::fs::write(scratch.head_file(), serde_json::to_string(&stale).unwrap()).unwrap();
        scratch.restart();

        assert!(
            scratch.store().verify(None).unwrap().ok,
            "the file alone is fooled"
        );
        let result = scratch.store().verify(Some(&saved)).unwrap();
        assert!(!result.ok);
        assert!(
            result.problems[0].contains("removed"),
            "{:?}",
            result.problems
        );
    }

    #[test]
    fn a_rewritten_ledger_fails_against_a_saved_head() {
        let mut scratch = two_records();
        let mut saved = scratch.store().verify(None).unwrap().head;
        saved.hash = "f".repeat(64);
        let result = scratch.store().verify(Some(&saved)).unwrap();
        assert!(!result.ok);
        assert!(
            result.problems[0].contains("rewritten"),
            "{:?}",
            result.problems
        );
    }

    #[test]
    fn a_head_file_behind_the_ledger_after_a_crash_still_verifies() {
        let mut scratch = two_records();
        let behind = std::fs::read_to_string(scratch.head_file()).unwrap();
        scratch.store().append(&release()).unwrap();
        std::fs::write(scratch.head_file(), behind).unwrap();
        let result = scratch.store().verify(None).unwrap();
        assert!(result.ok);
        assert!(result.notes[0].contains("behind"), "{:?}", result.notes);
    }

    #[test]
    fn a_missing_head_file_is_reported() {
        let mut scratch = two_records();
        std::fs::remove_file(scratch.head_file()).unwrap();
        assert!(!scratch.store().verify(None).unwrap().ok);
    }

    #[test]
    fn removing_the_newest_records_cannot_heal_on_the_next_write() {
        let mut scratch = two_records();
        remove(&scratch, "'2'");
        // Whether or not the writer has restarted since.
        assert!(
            scratch.store().append(&claim()).is_err(),
            "the next write should be refused"
        );
        scratch.restart();
        assert!(
            scratch.store().append(&claim()).is_err(),
            "the next write should be refused"
        );
        assert!(
            !scratch.store().verify(None).unwrap().ok,
            "the removal must still show"
        );
    }

    #[test]
    fn a_missing_head_file_refuses_writes() {
        let mut scratch = two_records();
        std::fs::remove_file(scratch.head_file()).unwrap();
        assert!(scratch.store().append(&release()).is_err());
        scratch.restart();
        assert!(scratch.store().append(&release()).is_err());
    }

    #[test]
    fn a_new_ledger_can_survive_a_crash_before_its_first_head_save() {
        let mut scratch = Scratch::new();
        assert!(scratch.head_file().exists());
        scratch.store().append(&ready()).unwrap();
        assert!(scratch.store().verify(None).unwrap().ok);
    }

    #[test]
    fn recomputing_one_records_hash_fails_at_the_next_record() {
        let mut scratch = two_records();
        let forged = ready().to_json().replace("alice", "mallory");
        let hash = chain_hash(CHAIN_START, 1, &forged);
        let mut sql = scratch.db.connect();
        sql.batch_execute("ALTER TABLE rally_events DISABLE TRIGGER rally_no_update")
            .unwrap();
        sql.execute(
            &format!(
                "UPDATE rally_events SET event = jsonb_set(jsonb_set(event,
                     '{{item,record}}', to_jsonb($1::text)), '{{item,hash}}', to_jsonb($2::text))
                 WHERE {ON_RECORD_1}"
            ),
            &[&forged, &hash],
        )
        .unwrap();
        scratch.restart();
        assert!(matches!(
            scratch.store().ledger(None),
            Err(StoreError::Tampered { seq: 2 })
        ));
        assert!(!scratch.store().verify(None).unwrap().ok);
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
        let mut scratch = two_records();
        let junk = Head {
            seq: 0,
            hash: "junk".into(),
        };
        assert!(!scratch.store().verify(Some(&junk)).unwrap().ok);
        let start = Head {
            seq: 0,
            hash: CHAIN_START.into(),
        };
        assert!(scratch.store().verify(Some(&start)).unwrap().ok);

        remove(&scratch, "'2'");
        std::fs::write(scratch.head_file(), serde_json::to_string(&junk).unwrap()).unwrap();
        assert!(!scratch.reader().verify(None).unwrap().ok);
    }

    #[test]
    fn a_saved_head_in_upper_case_still_matches() {
        let mut scratch = two_records();
        let mut head = scratch.store().verify(None).unwrap().head;
        head.hash = head.hash.to_uppercase();
        assert!(scratch.store().verify(Some(&head)).unwrap().ok);
    }

    #[test]
    fn adopting_the_head_recovers_from_a_lost_head_file() {
        let mut scratch = two_records();
        std::fs::remove_file(scratch.head_file()).unwrap();
        assert!(scratch.store().append(&release()).is_err());

        let head = scratch.reader().adopt_head().unwrap();
        assert_eq!(head.seq, 2);
        assert!(scratch.store().verify(None).unwrap().ok);
        scratch.store().append(&release()).unwrap();
    }

    #[test]
    fn adopting_the_head_refuses_a_broken_chain() {
        let scratch = two_records();
        altered_record(&scratch);
        assert!(matches!(
            scratch.reader().adopt_head(),
            Err(StoreError::Tampered { seq: 1 })
        ));
    }

    #[test]
    fn no_temporary_head_files_are_left_behind() {
        let scratch = two_records();
        let names: Vec<_> = std::fs::read_dir(scratch.dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(
            !names.iter().any(|name| name.ends_with(".new")),
            "{names:?}"
        );
    }

    #[test]
    fn a_read_only_store_reads_but_cannot_write() {
        let scratch = two_records();
        let mut reader = scratch.reader();
        assert_eq!(reader.ledger(None).unwrap().last_seq(), 2);
        assert!(reader.verify(None).unwrap().ok);
        assert!(reader.append(&release()).is_err());
        assert_eq!(reader.ledger(None).unwrap().last_seq(), 2);
        // Its connection is read-only in Postgres too.
        let error = reader
            .with_sql(|sql| Ok(sql.batch_execute("CREATE TABLE nope (x int)")))
            .unwrap()
            .unwrap_err();
        assert!(describe(&error).contains("read-only"), "{error}");
    }

    #[test]
    fn opening_a_missing_ledger_for_reading_creates_nothing() {
        let db = scratch_pg::shared().database();
        let dir = tempfile::tempdir().unwrap();
        let location = Location {
            url: db.url.clone(),
            head_file: dir.path().join("ledger.head"),
        };
        assert!(matches!(
            Store::open_read_only(&location),
            Err(StoreError::NoLedger(_))
        ));
        let missing = Location {
            url: format!("{}_missing", db.url),
            head_file: location.head_file.clone(),
        };
        assert!(matches!(
            Store::open_read_only(&missing),
            Err(StoreError::NoLedger(_))
        ));
        let row = db
            .connect()
            .query_one("SELECT to_regclass('rally_events') IS NULL", &[])
            .unwrap();
        assert!(row.get::<_, bool>(0));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_damaged_log_is_reported_not_read() {
        let mut scratch = Scratch::new();
        scratch.store().append(&ready()).unwrap();
        scratch.sql(&format!(
            "ALTER TABLE rally_events DISABLE TRIGGER rally_no_update;
             UPDATE rally_events SET event = jsonb_set(event, '{{item,record}}', '\"garbage\"')
             WHERE {ON_RECORD_1};"
        ));
        scratch.restart();
        assert!(matches!(
            scratch.store().ledger(None),
            Err(StoreError::Tampered { seq: 1 })
        ));
    }

    #[test]
    fn a_gap_in_the_log_is_reported() {
        let mut scratch = two_records();
        remove(&scratch, "'1'");
        scratch.restart();
        assert!(matches!(
            scratch.store().ledger(None),
            Err(StoreError::Inconsistent(LedgerError::OutOfOrder { .. }))
        ));
    }

    #[test]
    fn concurrent_writers_keep_one_chain_and_none_is_told_a_false_success() {
        let mut scratch = two_records();
        scratch.store = None;
        let writers: Vec<_> = (0..8)
            .map(|_| {
                let location = scratch.location.clone();
                std::thread::spawn(move || {
                    // Each is a writer of its own, as separate processes would be, so only the
                    // Postgres lock keeps them apart.
                    let mut store = Store::open_waiting(&location, Duration::from_secs(120))?;
                    let mut seqs = Vec::new();
                    for _ in 0..3 {
                        seqs.push(store.append(&note())?);
                    }
                    Ok::<_, StoreError>(seqs)
                })
            })
            .collect();
        let mut positions: Vec<u64> = writers
            .into_iter()
            .flat_map(|writer| writer.join().unwrap().unwrap())
            .collect();
        positions.sort_unstable();
        assert_eq!(positions, (3..=26).collect::<Vec<u64>>());
        let reader = scratch.reader();
        assert_eq!(reader.ledger(None).unwrap().last_seq(), 26);
        assert!(reader.verify(None).unwrap().ok);
    }

    #[test]
    fn a_second_writer_is_refused_while_one_holds_the_lock() {
        let scratch = two_records();
        let started = Instant::now();
        assert!(matches!(
            Store::open_waiting(&scratch.location, Duration::from_millis(300)),
            Err(StoreError::Busy)
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
        // Reading needs no lock.
        assert!(scratch.reader().verify(None).unwrap().ok);
    }

    #[test]
    fn in_process_threads_sharing_one_writer_keep_one_chain() {
        let mut scratch = two_records();
        let store = Arc::new(Mutex::new(scratch.store.take().unwrap()));
        let writers: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || {
                    (0..5)
                        .map(|_| store.lock().unwrap().append(&note()).unwrap())
                        .collect::<Vec<u64>>()
                })
            })
            .collect();
        let mut positions: Vec<u64> = writers
            .into_iter()
            .flat_map(|writer| writer.join().unwrap())
            .collect();
        positions.sort_unstable();
        assert_eq!(positions, (3..=42).collect::<Vec<u64>>());
        assert!(store.lock().unwrap().verify(None).unwrap().ok);
    }

    #[test]
    fn nothing_written_into_myko_past_the_append_reaches_postgres_and_check_sees_it() {
        // No client can send these, because nothing listens. Even run inside this process, the
        // generated delete command and a raw event only change myko's copy, and the check says so.
        let mut scratch = two_records();
        let ctx = scratch.store().myko_context().unwrap();
        let host = scratch.store().host_id().unwrap();
        let request = Arc::new(RequestContext::internal(Arc::from("t"), host, "test"));
        let command = CommandContext::new(Arc::from("DeleteLedgerRecord"), request, ctx.clone());
        DeleteLedgerRecord {
            id: LedgerRecordId(Arc::from("2")),
        }
        .execute(command)
        .unwrap();
        let result = scratch.store().verify(None).unwrap();
        assert!(!result.ok);
        assert!(
            result
                .problems
                .iter()
                .any(|p| p.contains("differs from Postgres")),
            "{result:?}"
        );
        assert!(matches!(
            scratch.store().append(&release()),
            Err(StoreError::Diverged(_) | StoreError::HeadMismatch(_))
        ));

        let forged = LedgerRecord {
            id: LedgerRecordId(Arc::from("3")),
            record: "forged".into(),
            hash: "f".repeat(64),
        };
        let event = myko::event::MEvent::from_item(&forged, myko::event::MEventType::SET, "x");
        ctx.apply_event(event).unwrap();
        assert!(!scratch.store().verify(None).unwrap().ok);

        let durable = scratch.reader().records(None).unwrap();
        assert_eq!(durable, vec![ready(), claim()]);
        scratch.restart();
        assert!(scratch.store().verify(None).unwrap().ok);
        assert_eq!(scratch.store().append(&release()).unwrap(), 3);
    }

    #[test]
    fn an_append_is_acknowledged_only_once_postgres_has_it() {
        let cluster = Cluster::start();
        let db = cluster.database();
        let dir = tempfile::tempdir().unwrap();
        let location = Location {
            url: db.url.clone(),
            head_file: dir.path().join("ledger.head"),
        };
        let mut store = Store::open(&location).unwrap();
        let mut acknowledged = vec![
            store.append(&ready()).unwrap(),
            store.append(&claim()).unwrap(),
        ];

        cluster.stop();
        for _ in 0..3 {
            let started = Instant::now();
            let result = store.append(&release());
            assert!(matches!(result, Err(StoreError::Database(_))), "{result:?}");
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "{:?}",
                started.elapsed()
            );
        }
        // myko's copy didn't take the failed record either.
        assert_eq!(store.ledger(None).unwrap().last_seq(), 2);

        cluster.restart();
        // The writer reconnects and takes its lock again.
        acknowledged.push(store.append(&release()).unwrap());
        assert_eq!(acknowledged, [1, 2, 3]);
        drop(store);
        let mut store = Store::open(&location).unwrap();
        assert_eq!(
            store.records(None).unwrap(),
            vec![ready(), claim(), release()]
        );
        assert!(store.verify(None).unwrap().ok);
        assert_eq!(store.append(&note()).unwrap(), 4);
        drop(store);
        drop(db);
        let directory = cluster.directory().unwrap().to_owned();
        drop(cluster);
        assert!(!directory.exists(), "the scratch Postgres was left behind");
    }

    #[cfg(unix)]
    #[test]
    fn the_head_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = two_records();
        let mode = std::fs::metadata(scratch.head_file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
