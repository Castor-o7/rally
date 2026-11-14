//! The jobs in flight, for the watchman's brief (RAL-191).
//!
//! From the DiscoSim rally's issues log, 1.73: on 2026-11-13 Josh closed every shell window by
//! accident, and every session's background jobs ended with them: a gate inside its quick suite,
//! the teams' probes, seed runs and renders. The sessions came back under the same nicknames, but
//! nothing said what had been running, so the restarted watchman had to rebuild it from the teams'
//! word. Its suggested fix: the watchman's handoff names every job in flight, so a restart can tell
//! what died.
//!
//! `rally brief --watchman` reads, for each project's `stalls.locks` in the projects list, every
//! lock the patterns name: who holds it (pid, state, command line and working directory, from
//! `/proc`, observed), with the owner its lock file names (declared, as dsm-heavy writes it), and
//! each fair pool's queue, holders and waiters by owner ([`crate::slot_queue`]). Read-only:
//! nothing is created, locked or signalled. What can't be read is a blind spot, named; an empty
//! list never stands for one it couldn't read.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::lock_holder::{Matched, Seen};

/// One lock as the brief shows it: held or not, and by what.
pub fn lock_row(lock: &str, seen: &Seen, owner: Option<&str>) -> Value {
    if seen.holders.is_empty() {
        return json!({"lock": lock, "held": false, "kind": "observed"});
    }
    let holders: Vec<Value> = seen
        .holders
        .iter()
        .map(|pid| {
            let proc = seen.procs.get(pid);
            let command = seen
                .cmdlines
                .get(pid)
                .cloned()
                .or_else(|| proc.map(|p| p.comm.clone()))
                .unwrap_or_else(|| "unknown: its process couldn't be read".into());
            json!({
                "pid": pid,
                "state": proc.map(|p| p.state.to_string()),
                "command": command.chars().take(300).collect::<String>(),
                "cwd": seen.cwds.get(pid),
            })
        })
        .collect();
    let mut row = json!({
        "lock": lock, "held": true, "holders": holders, "kind": "observed",
        "owner": owner, "owner_kind": "declared: the first word of the lock file",
    });
    if seen.by_descriptor {
        row["found_by"] = json!(crate::lock_holder::BY_DESCRIPTOR);
        row["kernel_holders"] = json!(seen.kernel_holders);
        if seen.unreadable > 0 {
            row["unreadable_processes"] = json!(seen.unreadable);
        }
    }
    if seen.matched == Matched::InodeOnly {
        row["matched_by"] = json!(
            "inode only: no line had the file's device, so a file elsewhere with the same inode \
             would match too"
        );
    }
    row
}

/// The first word of a held lock file, where its holder wrote one (dsm-heavy does).
fn owner_of(lock: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(lock).ok()?;
    let word = text.lines().next()?.split_whitespace().next()?;
    Some(word.chars().take(40).collect())
}

/// The brief's `in_flight` section, from each project's lock patterns (or why the projects list
/// couldn't be read): every lock read, each fair pool's queue, and what couldn't be seen.
pub fn section(projects: Result<Vec<(String, Vec<PathBuf>)>, String>) -> Value {
    let projects = match projects {
        Ok(projects) => projects,
        Err(why) => {
            return json!({
                "projects": [],
                "blind": [format!("the projects list: {why}, so no lock could be read")],
                "note": NOTE,
            });
        }
    };
    let mut blind = Vec::new();
    let mut out = Vec::new();
    let mut held = 0;
    let mut waiting = 0;
    for (prefix, patterns) in &projects {
        if patterns.is_empty() {
            continue;
        }
        let mut locks = Vec::new();
        for lock in crate::lock_holder::expand(patterns) {
            let shown = lock.display().to_string();
            match crate::lock_holder::read(&lock) {
                Ok(seen) => {
                    let owner = (!seen.holders.is_empty())
                        .then(|| owner_of(&lock))
                        .flatten();
                    held += usize::from(!seen.holders.is_empty());
                    locks.push(lock_row(&shown, &seen, owner.as_deref()));
                }
                Err(why) => blind.push(format!("the lock {shown} ({prefix}): {why}")),
            }
        }
        let (_, queues) =
            crate::slot_queue::check_now(patterns, None, crate::slot_queue::DEFAULT_WAIT_AFTER_MS);
        for queue in &queues {
            if queue["checked"] == false {
                blind.push(format!(
                    "the queue {} ({prefix}): {}",
                    queue["queue"].as_str().unwrap_or("?"),
                    queue["reason"].as_str().unwrap_or("can't see the queue")
                ));
            }
            for owner in queue["waiting"].as_array().into_iter().flatten() {
                waiting += owner["jobs"].as_u64().unwrap_or(0) as usize;
            }
        }
        // Only the held locks are listed one by one: a pool's free slots are counted.
        let free = locks.iter().filter(|l| l["held"] == false).count();
        let shown: Vec<Value> = locks.into_iter().filter(|l| l["held"] == true).collect();
        out.push(json!({"project": prefix, "held": shown, "free": free, "queues": queues}));
    }
    if out.is_empty() {
        blind.push(
            "no project's stalls in the projects list names a lock, so nothing here can say \
             what's running"
                .to_owned(),
        );
    }
    json!({
        "projects": out,
        "summary": format!(
            "{held} lock{} held, {waiting} job{} waiting for a slot",
            if held == 1 { "" } else { "s" },
            if waiting == 1 { "" } else { "s" }
        ),
        "blind": blind,
        "note": NOTE,
    })
}

const NOTE: &str = "The jobs a closed window would end (issues log 1.73), as /proc shows them now \
    (observed): each held lock's holder, and each fair pool's queue. Owners are what each job \
    wrote (declared). Only locks a project's stalls names are read; a job holding none isn't seen.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock_holder::Proc;
    use std::collections::BTreeMap;

    fn seen(holders: Vec<u32>) -> Seen {
        Seen {
            matched: Matched::DeviceAndInode,
            holders,
            procs: BTreeMap::from([(
                4242,
                Proc {
                    pid: 4242,
                    ppid: 1,
                    state: 'S',
                    comm: "timeout".into(),
                    cpu_ticks: 0,
                    start_ticks: 0,
                },
            )]),
            cwds: BTreeMap::from([(4242, "/home/u/.cache/teamA/tree".to_owned())]),
            cmdlines: BTreeMap::from([(4242, "timeout 2400 godot --headless".to_owned())]),
            ..Seen::default()
        }
    }

    #[test]
    fn a_held_lock_names_its_holders_pid_command_and_cwd_and_its_declared_owner() {
        let row = lock_row("/x/heavy.2.lock", &seen(vec![4242]), Some("teamA"));
        assert_eq!(row["held"], true);
        assert_eq!(
            row["holders"],
            json!([{"pid": 4242, "state": "S", "command": "timeout 2400 godot --headless",
                    "cwd": "/home/u/.cache/teamA/tree"}])
        );
        assert_eq!(row["owner"], "teamA");
        assert_eq!(row["kind"], "observed");
        assert!(row["owner_kind"].as_str().unwrap().starts_with("declared"));
    }

    #[test]
    fn a_holder_whose_process_couldnt_be_read_says_so() {
        let row = lock_row("/x/heavy.lock", &seen(vec![7]), None);
        assert_eq!(
            row["holders"][0]["command"],
            "unknown: its process couldn't be read"
        );
        assert!(row["owner"].is_null());
        assert_eq!(
            lock_row("/x/heavy.lock", &seen(vec![]), None),
            json!({"lock": "/x/heavy.lock", "held": false, "kind": "observed"})
        );
    }

    #[test]
    fn what_cant_be_read_is_blind_never_nothing_running() {
        let unread = section(Err("can't read /p/projects.json: denied".into()));
        assert_eq!(
            unread["blind"],
            json!([
                "the projects list: can't read /p/projects.json: denied, so no lock could be read"
            ])
        );
        let none = section(Ok(vec![("RAL".into(), vec![])]));
        assert!(
            none["blind"][0]
                .as_str()
                .unwrap()
                .starts_with("no project's stalls in the projects list names a lock"),
            "{none}"
        );
        // A pattern matching nothing is kept as given, and its read fails: blind, by name.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("none-*.lock");
        let gone = section(Ok(vec![("DSM".into(), vec![missing.clone()])]));
        let said = gone["blind"][0].as_str().unwrap();
        assert!(
            said.starts_with(&format!("the lock {} (DSM): ", missing.display())),
            "{gone}"
        );
    }
}
