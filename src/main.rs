//! The `rally` command. For now it works on a local ledger file; the service comes with RAL-13.
//!
//! All output is JSON, so that control characters in recorded text are escaped rather than reaching
//! the terminal.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

use rally::ledger::State;
use rally::record::{Kind, Record, Source};
use rally::store::Store;

/// A ledger of progress for a muster of agents.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// The ledger file.
    #[arg(long, env = "RALLY_DB")]
    db: PathBuf,
    /// The machine credential to record. Not authenticated until RAL-13.
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
        /// Record this as a verifier's correction, needed to move a story into or out of Verified
        /// or Sent back.
        #[arg(long)]
        as_verifier: bool,
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

fn run(cli: Cli) -> Result<String, Box<dyn std::error::Error>> {
    let mut store = Store::open(&cli.db)?;
    let (kind, body) = match cli.command {
        Command::Show { story, as_of } => {
            let ledger = store.ledger(as_of)?;
            let output = match story {
                Some(id) => {
                    let story = ledger
                        .story(&id)
                        .ok_or_else(|| format!("{id} isn't in the ledger"))?;
                    json!({"as_of": ledger.last_seq(), "story": id, "detail": story})
                }
                None => {
                    let summary: serde_json::Map<String, Value> = ledger
                        .stories()
                        .iter()
                        .map(|(id, story)| {
                            let last = story.history.last().map(|entry| entry.seq);
                            let row =
                                json!({"state": story.state, "team": story.team, "last": last});
                            (id.clone(), row)
                        })
                        .collect();
                    json!({"as_of": ledger.last_seq(), "stories": summary})
                }
            };
            return Ok(serde_json::to_string_pretty(&output)?);
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
            as_verifier,
        } => {
            let mut body = json!({
                "change": "correct", "story": story, "corrects": corrects, "note": note
            });
            if let Some(state) = state {
                body["state"] = json!(state);
            }
            let kind = if as_verifier {
                Kind::Verified
            } else {
                Kind::Declared
            };
            (kind, body)
        }
    };

    let credential = cli
        .credential
        .ok_or("--credential or RALLY_CREDENTIAL is needed to write")?;
    let name = cli.name.ok_or("--as or RALLY_NAME is needed to write")?;
    let source = Source {
        credential,
        declared_name: Some(name),
    };
    let record = Record::new(kind, source, now_ms()?, body)?;
    let seq = store.append(&record)?;
    Ok(json!({"recorded": seq}).to_string())
}

fn now_ms() -> Result<i64, Box<dyn std::error::Error>> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
    )?)
}
