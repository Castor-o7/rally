//! Rally: a ledger of progress for a muster of agents. See `docs/roadmap.md`.

pub mod access;
pub mod backup;
pub mod brief;
pub mod client;
pub mod designation;
pub mod digest;
pub mod digest_loop;
pub mod entity;
pub mod evidence;
pub mod gate;
pub mod inbox;
pub mod init;
pub mod ledger;
pub mod levi;
pub mod lock_holder;
pub mod next;
pub mod record;
pub mod resume;
pub mod retry;
pub mod roadmaps;
#[doc(hidden)]
pub mod scratch_pg;
pub mod secrets;
pub mod server;
pub mod session;
pub mod shims;
pub mod stall;
pub mod store;
pub mod view;
pub mod views;
pub mod wake;
pub mod watchman_brief;
pub mod web;
