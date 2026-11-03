//! The append path, and a barrier that waits for the Postgres producer to drain.
use std::sync::{Arc, Mutex, OnceLock, atomic::Ordering};
use std::time::{Duration, Instant};

use myko::{
    command::{CommandContext, CommandError, CommandHandler},
    myko_command,
    prelude::EventPublishing as _,
    server::PersistHealth,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::record::{GetAllLedgerRecords, LedgerRecord, LedgerRecordId};

pub const CHAIN_START: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Copied from Rally's `src/store.rs`.
pub fn chain_hash(previous: &str, seq: u64, record: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(previous.as_bytes());
    hash.update(b"\n");
    hash.update(seq.to_string().as_bytes());
    hash.update(b"\n");
    hash.update(record.as_bytes());
    hex::encode(hash.finalize())
}

/// The one writer. `SPIKE_NO_APPEND_LOCK` removes it, to see what ordering depends on it.
static WRITER: Mutex<()> = Mutex::new(());
pub static HEALTH: OnceLock<Arc<PersistHealth>> = OnceLock::new();

#[myko_command(AppendResult)]
pub struct AppendRecord {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppendResult {
    pub seq: u64,
    pub hash: String,
}

impl CommandHandler for AppendRecord {
    fn execute(self, ctx: CommandContext) -> Result<Self::Result, CommandError> {
        let _writer = if std::env::var_os("SPIKE_NO_APPEND_LOCK").is_some() {
            None
        } else {
            Some(WRITER.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
        };
        let records: Vec<Arc<LedgerRecord>> = ctx.exec_query(GetAllLedgerRecords {})?;
        let head = records
            .iter()
            .filter_map(|r| r.id.0.parse::<u64>().ok().map(|seq| (seq, r)))
            .max_by_key(|(seq, _)| *seq);
        let (seq, previous) = head.map_or((1, CHAIN_START.to_owned()), |(seq, r)| {
            (seq + 1, r.hash.clone())
        });
        let hash = chain_hash(&previous, seq, &self.text);
        ctx.emit_set(&LedgerRecord {
            id: LedgerRecordId(Arc::from(seq.to_string())),
            text: self.text,
            hash: hash.clone(),
        })?;
        Ok(AppendResult { seq, hash })
    }
}

#[myko_command(BarrierResult)]
pub struct DurableBarrier {
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BarrierResult {
    pub drained: bool,
    pub queued: u64,
    pub total_persisted: u64,
    pub total_errors: u64,
    pub last_error: Option<String>,
}

pub fn wait_for_drain(timeout: Duration) -> BarrierResult {
    let health = HEALTH.get().expect("health is set at startup");
    let started = Instant::now();
    while health.queued.load(Ordering::Relaxed) != 0 && started.elapsed() < timeout {
        std::thread::sleep(Duration::from_millis(5));
    }
    let queued = health.queued.load(Ordering::Relaxed);
    BarrierResult {
        drained: queued == 0,
        queued,
        total_persisted: health.total_persisted.load(Ordering::Relaxed),
        total_errors: health.total_errors.load(Ordering::Relaxed),
        last_error: health.last_error.read().ok().and_then(|e| e.clone()),
    }
}

impl CommandHandler for DurableBarrier {
    fn execute(self, _ctx: CommandContext) -> Result<Self::Result, CommandError> {
        Ok(wait_for_drain(Duration::from_millis(self.timeout_ms)))
    }
}
