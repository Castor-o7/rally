//! Rally: a ledger of progress for a muster of agents. See `docs/roadmap.md`.

/// The commit this build was made from, when it was made from a clean git checkout (RAL-100), or
/// empty otherwise: two builds of one commit aren't byte-identical, so the commit is what says
/// which code is running (RAL-156).
pub const COMMIT: &str = env!("RALLY_COMMIT");

/// The build's commit, or `None` for a build from a checkout with uncommitted changes, or with no
/// git at all, which no commit describes (RAL-156).
pub fn commit() -> Option<&'static str> {
    (!COMMIT.is_empty()).then_some(COMMIT)
}

/// What `rally --version` says: the package version and the commit it was built from (RAL-156).
pub fn version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| match commit() {
        Some(commit) => format!("{} (commit {commit})", env!("CARGO_PKG_VERSION")),
        None => format!(
            "{} (no commit: built from a checkout with uncommitted changes)",
            env!("CARGO_PKG_VERSION")
        ),
    })
}

pub mod access;
pub mod backup;
pub mod brief;
pub mod builds;
pub mod client;
pub mod designation;
pub mod digest;
pub mod digest_loop;
pub mod entity;
pub mod evidence;
pub mod gate;
pub mod gate_log;
pub mod inbox;
pub mod init;
pub mod ledger;
pub mod levi;
pub mod lock_holder;
pub mod merge_notes;
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
