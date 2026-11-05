//! Rally: a ledger of progress for a muster of agents. See `docs/roadmap.md`.

pub mod access;
pub mod backup;
pub mod brief;
pub mod client;
pub mod designation;
pub mod digest;
pub mod entity;
pub mod evidence;
pub mod init;
pub mod ledger;
pub mod levi;
pub mod next;
pub mod record;
#[doc(hidden)]
pub mod scratch_pg;
pub mod secrets;
pub mod server;
pub mod session;
pub mod stall;
pub mod store;
pub mod views;
pub mod wake;
pub mod web;
