//! A project gated on this machine, for the digest's "Base CI after merges" section (RAL-180).
//!
//! DiscoSim has no CI on GitHub: its trains are gated locally by `dsm-gate`, which writes
//! `<logs>/<tip>.log` (its first line names the tip and tree) and, only on green, `<tip>.ok`. Every
//! DiscoSim landing the watchman recorded showed in the digest as "the base's CI couldn't be
//! checked", five lines a train, noise that would hide a real red. With `rally digest --gate
//! DSM=<logs>,<checkout>`, each merged commit C of the project is looked for in the gate's own
//! files and the project's git checkout instead, and everything said is observed from them:
//!
//! - **green**: the earliest gated tip T that contains C, has its `.ok`, and whose log names T
//!   with no failure, and T is on the checkout's `origin/main`. A train that failed and was rebuilt
//!   with the same commit, or a stacked train still gating on top, can't turn a landed, gated
//!   merge red, since any green tip that landed wins.
//! - **green, not on main**: a green tip contains C, but none of them landed, so it vouches for
//!   nothing on main: said as its own line.
//! - **red**, **running**: no green tip contains C; the newest that does has failures, or no
//!   `.ok` after the running limit (red), or under it (running).
//! - **no gate log**: no gated tip contains C: it landed without a gate.
//! - **not in the checkout**: the checkout hasn't fetched C, so it can't be told yet.
//!
//! Paths are said as given (`~/…`), never expanded, and nothing here writes anything.

use std::path::{Path, PathBuf};

use crate::gate_log::GateLog;

/// The most gate logs read, newest first, for each commit.
pub const MAX_LOGS: usize = 200;
/// How long a gate can run, by default, before a tip with a log and no `.ok` counts as red
/// (tonight's gates ran 50 to 65 minutes under load).
pub const DEFAULT_RUNNING_MS: u64 = 2 * 3_600_000;

/// One `--gate` value: `<PREFIX>=<logs dir>,<checkout>[,running=<window>]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gate {
    /// The flag as given, kept so a restart passes it again unchanged.
    pub given: String,
    pub prefix: String,
    /// The logs directory and checkout, as given (for saying) and expanded (for reading).
    pub logs_given: String,
    pub logs: PathBuf,
    pub checkout_given: String,
    pub checkout: PathBuf,
    pub running_ms: u64,
}

fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(rest))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

/// A window such as `90m`, `2h` or `3600s`, in milliseconds.
fn window(text: &str) -> Option<u64> {
    let (number, unit) = text.split_at(text.find(|c: char| !c.is_ascii_digit())?);
    let n: u64 = number.parse().ok().filter(|n| *n > 0)?;
    Some(match unit {
        "s" => n * 1000,
        "m" => n * 60_000,
        "h" => n * 3_600_000,
        _ => return None,
    })
}

impl Gate {
    /// Reads a `--gate` value.
    pub fn parse(given: &str) -> Result<Self, String> {
        let usage = "--gate takes <PREFIX>=<logs dir>,<checkout>[,running=<window>], such as \
                     DSM=~/.local/state/dsm-gates,~/Documents/GitHub/Discoteque";
        let (prefix, rest) = given.split_once('=').ok_or(usage)?;
        let parts: Vec<&str> = rest.split(',').map(str::trim).collect();
        let (logs, checkout) = match parts.as_slice() {
            [logs, checkout, ..] if !logs.is_empty() && !checkout.is_empty() => (*logs, *checkout),
            _ => return Err(usage.to_owned()),
        };
        let mut running_ms = DEFAULT_RUNNING_MS;
        for extra in &parts[2..] {
            match extra.strip_prefix("running=").and_then(window) {
                Some(ms) => running_ms = ms,
                None => return Err(format!("{usage}: `{extra}` isn't running=<window>")),
            }
        }
        let prefix = prefix.trim().to_ascii_uppercase();
        if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(usage.to_owned());
        }
        Ok(Self {
            given: given.to_owned(),
            prefix,
            logs_given: logs.to_owned(),
            logs: expand(logs),
            checkout_given: checkout.to_owned(),
            checkout: expand(checkout),
            running_ms,
        })
    }

    /// Whether `story` is this project's, by its prefix.
    pub fn covers(&self, story: &str) -> bool {
        story
            .split_once('-')
            .is_some_and(|(prefix, _)| prefix.eq_ignore_ascii_case(&self.prefix))
    }

    /// A tip's log, as given, for saying.
    pub fn log_shown(&self, tip: &str) -> String {
        format!("{}/{tip}.log", self.logs_given.trim_end_matches('/'))
    }
}

/// What the gate's files and git say about one merged commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Gated green at `tip`, which contains the commit; `on_main` whether `tip` landed.
    Green {
        tip: String,
        on_main: bool,
        summary: String,
    },
    /// Gated red at `tip`, the newest gated tip containing the commit, with why.
    Red { tip: String, why: String },
    /// A gate at `tip` is still within the running limit.
    Running { tip: String, age_ms: u64 },
    /// No gated tip contains the commit.
    NoLog { read: usize },
    /// The checkout can't answer: the commit or `origin/main` isn't there.
    CantTell(String),
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn contains(dir: &Path, tip: &str, commit: &str) -> bool {
    tip.eq_ignore_ascii_case(commit)
        || git(dir, &["merge-base", "--is-ancestor", commit, tip]).is_some()
}

fn is_sha(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

fn mtime_ms(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis() as u64,
    )
}

/// Whether a log says green for `tip`: it names `tip` and no scene failed or gave no result.
fn green(log: &GateLog, tip: &str) -> Result<String, String> {
    if !log.commit.eq_ignore_ascii_case(tip) {
        return Err(format!("its log names {}, not the tip", &log.commit[..7]));
    }
    match log.failures() {
        (0, 0) => Ok(format!(
            "{}{} scenes, 0 failed",
            log.quick
                .as_deref()
                .map(|q| format!("{q}, "))
                .unwrap_or_default(),
            log.scenes.len()
        )),
        (failed, 0) => Err(format!("{failed} failed")),
        (failed, silent) => Err(format!("{failed} failed, {silent} gave no result")),
    }
}

/// What `gate`'s files and checkout say about `commit`, at `now_ms`.
pub fn verdict(gate: &Gate, commit: &str, now_ms: u64) -> Verdict {
    let dir = &gate.checkout;
    if git(dir, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_none() {
        return Verdict::CantTell(format!(
            "{} doesn't have {}: fetch it there",
            gate.checkout_given,
            &commit[..commit.len().min(7)]
        ));
    }
    let Some(main) = git(dir, &["rev-parse", "--verify", "origin/main^{commit}"]) else {
        return Verdict::CantTell(format!("{} has no origin/main", gate.checkout_given));
    };
    // The newest logs, each with its tip and time.
    let mut logs: Vec<(String, u64)> = std::fs::read_dir(&gate.logs)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let tip = name.strip_suffix(".log").filter(|t| is_sha(t))?.to_owned();
            Some((tip, mtime_ms(&entry.path())?))
        })
        .collect();
    logs.sort_by_key(|(_, ms)| std::cmp::Reverse(*ms));
    logs.truncate(MAX_LOGS);
    let read = logs.len();
    let holding: Vec<(String, u64)> = logs
        .into_iter()
        .filter(|(tip, _)| contains(dir, tip, commit))
        .collect();
    // The earliest green tip that landed wins; failing that, the earliest green one at all.
    let mut greens: Vec<(String, u64)> = holding
        .iter()
        .filter_map(|(tip, _)| Some((tip.clone(), mtime_ms(&gate.logs.join(format!("{tip}.ok")))?)))
        .collect();
    greens.sort_by_key(|(_, ok_ms)| *ok_ms);
    let mut off_main = None;
    for (tip, _) in &greens {
        let Ok(summary) = crate::gate_log::read(&gate.logs.join(format!("{tip}.log")))
            .and_then(|log| green(&log, tip))
        else {
            continue;
        };
        if contains(dir, &main, tip) {
            return Verdict::Green {
                tip: tip.clone(),
                on_main: true,
                summary,
            };
        }
        off_main.get_or_insert(Verdict::Green {
            tip: tip.clone(),
            on_main: false,
            summary,
        });
    }
    if let Some(found) = off_main {
        return found;
    }
    // No green: the newest tip that holds it says red or running.
    let Some((tip, log_ms)) = holding.first() else {
        return Verdict::NoLog { read };
    };
    let log = crate::gate_log::read(&gate.logs.join(format!("{tip}.log")));
    let has_ok = gate.logs.join(format!("{tip}.ok")).exists();
    let age_ms = now_ms.saturating_sub(*log_ms);
    match log.as_ref().map(|l| green(l, tip)) {
        Ok(Err(why)) => Verdict::Red {
            tip: tip.clone(),
            why,
        },
        Err(why) if has_ok => Verdict::Red {
            tip: tip.clone(),
            why: format!("its .ok is there but its log can't be read: {why}"),
        },
        _ if has_ok => Verdict::Red {
            tip: tip.clone(),
            why: "its .ok is there but its log doesn't say green".to_owned(),
        },
        _ if age_ms >= gate.running_ms => Verdict::Red {
            tip: tip.clone(),
            why: format!(
                "no .ok {} min after its log was last written: red, or it never finished",
                age_ms / 60_000
            ),
        },
        _ => Verdict::Running {
            tip: tip.clone(),
            age_ms,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) -> String {
        git(dir, args).unwrap_or_else(|| panic!("git {args:?} failed"))
    }

    /// A checkout with `origin/main`, a gate logs directory, and commits to gate.
    struct Fixture {
        _dir: tempfile::TempDir,
        repo: PathBuf,
        logs: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let repo = dir.path().join("repo");
            let logs = dir.path().join("gates");
            std::fs::create_dir_all(&repo).unwrap();
            std::fs::create_dir_all(&logs).unwrap();
            run(&repo, &["init", "-q", "-b", "main"]);
            run(&repo, &["config", "user.email", "t@t"]);
            run(&repo, &["config", "user.name", "t"]);
            let f = Self {
                _dir: dir,
                repo,
                logs,
            };
            f.commit("start");
            f
        }
        fn commit(&self, message: &str) -> String {
            run(
                &self.repo,
                &["commit", "-q", "--allow-empty", "-m", message],
            );
            run(&self.repo, &["rev-parse", "HEAD"])
        }
        /// Moves `origin/main` to `sha`, as a fetch after a landing would.
        fn land(&self, sha: &str) {
            run(&self.repo, &["update-ref", "refs/remotes/origin/main", sha]);
        }
        /// A gate log for `tip`, with `failed` failures, and its `.ok` when `ok`, the log written
        /// `age_ms` ago.
        fn gate(&self, tip: &str, failed: u64, ok: bool, age_ms: u64) {
            let log = self.logs.join(format!("{tip}.log"));
            std::fs::write(
                &log,
                format!(
                    "# dsm-gate: commit {tip} tree {} scenes: x started t script sha256 0\nQUICK OK\n\
                     == res://tests/test_ui_flow.tscn ==\n=== 12 passed, {failed} failed ===\n",
                    "b".repeat(40)
                ),
            )
            .unwrap();
            let when = std::time::SystemTime::now() - std::time::Duration::from_millis(age_ms);
            std::fs::File::options()
                .write(true)
                .open(&log)
                .unwrap()
                .set_modified(when)
                .unwrap();
            if ok {
                let ok_path = self.logs.join(format!("{tip}.ok"));
                std::fs::write(&ok_path, "2026-11-14T06:00:00-05:00\n").unwrap();
                std::fs::File::options()
                    .write(true)
                    .open(&ok_path)
                    .unwrap()
                    .set_modified(when)
                    .unwrap();
            }
        }
        fn gate_flag(&self, extra: &str) -> Gate {
            Gate::parse(&format!(
                "DSM={},{}{extra}",
                self.logs.display(),
                self.repo.display()
            ))
            .unwrap()
        }
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    const MIN: u64 = 60_000;

    #[test]
    fn the_flag_is_read_and_kept_as_given() {
        let g =
            Gate::parse("dsm=~/.local/state/dsm-gates,~/Documents/GitHub/Discoteque,running=90m")
                .unwrap();
        assert_eq!(g.prefix, "DSM");
        assert_eq!(g.running_ms, 90 * MIN);
        assert_eq!(g.logs_given, "~/.local/state/dsm-gates");
        assert!(!g.logs.starts_with("~"), "expanded for reading");
        assert_eq!(g.log_shown("abc"), "~/.local/state/dsm-gates/abc.log");
        assert!(g.covers("DSM-211") && !g.covers("RAL-1") && !g.covers("DSMX-1"));
        let default = Gate::parse("DSM=/a,/b").unwrap();
        assert_eq!(default.running_ms, DEFAULT_RUNNING_MS);
        assert!(Gate::parse("DSM=/a").is_err());
        assert!(Gate::parse("DSM=/a,/b,running=soon").is_err());
    }

    #[test]
    fn a_merge_in_a_landed_green_tip_is_green() {
        let f = Fixture::new();
        let car = f.commit("DSM-1: a car");
        let tip = f.commit("DSM-2: the train's tip");
        f.gate(&tip, 0, true, 30 * MIN);
        f.land(&tip);
        let v = verdict(&f.gate_flag(""), &car, now());
        assert_eq!(
            v,
            Verdict::Green {
                tip: tip.clone(),
                on_main: true,
                summary: "QUICK OK, 1 scenes, 0 failed".into()
            }
        );
    }

    /// The watchman's first condition: a failed train rebuilt with the same commit, or a stacked
    /// train gating on top, can't turn a landed, gated merge red.
    #[test]
    fn a_red_or_running_tip_beside_a_landed_green_one_doesnt_turn_it_red() {
        let f = Fixture::new();
        let car = f.commit("DSM-1: a car");
        let failed = f.commit("DSM-2: the first train, red");
        f.gate(&failed, 3, false, 90 * MIN);
        f.land(&car);
        run(&f.repo, &["reset", "-q", "--hard", &car]);
        let rebuilt = f.commit("DSM-2: rebuilt, green");
        f.gate(&rebuilt, 0, true, 40 * MIN);
        f.land(&rebuilt);
        let stacked = f.commit("DSM-3: a stacked train, gating now");
        f.gate(&stacked, 0, false, 5 * MIN);
        match verdict(&f.gate_flag(""), &car, now()) {
            Verdict::Green {
                tip, on_main: true, ..
            } => assert_eq!(tip, rebuilt),
            other => panic!("{other:?}"),
        }
    }

    /// The second condition: a green log for a tip that never landed vouches for nothing on main.
    #[test]
    fn a_green_tip_not_on_main_is_said_as_such() {
        let f = Fixture::new();
        let car = f.commit("DSM-1: a car");
        let tip = f.commit("DSM-2: gated, never landed");
        f.gate(&tip, 0, true, 10 * MIN);
        f.land(&car);
        match verdict(&f.gate_flag(""), &car, now()) {
            Verdict::Green {
                tip: t,
                on_main: false,
                ..
            } => assert_eq!(t, tip),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn no_green_tip_says_red_running_or_no_log() {
        let f = Fixture::new();
        let car = f.commit("DSM-1: a car");
        f.land(&car);
        let g = f.gate_flag(",running=60m");
        // No log at all.
        assert_eq!(verdict(&g, &car, now()), Verdict::NoLog { read: 0 });
        // A log with failures.
        let tip = f.commit("DSM-2: red");
        f.gate(&tip, 2, false, 10 * MIN);
        assert_eq!(
            verdict(&g, &car, now()),
            Verdict::Red {
                tip: tip.clone(),
                why: "2 failed".into()
            }
        );
        // A log still within the running limit, then past it.
        let next = f.commit("DSM-3: gating");
        f.gate(&next, 0, false, 10 * MIN);
        assert!(matches!(verdict(&g, &car, now()), Verdict::Running { tip, .. } if tip == next));
        // Past it, with the red log gone, so this one is the newest.
        std::fs::remove_file(f.logs.join(format!("{tip}.log"))).unwrap();
        f.gate(&next, 0, false, 61 * MIN);
        match verdict(&g, &car, now()) {
            Verdict::Red { tip, why } => {
                assert_eq!(tip, next);
                assert!(why.starts_with("no .ok 61 min"), "{why}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_commit_the_checkout_hasnt_fetched_cant_be_told() {
        let f = Fixture::new();
        let tip = f.commit("x");
        f.land(&tip);
        match verdict(&f.gate_flag(""), &"c".repeat(40), now()) {
            Verdict::CantTell(why) => assert!(why.contains("doesn't have ccccccc"), "{why}"),
            other => panic!("{other:?}"),
        }
    }
}
