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
use rally::views;
use rally::web::{self, DEFAULT_PROJECTS, LedgerReader, Project};

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
    },
    /// Run the read-only viewer on this machine's loopback address. See docs/web.md.
    Web {
        #[arg(long, default_value = "127.0.0.1:7300")]
        bind: String,
        /// The projects to show, instead of the defaults.
        #[arg(long)]
        projects: Option<PathBuf>,
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
    /// Confirm someone else's submitted story.
    Verify {
        story: String,
        #[arg(long)]
        note: String,
    },
    /// Return someone else's submitted story, saying what's missing.
    SendBack {
        story: String,
        #[arg(long)]
        note: String,
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
    },
    /// Check the ledger for tampering. Fails if there's a problem. Save the head it prints somewhere
    /// else, and pass it back later with --expect.
    Check {
        /// A head saved earlier, as SEQ:HASH.
        #[arg(long, value_parser = parse_head)]
        expect: Option<Head>,
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
    },
    /// Show the state of every story, or one story with its history.
    Show {
        story: Option<String>,
        /// Show the state as it stood after this record.
        #[arg(long)]
        as_of: Option<u64>,
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
    match run(Cli::parse()) {
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

    fn read(
        &self,
        path: &str,
        as_of: Option<u64>,
        local: impl FnOnce(&rally::ledger::Ledger) -> Result<Value, String>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let value = match self {
            Self::File(store) => local(&store.ledger(as_of)?)?,
            Self::Service(pairing) => {
                let path = match as_of {
                    Some(seq) => format!("{path}?as_of={seq}"),
                    None => path.to_owned(),
                };
                let reply = client::request(pairing, "GET", &path, None, &[])?;
                reply_value(&reply)?
            }
        };
        Ok(serde_json::to_string_pretty(&value)?)
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
        let stories: Value =
            serde_json::from_str(
                &backend.read("/v1/stories", None, |ledger| views::stories(ledger, None))?,
            )?;
        let teams: Value = serde_json::from_str(
            &backend.read("/v1/teams", None, |ledger| views::teams(ledger, None))?,
        )?;
        Ok(json!({
            "as_of": stories["as_of"],
            "stories": stories["stories"],
            "teams": teams["teams"],
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
) -> Result<String, Box<dyn std::error::Error>> {
    let text = match projects {
        Some(path) => std::fs::read_to_string(path)?,
        None => DEFAULT_PROJECTS.to_owned(),
    };
    let projects: Vec<Project> = serde_json::from_str(&text)?;
    let address: std::net::SocketAddr = bind.parse()?;
    if !address.ip().is_loopback() {
        return Err(format!("the viewer only runs on a loopback address, not {address}").into());
    }
    let listener = TcpListener::bind(address)?;
    let listening = listener.local_addr()?;
    println!("{}", json!({"listening": listening.to_string()}));
    std::io::Write::flush(&mut std::io::stdout())?;
    let reader = ledger_reader(location, config);
    tokio::runtime::Runtime::new()?.block_on(web::serve(listener, projects, reader))?;
    Ok(json!({"stopped": true}).to_string())
}

fn run_service(
    location: &Location,
    bind: &str,
    state_dir: &Path,
    limit: RateLimit,
) -> Result<String, Box<dyn std::error::Error>> {
    let state = StateDir::open(state_dir)?;
    let (_, _, fingerprint) = state.certificate()?;
    let listener = TcpListener::bind(bind)?;
    let listening =
        json!({"listening": listener.local_addr()?.to_string(), "fingerprint": fingerprint});
    println!("{listening}");
    std::io::Write::flush(&mut std::io::stdout())?;
    tokio::runtime::Runtime::new()?.block_on(server::serve(listener, location, state, limit))?;
    Ok(json!({"stopped": true}).to_string())
}

fn run(cli: Cli) -> Result<String, Box<dyn std::error::Error>> {
    let location = location(cli.db, cli.head_file, cli.db_password_file)?;
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
        } => {
            let location = location.ok_or("serve needs --db and --head-file")?;
            let limit = RateLimit {
                burst,
                per_second,
                write_burst,
                write_per_second,
                max_ledger_bytes,
                min_free_bytes,
            };
            return run_service(&location, &bind, &state_dir, limit);
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
        Command::Web { bind, projects } => {
            return run_viewer(&bind, projects, location, config_path(cli.config)?);
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
        Command::Show { story, as_of } => {
            let backend = Backend::choose(location.as_ref(), &config_path(cli.config)?, false)?;
            let path = match &story {
                Some(id) => format!("/v1/stories/{}", encode(id)),
                None => "/v1/stories".to_owned(),
            };
            return backend.read(&path, as_of, |ledger| {
                views::stories(ledger, story.as_deref())
            });
        }
        Command::Teams { team, as_of } => {
            let backend = Backend::choose(location.as_ref(), &config_path(cli.config)?, false)?;
            let path = match &team {
                Some(name) => format!("/v1/teams/{}", encode(name)),
                None => "/v1/teams".to_owned(),
            };
            return backend.read(&path, as_of, |ledger| views::teams(ledger, team.as_deref()));
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
        } => (
            Kind::Declared,
            json!({"change": "submit", "story": story, "team": team, "evidence": evidence}),
        ),
        Command::Verify { story, note } => (
            Kind::Verified,
            json!({"change": "verify", "story": story, "note": note}),
        ),
        Command::SendBack { story, note } => (
            Kind::Verified,
            json!({"change": "send_back", "story": story, "note": note}),
        ),
        Command::Correct {
            story,
            corrects,
            note,
            state,
            team,
            as_verifier,
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
            let kind = if as_verifier {
                Kind::Verified
            } else {
                Kind::Declared
            };
            (kind, body)
        }
    };

    let name = cli.name.ok_or("--as or RALLY_NAME is needed to write")?;
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

fn now_ms() -> Result<i64, Box<dyn std::error::Error>> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
    )?)
}
