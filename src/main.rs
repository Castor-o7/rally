//! The `rally` command. It reads and writes through the service once paired, or works directly on a
//! ledger file with `--db`, where nothing checks who is writing.
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
use rally::store::Store;
use rally::views;

/// A ledger of progress for a muster of agents.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Work directly on this ledger file rather than through the service.
    #[arg(long, env = "RALLY_DB")]
    db: Option<PathBuf>,
    /// The pairing file. Defaults to ~/.config/rally/client.json.
    #[arg(long, env = "RALLY_CONFIG")]
    config: Option<PathBuf>,
    /// The credential to record when writing directly to a file. The service sets its own.
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
    /// Run the service on this machine. Needs --db. Prints where it's listening.
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
        /// Refuse writes once the ledger file reaches this size.
        #[arg(long, default_value_t = RateLimit::default().max_ledger_bytes)]
        max_ledger_bytes: u64,
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

fn parse_status(text: &str) -> Result<TeamStatus, String> {
    serde_json::from_value(json!(text))
        .map_err(|_| format!("{text:?} isn't a state: available, working or blocked"))
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("rally: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Where reads and writes go.
enum Backend {
    File(Store),
    Service(Pairing),
}

impl Backend {
    fn choose(db: Option<&Path>, config: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(db) = db {
            return Ok(Self::File(Store::open(db)?));
        }
        let text = std::fs::read_to_string(config).map_err(|_| {
            format!(
                "no pairing at {}: run `rally pair`, or use --db for a local file",
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

fn config_path(config: Option<PathBuf>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    match config {
        Some(path) => Ok(path),
        None => {
            let home = std::env::var_os("HOME").ok_or("HOME isn't set; use --config")?;
            Ok(PathBuf::from(home).join(".config/rally/client.json"))
        }
    }
}

fn run_service(
    db: &Path,
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
    tokio::runtime::Runtime::new()?.block_on(server::serve(listener, db, state, limit))?;
    Ok(json!({"stopped": true}).to_string())
}

fn run(cli: Cli) -> Result<String, Box<dyn std::error::Error>> {
    let (kind, body) = match cli.command {
        Command::Serve {
            bind,
            state_dir,
            burst,
            per_second,
            write_burst,
            write_per_second,
            max_ledger_bytes,
        } => {
            let db = cli.db.ok_or("serve needs --db or RALLY_DB")?;
            let limit = RateLimit {
                burst,
                per_second,
                write_burst,
                write_per_second,
                max_ledger_bytes,
            };
            return run_service(&db, &bind, &state_dir, limit);
        }
        Command::AddMachine {
            name,
            address,
            state_dir,
            out,
            read_only,
        } => {
            if out.exists() {
                return Err(format!("{} already exists", out.display()).into());
            }
            let pairing = StateDir::open(&state_dir)?.add_machine(&name, &address, read_only)?;
            write_private(&out, pairing.code().as_bytes())?;
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
        Command::Show { story, as_of } => {
            let backend = Backend::choose(cli.db.as_deref(), &config_path(cli.config)?)?;
            let path = match &story {
                Some(id) => format!("/v1/stories/{}", encode(id)),
                None => "/v1/stories".to_owned(),
            };
            return backend.read(&path, as_of, |ledger| {
                views::stories(ledger, story.as_deref())
            });
        }
        Command::Teams { team, as_of } => {
            let backend = Backend::choose(cli.db.as_deref(), &config_path(cli.config)?)?;
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
    let backend = Backend::choose(cli.db.as_deref(), &config_path(cli.config)?)?;
    match backend {
        Backend::File(mut store) => {
            let credential = cli
                .credential
                .ok_or("--credential or RALLY_CREDENTIAL is needed to write to a file")?;
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
