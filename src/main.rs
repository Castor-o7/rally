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
use rally::web::{self, BackupWatch, DEFAULT_PROJECTS, LedgerReader, LocalSummaries, Project};

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
    },
    /// Run the read-only viewer on this machine's loopback address. See docs/web.md.
    Web {
        #[arg(long, default_value = "127.0.0.1:7300")]
        bind: String,
        /// The projects to show, instead of the defaults.
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
    },
    /// The operator designates the watchman, or with --end ends the designation (RAL-51). It prints
    /// the statement, then signs it with `ssh-keygen -Y sign` and the key given, which asks for
    /// Touch ID when the key is Secretive's. See docs/designation.md.
    Designate {
        /// The watchman's name.
        #[arg(long, conflicts_with = "end", required_unless_present = "end")]
        watchman: Option<String>,
        /// End the designation: no watchman.
        #[arg(long)]
        end: bool,
        /// The operator's public key file, as `ssh-keygen -f` takes it.
        #[arg(long)]
        key: PathBuf,
    },
    /// Who the watchman is, each designation checked again against the operator's key (RAL-51).
    Designation,
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
        /// For example, a repository and full commit hash.
        #[arg(long)]
        evidence: String,
    },
    /// Confirm someone else's submitted story, naming the commit you read.
    Verify {
        story: String,
        #[arg(long)]
        note: String,
        /// The full hash of the commit you read.
        #[arg(long)]
        commit: String,
    },
    /// Return someone else's submitted story, saying what's missing.
    SendBack {
        story: String,
        #[arg(long)]
        note: String,
        /// The full hash of the commit you read, if you read one.
        #[arg(long)]
        commit: Option<String>,
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
    /// Close an open blocker, saying why.
    Unblock {
        /// The blocker's record number, as `block` or `blockers` gave it.
        blocker: u64,
        #[arg(long)]
        note: String,
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
    /// Declare your team's state: available, working or blocked.
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
    },
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
        .map_err(|_| format!("{text:?} isn't a state: available, working or blocked"))
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
    if let Command::SessionStart { team, verbose } = &cli.command {
        session_start(cli.config.clone(), team.as_deref(), *verbose);
        return ExitCode::SUCCESS;
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

/// `rally session-start`: the block, or nothing at all. Never fails.
fn session_start(config: Option<PathBuf>, team: Option<&str>, verbose: bool) {
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
            println!("{}", rally::session::render(team, &stories, &blockers))
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
        let mut page = Page::default();
        let mut merged = serde_json::Map::new();
        let mut first: Option<Value> = None;
        for _ in 0..max_pages {
            let value = self.read_value(path, None, &page, &local)?;
            if let Some(entries) = value[field].as_object() {
                merged.extend(entries.clone());
            }
            let next = value["next"].as_str().map(str::to_owned);
            first.get_or_insert(value);
            match next {
                Some(next) => page.after = Some(next),
                None => {
                    let mut value = first.unwrap_or_default();
                    value[field] = Value::Object(merged);
                    return Ok((value, false));
                }
            }
        }
        let mut value = first.unwrap_or_default();
        value[field] = Value::Object(merged);
        Ok((value, true))
    }
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

/// The most pages of each list the viewer reads, each of up to [`views::LIST_LIMIT`] entries.
const VIEWER_PAGES: usize = 20;

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
        Ok(json!({
            "as_of": stories["as_of"],
            "stories": stories["stories"],
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

fn run_viewer(
    bind: &str,
    projects: Option<PathBuf>,
    location: Option<Location>,
    config: PathBuf,
    backup: Option<BackupWatch>,
    local: LocalSummaries,
) -> Result<String, Box<dyn std::error::Error>> {
    let text = match projects {
        Some(path) => std::fs::read_to_string(path)?,
        None => DEFAULT_PROJECTS.to_owned(),
    };
    let projects: Vec<Project> = serde_json::from_str(&text)?;
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
    let listener = TcpListener::bind(address)?;
    let listening = listener.local_addr()?;
    println!("{}", json!({"listening": listening.to_string()}));
    std::io::Write::flush(&mut std::io::stdout())?;
    let reader = ledger_reader(location, config);
    tokio::runtime::Runtime::new()?
        .block_on(web::serve(listener, projects, reader, backup, local))?;
    Ok(json!({"stopped": true}).to_string())
}

fn run_service(
    location: &Location,
    bind: &str,
    state_dir: &Path,
    limit: RateLimit,
    operator_key: Option<rally::designation::OperatorKey>,
) -> Result<String, Box<dyn std::error::Error>> {
    let state = StateDir::open(state_dir)?;
    let (_, _, fingerprint) = state.certificate()?;
    let listener = TcpListener::bind(bind)?;
    let listening = json!({"listening": listener.local_addr()?.to_string(), "fingerprint": fingerprint,
            "operator_key": operator_key.as_ref().map(rally::designation::OperatorKey::fingerprint)});
    println!("{listening}");
    std::io::Write::flush(&mut std::io::stdout())?;
    tokio::runtime::Runtime::new()?.block_on(server::serve(
        listener,
        location,
        state,
        limit,
        operator_key,
    ))?;
    Ok(json!({"stopped": true}).to_string())
}

/// `rally init`: no ledger, service or credential involved.
fn init(
    files: &[PathBuf],
    dir: Option<&std::path::Path>,
) -> Result<String, Box<dyn std::error::Error>> {
    let start = match dir {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let hint = if dir.is_some() { "" } else { "; pass --dir" };
    let root = rally::init::repo_root(&start)
        .ok_or_else(|| format!("{} isn't in a git repository{hint}", start.display()))?;
    let done = rally::init::run(&root, files)?;
    let files: Vec<_> = done
        .iter()
        .map(|(path, outcome)| json!({"file": path.display().to_string(), "outcome": outcome.label()}))
        .collect();
    Ok(json!({"init": files}).to_string())
}

fn run(cli: Cli) -> Result<String, Box<dyn std::error::Error>> {
    if let Command::Init { files, dir } = &cli.command {
        return init(files, dir.as_deref());
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
        } => {
            let location = location.ok_or("serve needs --db and --head-file")?;
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
            return run_service(&location, &bind, &state_dir, limit, operator_key);
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
        Command::SessionStart { .. } => unreachable!("handled in main"),
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
        } => {
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
            return run_viewer(
                &bind,
                projects,
                location,
                config_path(cli.config)?,
                backup,
                local,
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
                Backend::File(store) => json!(store.verify(expect.as_ref())?),
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
        Command::Show {
            story,
            as_of,
            after,
            before,
        } => {
            let backend = Backend::choose(location.as_ref(), &config_path(cli.config)?, false)?;
            let path = match &story {
                Some(id) => format!("/v1/stories/{}", encode(id)),
                None => "/v1/stories".to_owned(),
            };
            let page = Page { after, before };
            return backend.read(&path, as_of, &page, |ledger, page| {
                views::stories(ledger, story.as_deref(), page)
            });
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
        Command::Unblock { blocker, note } => (
            Kind::Declared,
            json!({"change": "unblock", "blocker": blocker, "note": note}),
        ),
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
        } => (
            Kind::Declared,
            json!({"change": "submit", "story": story, "team": team, "evidence": evidence}),
        ),
        Command::Verify {
            story,
            note,
            commit,
        } => (
            Kind::Verified,
            json!({"change": "verify", "story": story, "note": note, "commit": commit}),
        ),
        Command::SendBack {
            story,
            note,
            commit,
        } => {
            let mut body = json!({"change": "send_back", "story": story, "note": note});
            if let Some(commit) = commit {
                body["commit"] = json!(commit);
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
        Command::Designation => {
            if let Some(location) = &location {
                let store = Store::open_read_only(location)?;
                return Ok(store.designation_report(None)?.to_string());
            }
            let pairing = read_pairing(&config_path(cli.config.clone())?)?;
            let reply = client::request(&pairing, "GET", "/v1/designation", None, &[])?;
            return Ok(reply_value(&reply)?.to_string());
        }
        Command::Designate { watchman, end, key } => {
            debug_assert!(end != watchman.is_some());
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
            let statement =
                rally::ledger::designation_statement(watchman.as_deref(), after_seq, after_hash);
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
            (
                Kind::Declared,
                json!({
                    "change": "designate", "watchman": watchman, "after_seq": after_seq,
                    "after_hash": after_hash, "key_fingerprint": fingerprint,
                    "signature": signature,
                }),
            )
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
