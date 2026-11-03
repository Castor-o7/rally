//! The ledger's myko entity, and the one command that appends to it (RAL-39).
//!
//! Each record is its own `LedgerRecord`, keyed by its sequence number as a decimal string. myko
//! keeps only the latest version of each entity and gives no order, so readers sort by the number
//! (see `store.rs`). `#[myko_item]` also generates a `DeleteLedgerRecord` command and accepts raw
//! events, which would write past the checks if any client could reach them. None can: Rally never
//! starts myko's listener, so the only way in is Rally's own service (`docs/design.md`).

use myko::myko_command;
use myko::myko_item;
use myko::prelude::{EventPublishing as _, Querying as _, RegistryScoped as _, RequestScoped as _};
use serde::{Deserialize, Serialize};

/// One ledger record.
#[myko_item]
pub struct LedgerRecord {
    /// The record's JSON, byte for byte as the chain hash covers it.
    pub record: String,
    /// The chain hash this record claims.
    pub hash: String,
}

/// Appends one record after the ledger's checks. Only `Store::append` runs it, in this process.
#[myko_command(Appended)]
pub struct AppendLedgerRecord {
    /// The record's JSON.
    pub record: String,
}

/// Where an appended record went.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appended {
    pub seq: u64,
    pub hash: String,
}
