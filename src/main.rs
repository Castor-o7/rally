//! The `rally` command. It reads and writes through the service once paired, or works directly on a
//! ledger's Postgres database with `--db`, where nothing checks who is writing.
//!
//! All output is JSON, so that control characters in recorded text are escaped rather than reaching
//! the terminal.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

use rally::access::{Pairing, StateDir, write_private};
use rally::client;
use rally::ledger::{State, TeamStatus};
use rally::record::{Kind, Record, Source};
use rally::server::{self, RateLimit};
use rally::store::{Head, Location, Store};
use rally::views::{self, Page};
use rally::web::{
    self, BackupWatch, DEFAULT_PROJECTS, LedgerReader, LocalSummaries, PastReader, Project,
};

/// A ledger of progress for a muster of agents.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Work directly on the ledger in this Postgres database, such as
    /// postgres://rally@127.0.0.1/rally, rather than through the service. Needs --head-file.
    #[arg(long, env = "RALLY_DB", hide_env_values = true)]
    db: Option<String>,
    /// With --db: the ledger's head file, kept outside Postgres so that removing the newest
    /// records shows.
    #[arg(long, env = "RALLY_HEAD_FILE")]
    head_file: Option<PathBuf>,
    /// With --db: a file holding the Postgres role's password, readable only by this account, so
    /// the password stays out of the command line and the URL.
    #[arg(long, env = "RALLY_DB_PASSWORD_FILE", hide_env_values = true)]
    db_password_file: Option<PathBuf>,
    /// The pairing file. Defaults to ~/.config/rally/client.json.
    #[arg(long, env = "RALLY_CONFIG")]
    config: Option<PathBuf>,
    /// The credential to record when writing directly with --db. The service sets its own.
    #[arg(long, env = "RALLY_CREDENTIAL")]
    credential: Option<String>,
    /// Who you declare yourself to be.
    #[arg(long = "as", env = "RALLY_NAME")]
    name: Option<String>,
    /// With --entered-by: record this write as an entry made on the fallback board during an
    /// outage, at this time in milliseconds since the Unix epoch. The record keeps its own time
    /// too, so the gap shows.
    #[arg(long, requires = "entered_by")]
    entered_at_ms: Option<i64>,
    /// With --entered-at-ms: who made the entry on the fallback board. The entry is checked as
    /// that name, so a backfilled verification is still refused for its own submitter.
    #[arg(long, requires = "entered_at_ms")]
    entered_by: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the service on this machine. Needs --db and --head-file. Prints where it's listening.
    Serve {
        /// The address to listen on, such as a LAN address and port. Never the open internet.
        #[arg(long)]
        bind: String,
        #[arg(long, env = "RALLY_STATE_DIR")]
        state_dir: PathBuf,
        /// Requests a credential can make at once.
        #[arg(long, default_value_t = RateLimit::default().burst)]
        burst: f64,
        /// Requests a credential regains per second.
        #[arg(long, default_value_t = RateLimit::default().per_second)]
        per_second: f64,
        /// Writes a credential can make at once.
        #[arg(long, default_value_t = RateLimit::default().write_burst)]
        write_burst: f64,
        /// Writes a credential regains per second.
        #[arg(long, default_value_t = RateLimit::default().write_per_second)]
        write_per_second: f64,
        /// Refuse writes once the ledger's table in Postgres, with its indexes, reaches this size.
        #[arg(long, default_value_t = RateLimit::default().max_ledger_bytes)]
        max_ledger_bytes: u64,
        /// Refuse writes while the disk under Postgres's data has less free space than this.
        #[arg(long, default_value_t = RateLimit::default().min_free_bytes)]
        min_free_bytes: u64,
        /// Connections open at once, from anyone.
        #[arg(long, default_value_t = RateLimit::default().max_connections)]
        max_connections: usize,
        /// Connections open at once from any one address.
        #[arg(long, default_value_t = RateLimit::default().max_connections_per_address)]
        max_connections_per_address: usize,
        /// How long a connection may last, authenticated or not.
        #[arg(long, default_value_t = RateLimit::default().connection_seconds)]
        connection_seconds: f64,
        /// Missing or wrong tokens one address can present at once.
        #[arg(long, default_value_t = RateLimit::default().failed_token_burst)]
        failed_token_burst: f64,
        /// Failed token attempts an address regains per second.
        #[arg(long, default_value_t = RateLimit::default().failed_token_per_second)]
        failed_token_per_second: f64,
        /// Checks and past-state reads a credential can make at once.
        #[arg(long, default_value_t = RateLimit::default().heavy_burst)]
        heavy_burst: f64,
        /// Checks and past-state reads a credential regains per second.
        #[arg(long, default_value_t = RateLimit::default().heavy_per_second)]
        heavy_per_second: f64,
        /// Refuse to start unless this binary and the operator key, and every directory above
        /// each, are owned by root and writable by no one else, so that no agent on this host can
        /// change either. It prints the operator's sudo steps if not. Needed to accept watchman
        /// designations (RAL-51). See docs/designation.md.
        #[arg(long, requires = "operator_key")]
        secure: bool,
        /// With --secure: the operator's OpenSSH public key, which designations must be signed by.
        #[arg(long, requires = "secure")]
        operator_key: Option<PathBuf>,
        /// How long a team waits for the operator's answer before it can shelve the story and
        /// take its next one (RAL-56): a number with s, m, h or d. The operator agreed 5 minutes.
        #[arg(long, env = "RALLY_OPERATOR_WINDOW", default_value = "5m", value_parser = parse_window)]
        operator_window: u64,
        /// How many shelves in a row, across the rally, with no answer from the operator between
        /// them, mean the operator is away (RAL-56). The operator agreed 3.
        #[arg(long, env = "RALLY_OPERATOR_AWAY_AFTER", default_value_t = 3,
              value_parser = clap::value_parser!(u64).range(1..))]
        operator_away_after: u64,
        /// How long the operator is treated as away (RAL-56): no story waiting on the operator is
        /// offered, and then each is asked again. The operator agreed 1 hour.
        #[arg(long, env = "RALLY_OPERATOR_AWAY", default_value = "1h", value_parser = parse_window)]
        operator_away: u64,
        /// A read-only GitHub token, for checking submitted evidence (RAL-5, RAL-18). Without it,
        /// each check is recorded as not checked. With --secure, it must be root-owned too.
        #[arg(long, env = "RALLY_GITHUB_TOKEN_FILE")]
        github_token_file: Option<PathBuf>,
        /// The GitHub owners, comma-separated, whose repositories the token may read open pull
        /// requests' roadmaps in for `new-story` (RAL-68). Requests can't widen it.
        #[arg(long, env = "RALLY_GITHUB_OWNERS", default_value = rally::evidence::DEFAULT_OWNERS)]
        github_owners: String,
        /// The longest a submission's CI is waited for before its check is recorded as pending.
        #[arg(long, default_value = "30m", value_parser = parse_window)]
        evidence_ci_wait: u64,
        /// How often a submission whose CI is still running is checked again.
        #[arg(long, default_value = "1m", value_parser = parse_window)]
        evidence_poll: u64,
    },
    /// Run the read-only viewer on this machine's loopback address. See docs/web.md.
    Web {
        #[arg(long, default_value = "127.0.0.1:7300")]
        bind: String,
        /// The projects to show. Defaults to projects.json beside the pairing
        /// (~/.config/rally/projects.json) when that exists, and otherwise the built-in list.
        #[arg(long)]
        projects: Option<PathBuf>,
        /// This machine's backup directory, to show how old the last backup is and whether that's
        /// overdue (RAL-15).
        #[arg(long)]
        backup_dir: Option<PathBuf>,
        /// With --backup-dir: a backup older than this many hours is shown as overdue.
        #[arg(long, default_value_t = 1.0)]
        backup_max_age_hours: f64,
        /// Read a team's day summaries from a local folder instead of GitHub, as PREFIX=FOLDER, for
        /// example RAL=~/Documents/Gamoire/rally/docs/muster. Repeatable. The folder is only read.
        #[arg(long = "summaries", value_name = "PREFIX=FOLDER")]
        summaries: Vec<String>,
        /// Read the watchman's writeups from this local folder instead of GitHub.
        #[arg(long, value_name = "FOLDER")]
        watchman_summaries: Option<PathBuf>,
        /// The viewer key's private file (RAL-29), made if it isn't there. Defaults to web-key
        /// beside the pairing. The key never goes in a URL; sign in with --login.
        #[arg(long)]
        key_file: Option<PathBuf>,
        /// Ask the viewer running at --bind for a one-time code, and print the address to open
        /// with it. The code works once, within a minute.
        #[arg(long, conflicts_with = "new_key")]
        login: bool,
        /// Replace the viewer key, which signs every browser out of the running viewer.
        #[arg(long)]
        new_key: bool,
        /// How long a one-time code works, in seconds.
        #[arg(long, default_value_t = 60, hide = true)]
        code_seconds: u64,
    },
    /// On the service's host, add a machine and write its pair code to a private file.
    AddMachine {
        name: String,
        /// How the paired machine reaches the service.
        #[arg(long)]
        address: String,
        #[arg(long, env = "RALLY_STATE_DIR")]
        state_dir: PathBuf,
        /// Where to write the pair code, which holds a secret. It's never printed.
        #[arg(long)]
        out: PathBuf,
        /// It can read but not write, as the web app will.
        #[arg(long)]
        read_only: bool,
    },
    /// On the service's host, stop a machine's token working.
    Revoke {
        name: String,
        #[arg(long, env = "RALLY_STATE_DIR")]
        state_dir: PathBuf,
    },
    /// Teach this repository's agents about Rally: write a marked block of instructions into its
    /// CLAUDE.md and AGENTS.md, whichever exist, or a new CLAUDE.md. Running it again replaces the
    /// block and nothing else. It names no address or credential, so the result is safe to commit.
    Init {
        /// A file to write instead of the defaults, relative to the repository root. May be repeated.
        #[arg(long = "file")]
        files: Vec<PathBuf>,
        /// The repository to write in. Defaults to the one holding the current directory.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// The agents' harness, said explicitly: `prime-agent` adds a notice that its own
        /// messaging can stand in for Muster (RAL-59).
        #[arg(long, env = "RALLY_HARNESS")]
        harness: Option<String>,
    },
    /// For a Claude Code SessionStart hook: print what the team should know, read-only through the
    /// service, as one <rally_state> block. Prints nothing and succeeds if anything fails, so a
    /// session never fails to start because of Rally. See docs/session-hook.md.
    SessionStart {
        /// Your team. Without one, it prints nothing.
        #[arg(long, env = "RALLY_TEAM")]
        team: Option<String>,
        /// Say on standard error why nothing was printed.
        #[arg(long)]
        verbose: bool,
        /// The session's harness, said explicitly: `prime-agent` adds a notice that its own
        /// messaging can stand in for Muster (RAL-59).
        #[arg(long, env = "RALLY_HARNESS")]
        harness: Option<String>,
    },
    /// Send each designated watchman one batched digest when the ledger changes, and nothing when
    /// it hasn't (RAL-8). See docs/digest.md. Reads only; sends through `muster-shim tell`.
    Digest {
        /// Q: a digest goes once no new record has come for this long, such as 2m.
        #[arg(long = "quiet", env = "RALLY_DIGEST_QUIET", value_parser = parse_window)]
        quiet: u64,
        /// M: or once this long has passed since the first new record was seen, such as 10m.
        #[arg(long = "max", env = "RALLY_DIGEST_MAX", value_parser = parse_window)]
        max: u64,
        /// Repeat every this long until stopped, such as 30s. Without it, one pass.
        #[arg(long, value_parser = parse_window)]
        every: Option<u64>,
        /// Send to this watchman when none can be checked as designated (the service has no
        /// operator key). Declared: matched by name, not credential.
        #[arg(long)]
        watchman: Option<String>,
        /// Where each watchman's last digest is kept. Defaults to digest.json beside the pairing.
        #[arg(long)]
        state_file: Option<PathBuf>,
        /// Print the digest instead of sending it, and keep nothing.
        #[arg(long)]
        report_only: bool,
    },
    /// For the operator: show the service's GitHub token can't merge (RAL-18). Reads pull request
    /// --pull, which must be already merged or closed, then tries to merge it naming a head no
    /// commit has, so nothing can merge even if the token could write. Succeeds only when GitHub
    /// refuses for want of access. See docs/evidence.md.
    CheckGithubToken {
        #[arg(long, env = "RALLY_GITHUB_TOKEN_FILE")]
        github_token_file: PathBuf,
        /// The repository, as owner/repo.
        #[arg(long)]
        repo: String,
        /// An already-merged pull request in it. An open one is refused, and nothing is tried.
        #[arg(long)]
        pull: u64,
    },
    /// A checkout's Levi tasks, read-only, each fact labelled observed or declared (RAL-40). Runs
    /// `levi --no-sync ls --all --json` in the checkout; writes nothing to Levi and never contacts
    /// a Levi hub. Needs no pairing. See docs/levi.md.
    Levi {
        /// The checkout to read.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// The `levi` to run.
        #[arg(long, env = "RALLY_LEVI", default_value = "levi")]
        levi: PathBuf,
    },
    /// The operator designates the watchman, or with --end ends the designation (RAL-51). It prints
    /// the statement, then signs it with `ssh-keygen -Y sign` and the key given, which asks for
    /// Touch ID when the key is Secretive's. See docs/designation.md.
    Designate {
        /// The watchman's name. With --end, the one watchman whose designation to end, leaving
        /// the others in force (RAL-63).
        #[arg(long, required_unless_present = "end")]
        watchman: Option<String>,
        /// End the designation: with --watchman, that watchman's; without, every watchman's.
        #[arg(long)]
        end: bool,
        /// Bind the designation to the credential only the watchman's session holds (RAL-53), so a
        /// read counts as the watchman's only when made with it. See docs/designation.md.
        #[arg(long, conflicts_with = "end")]
        credential: Option<String>,
        /// Add this watchman to those in force, with this scope, instead of replacing them all
        /// (RAL-63): `all`, `team:<team>` or `machine:<credential>`. Needs --credential. The scope
        /// only decides whose read a merge waits for.
        #[arg(long, conflicts_with = "end", requires = "credential")]
        scope: Option<String>,
        /// The operator's public key file, as `ssh-keygen -f` takes it.
        #[arg(long)]
        key: PathBuf,
    },
    /// Who the watchman is, each designation checked again against the operator's key (RAL-51).
    Designation,
    /// Every verification and send-back, newest first, labelled watchman, peer or other by the
    /// designation in force when it was written (RAL-51).
    Verifications,
    /// For a newly designated watchman: every story a peer verified since the designation before
    /// it, merged or not, each awaiting the watchman's re-read until it records its own (RAL-52).
    Handover,
    /// One brief for a new watchman: each team's state, open claims (with whether the claimant's
    /// session is still on Muster's roster), submissions waiting, open blockers, questions to the
    /// operator and who each holds up, and the handover list, each fact with its kind and age
    /// (RAL-22a). It confers no authority.
    Brief,
    /// Submissions waiting for a reader, oldest first, with the evidence and any requested reads'
    /// commits to read (RAL-51).
    Waiting,
    /// The ledger as it stood at a past record: its stories, teams and open blockers, replayed to
    /// that record, and the corrections made since to what it held then, labelled later (RAL-28).
    /// It's what the viewer shows for that moment.
    Past {
        /// The record number to stand at.
        as_of: u64,
    },
    /// Save a pair code from `add-machine` on this machine, read from standard input so that it
    /// stays out of shell history and process listings.
    Pair,
    /// Mark a story as ready to claim.
    Ready { story: String },
    /// Claim a ready or sent-back story.
    Claim {
        story: String,
        #[arg(long)]
        team: String,
    },
    /// Give up a claim.
    Release {
        story: String,
        #[arg(long)]
        team: String,
    },
    /// Submit a claimed story for review.
    Submit {
        story: String,
        #[arg(long)]
        team: String,
        /// A repository and full commit, as owner/repo@<full commit>, which Rally checks (RAL-5).
        #[arg(long)]
        evidence: String,
        /// What you didn't test, in its own field.
        #[arg(long)]
        untested: Option<String>,
        /// The projects, whose `repo` the evidence must name for a story of that prefix (RAL-71),
        /// instead of the defaults.
        #[arg(long, env = "RALLY_PROJECTS")]
        projects: Option<PathBuf>,
        /// Skip this command's check of the evidence's form and repository, for evidence such as
        /// work outside GitHub. The service still checks what it can, and records what it couldn't.
        #[arg(long)]
        not_checkable: bool,
    },
    /// Correct the evidence of a story you submitted that's still in review (RAL-71), without a
    /// send-back. Checked as `submit` is, and recorded in the story's history as an amendment.
    Amend {
        story: String,
        #[arg(long)]
        team: String,
        /// A repository and full commit, as owner/repo@<full commit>, which Rally checks (RAL-5).
        #[arg(long)]
        evidence: String,
        /// What you didn't test, in its own field.
        #[arg(long)]
        untested: Option<String>,
        /// The projects, whose `repo` the evidence must name for a story of that prefix, instead
        /// of the defaults.
        #[arg(long, env = "RALLY_PROJECTS")]
        projects: Option<PathBuf>,
        /// Skip this command's check of the evidence's form and repository.
        #[arg(long)]
        not_checkable: bool,
    },
    /// Confirm someone else's submitted story, naming the commit you read.
    Verify {
        story: String,
        #[arg(long)]
        note: String,
        /// The full hash of the commit you read.
        #[arg(long)]
        commit: String,
        /// Your own team, so `waiting` can spread reads across teams (RAL-57).
        #[arg(long, env = "RALLY_TEAM")]
        team: Option<String>,
    },
    /// Return someone else's submitted story, saying what's missing.
    SendBack {
        story: String,
        #[arg(long)]
        note: String,
        /// The full hash of the commit you read, if you read one.
        #[arg(long)]
        commit: Option<String>,
        /// Your own team, so `waiting` can spread reads across teams (RAL-57).
        #[arg(long, env = "RALLY_TEAM")]
        team: Option<String>,
    },
    /// Say what your team, or one of its stories, is waiting on. Prints the blocker's record number.
    Block {
        #[arg(long)]
        team: String,
        /// The story that's waiting. Without it, the whole team is.
        #[arg(long)]
        story: Option<String>,
        /// Another story it waits on, which can be another team's.
        #[arg(long)]
        on_story: Option<String>,
        /// Another team it waits on.
        #[arg(long)]
        on_team: Option<String>,
        /// What it's waiting on, in words.
        #[arg(long)]
        note: String,
    },
    /// Close an open blocker, saying why. Closing a question to the operator (a blocker on the
    /// team `operator`) says where the answer came from: --direct, or --relayed-by.
    Unblock {
        /// The blocker's record number, as `block` or `blockers` gave it.
        blocker: u64,
        #[arg(long)]
        note: String,
        /// The operator answered you directly.
        #[arg(long, conflicts_with = "relayed_by")]
        direct: bool,
        /// Who relayed the operator's answer to you.
        #[arg(long)]
        relayed_by: Option<String>,
    },
    /// Record a hold in force and what releases it (RAL-64). Declared: it informs, and doesn't
    /// stop anything.
    Hold {
        #[arg(long)]
        team: String,
        #[arg(long)]
        story: Option<String>,
        /// What's held, in words.
        #[arg(long)]
        note: String,
        /// What releases it.
        #[arg(long)]
        releases: String,
    },
    /// Record a condition on the order of work (RAL-64), such as "RAL-9 before any deploy".
    Condition {
        #[arg(long)]
        team: String,
        #[arg(long)]
        story: Option<String>,
        #[arg(long)]
        note: String,
    },
    /// Record a standing grant from the operator, with its scope, and whether it came --direct
    /// or --relayed-by someone (RAL-64). The record confers no authority: approvals still come
    /// from the operator directly.
    Grant {
        /// The team it was given to.
        #[arg(long)]
        team: String,
        #[arg(long)]
        story: Option<String>,
        /// What it allows, and where.
        #[arg(long)]
        scope: String,
        /// The operator's words, or what was said.
        #[arg(long)]
        note: String,
        /// The operator gave it to you directly.
        #[arg(
            long,
            conflicts_with = "relayed_by",
            required_unless_present = "relayed_by"
        )]
        direct: bool,
        /// Who relayed it to you.
        #[arg(long)]
        relayed_by: Option<String>,
    },
    /// Open a rally (RAL-76): its name, the projects it works on by story prefix, and the teams
    /// in it. The viewer's board starts clean from here; the ledger keeps everything before it.
    OpenRally {
        #[arg(long)]
        name: String,
        /// A story prefix the rally works on, such as RAL. Repeat for each.
        #[arg(long = "project", required = true)]
        projects: Vec<String>,
        /// A team in the rally: a team name or a Muster session nickname. Repeat for each.
        #[arg(long = "team")]
        teams: Vec<String>,
    },
    /// Close the open rally, saying why (RAL-76). It's kept whole as a past rally.
    CloseRally {
        #[arg(long)]
        note: String,
    },
    /// List the rallies, newest first, and the one that's open (RAL-76).
    Rallies {
        /// Show them as they stood after this record.
        #[arg(long)]
        as_of: Option<u64>,
    },
    /// End a hold, condition or grant, saying why (RAL-64).
    EndStanding {
        /// Its record number, as `hold`, `condition`, `grant` or `brief` gave it.
        standing: u64,
        #[arg(long)]
        note: String,
    },
    /// Your team's next story (RAL-56): its own sent-back work, then shelved stories the operator
    /// has answered, then the first free story in roadmap order. Read-only: it says what to run
    /// to record a skip or a shelve. See docs/next.md.
    Next {
        #[arg(long, env = "RALLY_TEAM")]
        team: String,
        /// The roadmap to pick from.
        #[arg(long, default_value = "docs/roadmap.md")]
        roadmap: PathBuf,
        /// The story ID prefix, such as RAL. Without it, the one most of the roadmap's stories use.
        #[arg(long)]
        prefix: Option<String>,
    },
    /// Take the next free story number for a prefix, as a new ready story with a title (RAL-55).
    /// Prints the story's ID. Two teams asking at once get different numbers. IDs written by hand
    /// with `ready` still work.
    NewStory {
        /// The prefix, such as RAL.
        prefix: String,
        #[arg(long)]
        title: String,
        /// The prefix's roadmap, so a number already written there isn't handed out.
        #[arg(long, default_value = "docs/roadmap.md")]
        roadmap: PathBuf,
        /// The GitHub repository, as owner/repo, whose open pull requests' roadmaps are read too
        /// (RAL-68). Defaults to this checkout's `origin` on github.com.
        #[arg(long)]
        repo: Option<String>,
    },
    /// Prompts for the watchman from Muster's roster and Rally's records (RAL-7): each row of the
    /// design's stall table that holds for a team, and whether every team is waiting at once, as
    /// on 2026-10-30. Read-only. Prompts, never conclusions. See docs/stalls.md.
    Stalls {
        /// The projects, with each one's `team` and `sessions`, instead of the defaults.
        #[arg(long)]
        projects: Option<PathBuf>,
        /// How long a claim, or the whole ledger, can go without a record before it counts as
        /// quiet. No length has been agreed with the operator, so it has no default; it's the
        /// same setting as `rally wake --quiet-after`.
        #[arg(long, env = "RALLY_WAKE_QUIET_AFTER", value_parser = parse_window)]
        quiet_after: Option<u64>,
        /// A heavy-run lock file, as `flock <lock> <command>` takes it: flag it if its holder, or
        /// anything under it, is stopped or a zombie (RAL-75). Repeatable. Linux only.
        #[arg(long)]
        lock: Vec<PathBuf>,
    },
    /// Wake idle teams with their next story (RAL-54): for each team in the projects list with
    /// sessions mapped to it, idle (or its claim quiet) with a ready story, send one digest through
    /// Muster, at most once per interval. Without `muster-shim`, or with --report-only, it only
    /// reports. See docs/wake.md.
    Wake {
        /// The projects, with each one's `team` and `sessions`, instead of the defaults.
        #[arg(long)]
        projects: Option<PathBuf>,
        /// How long a claim can go without a record before its team counts as quiet, as agreed
        /// with the operator. Without it, only idle teams are woken.
        #[arg(long, env = "RALLY_WAKE_QUIET_AFTER", value_parser = parse_window)]
        quiet_after: Option<u64>,
        /// The least time between two digests to one team, as agreed with the operator. Without
        /// it, nothing is sent.
        #[arg(long, env = "RALLY_WAKE_INTERVAL", value_parser = parse_window)]
        interval: Option<u64>,
        /// Run a pass this often, until stopped. Without it, one pass.
        #[arg(long, value_parser = parse_window)]
        every: Option<u64>,
        /// Where to keep when each team was last woken. Defaults to wake.json beside the pairing.
        #[arg(long)]
        state_file: Option<PathBuf>,
        /// Report only: send nothing.
        #[arg(long)]
        report_only: bool,
    },
    /// Shelve a story whose question to the operator has had no answer within the window agreed
    /// with the operator, so your team can take its next story (RAL-56). The service checks the
    /// window against its own clock.
    Shelve {
        /// The record number of the blocker on the operator that holds the question.
        blocker: u64,
        #[arg(long, env = "RALLY_TEAM")]
        team: String,
    },
    /// Each team's track record as plain facts: stories verified, stories sent back and what was
    /// missing, and problems reported, which for now are the blockers it raised.
    TrackRecord {
        team: Option<String>,
        /// Show them as they stood after this record.
        #[arg(long)]
        as_of: Option<u64>,
        /// For the list: the teams after this one, as the last page's `next` gave it.
        #[arg(long)]
        after: Option<String>,
    },
    /// List open blockers, with what each waits on and its chain; with --all, closed ones too.
    Blockers {
        #[arg(long)]
        all: bool,
        /// Show them as they stood after this record.
        #[arg(long)]
        as_of: Option<u64>,
        /// The blockers after this record number, as the last page's `next` gave it.
        #[arg(long)]
        after: Option<String>,
    },
    /// Ask for a read of a story at a commit before it's merged. Open until a verify or send-back
    /// answers it, or it's withdrawn.
    RequestRead {
        story: String,
        /// The full hash of the commit to read.
        #[arg(long)]
        commit: String,
        #[arg(long)]
        note: String,
    },
    /// Withdraw a read you requested, or with --as-verifier, any requested read.
    WithdrawRead {
        story: String,
        /// The record that requested it, as `show` lists it.
        #[arg(long)]
        request: u64,
        #[arg(long)]
        note: String,
        #[arg(long)]
        as_verifier: bool,
    },
    /// Record that a story's work was merged at a commit. Allowed in any state and changes none;
    /// it doesn't say who merged, since every session shares one GitHub account.
    Merge {
        story: String,
        /// The full hash of the merged commit.
        #[arg(long)]
        commit: String,
    },
    /// Annotate an earlier record, which stays in the history, and optionally set the state.
    Correct {
        story: String,
        /// The record being corrected.
        #[arg(long)]
        corrects: u64,
        #[arg(long)]
        note: String,
        #[arg(long, value_parser = parse_state)]
        state: Option<State>,
        /// Your team, needed to set the state of a story your team holds.
        #[arg(long)]
        team: Option<String>,
        /// Record this as a verifier's correction, needed to move a story into or out of Verified
        /// or Sent back.
        #[arg(long)]
        as_verifier: bool,
        /// The full hash of the commit you read, needed with --state verified.
        #[arg(long)]
        commit: Option<String>,
    },
    /// Check the ledger for tampering. Fails if there's a problem. Save the head it prints somewhere
    /// else, and pass it back later with --expect.
    Check {
        /// A head saved earlier, as SEQ:HASH.
        #[arg(long, value_parser = parse_head)]
        expect: Option<Head>,
    },
    /// From another machine, through the service: pull what's new into a backup directory, after
    /// rechecking it. Refuses, writing nothing, if the service's ledger no longer extends the
    /// backup's saved head. See docs/service.md.
    Backup {
        /// The backup directory, made private if it's new.
        #[arg(long)]
        to: PathBuf,
        /// When the service can't be reached, say the backup is overdue past this many hours.
        #[arg(long, default_value_t = 24.0)]
        max_age_hours: f64,
    },
    /// With --db and --head-file naming a new, empty ledger: restore it from a backup directory,
    /// then check it. Refuses a ledger that already exists. See docs/service.md.
    Restore {
        #[arg(long)]
        from: PathBuf,
    },
    /// On the service's host, with --db: accept the current head after a lost or damaged head file.
    /// Refuses a broken chain. This also accepts any removal `check` was reporting, so compare with a
    /// head saved elsewhere first.
    AdoptHead,
    /// Declare your team's state: available, working, blocked or resting. A resting team isn't
    /// woken by `rally wake`.
    TeamState {
        team: String,
        #[arg(value_parser = parse_status)]
        status: TeamStatus,
        /// The story this is about, if any.
        #[arg(long)]
        story: Option<String>,
        /// Needed when blocked: what you're blocked on.
        #[arg(long)]
        note: Option<String>,
    },
    /// Show each team's declared state, or one team with its history.
    Teams {
        team: Option<String>,
        /// Show the state as it stood after this record.
        #[arg(long)]
        as_of: Option<u64>,
        /// For the list: the entries after this one, as the last page's `next` gave it.
        #[arg(long)]
        after: Option<String>,
        /// For a history: the entries before this record, as the last page's `earlier` gave it.
        #[arg(long)]
        before: Option<u64>,
    },
    /// Everything recorded about a team, oldest first: its declared states, its stories' records
    /// while it held them, and blockers by or on it, each with teams' annotations beside it (RAL-19).
    Record {
        team: String,
        /// Show it as it stood after this record.
        #[arg(long)]
        as_of: Option<u64>,
        /// The records after this one, as the last page's `next` gave it.
        #[arg(long)]
        after: Option<String>,
    },
    /// As a team, annotate an earlier record that names it, to correct it or disagree (RAL-19). It
    /// changes nothing else, and is shown beside the original.
    Annotate {
        /// The record being annotated.
        #[arg(long)]
        corrects: u64,
        #[arg(long)]
        team: String,
        #[arg(long)]
        note: String,
        /// On your own verification in force: a commit you also read, such as the head of a
        /// second pull request the story merged in (RAL-67). Repeatable. Only the verification's
        /// own credential and name can add one.
        #[arg(long = "also-read")]
        also_read: Vec<String>,
    },
    /// Before moving a branch: every story the commits in a range name must be verified (RAL-70).
    /// Lists each story named with its state, and fails if any is sent back, unverified, or not
    /// in the ledger under a prefix the ledger uses. Reads only commit messages.
    Gate {
        /// The commits to check, as git takes them, such as 19cc9d1..origin/main.
        range: String,
        /// The checkout to read the commits from.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// A story the range only mentions, such as one a commit files on the roadmap: listed,
        /// and not refused. Repeatable.
        #[arg(long)]
        allow: Vec<String>,
    },
    /// Show the state of every story, or one story with its history.
    Show {
        story: Option<String>,
        /// Show the state as it stood after this record.
        #[arg(long)]
        as_of: Option<u64>,
        /// For the list: the entries after this one, as the last page's `next` gave it.
        #[arg(long)]
        after: Option<String>,
        /// For a history: the entries before this record, as the last page's `earlier` gave it.
        #[arg(long)]
        before: Option<u64>,
        /// One top-level state per story and no history (RAL-72).
        #[arg(long, conflicts_with = "before")]
        brief: bool,
        /// Print one field alone as plain text: state, team, submitted_by, verified_commit or
        /// merged_commit. For the list, one `ID<tab>value` line per story (RAL-72).
        #[arg(long, conflicts_with_all = ["brief", "before"])]
        field: Option<String>,
    },
}

/// A window such as `90m` or `4h`, in milliseconds (RAL-56).
fn parse_window(text: &str) -> Result<u64, String> {
    let bad = || format!("{text:?} isn't a window: give a number with s, m, h or d, such as 4h");
    let split = text.len().checked_sub(1).ok_or_else(bad)?;
    let (number, unit) = text.split_at(split);
    let number: u64 = number.parse().map_err(|_| bad())?;
    let unit_ms: u64 = match unit {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return Err(bad()),
    };
    match number.checked_mul(unit_ms) {
        Some(ms) if ms > 0 => Ok(ms),
        _ => Err(bad()),
    }
}

fn parse_state(text: &str) -> Result<State, String> {
    serde_json::from_value(json!(text)).map_err(|_| {
        format!("{text:?} isn't a state: ready, claimed, in_review, verified or sent_back")
    })
}

fn parse_head(text: &str) -> Result<Head, String> {
    let (seq, hash) = text.split_once(':').ok_or("expected SEQ:HASH")?;
    let valid = hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit());
    match seq.parse() {
        Ok(seq) if valid => Ok(Head {
            seq,
            hash: hash.to_owned(),
        }),
        _ => Err("expected SEQ:HASH".to_owned()),
    }
}

fn parse_status(text: &str) -> Result<TeamStatus, String> {
    serde_json::from_value(json!(text))
        .map_err(|_| format!("{text:?} isn't a state: available, working, blocked or resting"))
}

/// Escapes what can move a terminal or disguise text: controls, including C1 and DEL, and invisible
/// and direction characters. JSON doesn't escape most of these, and they can only be in its strings,
/// where `\\uXXXX` means the same thing, so JSON output stays valid and unchanged in meaning.
fn escape_for_terminal(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        let invisible = matches!(
            c,
            '\u{ad}' | '\u{61c}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{2069}' | '\u{feff}'
        );
        if (c.is_control() && c != '\n') || invisible {
            escaped.push_str(&format!("\\u{:04x}", u32::from(c)));
        } else {
            escaped.push(c);
        }
    }
    escaped
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Command::SessionStart {
        team,
        verbose,
        harness,
    } = &cli.command
    {
        let harness = rally::session::Harness::parse(harness.as_deref());
        session_start(cli.config.clone(), team.as_deref(), *verbose, harness);
        return ExitCode::SUCCESS;
    }
    if let Command::CheckGithubToken {
        github_token_file,
        repo,
        pull,
    } = &cli.command
    {
        return match check_github_token(github_token_file, repo, *pull) {
            Ok(out) => {
                println!("{}", escape_for_terminal(&out));
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("rally: {}", escape_for_terminal(&error));
                ExitCode::FAILURE
            }
        };
    }
    if let Command::Levi { repo, levi } = &cli.command {
        return match rally::levi::read(levi, repo) {
            Ok(out) => {
                let text = serde_json::to_string_pretty(&out).unwrap_or_default();
                println!("{}", escape_for_terminal(&text));
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("rally: {}", escape_for_terminal(&error.to_string()));
                ExitCode::FAILURE
            }
        };
    }
    match run(cli) {
        Ok(output) => {
            println!("{}", escape_for_terminal(&output));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("rally: {}", escape_for_terminal(&error.to_string()));
            ExitCode::FAILURE
        }
    }
}

/// The one GitHub token in the file at `path`.
fn read_github_token(path: &Path) -> Result<String, String> {
    let token =
        std::fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let token = token.trim().to_owned();
    if token.is_empty() || token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("{} doesn't hold one token", path.display()));
    }
    Ok(token)
}

/// `rally check-github-token`: whether the token can merge, by trying to on a head that can't
/// match (RAL-18). Succeeds only when the merge is refused for want of access.
fn check_github_token(path: &Path, repo: &str, pull: u64) -> Result<String, String> {
    let token = read_github_token(path)?;
    let github = rally::evidence::GitHub::new(token);
    let probe = github.probe_merge(repo, pull)?;
    let out = json!({"repo": repo, "pull": pull, "probe": probe});
    let text = serde_json::to_string_pretty(&out).unwrap_or_default();
    match probe {
        rally::evidence::MergeProbe::Refused { .. } => Ok(text),
        rally::evidence::MergeProbe::CanWrite { .. } => Err(format!(
            "the token got past GitHub's access check, so it can write; nothing merged.\n{text}"
        )),
        rally::evidence::MergeProbe::Unclear { .. } => Err(format!(
            "GitHub's answer doesn't show whether the token can write.\n{text}"
        )),
    }
}

/// `rally session-start`: the block, or nothing at all. Never fails.
fn session_start(
    config: Option<PathBuf>,
    team: Option<&str>,
    verbose: bool,
    harness: rally::session::Harness,
) {
    let quiet = |why: &str| {
        if verbose {
            eprintln!("rally session-start: {why}");
        }
    };
    let Some(team) = team.filter(|t| !t.trim().is_empty()) else {
        return quiet("no team: pass --team or set RALLY_TEAM");
    };
    let path = match config_path(config) {
        Ok(path) => path,
        Err(error) => return quiet(&error.to_string()),
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return quiet(&format!("no pairing at {}", path.display()));
    };
    let Ok(pairing) = serde_json::from_str::<Pairing>(&text) else {
        return quiet(&format!("{} can't be read", path.display()));
    };
    match rally::session::fetch(&pairing, rally::session::DEADLINE) {
        Ok((stories, blockers)) => {
            println!(
                "{}",
                rally::session::render_for(team, &stories, &blockers, harness)
            )
        }
        Err(error) => quiet(&escape_for_terminal(&error)),
    }
}

/// The pairing at `path`, for commands that only work through the service.
fn read_pairing(path: &Path) -> Result<Pairing, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)
        .map_err(|_| format!("no pairing at {}: run `rally pair`", path.display()))?;
    Ok(serde_json::from_str(&text).map_err(|_| format!("{} can't be read", path.display()))?)
}

/// `ssh-keygen -Y sign` over `statement` in the designation namespace, with the key at `key` (a
/// public key file, signed through the SSH agent, such as Secretive's).
fn ssh_sign(key: &Path, statement: &str) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = std::process::Command::new("ssh-keygen")
        .args([
            "-q",
            "-Y",
            "sign",
            "-n",
            rally::designation::NAMESPACE,
            "-f",
        ])
        .arg(key)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("ssh-keygen can't be run: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("ssh-keygen's input")?
        .write_all(statement.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err("ssh-keygen didn't sign it, so nothing was recorded".into());
    }
    Ok(String::from_utf8(out.stdout)?)
}

/// Where reads and writes go.
enum Backend {
    /// Directly on the ledger's database, on the host.
    File(Box<Store>),
    Service(Pairing),
}

impl Backend {
    /// Writes go through the writer lock. Reads go straight to Postgres, read-only.
    fn choose(
        location: Option<&Location>,
        config: &Path,
        write: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(location) = location {
            let store = if write {
                Store::open(location)?
            } else {
                Store::open_read_only(location)?
            };
            return Ok(Self::File(Box::new(store)));
        }
        let text = std::fs::read_to_string(config).map_err(|_| {
            format!(
                "no pairing at {}: run `rally pair`, or use --db on the host",
                config.display()
            )
        })?;
        Ok(Self::Service(serde_json::from_str(&text).map_err(
            |_| format!("{} can't be read", config.display()),
        )?))
    }

    /// One page of a read, as a value.
    fn read_value(
        &self,
        path: &str,
        as_of: Option<u64>,
        page: &Page,
        local: impl FnOnce(&rally::ledger::Ledger, &Page) -> Result<Value, String>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        Ok(match self {
            Self::File(store) => store.with_ledger(as_of, |ledger| local(ledger, page))??,
            Self::Service(pairing) => {
                let mut query = Vec::new();
                if let Some(seq) = as_of {
                    query.push(format!("as_of={seq}"));
                }
                if let Some(after) = &page.after {
                    query.push(format!("after={}", encode(after)));
                }
                if let Some(before) = page.before {
                    query.push(format!("before={before}"));
                }
                let path = if query.is_empty() {
                    path.to_owned()
                } else if path.contains('?') {
                    format!("{path}&{}", query.join("&"))
                } else {
                    format!("{path}?{}", query.join("&"))
                };
                let reply = client::request(pairing, "GET", &path, None, &[])?;
                reply_value(&reply)?
            }
        })
    }

    fn read(
        &self,
        path: &str,
        as_of: Option<u64>,
        page: &Page,
        local: impl FnOnce(&rally::ledger::Ledger, &Page) -> Result<Value, String>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let value = self.read_value(path, as_of, page, local)?;
        Ok(serde_json::to_string_pretty(&value)?)
    }

    /// Every page of a list, up to `max_pages`, merged into one map under `field`, and whether
    /// there was more than that.
    fn read_all(
        &self,
        path: &str,
        field: &str,
        max_pages: usize,
        local: impl Fn(&rally::ledger::Ledger, &Page) -> Result<Value, String>,
    ) -> Result<(Value, bool), Box<dyn std::error::Error>> {
        self.read_all_as_of(path, None, field, max_pages, local)
    }

    /// [`Self::read_all`] as of a past record, if one is given. The field may be a map, whose
    /// pages are merged, or a list, whose pages are joined in order.
    fn read_all_as_of(
        &self,
        path: &str,
        as_of: Option<u64>,
        field: &str,
        max_pages: usize,
        local: impl Fn(&rally::ledger::Ledger, &Page) -> Result<Value, String>,
    ) -> Result<(Value, bool), Box<dyn std::error::Error>> {
        let mut page = Page::default();
        let mut merged = serde_json::Map::new();
        let mut listed: Vec<Value> = Vec::new();
        let mut is_list = false;
        let mut first: Option<Value> = None;
        let finish = |first: Option<Value>, merged, listed, is_list| {
            let mut value = first.unwrap_or_default();
            value[field] = if is_list {
                Value::Array(listed)
            } else {
                Value::Object(merged)
            };
            value
        };
        for _ in 0..max_pages {
            let value = self.read_value(path, as_of, &page, &local)?;
            if let Some(entries) = value[field].as_object() {
                merged.extend(entries.clone());
            } else if let Some(entries) = value[field].as_array() {
                is_list = true;
                listed.extend(entries.iter().cloned());
            }
            let next = value["next"].as_str().map(str::to_owned);
            first.get_or_insert(value);
            match next {
                Some(next) => page.after = Some(next),
                None => return Ok((finish(first, merged, listed, is_list), false)),
            }
        }
        Ok((finish(first, merged, listed, is_list), true))
    }
}

/// The most pages of stories that `rally gate` reads.
const GATE_PAGES: usize = 50;

/// The most pages of stories, and of blockers, that `rally next` reads.
const NEXT_PAGES: usize = 20;

/// `rally next`: the pick, worked out here, where the roadmap is (RAL-56).
/// What the pick reads from the ledger: stories, every blocker and the operator's away hour, all
/// as of the service's clock, with the service's operator window.
struct PickRead {
    stories: Value,
    blockers: Vec<Value>,
    away: Value,
    now_ms: i64,
    window_ms: Option<u64>,
    incomplete: bool,
    /// Set when the service is older than RAL-56 and gave no time, so this machine's was used.
    local_clock: bool,
}

impl PickRead {
    fn read(backend: &Backend) -> Result<Self, Box<dyn std::error::Error>> {
        let (stories, more_stories) =
            backend.read_all("/v1/stories", "stories", NEXT_PAGES, |ledger, page| {
                views::stories(ledger, None, page)
            })?;
        let mut blockers = Vec::new();
        let mut page = Page::default();
        let mut more_blockers = true;
        let mut away = Value::Null;
        let mut now_ms = None;
        for _ in 0..NEXT_PAGES {
            // Every page after the first is judged at the first page's instant, the service's.
            let path = match now_ms {
                Some(at_ms) => format!("/v1/blockers/all?at_ms={at_ms}"),
                None => "/v1/blockers/all".to_owned(),
            };
            let at_ms = now_ms;
            let value = backend.read_value(&path, None, &page, |ledger, page| match at_ms {
                Some(at_ms) => views::blockers_at(ledger, true, page, at_ms),
                None => views::blockers(ledger, true, page),
            })?;
            blockers.extend(value["blockers"].as_array().cloned().unwrap_or_default());
            // The first page's clock, the service's, decides everything time-based in the pick.
            if now_ms.is_none() {
                now_ms = value["now_ms"].as_i64();
                if value["operator_away_active"] == true {
                    away = value["operator_away"].clone();
                }
            }
            match value["next"].as_str() {
                Some(next) => page.after = Some(next.to_owned()),
                None => {
                    more_blockers = false;
                    break;
                }
            }
        }
        // The window is the service's, so what's due here is what it will accept.
        let window_ms = match backend {
            Backend::File(_) => None,
            Backend::Service(pairing) => reply_value(&client::request(
                pairing,
                "GET",
                "/v1/whoami",
                None,
                &[],
            )?)?["operator_window_ms"]
                .as_u64(),
        };
        let local_clock = now_ms.is_none();
        let now_ms = now_ms.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as i64)
        });
        Ok(Self {
            stories,
            blockers,
            away,
            now_ms,
            window_ms,
            incomplete: more_stories || more_blockers,
            local_clock,
        })
    }

    fn pick(&self, team: &str, items: &[rally::next::Item]) -> Value {
        let empty = serde_json::Map::new();
        let ledger = rally::next::Ledger {
            stories: self.stories["stories"].as_object().unwrap_or(&empty),
            blockers: &self.blockers,
            away: Some(&self.away).filter(|away| !away.is_null()),
        };
        let mut result = rally::next::pick(team, items, &ledger, self.window_ms, self.now_ms);
        if self.local_clock {
            result["clock"] = json!(
                "The service is older than RAL-56 and gave no time, so this machine's clock was used."
            );
        }
        if self.incomplete {
            result["incomplete"] = json!(
                "The ledger has more stories or blockers than were read, so this pick may be \
                 wrong: check `rally show` and `rally blockers` before taking it."
            );
        }
        result
    }
}

/// The most times `new-story` tries the next number when another team takes it first.
const NEW_STORY_TRIES: usize = 20;

/// A submit or amend record (RAL-71).
fn submission(
    story: &str,
    team: &str,
    evidence: &str,
    untested: Option<String>,
    amend: bool,
) -> (Kind, Value) {
    let mut body = json!({"change": "submit", "story": story, "team": team, "evidence": evidence});
    if let Some(untested) = untested {
        body["untested"] = json!(untested);
    }
    if amend {
        body["amend"] = json!(true);
    }
    (Kind::Declared, body)
}

/// Checks evidence at submit (RAL-71): it must be `owner/repo@<full commit>`, and for a story whose
/// prefix the projects list, name that project's repository. Caught here, a typo costs a retry
/// rather than a send-back.
fn check_evidence(
    story: &str,
    evidence: &str,
    projects: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let text = match projects {
        Some(path) => std::fs::read_to_string(path).map_err(|error| {
            format!(
                "couldn't read the projects list {}: {error}",
                path.display()
            )
        })?,
        None => DEFAULT_PROJECTS.to_owned(),
    };
    let projects: Vec<Project> = serde_json::from_str(&text)
        .map_err(|error| format!("the projects list isn't a list of projects: {error}"))?;
    match evidence_problem(story, evidence, &projects) {
        Some(problem) => Err(problem.into()),
        None => Ok(()),
    }
}

/// Why `evidence` won't do for `story`, if it won't (RAL-71).
fn evidence_problem(story: &str, evidence: &str, projects: &[Project]) -> Option<String> {
    const FORM: &str = "write it as owner/repo@<full commit>, such as \
                        Castor-o7/rally@<40 hex digits from git rev-parse HEAD>";
    let (repo, _) = match rally::evidence::parse(evidence) {
        Ok(parsed) => parsed,
        Err(why) => return Some(format!("{why}: {FORM}, or give --not-checkable")),
    };
    let prefix = story.split_once('-').map_or(story, |(prefix, _)| prefix);
    if let Some(project) = projects
        .iter()
        .find(|project| project.prefix.eq_ignore_ascii_case(prefix))
        && !project.repo.eq_ignore_ascii_case(&repo)
    {
        return Some(format!(
            "the evidence names {repo}, but {prefix} stories are in {}: check the repository's \
             spelling, or give --projects with the right repo for {prefix}",
            project.repo
        ));
    }
    None
}

/// `owner/repo` from a github.com remote URL, in https or ssh form.
fn github_repo(url: &str) -> Option<String> {
    let rest = url
        .trim()
        .strip_prefix("https://github.com/")
        .or_else(|| url.trim().strip_prefix("git@github.com:"))
        .or_else(|| url.trim().strip_prefix("ssh://git@github.com/"))?;
    let repo = rest.trim_end_matches('/').trim_end_matches(".git");
    rally::evidence::valid_repo(repo).then(|| repo.to_owned())
}

/// What the service read of open pull requests' roadmaps (RAL-68), or why it couldn't.
fn open_pull_ids(backend: &Backend, repo: Option<&str>, roadmap: &Path, prefix: &str) -> Value {
    let not = |reason: &str| json!({"checked": false, "reason": reason});
    let Backend::Service(pairing) = backend else {
        return not("on the host there's no GitHub token: run it through the service");
    };
    let repo = match repo {
        Some(repo) => Some(repo.to_owned()),
        None => std::process::Command::new("git")
            .args(["remote", "get-url", "origin"])
            .output()
            .ok()
            .and_then(|out| github_repo(&String::from_utf8_lossy(&out.stdout))),
    };
    let Some(repo) = repo else {
        return not("no GitHub repository: give --repo owner/repo");
    };
    let Some(path) = roadmap.to_str().filter(|p| rally::evidence::valid_path(p)) else {
        return not("the roadmap isn't a plain path inside the repository");
    };
    let query = format!(
        "/v1/open-ids?repo={}&path={}&prefix={}",
        encode(&repo),
        encode(path),
        encode(prefix)
    );
    match client::request(pairing, "GET", &query, None, &[])
        .map_err(|e| e.to_string())
        .and_then(|reply| reply_value(&reply).map_err(|e| e.to_string()))
    {
        Ok(value) => value,
        Err(error) => not(&format!("the service couldn't be asked: {error}")),
    }
}

/// `rally new-story`: the next free number for `prefix`, above every one in the ledger and in the
/// roadmap, written in one append. If another team takes it first, the ledger refuses and this
/// tries the next (RAL-55).
///
/// `roadmap` is the local roadmap and the GitHub repository whose open pull requests' roadmaps are
/// read too, if given.
fn new_story(
    prefix: &str,
    title: &str,
    (roadmap, repo): (&Path, Option<&str>),
    location: Option<&Location>,
    config: &Path,
    name: Option<&str>,
    credential: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(format!("{prefix:?} isn't a prefix: capital letters, such as RAL").into());
    }
    let name = name.ok_or("--as or RALLY_NAME is needed to write")?;
    // A roadmap that isn't there has no numbers to avoid.
    let written = std::fs::read_to_string(roadmap)
        .map(|text| {
            rally::next::items(&text, prefix)
                .iter()
                .filter_map(|item| item.id.split_once('-')?.1.parse::<u64>().ok())
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let lead = format!("{prefix}-");
    let backend = Backend::choose(location, config, true)?;
    let (stories, _) = backend.read_all("/v1/stories", "stories", NEXT_PAGES, |ledger, page| {
        views::stories(ledger, None, page)
    })?;
    let mut highest = stories["stories"]
        .as_object()
        .map(|rows| {
            rows.keys()
                .filter_map(|id| id.strip_prefix(&lead)?.parse::<u64>().ok())
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
        .max(written);
    // Numbers written in open pull requests' roadmaps aren't handed out either (RAL-68).
    let open = open_pull_ids(&backend, repo, roadmap, prefix);
    if open["checked"] == true {
        highest = highest.max(open["highest"].as_u64().unwrap_or(0));
    }
    for _ in 0..NEW_STORY_TRIES {
        let story = format!("{prefix}-{}", highest + 1);
        let body = json!({"change": "new_story", "story": story, "title": title});
        let outcome: Result<Value, String> = match &backend {
            Backend::File(store) => {
                let credential = credential
                    .ok_or("--credential or RALLY_CREDENTIAL is needed to write with --db")?;
                let source = Source {
                    credential: credential.to_owned(),
                    declared_name: Some(name.to_owned()),
                };
                let record = Record::new(Kind::Declared, source, now_ms()?, body)?;
                store
                    .append(&record)
                    .map(|seq| json!({"recorded": seq}))
                    .map_err(|e| e.to_string())
            }
            Backend::Service(pairing) => {
                let request = json!({"kind": Kind::Declared, "declared_name": name, "body": body})
                    .to_string();
                let reply = client::request(pairing, "POST", "/v1/records", Some(&request), &[])?;
                reply_value(&reply).map_err(|e| e.to_string())
            }
        };
        match outcome {
            Ok(mut recorded) => {
                recorded["story"] = json!(story);
                recorded["open_pull_requests"] = open;
                return Ok(recorded.to_string());
            }
            // Someone took it first: the refusal says how high the prefix now reaches, so take
            // the next after a short, uneven pause, so that racing writers spread out.
            Err(error) if error.contains("isn't the next free number") => {
                highest = error
                    .rsplit("already reaches ")
                    .next()
                    .and_then(|rest| {
                        rest.trim()
                            .trim_end_matches(|c: char| !c.is_ascii_digit())
                            .parse()
                            .ok()
                    })
                    .unwrap_or(highest + 1)
                    .max(highest + 1);
                let mut jitter = [0u8; 1];
                let _ = getrandom::fill(&mut jitter);
                std::thread::sleep(std::time::Duration::from_millis(
                    5 + u64::from(jitter[0] % 40),
                ));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(format!("other teams kept taking {prefix} numbers first; try again").into())
}

/// One pass of `rally wake` (RAL-54).
/// One `rally stalls` report (RAL-7). Gathers what `rally wake` does, plus each team's open
/// blockers, and leaves the judgement to `rally::stall`.
fn stalls_pass(
    projects: Option<&Path>,
    location: Option<&Location>,
    config: &Path,
    quiet_after: Option<u64>,
    locks: &[PathBuf],
) -> Result<String, Box<dyn std::error::Error>> {
    let text = match projects {
        Some(path) => std::fs::read_to_string(path)?,
        None => DEFAULT_PROJECTS.to_owned(),
    };
    let projects: Vec<Project> = serde_json::from_str(&text)?;
    let backend = Backend::choose(location, config, false)?;
    let read = PickRead::read(&backend)?;
    let now_ms = read.now_ms;
    let teams = backend.read_value("/v1/teams", None, &Page::default(), |ledger, page| {
        views::teams(ledger, None, page)
    })?;
    let roster = rally::wake::muster_available()
        .then(|| {
            std::process::Command::new("muster-shim")
                .arg("roster")
                .output()
                .ok()
                .filter(|out| out.status.success())
                .and_then(|out| serde_json::from_slice::<Value>(&out.stdout).ok())
        })
        .flatten();
    let empty = serde_json::Map::new();
    let stories = read.stories["stories"].as_object().unwrap_or(&empty);
    let open: Vec<&Value> = read
        .blockers
        .iter()
        .filter(|blocker| blocker["closed"].is_null())
        .collect();
    // The newest record Rally has seen: on a story, a blocker or a team's state.
    let last_record_ms = stories
        .values()
        .filter_map(|story| story["last_at_ms"].as_i64())
        .chain(read.blockers.iter().filter_map(|b| b["at_ms"].as_i64()))
        .chain(
            teams["teams"]
                .as_object()
                .unwrap_or(&empty)
                .values()
                .filter_map(|team| team["at_ms"].as_i64()),
        )
        .max()
        .unwrap_or(0);
    let mut facts = Vec::new();
    let mut errors = Vec::new();
    for project in &projects {
        let Some(team) = project.team.as_deref() else {
            continue;
        };
        let key = team.trim().to_lowercase();
        let same = |name: Option<&str>| name.is_some_and(|name| name.trim().to_lowercase() == key);
        let ready = match web::read_roadmap(project) {
            Ok((_, text)) => {
                read.pick(team, &rally::next::items(&text, &project.prefix))["next"]["story"]
                    .as_str()
                    .map(str::to_owned)
            }
            Err(error) => {
                errors.push(json!({"team": team, "error":
                    format!("the roadmap couldn't be read: {error}")}));
                continue;
            }
        };
        let claims: Vec<(String, i64)> = stories
            .iter()
            .filter(|(_, story)| {
                matches!(story["state"].as_str(), Some("claimed" | "sent_back"))
                    && same(story["team"].as_str())
            })
            .map(|(id, story)| (id.clone(), story["last_at_ms"].as_i64().unwrap_or(0)))
            .collect();
        let blockers = open
            .iter()
            .filter(|blocker| same(blocker["team"].as_str()))
            .map(|blocker| rally::stall::Waiting {
                seq: blocker["seq"].as_u64().unwrap_or(0),
                story: blocker["story"].as_str().map(str::to_owned),
                on: blocker["on_team"]
                    .as_str()
                    .or(blocker["on_story"].as_str())
                    .map(str::to_owned),
                note: blocker["note"].as_str().unwrap_or_default().to_owned(),
            })
            .collect();
        facts.push(rally::stall::Facts {
            name: team.to_owned(),
            sessions: roster
                .as_ref()
                .map(|roster| rally::wake::sessions_of(roster, &project.sessions))
                .unwrap_or_default(),
            status: teams["teams"][&key]["status"].as_str().map(str::to_owned),
            claims,
            ready,
            blockers,
        });
    }
    let mut report = rally::stall::report(&facts, last_record_ms, quiet_after, now_ms);
    if roster.is_none() {
        // Without the roster, only what Rally alone says holds: the declared blockers.
        if let Some(prompts) = report["prompts"].as_array_mut() {
            prompts.retain(|row| row["row"] == "declared_blocked");
        }
        report["every_team_waiting"] = Value::Null;
        report["roster"] = json!({"available": false, "note": "muster-shim's roster couldn't be \
            read, so only declared blockers are shown"});
    } else {
        report["roster"] = json!({"available": true});
    }
    if !locks.is_empty() {
        // The kernel's account of each lock, which needs no roster: shown with or without it.
        let (held, seen) = rally::lock_holder::check(locks, roster.as_ref());
        if let Some(prompts) = report["prompts"].as_array_mut() {
            prompts.extend(held);
        }
        report["locks"] = json!(seen);
    }
    report["last_record_ms"] = json!(last_record_ms);
    if !errors.is_empty() {
        report["errors"] = json!(errors);
    }
    Ok(serde_json::to_string_pretty(&report)?)
}

/// The most pages of records one digest pass reads.
const DIGEST_PAGES: usize = 20;

/// One pass of `rally digest` (RAL-8): for each watchman, read the records after its last digest,
/// decide, and send a digest when one is due.
fn digest_pass(
    location: Option<&Location>,
    config: &Path,
    state_file: &Path,
    times: rally::digest::Times,
    named: Option<&str>,
    report_only: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    use rally::digest::{Decision, Kept, Watchman};
    use rally::wake::Sender;
    let backend = Backend::choose(location, config, false)?;
    let get = |path: &str| -> Result<Value, Box<dyn std::error::Error>> {
        match &backend {
            Backend::Service(pairing) => Ok(reply_value(&client::request(
                pairing,
                "GET",
                path,
                None,
                &[],
            )?)?),
            Backend::File(store) => Ok(match path {
                "/v1/handover" => store.handover_report(None)?,
                _ => return Err(format!("{path} isn't read from the database here").into()),
            }),
        }
    };
    let export = |after: u64| -> Result<Value, Box<dyn std::error::Error>> {
        match &backend {
            Backend::Service(_) => get(&format!("/v1/export?after={after}")),
            Backend::File(store) => {
                let (records, head) = store.export(after)?;
                Ok(json!({"records": records, "head": head}))
            }
        }
    };
    // Every page of the story list, for RAL-31's comparison of merges.
    let all_stories = || -> Result<Value, Box<dyn std::error::Error>> {
        let mut merged = serde_json::Map::new();
        let mut path = "/v1/stories".to_owned();
        for _ in 0..DIGEST_PAGES {
            let page = get(&path)?;
            if let Some(rows) = page["stories"].as_object() {
                merged.extend(rows.clone());
            }
            match page["next"].as_str() {
                Some(next) => path = format!("/v1/stories?after={}", encode(next)),
                None => break,
            }
        }
        Ok(json!({"stories": merged}))
    };
    let handover = get("/v1/handover")?;
    let mut watchmen: Vec<Watchman> = handover["watchmen"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            Some(Watchman {
                name: w["name"].as_str()?.to_owned(),
                credential: w["credential"].as_str().map(str::to_owned),
            })
        })
        .collect();
    let mut how = "designated";
    if watchmen.is_empty()
        && let Some(name) = named
    {
        watchmen.push(Watchman {
            name: name.to_owned(),
            credential: None,
        });
        how = "named with --watchman (declared)";
    }
    if watchmen.is_empty() {
        return Ok(
            json!({"sent": [], "note": "no watchman is designated or named, so nothing is sent"})
                .to_string(),
        );
    }
    let mut state: std::collections::BTreeMap<String, Kept> = std::fs::read_to_string(state_file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64);
    let muster = rally::wake::muster_available();
    let mut rows = Vec::new();
    for watch in &watchmen {
        let key = watch.name.trim().to_lowercase();
        let mut kept = match state.get(&key) {
            Some(kept) => kept.clone(),
            // A first run starts at the current head, so it doesn't send the whole history.
            None => rally::digest::start_at(export(u64::MAX)?["head"]["seq"].as_u64().unwrap_or(0)),
        };
        let mut recs = Vec::new();
        let mut after = kept.head;
        for _ in 0..DIGEST_PAGES {
            let page = export(after)?;
            let got = rally::digest::records(&page);
            let Some(last) = got.last().map(|r| r.seq) else {
                break;
            };
            recs.extend(got);
            after = last;
            if page.get("next").is_none() {
                break;
            }
        }
        let mut row = json!({"watchman": watch.name, "how": how});
        match rally::digest::step(&mut kept, &recs, watch, times, now_ms) {
            Decision::Nothing => row["decision"] = json!("nothing new"),
            Decision::Batching { due_in_ms } => {
                row["decision"] = json!("batching");
                row["due_in_s"] = json!(due_in_ms / 1000);
                if report_only {
                    // Nothing is kept, so show what the digest would say now.
                    let to = recs.last().map_or(kept.head, |r| r.seq);
                    let waiting = get("/v1/waiting")?;
                    let stories = all_stories()?;
                    let reads = rally::digest::Reads {
                        waiting: &waiting,
                        handover: &handover,
                        stories: &stories,
                        landed: &web::pr_head,
                    };
                    row["preview"] = json!(rally::digest::render(
                        kept.head, to, &recs, watch, &reads, now_ms
                    ));
                }
            }
            Decision::Send { from, to } => {
                let waiting = get("/v1/waiting")?;
                let stories = all_stories()?;
                let reads = rally::digest::Reads {
                    waiting: &waiting,
                    handover: &handover,
                    stories: &stories,
                    landed: &web::pr_head,
                };
                let digest = rally::digest::render(from, to, &recs, watch, &reads, now_ms);
                if report_only {
                    row["decision"] = json!("due (report only: not sent)");
                    row["digest"] = json!(digest);
                } else if !muster {
                    row["decision"] = json!("due, but muster-shim isn't on this machine");
                } else {
                    match rally::wake::MusterShim.send(&watch.name, &digest) {
                        Ok(()) => {
                            rally::digest::sent(&mut kept, to);
                            row["decision"] = json!("sent");
                            row["covered"] = json!(format!("{}–{to}", from + 1));
                        }
                        Err(error) => {
                            row["decision"] = json!("due, but sending failed");
                            row["error"] = json!(error);
                        }
                    }
                }
            }
        }
        state.insert(key, kept);
        rows.push(row);
    }
    if !report_only {
        write_private(state_file, serde_json::to_string_pretty(&state)?.as_bytes())?;
    }
    Ok(json!({"at_ms": now_ms, "watchmen": rows}).to_string())
}

fn wake_pass(
    projects: Option<&Path>,
    location: Option<&Location>,
    config: &Path,
    state_file: &Path,
    times: rally::wake::Times,
    report_only: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    use rally::wake::{Decision, Sender};
    let text = match projects {
        Some(path) => std::fs::read_to_string(path)?,
        None => DEFAULT_PROJECTS.to_owned(),
    };
    let projects: Vec<Project> = serde_json::from_str(&text)?;
    let backend = Backend::choose(location, config, false)?;
    let read = PickRead::read(&backend)?;
    let now_ms = read.now_ms;
    let teams = backend.read_value("/v1/teams", None, &Page::default(), |ledger, page| {
        views::teams(ledger, None, page)
    })?;
    // Muster is optional: without it, teams are only reported.
    let muster = rally::wake::muster_available();
    let sender: Option<&dyn Sender> = (muster && !report_only).then_some(&rally::wake::MusterShim);
    let roster = if muster {
        std::process::Command::new("muster-shim")
            .arg("roster")
            .output()
            .ok()
            .and_then(|out| serde_json::from_slice::<Value>(&out.stdout).ok())
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let mut state: Value = std::fs::read_to_string(state_file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({"teams": {}}));
    let mut decisions = std::collections::BTreeMap::new();
    let mut rows = Vec::new();
    for project in &projects {
        let Some(team) = project.team.as_deref() else {
            continue;
        };
        let key = team.trim().to_lowercase();
        let mut row = json!({"team": team, "project": project.name});
        let pick = match web::read_roadmap(project) {
            Ok((_, text)) => read.pick(team, &rally::next::items(&text, &project.prefix)),
            Err(error) => {
                row["error"] = json!(format!("the roadmap couldn't be read: {error}"));
                rows.push(row);
                continue;
            }
        };
        let empty = serde_json::Map::new();
        let claims: Vec<(String, i64)> = read.stories["stories"]
            .as_object()
            .unwrap_or(&empty)
            .iter()
            .filter(|(_, story)| {
                story["state"] == "claimed"
                    && story["team"]
                        .as_str()
                        .is_some_and(|t| t.trim().to_lowercase() == key)
            })
            .map(|(id, story)| (id.clone(), story["last_at_ms"].as_i64().unwrap_or(0)))
            .collect();
        let seen: Vec<u64> = state["teams"][&key]["answers_seen"]
            .as_array()
            .map(|seen| seen.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();
        let answers: Vec<u64> = pick["shelved"]
            .as_array()
            .map(|shelves| {
                shelves
                    .iter()
                    .filter(|shelf| shelf["answered"] == true)
                    .filter_map(|shelf| shelf["blocker"].as_u64())
                    .collect()
            })
            .unwrap_or_default();
        let fresh: Vec<u64> = answers
            .iter()
            .copied()
            .filter(|b| !seen.contains(b))
            .collect();
        let facts = rally::wake::Team {
            name: team.to_owned(),
            sessions: rally::wake::sessions_of(&roster, &project.sessions),
            status: teams["teams"][&key]["status"].as_str().map(str::to_owned),
            pick: pick.clone(),
            claims,
            last_woken_ms: state["teams"][&key]["last_woken_ms"].as_i64(),
            answered: !fresh.is_empty(),
        };
        let decision = rally::wake::decide(&facts, times, now_ms);
        row["next"] = pick["next"]["story"].clone();
        if let Decision::Wake(digest) = &decision {
            match sender {
                Some(sender) => match sender.send(team, digest) {
                    Ok(()) => {
                        row["sent_by"] = json!(sender.name());
                        state["teams"][&key]["last_woken_ms"] = json!(now_ms);
                        let mut all = seen.clone();
                        all.extend(fresh);
                        state["teams"][&key]["answers_seen"] = json!(all);
                    }
                    Err(error) => row["error"] = json!(error),
                },
                None => row["sent_by"] = json!(null),
            }
        }
        decisions.insert(team.to_owned(), decision);
        rows.push(row);
    }
    let mut report = rally::wake::report(&decisions, now_ms);
    // Each team's decision, with what was read and sent.
    if let Some(list) = report["teams"].as_array_mut() {
        for entry in list.iter_mut() {
            if let Some(row) = rows.iter().find(|row| row["team"] == entry["team"]) {
                for field in ["project", "next", "sent_by", "error"] {
                    if let Some(value) = row.get(field) {
                        entry[field] = value.clone();
                    }
                }
            }
        }
        for row in rows
            .iter()
            .filter(|row| !decisions.contains_key(row["team"].as_str().unwrap_or("")))
        {
            list.push(row.clone());
        }
    }
    report["sender"] = json!(sender.map_or("none: report only", |s| s.name()));
    report["untrusted_text"] = json!(views::UNTRUSTED_TEXT);
    if let Some(parent) = state_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(state_file, serde_json::to_string_pretty(&state)?)?;
    std::fs::write(
        state_file.with_file_name("wake-status.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    Ok(serde_json::to_string_pretty(&report)?)
}

fn next(
    team: &str,
    roadmap: &Path,
    prefix: Option<String>,
    location: Option<&Location>,
    config: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(roadmap)
        .map_err(|e| format!("can't read {}: {e}", roadmap.display()))?;
    let prefix = match prefix {
        Some(prefix) => prefix,
        None => rally::next::prefix_of(&text).ok_or_else(|| {
            format!(
                "{} has no stories with IDs: pass --prefix",
                roadmap.display()
            )
        })?,
    };
    let items = rally::next::items(&text, &prefix);
    let backend = Backend::choose(location, config, false)?;
    let read = PickRead::read(&backend)?;
    let mut result = read.pick(team, &items);
    // A team's own declared state, such as resting (RAL-54).
    let teams = backend.read_value("/v1/teams", None, &Page::default(), |ledger, page| {
        views::teams(ledger, None, page)
    })?;
    if let Some(state) = teams["teams"].get(team.trim().to_lowercase()) {
        result["team_status"] = state["status"].clone();
    }
    result["roadmap"] = json!(roadmap);
    result["prefix"] = json!(prefix);
    result["untrusted_text"] = json!(views::UNTRUSTED_TEXT);
    Ok(serde_json::to_string_pretty(&result)?)
}

/// The reply's JSON, or the service's reason for refusing.
fn reply_value(reply: &client::Reply) -> Result<Value, Box<dyn std::error::Error>> {
    let value: Value = serde_json::from_str(&reply.body)
        .map_err(|_| format!("the service sent an unreadable reply ({})", reply.status))?;
    if (200..300).contains(&reply.status) {
        Ok(value)
    } else {
        let reason = value["error"].as_str().unwrap_or("no reason given");
        Err(format!("refused ({}): {reason}", reply.status).into())
    }
}

/// Makes a name safe to put in a URL path.
fn encode(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Creates an empty private file, refusing one that already exists.
fn create_new_private(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(())
}

/// The ledger named by --db and --head-file, if any.
fn location(
    db: Option<String>,
    head_file: Option<PathBuf>,
    password_file: Option<PathBuf>,
) -> Result<Option<Location>, Box<dyn std::error::Error>> {
    match (db, head_file) {
        (Some(url), Some(head_file)) => Ok(Some(Location {
            url,
            head_file,
            password_file,
        })),
        (Some(_), None) => Err("--db needs --head-file or RALLY_HEAD_FILE".into()),
        (None, _) if password_file.is_some() => {
            Err("--db-password-file is only for --db, on the host".into())
        }
        (None, _) => Ok(None),
    }
}

fn config_path(config: Option<PathBuf>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    match config {
        Some(path) => Ok(path),
        None => {
            let home = std::env::var_os("HOME").ok_or("HOME isn't set; use --config")?;
            Ok(PathBuf::from(home).join(".config/rally/client.json"))
        }
    }
}

/// The viewer's projects file when `--projects` isn't given (RAL-74): `projects.json` beside the
/// pairing (`~/.config/rally/projects.json` by default), or `None` for the built-in list when there
/// isn't one. A file that's there but can't be read is an error, never a quiet fall back to the
/// built-in list.
fn default_projects_file(config: &Path) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let path = config.with_file_name("projects.json");
    match std::fs::metadata(&path) {
        Ok(_) => Ok(Some(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("can't read {}: {error}", path.display()).into()),
    }
}

/// The most pages of each list the viewer reads, each of up to [`views::LIST_LIMIT`] entries.
const VIEWER_PAGES: usize = 20;

/// The most pages of later corrections the viewer reads for a past moment.
const PAST_CORRECTION_PAGES: usize = 5;

/// Reads the ledger as it stood at a past record, and the corrections made since to what it
/// held then (RAL-28). The past state is a replay to that record, through the same views as
/// the present one, so it's what `rally show --as-of` would show.
fn past_reader(location: Option<Location>, config: PathBuf) -> PastReader {
    let read = move |seq: u64| -> Result<Value, Box<dyn std::error::Error>> {
        let backend = Backend::choose(location.as_ref(), &config, false)?;
        read_past(&backend, seq)
    };
    Box::new(move |seq| read(seq).map_err(|error| error.to_string()))
}

/// The past moment [`past_reader`] and `rally past` show.
fn read_past(backend: &Backend, seq: u64) -> Result<Value, Box<dyn std::error::Error>> {
    if seq == 0 {
        return Err("a past moment is a record number, from 1".into());
    }
    let as_of = Some(seq);
    let (stories, more_stories) = backend.read_all_as_of(
        "/v1/stories",
        as_of,
        "stories",
        VIEWER_PAGES,
        |ledger, page| views::stories(ledger, None, page),
    )?;
    // The moment the replay stands at, which may be earlier than asked for past the end.
    let then = stories["as_of"].as_u64().unwrap_or(seq);
    let then_ms = stories["as_of_at_ms"].as_i64().unwrap_or(0);
    let (teams, more_teams) =
        backend.read_all_as_of("/v1/teams", as_of, "teams", VIEWER_PAGES, |ledger, page| {
            views::teams(ledger, None, page)
        })?;
    // Blockers as they stood, judged at that moment's own time, not today's.
    let blockers = backend.read_value(
        &format!("/v1/blockers?at_ms={then_ms}"),
        as_of,
        &Page::default(),
        |ledger, page| views::blockers_at(ledger, false, page, ledger.last_at_ms()),
    )?;
    let mut value = json!({
        "untrusted_text": views::UNTRUSTED_TEXT,
        "as_of": then,
        "as_of_at_ms": then_ms,
        "stories": stories["stories"],
        "teams": teams["teams"],
        "blockers": blockers["blockers"],
        "blockers_truncated": blockers.get("next").is_some(),
        "truncated": more_stories || more_teams,
    });
    // Corrections come from the ledger as it is now, so they're shown as later. A service from
    // before RAL-28 can't list them, which is said rather than shown as none.
    match backend.read_all(
        &format!("/v1/corrections?since={then}"),
        "corrections",
        PAST_CORRECTION_PAGES,
        |ledger, page| views::corrections_since(ledger, then, page),
    ) {
        Ok((corrections, more)) => {
            value["now"] = corrections["as_of"].clone();
            value["corrections"] = corrections["corrections"].clone();
            value["corrections_truncated"] = json!(more);
        }
        Err(error) => value["corrections_unavailable"] = json!(error.to_string()),
    }
    Ok(value)
}

/// Reads the ledger's stories, teams and check, from its database or through the service.
fn ledger_reader(location: Option<Location>, config: PathBuf) -> LedgerReader {
    let read = move || -> Result<Value, Box<dyn std::error::Error>> {
        // Read-only, straight from Postgres: the viewer has no write path.
        let backend = Backend::choose(location.as_ref(), &config, false)?;
        // Check first: reads refuse a broken chain, and the failure itself is what has to show.
        let (check, read_only) = match &backend {
            // Opened read-only above.
            Backend::File(store) => (json!(store.verify(None)?), json!(true)),
            Backend::Service(pairing) => {
                let check = reply_value(&client::request(pairing, "GET", "/v1/check", None, &[])?)?;
                let who = reply_value(&client::request(pairing, "GET", "/v1/whoami", None, &[])?)?;
                (check, who["read_only"].clone())
            }
        };
        if check["ok"] != json!(true) {
            return Ok(json!({"check": check, "read_only": read_only}));
        }
        // Pages of a list may come from different moments if writes land in between; each
        // shows its own `as_of`, and the first page's is the one reported.
        let (stories, more_stories) =
            backend.read_all("/v1/stories", "stories", VIEWER_PAGES, |ledger, page| {
                views::stories(ledger, None, page)
            })?;
        let (teams, more_teams) =
            backend.read_all("/v1/teams", "teams", VIEWER_PAGES, |ledger, page| {
                views::teams(ledger, None, page)
            })?;
        // Open blockers for RAL-23, one page; more than that is flagged rather than read.
        let blockers =
            backend.read_value("/v1/blockers", None, &Page::default(), |ledger, page| {
                views::blockers(ledger, false, page)
            })?;
        // Each team's track record, one page (RAL-27); more teams than that is flagged.
        let tracks = backend.read_value(
            "/v1/track-records",
            None,
            &Page::default(),
            |ledger, page| views::track_records(ledger, None, page),
        )?;
        // The rallies (RAL-76). A service older than RAL-76 has none to give, which the page says.
        let rallies = backend
            .read_value("/v1/rallies", None, &Page::default(), views::rallies)
            .unwrap_or_else(|error| json!({"available": false, "reason": error.to_string()}));
        Ok(json!({
            "rallies": rallies,
            "as_of": stories["as_of"],
            "as_of_at_ms": stories["as_of_at_ms"],
            "stories": stories["stories"],
            "track_records": tracks["teams"],
            "verifiers": tracks["verifiers"],
            "track_records_truncated": tracks.get("next").is_some(),
            "plain_facts": tracks["plain_facts"],
            "problems_reported_are": tracks["problems_reported_are"],
            "teams": teams["teams"],
            "blockers": blockers["blockers"],
            "blockers_truncated": blockers.get("next").is_some(),
            "truncated": more_stories || more_teams,
            "check": check,
            "read_only": read_only,
        }))
    };
    Box::new(move || read().map_err(|error| error.to_string()))
}

/// `rally web --login`: asks the running viewer for a one-time code, showing the key in a header
/// over loopback, never in a URL or an argument, and prints the address to open (RAL-29).
fn web_login(bind: &str, key_file: &Path) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    let address: std::net::SocketAddr = bind.parse()?;
    if !address.ip().is_loopback() {
        return Err(format!("the viewer only runs on a loopback address, not {address}").into());
    }
    let key = std::fs::read_to_string(key_file)
        .map_err(|e| format!("can't read {}: {e}", key_file.display()))?;
    let key = key.trim();
    let mut stream =
        std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_secs(5))
            .map_err(|e| format!("no viewer is answering at {address}: {e}"))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    write!(
        stream,
        "POST {} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {key}\r\n\
         Content-Length: 0\r\nConnection: close\r\n\r\n",
        web::CODE_PATH
    )?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    let (head, body) = reply.split_once("\r\n\r\n").unwrap_or((&reply, ""));
    if !head.starts_with("HTTP/1.1 200") {
        let status = head.lines().next().unwrap_or("no answer");
        return Err(format!("the viewer refused: {status}: {}", body.trim()).into());
    }
    let value: Value =
        serde_json::from_str(body.trim()).map_err(|_| "the viewer sent an unreadable code")?;
    let code = value["code"].as_str().ok_or("the viewer sent no code")?;
    Ok(json!({
        "open": format!("http://{address}/?code={code}"),
        "expires_in_seconds": value["expires_in_seconds"],
        "note": "The code works once. Open it in the browser you'll view from.",
    })
    .to_string())
}

fn run_viewer(
    bind: &str,
    projects: Option<PathBuf>,
    location: Option<Location>,
    config: PathBuf,
    backup: Option<BackupWatch>,
    local: LocalSummaries,
    sign_in: web::SignIn,
) -> Result<String, Box<dyn std::error::Error>> {
    let key_file = sign_in.key_file.as_path();
    let projects_from = projects
        .as_ref()
        .map_or_else(|| json!("built-in"), |path| json!(path));
    let text = match &projects {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("can't read {}: {e}", path.display()))?,
        None => DEFAULT_PROJECTS.to_owned(),
    };
    let projects: Vec<Project> = serde_json::from_str(&text).map_err(|e| {
        let from = projects_from.as_str().unwrap_or("built-in");
        format!("{from} isn't a projects list: {e}")
    })?;
    // A prefix the viewer doesn't list would be silently ignored, so refuse it.
    if let Some(prefix) = local
        .by_prefix
        .keys()
        .find(|prefix| !projects.iter().any(|project| &project.prefix == *prefix))
    {
        return Err(format!("--summaries names {prefix}, which isn't a listed project").into());
    }
    let address: std::net::SocketAddr = bind.parse()?;
    if !address.ip().is_loopback() {
        return Err(format!("the viewer only runs on a loopback address, not {address}").into());
    }
    // The viewer key, once the address is known to be loopback: kept in a private file and never
    // printed (RAL-29). A key others can read isn't a key.
    #[cfg(unix)]
    if let Ok(metadata) = std::fs::metadata(key_file) {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(format!(
                "{} can be read by others; make it private (chmod 600) or remove it",
                key_file.display()
            )
            .into());
        }
    }
    match std::fs::read_to_string(key_file) {
        Ok(text) if web::is_key(text.trim()) => {}
        Ok(_) => return Err(format!("{} doesn't hold a viewer key", key_file.display()).into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = key_file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            rally::access::write_private(key_file, web::new_key()?.as_bytes())?;
        }
        Err(error) => return Err(format!("can't read {}: {error}", key_file.display()).into()),
    }
    let listener = TcpListener::bind(address)?;
    let listening = listener.local_addr()?;
    println!(
        "{}",
        json!({"listening": listening.to_string(), "key_file": key_file,
               "projects": projects_from,
               "sign_in": format!("rally web --bind {listening} --login")})
    );
    std::io::Write::flush(&mut std::io::stdout())?;
    let reader = ledger_reader(location.clone(), config.clone());
    let past = past_reader(location, config);
    tokio::runtime::Runtime::new()?.block_on(web::serve(
        listener,
        sign_in.clone(),
        projects,
        reader,
        past,
        backup,
        local,
    ))?;
    Ok(json!({"stopped": true}).to_string())
}

fn run_service(
    location: &Location,
    bind: &str,
    state_dir: &Path,
    limit: RateLimit,
    operator_key: Option<rally::designation::OperatorKey>,
    operator_times: rally::store::OperatorTimes,
    evidence: rally::evidence::Settings,
) -> Result<String, Box<dyn std::error::Error>> {
    let state = StateDir::open(state_dir)?;
    let evidence_checked = evidence.token.is_some();
    let (_, _, fingerprint) = state.certificate()?;
    let listener = TcpListener::bind(bind)?;
    let listening = json!({"listening": listener.local_addr()?.to_string(), "fingerprint": fingerprint,
            "operator_key": operator_key.as_ref().map(rally::designation::OperatorKey::fingerprint),
            "operator_window_ms": operator_times.window_ms,
            "operator_away_after": operator_times.away_after,
            "operator_away_ms": operator_times.away_ms,
            "evidence_checked_with_token": evidence_checked});
    println!("{listening}");
    std::io::Write::flush(&mut std::io::stdout())?;
    tokio::runtime::Runtime::new()?.block_on(server::serve(
        listener,
        location,
        state,
        limit,
        operator_key,
        Some(operator_times),
        Some(evidence),
    ))?;
    Ok(json!({"stopped": true}).to_string())
}

/// `rally init`: no ledger, service or credential involved.
fn init(
    files: &[PathBuf],
    dir: Option<&std::path::Path>,
    harness: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    let start = match dir {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let hint = if dir.is_some() { "" } else { "; pass --dir" };
    let root = rally::init::repo_root(&start)
        .ok_or_else(|| format!("{} isn't in a git repository{hint}", start.display()))?;
    let harness = rally::session::Harness::parse(harness);
    let done = rally::init::run_for(&root, files, harness)?;
    let files: Vec<_> = done
        .iter()
        .map(|(path, outcome)| json!({"file": path.display().to_string(), "outcome": outcome.label()}))
        .collect();
    Ok(json!({"init": files}).to_string())
}

fn run(cli: Cli) -> Result<String, Box<dyn std::error::Error>> {
    if let Command::Init {
        files,
        dir,
        harness,
    } = &cli.command
    {
        return init(files, dir.as_deref(), harness.as_deref());
    }
    // These never touch the database, so a stray password file setting mustn't block them.
    let uses_database = !matches!(
        cli.command,
        Command::AddMachine { .. } | Command::Revoke { .. } | Command::Pair
    );
    let password_file = cli.db_password_file.filter(|_| uses_database);
    let location = location(cli.db, cli.head_file, password_file)?;
    let (kind, body) = match cli.command {
        Command::Serve {
            bind,
            state_dir,
            burst,
            per_second,
            write_burst,
            write_per_second,
            max_ledger_bytes,
            min_free_bytes,
            max_connections,
            max_connections_per_address,
            connection_seconds,
            failed_token_burst,
            failed_token_per_second,
            heavy_burst,
            heavy_per_second,
            secure,
            operator_key,
            operator_window,
            operator_away_after,
            operator_away,
            github_token_file,
            github_owners,
            evidence_ci_wait,
            evidence_poll,
        } => {
            let location = location.ok_or("serve needs --db and --head-file")?;
            let token = match &github_token_file {
                Some(path) => {
                    if secure {
                        rally::designation::check_owned(path, 0, None)
                            .map_err(|why| format!("--secure refused the GitHub token: {why}"))?;
                    }
                    Some(read_github_token(path)?)
                }
                None => None,
            };
            let evidence = rally::evidence::Settings {
                token,
                owners: github_owners
                    .split(',')
                    .map(|owner| owner.trim().to_owned())
                    .filter(|owner| !owner.is_empty())
                    .collect(),
                timing: rally::evidence::Timing {
                    ceiling_ms: evidence_ci_wait,
                    poll_ms: evidence_poll,
                },
            };
            let operator_key = if secure {
                let key_path = operator_key.ok_or("--secure needs --operator-key")?;
                let exe = std::env::current_exe()?;
                for path in [&exe, &key_path] {
                    rally::designation::check_owned(path, 0, None).map_err(|why| {
                        format!(
                            "--secure refused: {why}.\n{}",
                            rally::designation::secure_steps(&exe, &key_path)
                        )
                    })?;
                }
                Some(rally::designation::OperatorKey::read(&key_path)?)
            } else {
                None
            };
            if !(connection_seconds > 0.0 && connection_seconds.is_finite()) {
                return Err("--connection-seconds must be a positive number".into());
            }
            // A burst below 1 would refuse everything, which is never what was meant.
            for (name, burst, per_second) in [
                ("burst", burst, per_second),
                ("write-burst", write_burst, write_per_second),
                (
                    "failed-token-burst",
                    failed_token_burst,
                    failed_token_per_second,
                ),
                ("heavy-burst", heavy_burst, heavy_per_second),
            ] {
                if !(burst >= 1.0 && burst.is_finite()) {
                    return Err(format!("--{name} must be at least 1").into());
                }
                if !(per_second >= 0.0 && per_second.is_finite()) {
                    return Err(
                        format!("the rate that goes with --{name} can't be negative").into(),
                    );
                }
            }
            if max_connections == 0 || max_connections_per_address == 0 {
                return Err("connection limits must be at least 1".into());
            }
            let limit = RateLimit {
                burst,
                per_second,
                write_burst,
                write_per_second,
                max_ledger_bytes,
                min_free_bytes,
                max_connections,
                max_connections_per_address,
                connection_seconds,
                failed_token_burst,
                failed_token_per_second,
                heavy_burst,
                heavy_per_second,
            };
            return run_service(
                &location,
                &bind,
                &state_dir,
                limit,
                operator_key,
                rally::store::OperatorTimes {
                    window_ms: operator_window,
                    away_after: usize::try_from(operator_away_after).unwrap_or(usize::MAX),
                    away_ms: operator_away,
                },
                evidence,
            );
        }
        Command::AddMachine {
            name,
            address,
            state_dir,
            out,
            read_only,
        } => {
            // Claim the file first, so a failure can't leave a machine whose token nobody has.
            create_new_private(&out)?;
            let state = StateDir::open(&state_dir)?;
            let pairing = match state.add_machine(&name, &address, read_only) {
                Ok(pairing) => pairing,
                Err(error) => {
                    let _ = std::fs::remove_file(&out);
                    return Err(error.into());
                }
            };
            if let Err(error) = write_private(&out, pairing.code().as_bytes()) {
                if let Err(revoke) = state.revoke(&name) {
                    eprintln!(
                        "rally: warning: {name} is still paired and its token exists only on this \
                         machine's state, so revoke it by hand: {revoke}"
                    );
                }
                return Err(error.into());
            }
            return Ok(json!({"machine": name, "pair_code_file": out}).to_string());
        }
        Command::Revoke { name, state_dir } => {
            StateDir::open(&state_dir)?.revoke(&name)?;
            return Ok(json!({"revoked": name}).to_string());
        }
        Command::Init { .. } => unreachable!("handled before the ledger is opened"),
        Command::SessionStart { .. } | Command::Levi { .. } | Command::CheckGithubToken { .. } => {
            unreachable!("handled in main")
        }
        Command::Pair => {
            let mut code = String::new();
            std::io::stdin().read_line(&mut code)?;
            let pairing = Pairing::parse(&code)?;
            let path = config_path(cli.config)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            write_private(&path, serde_json::to_string(&pairing)?.as_bytes())?;
            return Ok(json!({"paired": pairing.address, "config": path}).to_string());
        }
        Command::Web {
            bind,
            projects,
            backup_dir,
            backup_max_age_hours,
            summaries,
            watchman_summaries,
            key_file,
            login,
            new_key,
            code_seconds,
        } => {
            let config = config_path(cli.config)?;
            let key_file = key_file.unwrap_or_else(|| config.with_file_name("web-key"));
            if new_key {
                let key = web::new_key()?;
                if let Some(parent) = key_file.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                rally::access::write_private(&key_file, key.as_bytes())?;
                return Ok(json!({"new_key": key_file,
                                 "signed_out": "every browser signed in to a viewer using this file"})
                .to_string());
            }
            if login {
                return web_login(&bind, &key_file);
            }
            let backup = backup_dir.map(|dir| BackupWatch {
                dir,
                max_age_hours: backup_max_age_hours,
            });
            let mut local = LocalSummaries {
                watchman: watchman_summaries.map(|path| expand_home(&path)),
                ..LocalSummaries::default()
            };
            for pair in summaries {
                let (prefix, folder) = pair
                    .split_once('=')
                    .ok_or_else(|| format!("--summaries takes PREFIX=FOLDER, not {pair:?}"))?;
                local
                    .by_prefix
                    .insert(prefix.to_owned(), expand_home(std::path::Path::new(folder)));
            }
            // RAL-74: the operator's own list beside the pairing, if there is one, then the built-in.
            let projects = match projects {
                Some(path) => Some(path),
                None => default_projects_file(&config)?,
            };
            return run_viewer(
                &bind,
                projects,
                location,
                config,
                backup,
                local,
                web::SignIn {
                    key_file,
                    code_ttl: std::time::Duration::from_secs(code_seconds),
                },
            );
        }
        Command::Backup { to, max_age_hours } => {
            if location.is_some() {
                return Err(
                    "backup reads through the service, from another machine; drop --db".into(),
                );
            }
            let Backend::Service(pairing) =
                Backend::choose(None, &config_path(cli.config)?, false)?
            else {
                unreachable!("without --db, the backend is the service");
            };
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as i64);
            let fetch = |after: u64| -> Result<rally::backup::Page, String> {
                let path = format!("/v1/export?after={after}");
                let reply = client::request(&pairing, "GET", &path, None, &[])
                    .map_err(|e| e.to_string())?;
                let value = reply_value(&reply).map_err(|e| e.to_string())?;
                serde_json::from_value(value).map_err(|_| "the export can't be read".to_owned())
            };
            return match rally::backup::pull(&to, now, fetch) {
                Ok(pulled) => Ok(json!({
                    "backed_up_to": pulled.head.seq,
                    "added": pulled.added,
                    "expect": format!("{}:{}", pulled.head.seq, pulled.head.hash),
                })
                .to_string()),
                Err(error) => {
                    let age = rally::backup::saved(&to)
                        .ok()
                        .flatten()
                        .map(|saved| (now - saved.at_ms) as f64 / 3_600_000.0);
                    let when = match age {
                        Some(hours) if hours > max_age_hours => {
                            format!("; the last backup was {hours:.1} hours ago, which is overdue")
                        }
                        Some(hours) => format!("; the last backup was {hours:.1} hours ago"),
                        None => "; there's no backup yet".to_owned(),
                    };
                    Err(format!("{error}{when}").into())
                }
            };
        }
        Command::Restore { from } => {
            let location =
                location.ok_or("restore needs --db and --head-file for the new ledger")?;
            let (records, head) = rally::backup::read(&from)?;
            let saved = rally::backup::saved(&from)?;
            let restored = Store::restore(&location, &records)?;
            debug_assert_eq!(restored, head);
            return Ok(json!({
                "restored_to": restored.seq,
                "expect": format!("{}:{}", restored.seq, restored.hash),
                // Behind only if the last backup stopped before saving its head; the records are
                // what was restored.
                "backup_head_file": saved.map(|saved| format!("{}:{}", saved.seq, saved.hash)),
            })
            .to_string());
        }
        Command::AdoptHead => {
            let location = location.ok_or("adopt-head needs --db, on the service's host")?;
            // Under the writer lock, so it can't race the service and wind the head back.
            let head = Store::adopt_head_at(&location)?;
            return Ok(json!({"adopted": head}).to_string());
        }
        Command::Check { expect } => {
            let backend = Backend::choose(location.as_ref(), &config_path(cli.config)?, false)?;
            let result: Value = match &backend {
                Backend::File(store) => {
                    let mut value = json!(store.verify(expect.as_ref())?);
                    value["untrusted_text"] = json!(rally::views::UNTRUSTED_TEXT);
                    value
                }
                Backend::Service(pairing) => {
                    let path = match &expect {
                        Some(head) => format!("/v1/check?seq={}&hash={}", head.seq, head.hash),
                        None => "/v1/check".to_owned(),
                    };
                    reply_value(&client::request(pairing, "GET", &path, None, &[])?)?
                }
            };
            let text = serde_json::to_string_pretty(&result)?;
            if result["ok"] == json!(true) {
                return Ok(text);
            }
            return Err(format!("the ledger failed its check:\n{text}").into());
        }
        Command::Gate { range, repo, allow } => {
            let commits = rally::gate::commits(&repo, &range)?;
            let backend = Backend::choose(location.as_ref(), &config_path(cli.config)?, false)?;
            let (read, more) =
                backend.read_all("/v1/stories", "stories", GATE_PAGES, |ledger, page| {
                    views::stories(ledger, None, page)
                })?;
            if more {
                return Err("the ledger has more stories than the gate reads: refused".into());
            }
            let stories = read["stories"].as_object().cloned().unwrap_or_default();
            let found =
                rally::gate::judge(&range, &commits, &stories, &allow.into_iter().collect());
            let text = serde_json::to_string_pretty(&found)?;
            if found["ok"] == json!(true) {
                return Ok(text);
            }
            return Err(format!("the gate refused {range}:\n{text}").into());
        }
        Command::Show {
            story,
            as_of,
            after,
            before,
            brief,
            field,
        } => {
            let backend = Backend::choose(location.as_ref(), &config_path(cli.config)?, false)?;
            let path = match &story {
                Some(id) => format!("/v1/stories/{}", encode(id)),
                None => "/v1/stories".to_owned(),
            };
            let page = Page { after, before };
            let value = backend.read_value(&path, as_of, &page, |ledger, page| {
                views::stories(ledger, story.as_deref(), page)
            })?;
            if let Some(name) = field {
                return Ok(views::field(&value, &name)?);
            }
            let value = if brief { views::brief(&value) } else { value };
            return Ok(serde_json::to_string_pretty(&value)?);
        }
        Command::Teams {
            team,
            as_of,
            after,
            before,
        } => {
            let backend = Backend::choose(location.as_ref(), &config_path(cli.config)?, false)?;
            let path = match &team {
                Some(name) => format!("/v1/teams/{}", encode(name)),
                None => "/v1/teams".to_owned(),
            };
            let page = Page { after, before };
            return backend.read(&path, as_of, &page, |ledger, page| {
                views::teams(ledger, team.as_deref(), page)
            });
        }
        Command::Record { team, as_of, after } => {
            let backend = Backend::choose(location.as_ref(), &config_path(cli.config)?, false)?;
            let path = format!("/v1/teams/{}/record", encode(&team));
            let page = Page {
                after,
                before: None,
            };
            return backend.read(&path, as_of, &page, |ledger, page| {
                views::team_record(ledger, &team, page)
            });
        }
        Command::TrackRecord { team, as_of, after } => {
            let backend = Backend::choose(location.as_ref(), &config_path(cli.config)?, false)?;
            let path = match &team {
                Some(name) => format!("/v1/track-records/{}", encode(name)),
                None => "/v1/track-records".to_owned(),
            };
            let page = Page {
                after,
                before: None,
            };
            return backend.read(&path, as_of, &page, |ledger, page| {
                views::track_records(ledger, team.as_deref(), page)
            });
        }
        Command::Rallies { as_of } => {
            let backend = Backend::choose(location.as_ref(), &config_path(cli.config)?, false)?;
            return backend.read("/v1/rallies", as_of, &Page::default(), views::rallies);
        }
        Command::Blockers { all, as_of, after } => {
            let backend = Backend::choose(location.as_ref(), &config_path(cli.config)?, false)?;
            let path = if all {
                "/v1/blockers/all"
            } else {
                "/v1/blockers"
            };
            let page = Page {
                after,
                before: None,
            };
            return backend.read(path, as_of, &page, |ledger, page| {
                views::blockers(ledger, all, page)
            });
        }
        Command::Block {
            team,
            story,
            on_story,
            on_team,
            note,
        } => {
            let mut body = json!({"change": "block", "team": team, "note": note});
            for (field, value) in [
                ("story", story),
                ("on_story", on_story),
                ("on_team", on_team),
            ] {
                if let Some(value) = value {
                    body[field] = json!(value);
                }
            }
            (Kind::Declared, body)
        }
        Command::Hold {
            team,
            story,
            note,
            releases,
        } => {
            let mut body = json!({"change": "standing", "standing": "hold", "team": team,
                                  "note": note, "releases": releases});
            if let Some(story) = story {
                body["story"] = json!(story);
            }
            (Kind::Declared, body)
        }
        Command::Condition { team, story, note } => {
            let mut body =
                json!({"change": "standing", "standing": "condition", "team": team, "note": note});
            if let Some(story) = story {
                body["story"] = json!(story);
            }
            (Kind::Declared, body)
        }
        Command::Grant {
            team,
            story,
            scope,
            note,
            direct,
            relayed_by,
        } => {
            let given = match (direct, relayed_by) {
                (true, _) => json!({"from": "direct"}),
                (false, Some(by)) => json!({"from": "relayed", "by": by}),
                (false, None) => return Err("say --direct or --relayed-by".into()),
            };
            let mut body = json!({"change": "standing", "standing": "grant", "team": team,
                                  "scope": scope, "note": note, "given": given});
            if let Some(story) = story {
                body["story"] = json!(story);
            }
            (Kind::Declared, body)
        }
        Command::OpenRally {
            name,
            projects,
            teams,
        } => {
            let projects: Vec<String> = projects.iter().map(|p| p.trim().to_uppercase()).collect();
            let mut body = json!({"change": "open_rally", "name": name, "projects": projects});
            if !teams.is_empty() {
                body["teams"] = json!(teams);
            }
            (Kind::Declared, body)
        }
        Command::CloseRally { note } => (
            Kind::Declared,
            json!({"change": "close_rally", "note": note}),
        ),
        Command::EndStanding { standing, note } => (
            Kind::Declared,
            json!({"change": "end_standing", "standing": standing, "note": note}),
        ),
        Command::Unblock {
            blocker,
            note,
            direct,
            relayed_by,
        } => {
            let mut body = json!({"change": "unblock", "blocker": blocker, "note": note});
            if direct {
                body["answer"] = json!({"from": "direct"});
            } else if let Some(by) = relayed_by {
                body["answer"] = json!({"from": "relayed", "by": by});
            }
            (Kind::Declared, body)
        }
        Command::Shelve { blocker, team } => {
            if location.is_some() {
                return Err(
                    "a shelve goes through the service, whose clock and agreed window decide it; \
                     don't use --db"
                        .into(),
                );
            }
            (
                Kind::Declared,
                json!({"change": "shelve", "blocker": blocker, "team": team}),
            )
        }
        Command::TeamState {
            team,
            status,
            story,
            note,
        } => {
            let mut body = json!({"change": "team_state", "team": team, "status": status});
            if let Some(story) = story {
                body["story"] = json!(story);
            }
            if let Some(note) = note {
                body["note"] = json!(note);
            }
            (Kind::Declared, body)
        }
        Command::Ready { story } => (Kind::Declared, json!({"change": "ready", "story": story})),
        Command::Claim { story, team } => (
            Kind::Declared,
            json!({"change": "claim", "story": story, "team": team}),
        ),
        Command::Release { story, team } => (
            Kind::Declared,
            json!({"change": "release", "story": story, "team": team}),
        ),
        Command::Submit {
            story,
            team,
            evidence,
            untested,
            projects,
            not_checkable,
        } => {
            if !not_checkable {
                check_evidence(&story, &evidence, projects.as_deref())?;
            }
            submission(&story, &team, &evidence, untested, false)
        }
        Command::Amend {
            story,
            team,
            evidence,
            untested,
            projects,
            not_checkable,
        } => {
            if !not_checkable {
                check_evidence(&story, &evidence, projects.as_deref())?;
            }
            submission(&story, &team, &evidence, untested, true)
        }
        Command::Verify {
            story,
            note,
            commit,
            team,
        } => {
            let mut body =
                json!({"change": "verify", "story": story, "note": note, "commit": commit});
            if let Some(team) = team {
                body["team"] = json!(team);
            }
            (Kind::Verified, body)
        }
        Command::SendBack {
            story,
            note,
            commit,
            team,
        } => {
            let mut body = json!({"change": "send_back", "story": story, "note": note});
            if let Some(commit) = commit {
                body["commit"] = json!(commit);
            }
            if let Some(team) = team {
                body["team"] = json!(team);
            }
            (Kind::Verified, body)
        }
        Command::RequestRead {
            story,
            commit,
            note,
        } => (
            Kind::Declared,
            json!({"change": "request_read", "story": story, "commit": commit, "note": note}),
        ),
        Command::WithdrawRead {
            story,
            request,
            note,
            as_verifier,
        } => {
            let kind = if as_verifier {
                Kind::Verified
            } else {
                Kind::Declared
            };
            (
                kind,
                json!({"change": "withdraw_read", "story": story, "request": request, "note": note}),
            )
        }
        Command::Designation | Command::Verifications | Command::Handover => {
            if let Some(location) = &location {
                // On the host, without the service's key: nothing counts as designated.
                let store = Store::open_read_only(location)?;
                let report = match cli.command {
                    Command::Verifications => store.verifications_report(None)?,
                    Command::Handover => store.handover_report(None)?,
                    _ => store.designation_report(None)?,
                };
                return Ok(report.to_string());
            }
            let pairing = read_pairing(&config_path(cli.config.clone())?)?;
            let path = match cli.command {
                Command::Verifications => "/v1/verifications",
                Command::Handover => "/v1/handover",
                _ => "/v1/designation",
            };
            let reply = client::request(&pairing, "GET", path, None, &[])?;
            return Ok(reply_value(&reply)?.to_string());
        }
        Command::Stalls {
            projects,
            quiet_after,
            lock,
        } => {
            let config = config_path(cli.config)?;
            return stalls_pass(
                projects.as_deref(),
                location.as_ref(),
                &config,
                quiet_after,
                &lock,
            );
        }
        Command::Wake {
            projects,
            quiet_after,
            interval,
            every,
            state_file,
            report_only,
        } => {
            let config = config_path(cli.config)?;
            let state_file = match state_file {
                Some(path) => path,
                None => config.with_file_name("wake.json"),
            };
            let times = rally::wake::Times {
                quiet_after_ms: quiet_after,
                interval_ms: interval,
            };
            loop {
                let report = wake_pass(
                    projects.as_deref(),
                    location.as_ref(),
                    &config,
                    &state_file,
                    times,
                    report_only,
                )?;
                let Some(every) = every else {
                    return Ok(report);
                };
                println!("{}", escape_for_terminal(&report));
                std::thread::sleep(std::time::Duration::from_millis(every));
            }
        }
        Command::Digest {
            quiet,
            max,
            every,
            watchman,
            state_file,
            report_only,
        } => {
            let config = config_path(cli.config)?;
            let state_file = state_file.unwrap_or_else(|| config.with_file_name("digest.json"));
            let times = rally::digest::Times {
                quiet_ms: quiet,
                max_ms: max,
            };
            loop {
                let report = digest_pass(
                    location.as_ref(),
                    &config,
                    &state_file,
                    times,
                    watchman.as_deref(),
                    report_only,
                )?;
                let Some(every) = every else {
                    return Ok(report);
                };
                println!("{}", escape_for_terminal(&report));
                std::thread::sleep(std::time::Duration::from_millis(every));
            }
        }
        Command::NewStory {
            prefix,
            title,
            roadmap,
            repo,
        } => {
            let config = config_path(cli.config)?;
            return new_story(
                &prefix,
                &title,
                (&roadmap, repo.as_deref()),
                location.as_ref(),
                &config,
                cli.name.as_deref(),
                cli.credential.as_deref(),
            );
        }
        Command::Next {
            team,
            roadmap,
            prefix,
        } => {
            return next(
                &team,
                &roadmap,
                prefix,
                location.as_ref(),
                &config_path(cli.config)?,
            );
        }
        Command::Past { as_of } => {
            let backend = Backend::choose(location.as_ref(), &config_path(cli.config)?, false)?;
            return Ok(serde_json::to_string_pretty(&read_past(&backend, as_of)?)?);
        }
        Command::Brief => {
            let mut report = if let Some(location) = &location {
                // On the host, without the service's key: designations can't be checked.
                Store::open_read_only(location)?.brief_report(None, now_ms()?)?
            } else {
                let pairing = read_pairing(&config_path(cli.config.clone())?)?;
                let reply = client::request(&pairing, "GET", "/v1/brief", None, &[])?;
                reply_value(&reply)?
            };
            // Muster is optional: without it, claims say their sessions couldn't be checked.
            let roster = rally::wake::muster_available()
                .then(|| {
                    std::process::Command::new("muster-shim")
                        .arg("roster")
                        .output()
                        .ok()
                        .filter(|out| out.status.success())
                        .and_then(|out| serde_json::from_slice::<Value>(&out.stdout).ok())
                })
                .flatten();
            rally::brief::mark_sessions(&mut report, roster.as_ref());
            return Ok(report.to_string());
        }
        Command::Waiting => {
            let backend = Backend::choose(location.as_ref(), &config_path(cli.config)?, false)?;
            let page = Page {
                after: None,
                before: None,
            };
            return backend.read("/v1/waiting", None, &page, |ledger, _| {
                Ok(views::waiting(ledger))
            });
        }
        Command::Designate {
            watchman,
            end,
            credential,
            scope,
            key,
        } => {
            // v3 (RAL-63): adding one watchman with a scope, or ending one by name. Otherwise v1 or
            // v2, which replace every watchman in force, or end them all.
            let action = match (end, &watchman, &scope) {
                (false, _, Some(_)) => Some(rally::ledger::DesignateAction::Add),
                (true, Some(_), _) => Some(rally::ledger::DesignateAction::End),
                _ => None,
            };
            // Ending every watchman names none.
            let watchman = if end && action.is_none() {
                None
            } else {
                watchman
            };
            if let Some(scope) = &scope
                && !rally::ledger::is_scope(scope)
            {
                return Err(format!(
                    "--scope must be all, team:<team> or machine:<credential>, not {scope:?}"
                )
                .into());
            }
            if location.is_some() {
                return Err(
                    "designations go through the service, which holds the operator's key; \
                            don't use --db"
                        .into(),
                );
            }
            let pairing = read_pairing(&config_path(cli.config.clone())?)?;
            let check = reply_value(&client::request(&pairing, "GET", "/v1/check", None, &[])?)?;
            let (Some(after_seq), Some(after_hash)) = (
                check["head"]["seq"].as_u64(),
                check["head"]["hash"].as_str(),
            ) else {
                return Err("the service didn't say where the ledger's head is".into());
            };
            let terms = rally::ledger::Terms {
                watchman: watchman.as_deref(),
                credential: credential.as_deref(),
                action,
                scope: scope.as_deref(),
            };
            // A v1 or v2 designation replaces every watchman in force. With more than one in force,
            // that's probably not meant, since adding one takes --scope.
            if action.is_none() && watchman.is_some() {
                let current = client::request(&pairing, "GET", "/v1/designation", None, &[])
                    .ok()
                    .and_then(|reply| reply_value(&reply).ok());
                let names: Vec<String> = current
                    .as_ref()
                    .and_then(|report| report["watchmen"].as_array())
                    .map(|list| {
                        list.iter()
                            .filter_map(|w| w["name"].as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                if names.len() > 1 {
                    eprintln!(
                        "Warning: {} watchmen are in force ({}). Without --scope this designation \
                         replaces all of them with one. To add a watchman beside them, give \
                         --scope; to end one, use --end with its name.",
                        names.len(),
                        names.join(", ")
                    );
                }
            }
            let statement = terms.statement(after_seq, after_hash);
            let public = if key.extension().is_some_and(|e| e == "pub") {
                key.clone()
            } else {
                PathBuf::from(format!("{}.pub", key.display()))
            };
            let fingerprint = rally::designation::OperatorKey::read(&public)?.fingerprint();
            eprintln!(
                "Signing this with {} ({fingerprint}). Read it before you touch:\n{statement}",
                key.display()
            );
            let signature = ssh_sign(&key, &statement)?;
            (Kind::Declared, {
                let mut body = json!({
                    "change": "designate", "watchman": watchman, "after_seq": after_seq,
                    "after_hash": after_hash, "key_fingerprint": fingerprint,
                    "signature": signature,
                });
                if let Some(credential) = &credential {
                    body["credential"] = json!(credential);
                }
                if let Some(action) = action {
                    body["action"] = json!(action);
                }
                if let Some(scope) = &scope {
                    body["scope"] = json!(scope);
                }
                body
            })
        }
        Command::Annotate {
            corrects,
            team,
            note,
            also_read,
        } => {
            let mut body =
                json!({"change": "annotate", "corrects": corrects, "team": team, "note": note});
            if !also_read.is_empty() {
                body["also_read"] = json!(also_read);
            }
            (Kind::Declared, body)
        }
        Command::Merge { story, commit } => (
            Kind::Declared,
            json!({"change": "merge", "story": story, "commit": commit}),
        ),
        Command::Correct {
            story,
            corrects,
            note,
            state,
            team,
            as_verifier,
            commit,
        } => {
            let mut body = json!({
                "change": "correct", "story": story, "corrects": corrects, "note": note
            });
            if let Some(state) = state {
                body["state"] = json!(state);
            }
            if let Some(team) = team {
                body["team"] = json!(team);
            }
            if let Some(commit) = commit {
                body["commit"] = json!(commit);
            }
            let kind = if as_verifier {
                Kind::Verified
            } else {
                Kind::Declared
            };
            (kind, body)
        }
    };

    let name = cli.name.ok_or("--as or RALLY_NAME is needed to write")?;
    let body = match (cli.entered_at_ms, cli.entered_by) {
        (Some(entered_at_ms), Some(entered_by)) => json!({
            "change": "backfill", "entered_at_ms": entered_at_ms, "entered_by": entered_by,
            "entry": body
        }),
        _ => body,
    };
    let backend = Backend::choose(location.as_ref(), &config_path(cli.config)?, true)?;
    match backend {
        Backend::File(store) => {
            let credential = cli
                .credential
                .ok_or("--credential or RALLY_CREDENTIAL is needed to write with --db")?;
            let source = Source {
                credential,
                declared_name: Some(name),
            };
            let record = Record::new(kind, source, now_ms()?, body)?;
            let seq = store.append(&record)?;
            Ok(json!({"recorded": seq}).to_string())
        }
        Backend::Service(pairing) => {
            let request = json!({"kind": kind, "declared_name": name, "body": body}).to_string();
            let reply = client::request(&pairing, "POST", "/v1/records", Some(&request), &[])?;
            Ok(reply_value(&reply)?.to_string())
        }
    }
}

/// A leading `~/` as the home folder, since a shell doesn't expand it after `=`.
fn expand_home(path: &std::path::Path) -> PathBuf {
    match (path.strip_prefix("~"), std::env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.to_owned(),
    }
}

fn now_ms() -> Result<i64, Box<dyn std::error::Error>> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
    )?)
}
