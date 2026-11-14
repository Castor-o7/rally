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
    /// CPU time used so far, user and system, in clock ticks (RAL-163).
    pub cpu_ticks: u64,
    /// When it started, in clock ticks after boot (RAL-163).
    pub start_ticks: u64,
}

/// Parses one `/proc/<pid>/stat`. The command name is in parentheses and may itself hold spaces
/// and parentheses, so the fields after it are found from the last `)`.
pub fn parse_stat(text: &str) -> Option<Proc> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let pid = text[..open].trim().parse().ok()?;
    let comm = text.get(open + 1..close)?.to_owned();
    let rest: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    let state = rest.first()?.chars().next()?;
    let ppid = rest.get(1)?.parse().ok()?;
    // Fields 14 and 15 (utime, stime) and 22 (starttime), counted from the pid as 1.
    let field = |n: usize| rest.get(n - 3).and_then(|v| v.parse::<u64>().ok());
    Some(Proc {
        pid,
        ppid,
        state,
        comm,
        cpu_ticks: field(14).unwrap_or(0) + field(15).unwrap_or(0),
        start_ticks: field(22).unwrap_or(0),
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
    /// Each process's command line, where it could be read (RAL-163).
    pub cmdlines: BTreeMap<u32, String>,
    /// How long the machine had been up when this was read, in milliseconds (RAL-163).
    pub uptime_ms: u64,
}

/// Below this share of one CPU over the sample, a lock holder's whole process tree is possibly
/// hung (RAL-163). Tonight's was a godot at 0.9% for 11 minutes.
pub const HUNG_CPU_PERCENT: f64 = 2.0;

/// The team a working directory names, guessed: a path component that's a roster nickname, as
/// `~/.cache/<nickname>/...` is (RAL-163).
fn nickname_in(cwd: &str, nicknames: &BTreeSet<String>) -> Option<String> {
    cwd.split('/')
        .map(str::to_lowercase)
        .find(|part| nicknames.contains(part))
}

/// The `possibly_hung_lock_holder` prompt for one lock (RAL-163): a holder whose whole process
/// tree used less than [`HUNG_CPU_PERCENT`] of a CPU between two reads `sample_ms` apart, and has
/// been running at least `after_ms`. It names the process in the tree that has used the most CPU
/// in its life, with its command and working directory. A prompt, never a finding: a run waiting
/// on something legitimate looks the same.
pub fn hung(
    lock: &str,
    before: &Seen,
    after: &Seen,
    sample_ms: u64,
    ticks_per_s: u64,
    after_ms: u64,
    roster: &BTreeMap<String, String>,
) -> Option<Value> {
    let ticks = ticks_per_s.max(1);
    let nicknames: BTreeSet<String> = roster.values().map(|n| n.to_lowercase()).collect();
    let mut quiet = Vec::new();
    for &holder in &after.holders {
        let Some(top) = after.procs.get(&holder) else {
            continue;
        };
        // Only the tree both reads saw, so a process that came or went isn't counted.
        let pids: Vec<u32> = tree(holder, &after.procs)
            .into_iter()
            .filter(|pid| before.procs.contains_key(pid))
            .collect();
        if pids.is_empty() {
            continue;
        }
        let used: u64 = pids
            .iter()
            .map(|pid| {
                after.procs[pid]
                    .cpu_ticks
                    .saturating_sub(before.procs[pid].cpu_ticks)
            })
            .sum();
        let percent = used as f64 * 1000.0 * 100.0 / (ticks as f64 * sample_ms.max(1) as f64);
        let running_ms = after
            .uptime_ms
            .saturating_sub(top.start_ticks * 1000 / ticks);
        if percent >= HUNG_CPU_PERCENT || running_ms < after_ms {
            continue;
        }
        let busiest = pids
            .iter()
            .max_by_key(|pid| after.procs[pid].cpu_ticks)
            .copied()
            .unwrap_or(holder);
        let cwd = after.cwds.get(&busiest).or_else(|| after.cwds.get(&holder));
        let command = after
            .cmdlines
            .get(&busiest)
            .cloned()
            .unwrap_or_else(|| after.procs[&busiest].comm.clone());
        let command: String = command.chars().take(200).collect();
        let team = cwd.and_then(|cwd| nickname_in(cwd, &nicknames));
        let session = cwd.and_then(|cwd| session_in(cwd));
        quiet.push(json!({
            "holder": holder, "pid": busiest, "command": command, "cwd": cwd,
            "cpu_percent": (percent * 10.0).round() / 10.0, "sample_ms": sample_ms,
            "running_min": running_ms / 60_000,
            "summary": format!(
                "possibly hung: {command}, {}, {:.1}% CPU over {} s, running {} min",
                cwd.map_or("cwd unknown", String::as_str),
                percent,
                sample_ms / 1000,
                running_ms / 60_000
            ),
            "team": team, "session": session,
            "team_kind": "guessed from the working directory",
        }));
    }
    if quiet.is_empty() {
        return None;
    }
    Some(json!({
        "row": "possibly_hung_lock_holder",
        "lock": lock,
        "likely_meaning": "The lock's holder is running but doing almost nothing, so it may be hung",
        "action": "Ask the team whose run it is to look at it; nothing is stopped or killed by Rally.",
        "processes": quiet,
        "waiters": after.waiters.len(),
        "kind": "observed",
        "note": "A prompt for the watchman, not a conclusion: a run waiting on something \
                 legitimate looks the same.",
    }))
}

/// The lock files `patterns` name: each as given, or, where its file name has `*` or `?`, every
/// file in that directory whose name matches (RAL-163), so `/tmp/discosim-heavy*.lock` takes
/// every slot. A pattern matching nothing is kept as given, so it's reported as unreadable.
pub fn expand(patterns: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    fn matches(pattern: &[char], name: &[char]) -> bool {
        match (pattern.first(), name.first()) {
            (None, None) => true,
            (Some('*'), _) => {
                matches(&pattern[1..], name) || (!name.is_empty() && matches(pattern, &name[1..]))
            }
            (Some('?'), Some(_)) => matches(&pattern[1..], &name[1..]),
            (Some(p), Some(n)) if p == n => matches(&pattern[1..], &name[1..]),
            _ => false,
        }
    }
    let mut out = Vec::new();
    for pattern in patterns {
        let name = pattern
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !name.contains(['*', '?']) {
            out.push(pattern.clone());
            continue;
        }
        let dir = pattern
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let wanted: Vec<char> = name.chars().collect();
        let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| {
                let have: Vec<char> = entry.file_name().to_string_lossy().chars().collect();
                matches(&wanted, &have)
            })
            .map(|entry| entry.path())
            .collect();
        found.sort();
        if found.is_empty() {
            out.push(pattern.clone());
        }
        out.extend(found);
    }
    out
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
        if let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) {
            let line: Vec<String> = raw
                .split(|b| *b == 0)
                .filter(|part| !part.is_empty())
                .map(|part| String::from_utf8_lossy(part).into_owned())
                .collect();
            seen.cmdlines.insert(pid, line.join(" "));
        }
    }
    seen.uptime_ms = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|text| text.split_whitespace().next()?.parse::<f64>().ok())
        .map_or(0, |secs| (secs * 1000.0) as u64);
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
    check_with(locks, roster, None)
}

/// When to look for a possibly hung holder (RAL-163): running at least `after_ms`, sampled over
/// `sample_ms`.
#[derive(Clone, Copy, Debug)]
pub struct Hung {
    pub after_ms: u64,
    pub sample_ms: u64,
}

/// [`check`], over every lock the patterns name, and with `hung`, a second read `sample_ms` after
/// the first for each holder's CPU use.
pub fn check_with(
    patterns: &[std::path::PathBuf],
    roster: Option<&Value>,
    hung_check: Option<Hung>,
) -> (Vec<Value>, Vec<Value>) {
    let names = nicknames(roster);
    let mut prompts = Vec::new();
    let mut seen_all = Vec::new();
    let locks = expand(patterns);
    let first: Vec<Result<Seen, String>> = locks.iter().map(|lock| read(lock)).collect();
    let second: Option<Vec<Result<Seen, String>>> = hung_check.map(|h| {
        std::thread::sleep(std::time::Duration::from_millis(h.sample_ms));
        locks.iter().map(|lock| read(lock)).collect()
    });
    let ticks = rustix::param::clock_ticks_per_second();
    for (index, (lock, first)) in locks.iter().zip(first).enumerate() {
        let shown = lock.display().to_string();
        if let (Some(h), Some(second), Ok(before)) = (hung_check, &second, &first)
            && let Ok(after) = &second[index]
        {
            prompts.extend(hung(
                &shown,
                before,
                after,
                h.sample_ms,
                ticks,
                h.after_ms,
                &names,
            ));
        }
        match first {
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
            cpu_ticks: 0,
            start_ticks: 0,
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
            ..Seen::default()
        }
    }

    /// RAL-163: a holder's tree that used almost no CPU between two reads, and has run long
    /// enough, is possibly hung, named by its busiest process's command and working directory,
    /// with the team guessed from a nickname in that directory.
    #[test]
    fn a_holder_doing_almost_nothing_is_possibly_hung() {
        let at = |cpu: u64| {
            let mut godot = proc(101, 100, 'S', "godot");
            godot.cpu_ticks = cpu;
            let mut holder = proc(100, 1, 'S', "flock");
            holder.start_ticks = 100; // 1 s after boot at 100 ticks a second
            Seen {
                matched: Matched::DeviceAndInode,
                holders: vec![100],
                procs: BTreeMap::from([(100, holder), (101, godot)]),
                cwds: BTreeMap::from([(101, "/home/j/.cache/sleek-moth/discosim".into())]),
                cmdlines: BTreeMap::from([(101, "godot --headless -s run_tests.gd".into())]),
                uptime_ms: 12 * 60_000,
                ..Seen::default()
            }
        };
        let roster = BTreeMap::from([("id".to_owned(), "sleek-moth".to_owned())]);
        // 0.9% of a CPU over 5 s: 4.5 ticks, so 4 at 100 a second.
        let row = hung(
            "/tmp/discosim-heavy.1.lock",
            &at(5_000),
            &at(5_004),
            5_000,
            100,
            5 * 60_000,
            &roster,
        )
        .unwrap();
        assert_eq!(row["row"], "possibly_hung_lock_holder");
        let p = &row["processes"][0];
        assert_eq!(p["pid"], 101);
        assert_eq!(p["team"], "sleek-moth");
        assert_eq!(p["running_min"], 11);
        assert_eq!(
            p["summary"],
            "possibly hung: godot --headless -s run_tests.gd, /home/j/.cache/sleek-moth/discosim, \
             0.8% CPU over 5 s, running 11 min"
        );
        // Busy, it isn't.
        assert!(hung("l", &at(5_000), &at(5_400), 5_000, 100, 5 * 60_000, &roster).is_none());
        // Not long enough running, it isn't either.
        assert!(
            hung(
                "l",
                &at(5_000),
                &at(5_004),
                5_000,
                100,
                60 * 60_000,
                &roster
            )
            .is_none()
        );
    }

    /// RAL-163: a pattern with `*` or `?` in its file name takes every matching file; one with
    /// none is kept as given.
    #[test]
    fn lock_patterns_take_every_slot() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "discosim-heavy.lock",
            "discosim-heavy.1.lock",
            "discosim-heavy.2.lock",
            "other.lock",
        ] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        let found = expand(&[dir.path().join("discosim-heavy*.lock")]);
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "discosim-heavy.1.lock",
                "discosim-heavy.2.lock",
                "discosim-heavy.lock"
            ]
        );
        let plain = dir.path().join("other.lock");
        assert_eq!(expand(std::slice::from_ref(&plain)), vec![plain]);
        let none = dir.path().join("nothing-*.lock");
        assert_eq!(expand(std::slice::from_ref(&none)), vec![none]);
    }

    #[test]
    fn a_stat_line_gives_cpu_and_start_time() {
        let line = "4242 (godot (x)) S 1 2 3 4 5 6 7 8 9 10 250 50 0 0 20 0 1 0 900 1000 10";
        let p = parse_stat(line).unwrap();
        assert_eq!((p.pid, p.ppid, p.comm.as_str()), (4242, 1, "godot (x)"));
        assert_eq!(p.cpu_ticks, 300);
        assert_eq!(p.start_ticks, 900);
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
