//! A heavy-run lock whose holder has stopped (RAL-75), for `rally stalls --lock <path>`.
//!
//! From the DiscoSim rally's issues log, §4.4: on 2026-11-06 a runner shell sat stopped (state T) for
//! about 57 minutes while it held its place in the one-heavy-run order, so no sweep ran and the
//! queue behind it waited. With `flock <lock> <command>`, the holder is a process that can be
//! inspected: this reads `/proc/locks` for the lock file's holder and waiters, walks the holder's
//! process tree, and gives a prompt if anything in it is stopped, or the holder is a zombie.
//!
//! Linux only. What's read is the kernel's account (observed). Which session a process belongs to
//! is a guess from a session ID in its working directory, such as a scratchpad path, and is
//! labelled so.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Value, json};

/// One process, as `/proc/<pid>/stat` has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// The kernel's state letter: `R`, `S`, `D`, `T`, `t`, `Z` and so on.
    pub state: char,
    pub comm: String,
}

/// Parses one `/proc/<pid>/stat`. The command name is in parentheses and may itself hold spaces
/// and parentheses, so the fields after it are found from the last `)`.
pub fn parse_stat(text: &str) -> Option<Proc> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let pid = text[..open].trim().parse().ok()?;
    let comm = text.get(open + 1..close)?.to_owned();
    let mut rest = text.get(close + 1..)?.split_whitespace();
    let state = rest.next()?.chars().next()?;
    let ppid = rest.next()?.parse().ok()?;
    Some(Proc {
        pid,
        ppid,
        state,
        comm,
    })
}

/// The pids holding, and waiting for, a lock on the file with this device and inode, from
/// `/proc/locks`. Its lines read `1: FLOCK  ADVISORY  WRITE 4242 00:2b:1234 0 EOF`, with `->`
/// after the number for a waiter; the device is `major:minor` in hex.
pub fn parse_locks(text: &str, major: u32, minor: u32, inode: u64) -> (Vec<u32>, Vec<u32>) {
    let mut holders = Vec::new();
    let mut waiters = Vec::new();
    for line in text.lines() {
        let mut fields: Vec<&str> = line.split_whitespace().collect();
        if fields.is_empty() {
            continue;
        }
        fields.remove(0);
        let waiting = fields.first() == Some(&"->");
        if waiting {
            fields.remove(0);
        }
        // kind, advisory, mode, pid, device:inode, ...
        let (Some(pid), Some(file)) = (fields.get(3), fields.get(4)) else {
            continue;
        };
        let parts: Vec<&str> = file.split(':').collect();
        let [maj, min, ino] = parts[..] else {
            continue;
        };
        let same = u32::from_str_radix(maj, 16).ok() == Some(major)
            && u32::from_str_radix(min, 16).ok() == Some(minor)
            && ino.parse::<u64>().ok() == Some(inode);
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        if same {
            if waiting {
                waiters.push(pid);
            } else {
                holders.push(pid);
            }
        }
    }
    (holders, waiters)
}

/// The first path component shaped like a session ID (a UUID), such as a scratchpad's.
pub fn session_in(cwd: &str) -> Option<String> {
    cwd.split('/')
        .find(|part| {
            part.len() == 36
                && part.char_indices().all(|(i, c)| {
                    if matches!(i, 8 | 13 | 18 | 23) {
                        c == '-'
                    } else {
                        c.is_ascii_hexdigit()
                    }
                })
        })
        .map(str::to_lowercase)
}

/// The holder and everything under it, holder first.
fn tree(root: u32, procs: &BTreeMap<u32, Proc>) -> Vec<u32> {
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for proc in procs.values() {
        children.entry(proc.ppid).or_default().push(proc.pid);
    }
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut todo = vec![root];
    while let Some(pid) = todo.pop() {
        if !seen.insert(pid) {
            continue;
        }
        out.push(pid);
        if let Some(kids) = children.get(&pid) {
            todo.extend(kids.iter().rev());
        }
    }
    out
}

/// What was read for one lock.
#[derive(Clone, Debug, Default)]
pub struct Seen {
    pub holders: Vec<u32>,
    pub waiters: Vec<u32>,
    pub procs: BTreeMap<u32, Proc>,
    /// Each process's working directory, where it could be read.
    pub cwds: BTreeMap<u32, String>,
}

/// The `stopped_lock_holder` prompt for one lock, if anything holding it is stopped or the holder
/// is a zombie. `roster` maps a session ID to its nickname.
pub fn judge(lock: &str, seen: &Seen, roster: &BTreeMap<String, String>) -> Option<Value> {
    let mut stuck = Vec::new();
    for &holder in &seen.holders {
        let Some(top) = seen.procs.get(&holder) else {
            continue;
        };
        for pid in tree(holder, &seen.procs) {
            let proc = &seen.procs[&pid];
            let stopped = matches!(proc.state, 'T' | 't');
            let dead = pid == top.pid && proc.state == 'Z';
            if !(stopped || dead) {
                continue;
            }
            let cwd = seen.cwds.get(&pid);
            let session = cwd.and_then(|cwd| session_in(cwd));
            let nickname = session.as_ref().and_then(|id| roster.get(id));
            stuck.push(json!({
                "pid": pid, "holder": holder, "state": proc.state.to_string(),
                "state_means": if stopped { "stopped" } else { "zombie" },
                "comm": proc.comm, "cwd": cwd,
                "session": session, "nickname": nickname,
                "session_kind": "guessed from the working directory",
            }));
        }
    }
    if stuck.is_empty() {
        return None;
    }
    Some(json!({
        "row": "stopped_lock_holder",
        "lock": lock,
        "likely_meaning": "The lock is held by a run that isn't running, so the queue behind it waits",
        "action": "Ask the holder's session, or the operator, to resume it (`kill -CONT <pid>`) or \
                   end it. Ask it not to start the next run, rather than to hold.",
        "processes": stuck,
        "waiters": seen.waiters.len(),
        "kind": "observed",
        "note": "A prompt for the watchman, not a conclusion.",
    }))
}

/// Reads one lock's holders, waiters and the processes on this machine, from `/proc`.
#[cfg(target_os = "linux")]
pub fn read(lock: &Path) -> Result<Seen, String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(lock).map_err(|e| format!("can't read the lock file: {e}"))?;
    let dev = meta.dev();
    let locks = std::fs::read_to_string("/proc/locks")
        .map_err(|e| format!("can't read /proc/locks: {e}"))?;
    let (holders, waiters) = parse_locks(
        &locks,
        rustix::fs::major(dev),
        rustix::fs::minor(dev),
        meta.ino(),
    );
    let mut seen = Seen {
        holders,
        waiters,
        ..Seen::default()
    };
    if seen.holders.is_empty() {
        return Ok(seen);
    }
    let entries = std::fs::read_dir("/proc").map_err(|e| format!("can't list /proc: {e}"))?;
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        if let Some(proc) = parse_stat(&text) {
            seen.procs.insert(pid, proc);
        }
    }
    let wanted: Vec<u32> = seen
        .holders
        .iter()
        .flat_map(|&holder| tree(holder, &seen.procs))
        .collect();
    for pid in wanted {
        if let Ok(cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) {
            seen.cwds.insert(pid, cwd.to_string_lossy().into_owned());
        }
    }
    Ok(seen)
}

#[cfg(not(target_os = "linux"))]
pub fn read(_lock: &Path) -> Result<Seen, String> {
    Err("a lock's holder can only be read on Linux".into())
}

/// The roster's session IDs and nicknames.
pub fn nicknames(roster: Option<&Value>) -> BTreeMap<String, String> {
    roster
        .and_then(|roster| roster["sessions"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some((
                s["session_id"].as_str()?.to_lowercase(),
                s["nickname"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// Every `--lock`, read and judged: the prompts, and what was seen of each lock.
pub fn check(locks: &[std::path::PathBuf], roster: Option<&Value>) -> (Vec<Value>, Vec<Value>) {
    let names = nicknames(roster);
    let mut prompts = Vec::new();
    let mut seen_all = Vec::new();
    for lock in locks {
        let shown = lock.display().to_string();
        match read(lock) {
            Ok(seen) => {
                seen_all.push(json!({
                    "lock": shown, "checked": true, "holders": seen.holders,
                    "waiters": seen.waiters.len(), "kind": "observed",
                }));
                prompts.extend(judge(&shown, &seen, &names));
            }
            Err(reason) => {
                seen_all.push(json!({"lock": shown, "checked": false, "reason": reason}));
            }
        }
    }
    (prompts, seen_all)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "53081362-0b15-46d0-b883-78727342c30c";

    fn proc(pid: u32, ppid: u32, state: char, comm: &str) -> Proc {
        Proc {
            pid,
            ppid,
            state,
            comm: comm.into(),
        }
    }

    fn seen(procs: &[Proc]) -> Seen {
        Seen {
            holders: vec![100],
            waiters: vec![300, 301],
            procs: procs.iter().map(|p| (p.pid, p.clone())).collect(),
            cwds: BTreeMap::from([(
                101,
                format!("/tmp/claude-1000/-home-joshd-src/{SESSION}/scratchpad"),
            )]),
        }
    }

    #[test]
    fn stat_lines_are_read_even_with_parentheses_in_the_name() {
        let p = parse_stat("4242 (godot (x) y) T 4200 4242 4200 0 -1").unwrap();
        assert_eq!(p, proc(4242, 4200, 'T', "godot (x) y"));
        assert!(parse_stat("garbage").is_none());
    }

    #[test]
    fn locks_are_matched_by_device_and_inode_with_waiters_apart() {
        let text = "1: FLOCK  ADVISORY  WRITE 100 00:2b:777 0 EOF\n\
                    1: -> FLOCK  ADVISORY  WRITE 300 00:2b:777 0 EOF\n\
                    1: -> FLOCK  ADVISORY  WRITE 301 00:2b:777 0 EOF\n\
                    2: POSIX  ADVISORY  WRITE 200 00:2b:778 0 EOF\n\
                    3: FLOCK  ADVISORY  WRITE 201 fd:01:777 0 EOF\n";
        assert_eq!(parse_locks(text, 0, 0x2b, 777), (vec![100], vec![300, 301]));
        assert_eq!(parse_locks(text, 0xfd, 1, 777), (vec![201], vec![]));
    }

    #[test]
    fn a_stopped_shell_under_the_holder_is_flagged_with_its_session() {
        let procs = [
            proc(100, 1, 'S', "flock"),
            proc(101, 100, 'T', "bash"),
            proc(102, 101, 'T', "godot"),
            proc(103, 1, 'T', "unrelated"),
        ];
        let roster = BTreeMap::from([(SESSION.to_owned(), "silken-birch".to_owned())]);
        let row = judge("/tmp/discosim-heavy.lock", &seen(&procs), &roster).unwrap();
        assert_eq!(row["row"], "stopped_lock_holder");
        assert_eq!(row["waiters"], 2);
        let pids: Vec<u64> = row["processes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["pid"].as_u64().unwrap())
            .collect();
        assert_eq!(pids, [101, 102]);
        assert_eq!(row["processes"][0]["nickname"], "silken-birch");
        assert_eq!(row["processes"][0]["session"], SESSION);
        assert_eq!(row["processes"][0]["state_means"], "stopped");
    }

    #[test]
    fn a_zombie_holder_is_flagged() {
        let row = judge("l", &seen(&[proc(100, 1, 'Z', "flock")]), &BTreeMap::new()).unwrap();
        assert_eq!(row["processes"][0]["state_means"], "zombie");
    }

    #[test]
    fn a_running_or_sleeping_holder_is_not_flagged() {
        let procs = [
            proc(100, 1, 'S', "flock"),
            proc(101, 100, 'R', "godot"),
            proc(102, 101, 'Z', "reaped soon"),
        ];
        assert!(judge("l", &seen(&procs), &BTreeMap::new()).is_none());
        let mut gone = seen(&procs);
        gone.procs.clear();
        assert!(judge("l", &gone, &BTreeMap::new()).is_none());
    }

    #[test]
    fn a_session_id_is_found_in_a_path_and_nothing_else_is() {
        assert_eq!(
            session_in(&format!("/x/{}/scratchpad", SESSION.to_uppercase())).as_deref(),
            Some(SESSION)
        );
        assert_eq!(session_in("/home/joshd/src/rally"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_real_stopped_holder_is_found_through_proc() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("heavy.lock");
        std::fs::write(&lock, "").unwrap();
        let Ok(mut child) = Command::new("flock")
            .arg(&lock)
            .args(["sh", "-c", "kill -STOP $$; true"])
            .spawn()
        else {
            eprintln!("no flock(1) here; skipped");
            return;
        };
        let mut row = None;
        for _ in 0..100 {
            let (prompts, seen) = check(std::slice::from_ref(&lock), None);
            assert_eq!(seen[0]["checked"], true, "{seen:?}");
            if let Some(found) = prompts.into_iter().next() {
                row = Some(found);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let pids: Vec<u32> = row
            .as_ref()
            .and_then(|row| row["processes"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|p| p["pid"].as_u64().map(|p| p as u32))
            .collect();
        for pid in &pids {
            let _ = Command::new("kill")
                .args(["-CONT", &pid.to_string()])
                .status();
        }
        let _ = child.wait();
        let row = row.expect("the stopped holder wasn't flagged");
        assert_eq!(row["processes"][0]["state_means"], "stopped");
        assert_eq!(row["processes"][0]["holder"], child.id());
        let (prompts, _) = check(std::slice::from_ref(&lock), None);
        assert!(
            prompts.is_empty(),
            "the lock is released and nothing is flagged"
        );
    }
}
