//! A fair slot pool's queue (RAL-188), for `rally stalls --lock`.
//!
//! From the DiscoSim rally's issues log, 1.84: one team's 25 heavy jobs took most of dsm-heavy's
//! slots and another team's suite waited 21 minutes. dsm-heavy now serves the owners holding
//! fewer slots first, and its waiters no longer wait inside `flock` on a slot file: each holds a
//! file of its own locked in a waiting directory beside the slots and polls. So `/proc/locks`
//! shows no waiter on any slot, and stalls' waiter count reads 0 however long the queue is.
//!
//! This reads that queue instead: each held slot's owner (the first word of its lock file, as
//! dsm-heavy writes it), and each waiter (a file in the waiting directory that something holds a
//! lock on, by device and inode, as [`crate::lock_holder::parse_locks`] matches them), by owner
//! and with how long it has waited, from the file's modification time. A waiter's file nobody
//! holds is a dead waiter's and isn't counted. Read-only: nothing is created, removed or locked.
//!
//! What's read is the kernel's account and the files' own text (observed). Who an owner is comes
//! from the job itself (declared): dsm-heavy names it from DSM_HEAVY_OWNER, RALLY_NAME, the
//! `~/.cache/<team>` folder or the session's nickname.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// How long a waiter may wait while an owner holding more slots runs, before it's a prompt, when
/// `--queue-wait-after` isn't given. Tonight's (1.84) was 21 minutes.
pub const DEFAULT_WAIT_AFTER_MS: u64 = 10 * 60 * 1000;

/// One job waiting for a slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiter {
    pub owner: String,
    pub file: String,
    pub waited_ms: u64,
}

/// What was read of one queue.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Queue {
    pub dir: String,
    /// Each slot's lock file, with its owner when held ("?" for a holder that wrote none).
    pub slots: Vec<(String, Option<String>)>,
    pub waiting: Vec<Waiter>,
    /// Files in the waiting directory that nothing holds: dead waiters', left for dsm-heavy to
    /// clear.
    pub dead: usize,
}

/// The waiting directory a `--lock` pattern's pool keeps: beside its slots, named for the part
/// of the file name before any `*` or `?` (or before `.lock`), with `.waiting`:
/// `/tmp/discosim-heavy*.lock` and `/tmp/discosim-heavy.lock` both give
/// `/tmp/discosim-heavy.waiting`.
pub fn queue_dir(pattern: &Path) -> PathBuf {
    let name = pattern
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = match name.find(['*', '?']) {
        Some(at) => &name[..at],
        None => name.strip_suffix(".lock").unwrap_or(&name),
    };
    let stem = stem.trim_end_matches('.');
    pattern
        .parent()
        .unwrap_or(Path::new(""))
        .join(format!("{stem}.waiting"))
}

/// A lock file's owner, as dsm-heavy writes it: the first word of its first line, at most 40
/// characters, or `None` when the file says nothing.
fn owner_in(text: &str) -> Option<String> {
    let word = text.lines().next()?.split_whitespace().next()?;
    Some(word.chars().take(40).collect())
}

/// Whether `/proc/locks` has a lock on this file, matched by device and inode.
#[cfg(unix)]
fn held(meta: &std::fs::Metadata, locks: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    let dev = meta.dev();
    let (holders, _, _) = crate::lock_holder::parse_locks(
        locks,
        rustix::fs::major(dev),
        rustix::fs::minor(dev),
        meta.ino(),
    );
    !holders.is_empty()
}

#[cfg(not(unix))]
fn held(_meta: &std::fs::Metadata, _locks: &str) -> bool {
    false
}

/// Reads one queue: `slots` are the pool's lock files, `dir` its waiting directory, `locks` the
/// text of `/proc/locks`, `now_ms` the time to measure waits from.
pub fn read(dir: &Path, slots: &[PathBuf], locks: &str, now_ms: i64) -> Result<Queue, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("can't list it: {e}"))?;
    let mut queue = Queue {
        dir: dir.display().to_string(),
        ..Queue::default()
    };
    for slot in slots {
        let owner = std::fs::metadata(slot)
            .ok()
            .filter(|meta| held(meta, locks))
            .map(|_| {
                std::fs::read_to_string(slot)
                    .ok()
                    .and_then(|text| owner_in(&text))
                    .unwrap_or_else(|| "?".into())
            });
        queue.slots.push((slot.display().to_string(), owner));
    }
    let mut files: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    files.sort();
    for file in files {
        // A file that went between the listing and now was a waiter that got its slot.
        let Ok(meta) = std::fs::metadata(&file) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if !held(&meta, locks) {
            queue.dead += 1;
            continue;
        }
        let since_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(now_ms, |d| d.as_millis() as i64);
        queue.waiting.push(Waiter {
            owner: std::fs::read_to_string(&file)
                .ok()
                .and_then(|text| owner_in(&text))
                .unwrap_or_else(|| "?".into()),
            file: file.display().to_string(),
            waited_ms: now_ms.saturating_sub(since_ms).max(0) as u64,
        });
    }
    Ok(queue)
}

/// Each owner's held slots.
fn holding(queue: &Queue) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for owner in queue.slots.iter().filter_map(|(_, owner)| owner.as_ref()) {
        *out.entry(owner.clone()).or_insert(0) += 1;
    }
    out
}

/// The owners holding more than `holds` slots, most first: "teamA holds 3, teamC holds 2".
fn more_than(holding: &BTreeMap<String, usize>, holds: usize) -> String {
    let mut more: Vec<(&String, &usize)> = holding.iter().filter(|(_, n)| **n > holds).collect();
    more.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    more.iter()
        .map(|(o, n)| format!("{o} holds {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What was seen of one queue, for the report's `queues`.
pub fn row(queue: &Queue) -> Value {
    let mut waiting: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for waiter in &queue.waiting {
        let entry = waiting.entry(waiter.owner.clone()).or_insert((0, 0));
        entry.0 += 1;
        entry.1 = entry.1.max(waiter.waited_ms);
    }
    let held = queue.slots.iter().filter(|(_, o)| o.is_some()).count();
    json!({
        "queue": queue.dir, "checked": true, "kind": "observed",
        "slots": queue.slots.len(), "held": held,
        "holding": holding(queue),
        "waiting": waiting.iter().map(|(owner, (jobs, longest))| json!({
            "owner": owner, "jobs": jobs, "longest_wait_min": longest / 60_000,
        })).collect::<Vec<_>>(),
        "dead_waiter_files": queue.dead,
        "owner_kind": "declared: the first word each job writes in its lock or waiting file",
        "note": "Slots are every lock file the pattern names, which may be more than the pool's \
                 dial allows; \"?\" holds a slot without writing an owner (a job from before fair \
                 shares).",
    })
}

/// The `unfair_slot_wait` prompt for one queue: an owner whose job has waited at least
/// `after_ms` while another owner holds more slots than it does. dsm-heavy gives a free slot to a
/// waiter within a poll (5 s), so a wait that long means every slot it may use stayed busy, held
/// by owners with more. A prompt, never a finding: a pool whose dial was lowered, or one owner's
/// long jobs, looks the same.
pub fn judge(queue: &Queue, after_ms: u64) -> Option<Value> {
    let holding = holding(queue);
    let most = holding.values().copied().max().unwrap_or(0);
    let mut longest: BTreeMap<&str, u64> = BTreeMap::new();
    for waiter in &queue.waiting {
        let entry = longest.entry(&waiter.owner).or_insert(0);
        *entry = (*entry).max(waiter.waited_ms);
    }
    let starved: Vec<Value> = longest
        .iter()
        .filter(|(owner, waited)| {
            **waited >= after_ms && holding.get(**owner).copied().unwrap_or(0) < most
        })
        .map(|(owner, waited)| {
            let holds = holding.get(*owner).copied().unwrap_or(0);
            json!({
                "owner": owner, "holds": holds, "waited_min": waited / 60_000,
                "summary": format!(
                    "{owner} has waited {} min holding {holds} slot{} while {}",
                    waited / 60_000,
                    if holds == 1 { "" } else { "s" },
                    more_than(&holding, holds)
                ),
            })
        })
        .collect();
    if starved.is_empty() {
        return None;
    }
    Some(json!({
        "row": "unfair_slot_wait",
        "queue": queue.dir,
        "likely_meaning": "A team's job has waited a long time for a slot while others hold more",
        "action": "Ask the teams holding most to let their queued jobs wait or run fewer at once, \
                   or raise the pool's dial; nothing is stopped or killed by Rally.",
        "waiting": starved,
        "holding": holding,
        "kind": "observed",
        "note": "A prompt for the watchman, not a conclusion: long jobs started before the wait \
                 look the same.",
    }))
}

/// Every queue the `--lock` patterns' pools keep, read and judged: the prompts, and a row for
/// each queue. With `given`, that's the waiting directory for all of them, and it must exist;
/// otherwise each pattern's own [`queue_dir`] is read where it exists, and a pool without one is
/// a plain lock, with nothing to report here. A waiting directory that exists while `/proc/locks`
/// can't be read is a row that says it couldn't see the queue, never one with nobody waiting.
pub fn check(
    patterns: &[PathBuf],
    given: Option<&Path>,
    after_ms: u64,
    now_ms: i64,
    locks: Result<String, String>,
) -> (Vec<Value>, Vec<Value>) {
    let mut pools: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    for pattern in patterns {
        let dir = given.map_or_else(|| queue_dir(pattern), Path::to_path_buf);
        let slots = crate::lock_holder::expand(std::slice::from_ref(pattern));
        match pools.iter_mut().find(|(d, _)| *d == dir) {
            Some((_, have)) => {
                for slot in slots {
                    if !have.contains(&slot) {
                        have.push(slot);
                    }
                }
            }
            None => pools.push((dir, slots)),
        }
    }
    let mut prompts = Vec::new();
    let mut rows = Vec::new();
    for (dir, slots) in pools {
        let shown = dir.display().to_string();
        if !dir.is_dir() {
            if given.is_some() {
                rows.push(json!({"queue": shown, "checked": false,
                    "reason": "can't see the queue: the waiting directory given with --queue \
                               isn't there"}));
            }
            continue;
        }
        let read = locks
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|text| read(&dir, &slots, text, now_ms));
        match read {
            Ok(queue) => {
                rows.push(row(&queue));
                prompts.extend(judge(&queue, after_ms));
            }
            Err(reason) => rows.push(json!({"queue": shown, "checked": false,
                "reason": format!("can't see the queue: {reason}")})),
        }
    }
    (prompts, rows)
}

/// [`check`], reading `/proc/locks` and the clock.
pub fn check_now(
    patterns: &[PathBuf],
    given: Option<&Path>,
    after_ms: u64,
) -> (Vec<Value>, Vec<Value>) {
    let locks =
        std::fs::read_to_string("/proc/locks").map_err(|e| format!("can't read /proc/locks: {e}"));
    check(
        patterns,
        given,
        after_ms,
        crate::digest_loop::now_ms(),
        locks,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `/proc/locks` line holding `path`, by its device and inode.
    #[cfg(unix)]
    fn lock_line(path: &Path, pid: u32) -> String {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).unwrap();
        format!(
            "{pid}: FLOCK  ADVISORY  WRITE {pid} {:02x}:{:02x}:{} 0 EOF\n",
            rustix::fs::major(meta.dev()),
            rustix::fs::minor(meta.dev()),
            meta.ino()
        )
    }

    #[test]
    fn the_waiting_directory_is_named_from_the_pattern() {
        for (pattern, dir) in [
            ("/tmp/discosim-heavy*.lock", "/tmp/discosim-heavy.waiting"),
            ("/tmp/discosim-heavy.lock", "/tmp/discosim-heavy.waiting"),
            ("/x/pool.?.lock", "/x/pool.waiting"),
            ("/x/pool.*.lock", "/x/pool.waiting"),
        ] {
            assert_eq!(
                queue_dir(Path::new(pattern)),
                PathBuf::from(dir),
                "{pattern}"
            );
        }
    }

    #[test]
    fn an_owner_is_the_first_word_of_the_first_line() {
        assert_eq!(
            owner_in("teamA 2026-11-14T10:00:00 run_tests.sh quick\n").as_deref(),
            Some("teamA")
        );
        assert_eq!(owner_in("").as_deref(), None);
        assert_eq!(owner_in("\n").as_deref(), None);
        assert_eq!(owner_in(&"x".repeat(80)).map(|o| o.len()), Some(40));
    }

    /// Slots: 1 and 2 held by teamA, 3 by a job that wrote no owner, 4 free. Waiters: teamB for
    /// 25 minutes, teamA for 30, and a dead teamC file nobody holds.
    #[cfg(unix)]
    fn pool() -> (tempfile::TempDir, Vec<PathBuf>, PathBuf, String, i64) {
        let dir = tempfile::tempdir().unwrap();
        let mut slots = Vec::new();
        let mut locks = String::from("1: POSIX  ADVISORY  WRITE 9 00:01:1 0 EOF\n");
        for (i, text) in [(1, "teamA x"), (2, "teamA y"), (3, ""), (4, "teamB old")] {
            let slot = dir.path().join(format!("heavy.{i}.lock"));
            std::fs::write(&slot, text).unwrap();
            if i != 4 {
                locks.push_str(&lock_line(&slot, 100 + i));
            }
            slots.push(slot);
        }
        let waiting = dir.path().join("heavy.waiting");
        std::fs::create_dir(&waiting).unwrap();
        for (name, owner, held) in [
            ("teamB.1", "teamB\n", true),
            ("teamA.2", "teamA\n", true),
            ("teamC.3", "teamC\n", false),
        ] {
            let file = waiting.join(name);
            std::fs::write(&file, owner).unwrap();
            if held {
                locks.push_str(&lock_line(&file, 200));
            }
        }
        let ago = |file: &str, min: u64| {
            let at = std::time::SystemTime::now() - std::time::Duration::from_secs(min * 60);
            std::fs::File::options()
                .write(true)
                .open(waiting.join(file))
                .unwrap()
                .set_modified(at)
                .unwrap();
        };
        ago("teamB.1", 25);
        ago("teamA.2", 30);
        // Read after the times are set, so each wait is whole minutes and a little more.
        let now_ms = crate::digest_loop::now_ms();
        (dir, slots, waiting, locks, now_ms)
    }

    #[cfg(unix)]
    #[test]
    fn a_queue_is_read_by_owner_and_a_dead_waiters_file_isnt_counted() {
        let (_dir, slots, waiting, locks, now_ms) = pool();
        let queue = read(&waiting, &slots, &locks, now_ms).unwrap();
        let owners: Vec<Option<&str>> = queue.slots.iter().map(|(_, o)| o.as_deref()).collect();
        assert_eq!(owners, [Some("teamA"), Some("teamA"), Some("?"), None]);
        assert_eq!(queue.dead, 1);
        let waiting: Vec<(&str, u64)> = queue
            .waiting
            .iter()
            .map(|w| (w.owner.as_str(), w.waited_ms / 60_000))
            .collect();
        assert_eq!(waiting, [("teamA", 30), ("teamB", 25)]);
        let row = row(&queue);
        assert_eq!(row["held"], 3);
        assert_eq!(row["holding"], json!({"?": 1, "teamA": 2}));
        assert_eq!(row["dead_waiter_files"], 1);
        assert_eq!(
            row["waiting"][1],
            json!({"owner": "teamB", "jobs": 1, "longest_wait_min": 25})
        );
    }

    /// teamB (holding 0) has waited 25 min while teamA holds 2: a prompt. teamA has waited
    /// longer, but holds the most, so it isn't named; and under the bound, nothing.
    #[cfg(unix)]
    #[test]
    fn a_long_wait_behind_an_owner_holding_more_is_a_prompt() {
        let (_dir, slots, waiting, locks, now_ms) = pool();
        let queue = read(&waiting, &slots, &locks, now_ms).unwrap();
        let prompt = judge(&queue, DEFAULT_WAIT_AFTER_MS).unwrap();
        assert_eq!(prompt["row"], "unfair_slot_wait");
        let named = prompt["waiting"].as_array().unwrap();
        assert_eq!(named.len(), 1, "{prompt}");
        assert_eq!(named[0]["owner"], "teamB");
        assert_eq!(
            named[0]["summary"],
            "teamB has waited 25 min holding 0 slots while teamA holds 2, ? holds 1"
        );
        assert!(judge(&queue, 26 * 60 * 1000).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_queue_unseen_says_so_and_a_pool_without_one_is_left_out() {
        let (dir, _slots, waiting, locks, now_ms) = pool();
        let pattern = dir.path().join("heavy.*.lock");
        // /proc/locks unreadable while the waiting directory is there: can't see, never 0.
        let (prompts, rows) = check(
            std::slice::from_ref(&pattern),
            None,
            DEFAULT_WAIT_AFTER_MS,
            now_ms,
            Err("can't read /proc/locks: denied".into()),
        );
        assert!(prompts.is_empty());
        assert_eq!(
            rows,
            [
                json!({"queue": waiting.display().to_string(), "checked": false,
                "reason": "can't see the queue: can't read /proc/locks: denied"})
            ]
        );
        // Read: the derived directory, with the prompt.
        let (prompts, rows) = check(
            std::slice::from_ref(&pattern),
            None,
            DEFAULT_WAIT_AFTER_MS,
            now_ms,
            Ok(locks.clone()),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["slots"], 4);
        assert_eq!(prompts.len(), 1);
        // A plain lock with no waiting directory beside it: nothing here.
        let plain = dir.path().join("plain.lock");
        std::fs::write(&plain, "").unwrap();
        let (prompts, rows) = check(
            std::slice::from_ref(&plain),
            None,
            0,
            now_ms,
            Ok(locks.clone()),
        );
        assert!(prompts.is_empty() && rows.is_empty(), "{rows:?}");
        // Unless --queue names one, which must be there.
        let missing = dir.path().join("nope.waiting");
        let (_, rows) = check(&[plain], Some(&missing), 0, now_ms, Ok(locks.clone()));
        assert_eq!(rows[0]["checked"], false);
        assert!(
            rows[0]["reason"]
                .as_str()
                .unwrap()
                .starts_with("can't see the queue")
        );
        // --queue for a pattern whose own name gives another directory.
        let other = dir.path().join("other*.lock");
        let (prompts, rows) = check(
            &[other],
            Some(&waiting),
            DEFAULT_WAIT_AFTER_MS,
            now_ms,
            Ok(locks),
        );
        assert_eq!(rows[0]["queue"], waiting.display().to_string());
        assert_eq!(rows[0]["holding"], json!({}));
        // Nobody holds a slot, so nobody holds more than teamB: no prompt.
        assert!(prompts.is_empty());
    }
}
