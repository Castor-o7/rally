//! Rally's way to Muster or Marshal (RAL-120): one ordered list of shim commands, read from
//! `RALLY_ROSTER_FROM` (comma-separated), by default `marshal-shim` then `muster-shim`. The viewer
//! reads the roster with it (RAL-116), and so do `rally stalls`, `rally wake` and `rally brief`;
//! `rally wake` and `rally digest` send through it.
//!
//! During the move from Muster's daemon to Marshal's, each session lives on exactly one daemon. So a
//! roster is read from the first command that gives one, and a message is sent through the first
//! command that knows its recipient. A send falls back to the next command only when the shim says
//! nothing was sent because the recipient isn't on its roster or its daemon couldn't be reached, or
//! when the command can't be run at all. Any other failure stops there and is reported, so a shim
//! that delivered and then failed can't lead to a second copy through the other daemon.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// The environment variable holding the list, comma-separated.
pub const ENV: &str = "RALLY_ROSTER_FROM";

/// The list by default: Marshal's, then Muster's, which is frozen but still where the sessions
/// are until they move to Marshal's daemon.
pub const DEFAULT: [&str; 2] = ["marshal-shim", "muster-shim"];

/// The list from `RALLY_ROSTER_FROM`, or [`DEFAULT`] when it's unset or empty.
pub fn from_env() -> Vec<String> {
    parse(std::env::var(ENV).ok().as_deref())
}

/// The list from a `RALLY_ROSTER_FROM` value.
pub fn parse(value: Option<&str>) -> Vec<String> {
    let listed: Vec<String> = value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(str::to_owned)
        .collect();
    if listed.is_empty() {
        DEFAULT.map(String::from).to_vec()
    } else {
        listed
    }
}

/// Whether `command` can be run here: a path that's a file, or a name found on `PATH`.
pub fn on_path(command: &str) -> bool {
    if command.contains('/') {
        return Path::new(command).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(command).is_file()))
}

/// Whether any command in `list` can be run here.
pub fn available(list: &[String]) -> bool {
    list.iter().any(|command| on_path(command))
}

/// A roster, the command it was read with, and why any before it weren't.
#[derive(Clone, Debug)]
pub struct Roster {
    pub value: Value,
    pub source: String,
    pub passed_over: Vec<(String, String)>,
}

/// The roster from the first of `list` whose `<command> roster` succeeds with a roster on it, or
/// why each one failed.
pub fn roster(list: &[String]) -> Result<Roster, Vec<(String, String)>> {
    let mut passed_over = Vec::new();
    for command in list {
        let reason = match Command::new(command).arg("roster").output() {
            Err(error) => format!("couldn't be run ({error})"),
            Ok(out) if !out.status.success() => format!(
                "reported a failure: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Ok(out) => match serde_json::from_slice::<Value>(&out.stdout) {
                Ok(value) if value["sessions"].is_array() => {
                    return Ok(Roster {
                        value,
                        source: command.clone(),
                        passed_over,
                    });
                }
                Ok(_) => "printed no roster".to_owned(),
                Err(_) => "printed something that isn't JSON".to_owned(),
            },
        };
        passed_over.push((command.clone(), reason));
    }
    Err(passed_over)
}

/// The roster as `Option`, for the commands where it's optional, and a note naming the command it
/// was read with, or why none gave one.
pub fn roster_or_note(list: &[String]) -> (Option<Value>, Value) {
    match roster(list) {
        Ok(found) => {
            let note = serde_json::json!({"read_with": found.source,
                "passed_over": passed(&found.passed_over)});
            (Some(found.value), note)
        }
        Err(failed) => (
            None,
            serde_json::json!({"read_with": null, "passed_over": passed(&failed)}),
        ),
    }
}

fn passed(list: &[(String, String)]) -> Value {
    list.iter()
        .map(|(command, reason)| serde_json::json!({"command": command, "reason": reason}))
        .collect()
}

/// Whether a failed `tell`'s message says nothing was sent because the recipient isn't on that
/// daemon's roster or the daemon couldn't be reached: both shims say so in these words.
pub fn try_the_next(stderr: &str) -> bool {
    stderr.contains("nothing was sent") || stderr.contains("no team or session named")
}

/// Sends `message` to `team` through the first of `list` that knows it: `<command> tell <team>
/// <message>`. Returns the command that sent it. Falls back only as the module says.
pub fn tell(list: &[String], team: &str, message: &str) -> Result<String, String> {
    let mut tried = Vec::new();
    for command in list {
        match Command::new(command).args(["tell", team, message]).output() {
            Err(error) => tried.push(format!("{command} couldn't be run ({error})")),
            Ok(out) if out.status.success() => return Ok(command.clone()),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
                if try_the_next(&stderr) {
                    tried.push(format!("{command}: {stderr}"));
                } else {
                    // It may have delivered: never send a second copy through another daemon.
                    tried.push(format!("{command} tell failed: {stderr}"));
                    return Err(tried.join("; "));
                }
            }
        }
    }
    Err(if tried.is_empty() {
        "no shim command is set".to_owned()
    } else {
        tried.join("; ")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_marshal_then_muster_unless_set() {
        assert_eq!(parse(None), ["marshal-shim", "muster-shim"]);
        assert_eq!(parse(Some(" , ")), ["marshal-shim", "muster-shim"]);
        assert_eq!(parse(Some("muster-shim")), ["muster-shim"]);
        assert_eq!(parse(Some("a, b")), ["a", "b"]);
    }

    #[test]
    fn only_nothing_sent_falls_through() {
        assert!(try_the_next(
            "Error: no team or session named `x` on the roster"
        ));
        assert!(try_the_next(
            "Error: couldn't register with the daemon within 10s; nothing was sent"
        ));
        assert!(try_the_next(
            "Error: no active sessions on the roster; nothing was sent"
        ));
        assert!(!try_the_next("Error: 1 of 2 messages weren't sent"));
        assert!(!try_the_next(""));
    }
}
