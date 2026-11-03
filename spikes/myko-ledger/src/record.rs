//! One ledger record: `id` is the sequence number as a decimal string.
use myko::myko_item;
use myko::prelude::{EventPublishing as _, Querying as _, RegistryScoped as _, RequestScoped as _};

#[myko_item]
pub struct LedgerRecord {
    /// The record's text, byte for byte. `store.rs` hashes this.
    pub text: String,
    /// The chain hash this record claims.
    pub hash: String,
}
