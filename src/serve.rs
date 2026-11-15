//! `rally serve`: the service's command, apart from every other command (RAL-196).
//!
//! `src/main.rs` holds each of the CLI's commands. While it held this one too, a change to it
//! couldn't be told from its path to leave the service alone, so the watchman's brief read
//! "can't tell" for every story that touched it (RAL-195). What the service runs at startup is
//! here: its flags, their checks, and the start of [`crate::server::serve`]. `main` only passes
//! the command on.

use std::net::TcpListener;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::access::StateDir;
use crate::server::{self, RateLimit};
use crate::store::Location;

/// The flags of `rally serve`.
#[derive(clap::Args)]
pub struct Args {
    /// The address to listen on, such as a LAN address and port. Never the open internet.
    #[arg(long)]
    pub bind: String,
    #[arg(long, env = "RALLY_STATE_DIR")]
    pub state_dir: PathBuf,
    /// Requests a credential can make at once.
    #[arg(long, default_value_t = RateLimit::default().burst)]
    pub burst: f64,
    /// Requests a credential regains per second.
    #[arg(long, default_value_t = RateLimit::default().per_second)]
    pub per_second: f64,
    /// Writes a credential can make at once.
    #[arg(long, default_value_t = RateLimit::default().write_burst)]
    pub write_burst: f64,
    /// Writes a credential regains per second.
    #[arg(long, default_value_t = RateLimit::default().write_per_second)]
    pub write_per_second: f64,
    /// Refuse writes once the ledger's table in Postgres, with its indexes, reaches this size.
    #[arg(long, default_value_t = RateLimit::default().max_ledger_bytes)]
    pub max_ledger_bytes: u64,
    /// Refuse writes while the disk under Postgres's data has less free space than this.
    #[arg(long, default_value_t = RateLimit::default().min_free_bytes)]
    pub min_free_bytes: u64,
    /// Connections open at once, from anyone.
    #[arg(long, default_value_t = RateLimit::default().max_connections)]
    pub max_connections: usize,
    /// Connections open at once from any one address.
    #[arg(long, default_value_t = RateLimit::default().max_connections_per_address)]
    pub max_connections_per_address: usize,
    /// How long a connection may last, authenticated or not.
    #[arg(long, default_value_t = RateLimit::default().connection_seconds)]
    pub connection_seconds: f64,
    /// Missing or wrong tokens one address can present at once.
    #[arg(long, default_value_t = RateLimit::default().failed_token_burst)]
    pub failed_token_burst: f64,
    /// Failed token attempts an address regains per second.
    #[arg(long, default_value_t = RateLimit::default().failed_token_per_second)]
    pub failed_token_per_second: f64,
    /// Checks and past-state reads a credential can make at once.
    #[arg(long, default_value_t = RateLimit::default().heavy_burst)]
    pub heavy_burst: f64,
    /// Checks and past-state reads a credential regains per second.
    #[arg(long, default_value_t = RateLimit::default().heavy_per_second)]
    pub heavy_per_second: f64,
    /// Refuse to start unless this binary and the operator key, and every directory above
    /// each, are owned by root and writable by no one else, so that no agent on this host can
    /// change either. It prints the operator's sudo steps if not. Needed to accept watchman
    /// designations (RAL-51). See docs/designation.md.
    #[arg(long, requires = "operator_key")]
    pub secure: bool,
    /// With --secure: the operator's OpenSSH public key, which designations must be signed by.
    #[arg(long, requires = "secure")]
    pub operator_key: Option<PathBuf>,
    /// How long a team waits for the operator's answer before it can shelve the story and
    /// take its next one (RAL-56): a number with s, m, h or d. The operator agreed 5 minutes.
    #[arg(long, env = "RALLY_OPERATOR_WINDOW", default_value = "5m", value_parser = parse_window)]
    pub operator_window: u64,
    /// How many shelves in a row, across the rally, with no answer from the operator between
    /// them, mean the operator is away (RAL-56). The operator agreed 3.
    #[arg(long, env = "RALLY_OPERATOR_AWAY_AFTER", default_value_t = 3,
          value_parser = clap::value_parser!(u64).range(1..))]
    pub operator_away_after: u64,
    /// How long the operator is treated as away (RAL-56): no story waiting on the operator is
    /// offered, and then each is asked again. The operator agreed 1 hour.
    #[arg(long, env = "RALLY_OPERATOR_AWAY", default_value = "1h", value_parser = parse_window)]
    pub operator_away: u64,
    /// A read-only GitHub token, for checking submitted evidence (RAL-5, RAL-18). Without it,
    /// each check is recorded as not checked. With --secure, it must be root-owned too.
    #[arg(long, env = "RALLY_GITHUB_TOKEN_FILE")]
    pub github_token_file: Option<PathBuf>,
    /// The GitHub owners, comma-separated, whose repositories the token may read open pull
    /// requests' roadmaps in for `new-story` (RAL-68). Requests can't widen it.
    #[arg(long, env = "RALLY_GITHUB_OWNERS", default_value = crate::evidence::DEFAULT_OWNERS)]
    pub github_owners: String,
    /// The credentials, comma-separated, that may open or close a rally (RAL-77), besides
    /// one a designated watchman is bound to. Unset, any paired credential may, and each
    /// answer says so. See docs/service.md.
    #[arg(long, env = "RALLY_RALLY_OPENERS")]
    pub rally_openers: Option<String>,
    /// The longest a submission's CI is waited for before its check is recorded as pending.
    #[arg(long, default_value = "30m", value_parser = parse_window)]
    pub evidence_ci_wait: u64,
    /// How often a submission whose CI is still running is checked again.
    #[arg(long, default_value = "1m", value_parser = parse_window)]
    pub evidence_poll: u64,
}

/// Runs the service until it stops. `location` is the ledger's, from `--db` and `--head-file`.
pub fn run(location: Option<Location>, args: Args) -> Result<String, Box<dyn std::error::Error>> {
    let Args {
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
        rally_openers,
        evidence_ci_wait,
        evidence_poll,
    } = args;
    let location = location.ok_or("serve needs --db and --head-file")?;
    let token = match &github_token_file {
        Some(path) => {
            if secure {
                crate::designation::check_owned(path, 0, None)
                    .map_err(|why| format!("--secure refused the GitHub token: {why}"))?;
            }
            Some(read_github_token(path)?)
        }
        None => None,
    };
    let evidence = crate::evidence::Settings {
        token,
        owners: github_owners
            .split(',')
            .map(|owner| owner.trim().to_owned())
            .filter(|owner| !owner.is_empty())
            .collect(),
        timing: crate::evidence::Timing {
            ceiling_ms: evidence_ci_wait,
            poll_ms: evidence_poll,
        },
        test_base: match crate::evidence::test_base(secure) {
            Ok(Some(base)) => {
                eprintln!(
                    "rally serve: evidence checks use a stand-in GitHub at {base}, for tests ({})",
                    crate::evidence::TEST_BASE_VAR
                );
                Some(base)
            }
            Ok(None) => None,
            Err(why) => {
                eprintln!("rally serve: {why}");
                None
            }
        },
    };
    let operator_key = if secure {
        let key_path = operator_key.ok_or("--secure needs --operator-key")?;
        let exe = std::env::current_exe()?;
        for path in [&exe, &key_path] {
            crate::designation::check_owned(path, 0, None).map_err(|why| {
                format!(
                    "--secure refused: {why}.\n{}",
                    crate::designation::secure_steps(&exe, &key_path)
                )
            })?;
        }
        Some(crate::designation::OperatorKey::read(&key_path)?)
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
            return Err(format!("the rate that goes with --{name} can't be negative").into());
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
    run_service(
        &location,
        &bind,
        &state_dir,
        limit,
        server::Rules {
            operator_key,
            operator_times: Some(crate::store::OperatorTimes {
                window_ms: operator_window,
                away_after: usize::try_from(operator_away_after).unwrap_or(usize::MAX),
                away_ms: operator_away,
            }),
            rally_openers: parse_rally_openers(rally_openers.as_deref())?,
        },
        evidence,
    )
}

fn run_service(
    location: &Location,
    bind: &str,
    state_dir: &Path,
    limit: RateLimit,
    rules: server::Rules,
    evidence: crate::evidence::Settings,
) -> Result<String, Box<dyn std::error::Error>> {
    let state = StateDir::open(state_dir)?;
    // A listed credential that isn't paired can't open anything; say so, rather than refuse to
    // start over a pairing revoked since.
    for opener in rules.rally_openers.iter().flatten() {
        if !matches!(state.paired(opener), Ok(Some(_))) {
            eprintln!("rally: --rally-openers names {opener}, which isn't paired");
        }
    }
    let evidence_checked = evidence.token.is_some();
    let (_, _, fingerprint) = state.certificate()?;
    let listener = TcpListener::bind(bind)?;
    let listening = json!({"listening": listener.local_addr()?.to_string(), "fingerprint": fingerprint,
    // The code that's running (RAL-156): what's live is a read, not a guess.
    "commit": crate::commit(),
    "operator_key": rules.operator_key.as_ref().map(crate::designation::OperatorKey::fingerprint),
    "operator_window_ms": rules.operator_times.map(|times| times.window_ms),
    "operator_away_after": rules.operator_times.map(|times| times.away_after),
    "operator_away_ms": rules.operator_times.map(|times| times.away_ms),
    "evidence_checked_with_token": evidence_checked,
    "rally_openers": rules.rally_openers.as_ref().map_or_else(
        || json!("any paired credential: set --rally-openers"),
        |openers| json!(openers),
    )});
    println!("{listening}");
    std::io::Write::flush(&mut std::io::stdout())?;
    tokio::runtime::Runtime::new()?.block_on(server::serve(
        listener,
        location,
        state,
        limit,
        rules,
        Some(evidence),
    ))?;
    Ok(json!({"stopped": true}).to_string())
}

/// `--rally-openers` (RAL-77): unset is `None`, anyone; otherwise at least one credential name.
fn parse_rally_openers(text: Option<&str>) -> Result<Option<Vec<String>>, String> {
    let Some(text) = text else {
        return Ok(None);
    };
    let openers: Vec<String> = text
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    if openers.is_empty() {
        return Err(
            "--rally-openers names no credential; leave it unset to let any \
                    paired credential open a rally"
                .into(),
        );
    }
    Ok(Some(openers))
}

/// The one GitHub token in the file at `path`.
pub fn read_github_token(path: &Path) -> Result<String, String> {
    let token =
        std::fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let token = token.trim().to_owned();
    if token.is_empty() || token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("{} doesn't hold one token", path.display()));
    }
    Ok(token)
}

/// A window such as `90m` or `4h`, in milliseconds (RAL-56).
pub fn parse_window(text: &str) -> Result<u64, String> {
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
