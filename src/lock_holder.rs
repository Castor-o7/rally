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

/// How a lock's lines in `/proc/locks` were matched to the file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Matched {
    /// No line names the file.
    #[default]
    Nothing,
    /// By device and inode.
    DeviceAndInode,
    /// By inode alone: no line had the file's device, as on btrfs subvolumes and overlayfs, where
    /// `stat` gives a different device from the superblock's that `/proc/locks` prints. Another
    /// file on another device with the same inode would match too.
    InodeOnly,
}

/// The pids holding, and waiting for, a lock on the file with this device and inode, from
/// `/proc/locks`. Its lines read `1: FLOCK  ADVISORY  WRITE 4242 00:2b:1234 0 EOF`, with `->`
/// after the number for a waiter; the device is `major:minor` in hex. Open file description
/// locks have no owning process (pid -1) and are skipped. If no line has the file's device and
/// inode, lines with its inode alone are taken, and the match says so.
pub fn parse_locks(
    text: &str,
    major: u32,
    minor: u32,
    inode: u64,
) -> (Vec<u32>, Vec<u32>, Matched) {
    // (pid, waiting, same device)
    let mut found: Vec<(u32, bool, bool)> = Vec::new();
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
        if ino.parse::<u64>().ok() != Some(inode) {
            continue;
        }
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        let same = u32::from_str_radix(maj, 16).ok() == Some(major)
            && u32::from_str_radix(min, 16).ok() == Some(minor);
        found.push((pid, waiting, same));
    }
    let matched = if found.iter().any(|f| f.2) {
        found.retain(|f| f.2);
        Matched::DeviceAndInode
    } else if found.is_empty() {
        Matched::Nothing
    } else {
        Matched::InodeOnly
    };
    let holders = found.iter().filter(|f| !f.1).map(|f| f.0).collect();
    let waiters = found.iter().filter(|f| f.1).map(|f| f.0).collect();
    (holders, waiters, matched)
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
    pub matched: Matched,
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
            let means = match proc.state {
                'T' => "stopped",
                't' => "traced (maybe a debugger)",
                'Z' if pid == top.pid => "zombie",
                _ => continue,
            };
            let cwd = seen.cwds.get(&pid);
            let session = cwd.and_then(|cwd| session_in(cwd));
            let nickname = session.as_ref().and_then(|id| roster.get(id));
            stuck.push(json!({
                "pid": pid, "holder": holder, "state": proc.state.to_string(),
                "state_means": means,
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
    let (holders, waiters, matched) = parse_locks(
        &locks,
        rustix::fs::major(dev),
        rustix::fs::minor(dev),
        meta.ino(),
    );
    let mut seen = Seen {
        matched,
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
                let mut row = json!({
                    "lock": shown, "checked": true, "holders": seen.holders,
                    "waiters": seen.waiters.len(), "kind": "observed",
                    "note": "Open file description locks (pid -1) are skipped.",
                });
                match seen.matched {
                    Matched::Nothing => {
                        row["holder"] = json!("no holder found in /proc/locks");
                    }
                    Matched::DeviceAndInode => row["matched_by"] = json!("device and inode"),
                    Matched::InodeOnly => {
                        row["matched_by"] = json!(
                            "inode only: no line had the file's device (btrfs subvolumes and \
                             overlayfs show another), so a file elsewhere with the same inode \
                             would match too"
                        );
                    }
                }
                seen_all.push(row);
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
            matched: Matched::DeviceAndInode,
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
        assert_eq!(
            parse_locks(text, 0, 0x2b, 777),
            (vec![100], vec![300, 301], Matched::DeviceAndInode)
        );
        assert_eq!(
            parse_locks(text, 0xfd, 1, 777),
            (vec![201], vec![], Matched::DeviceAndInode)
        );
        assert_eq!(
            parse_locks(text, 0, 0x2b, 999),
            (vec![], vec![], Matched::Nothing)
        );
    }

    #[test]
    fn with_no_line_on_the_files_device_the_inode_alone_is_matched_and_said() {
        // A btrfs subvolume: stat says 0:48, /proc/locks prints the superblock's 0:2b.
        let text = "1: FLOCK  ADVISORY  WRITE 100 00:2b:777 0 EOF\n\
                    1: -> FLOCK  ADVISORY  WRITE 300 00:2b:777 0 EOF\n\
                    2: OFDLCK ADVISORY  WRITE -1 00:2b:777 0 EOF\n";
        assert_eq!(
            parse_locks(text, 0, 0x30, 777),
            (vec![100], vec![300], Matched::InodeOnly)
        );
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
    fn a_traced_process_is_labelled_as_maybe_a_debugger() {
        let procs = [proc(100, 1, 'S', "flock"), proc(101, 100, 't', "godot")];
        let row = judge("l", &seen(&procs), &BTreeMap::new()).unwrap();
        assert_eq!(
            row["processes"][0]["state_means"],
            "traced (maybe a debugger)"
        );
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

    /// Resumes and ends the test's process group however the test ends, so a failed assert
    /// never leaves a stopped shell, or a `wait` that blocks forever.
    #[cfg(target_os = "linux")]
    struct Group(std::process::Child);

    #[cfg(target_os = "linux")]
    impl Drop for Group {
        fn drop(&mut self) {
            let group = format!("-{}", self.0.id());
            for signal in ["-CONT", "-KILL"] {
                let _ = std::process::Command::new("kill")
                    .args([signal, "--", &group])
                    .status();
            }
            let _ = self.0.wait();
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_real_stopped_holder_is_found_through_proc() {
        use std::os::unix::process::CommandExt;
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("heavy.lock");
        std::fs::write(&lock, "").unwrap();
        // Its own process group, so the guard can resume and end all of it.
        let Ok(child) = Command::new("flock")
            .arg(&lock)
            .args(["sh", "-c", "kill -STOP $$; true"])
            .process_group(0)
            .spawn()
        else {
            eprintln!("no flock(1) here; skipped");
            return;
        };
        let mut group = Group(child);
        let holder = group.0.id();
        let mut row = None;
        for _ in 0..200 {
            let (prompts, seen) = check(std::slice::from_ref(&lock), None);
            assert_eq!(seen[0]["checked"], true, "{seen:?}");
            if let Some(found) = prompts.into_iter().next() {
                row = Some(found);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let row = row.expect("the stopped holder wasn't flagged");
        assert_eq!(row["processes"][0]["state_means"], "stopped");
        assert_eq!(row["processes"][0]["holder"], holder);
        // Resumed, the shell finishes and flock lets go of the lock.
        let _ = Command::new("kill")
            .args(["-CONT", "--", &format!("-{holder}")])
            .status();
        assert!(group.0.wait().unwrap().success());
        let (prompts, seen) = check(std::slice::from_ref(&lock), None);
        assert!(
            prompts.is_empty(),
            "the lock is released and nothing is flagged"
        );
        assert_eq!(seen[0]["holder"], "no holder found in /proc/locks");
    }
}
