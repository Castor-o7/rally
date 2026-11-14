//! `rally wait-green <PR or sha>` (RAL-185): wait until every check on a pinned head has finished,
//! and say whether each passed. On 2026-11-14 a team's home-made waiter, a jq loop, never exited,
//! so for a whole night green pull requests sat unsubmitted while the merge queue waited on them.
//! This is the one waiter every team can use instead.
//!
//! Observed, recording nothing. Which checks to wait for is read, not assumed: every check that ran
//! on the base branch's tip (GitHub's check runs and commit statuses, such as ci-local's
//! `local/check`), unless `--check` names them. So a head whose GitHub checks have finished but
//! whose local statuses haven't been posted yet is still waiting, not green. A pull request's head
//! is pinned when the wait starts: if the pull request moves to another commit, the wait ends and
//! says so, since a green on the old head says nothing about the new one.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Value, json};

/// One check on a commit, the newest of its name: a check run or a commit status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    /// `pending`, `success`, or what else it ended as (`failure`, `cancelled`, `timed_out`, ...).
    pub state: String,
    pub url: Option<String>,
}

/// The checks on a commit, from `GET /repos/{repo}/commits/{sha}/check-runs` and `.../status`.
/// A check run re-run under the same name is given once, its newest run.
pub fn checks(check_runs: &Value, status: &Value) -> Vec<Check> {
    let mut by_name: BTreeMap<String, (u64, Check)> = BTreeMap::new();
    for run in check_runs["check_runs"].as_array().into_iter().flatten() {
        let Some(name) = run["name"].as_str() else {
            continue;
        };
        let id = run["id"].as_u64().unwrap_or(0);
        let state = if run["status"] == "completed" {
            run["conclusion"].as_str().unwrap_or("unknown").to_owned()
        } else {
            "pending".to_owned()
        };
        let check = Check {
            name: name.to_owned(),
            state,
            url: run["html_url"].as_str().map(str::to_owned),
        };
        match by_name.get(name) {
            Some((newer, _)) if *newer > id => {}
            _ => {
                by_name.insert(name.to_owned(), (id, check));
            }
        }
    }
    // GitHub's combined status already gives the newest status of each context.
    for status in status["statuses"].as_array().into_iter().flatten() {
        let Some(name) = status["context"].as_str() else {
            continue;
        };
        let state = match status["state"].as_str() {
            Some("success") => "success",
            Some("pending") => "pending",
            Some("error") => "error",
            _ => "failure",
        };
        by_name.insert(
            name.to_owned(),
            (
                u64::MAX,
                Check {
                    name: name.to_owned(),
                    state: state.to_owned(),
                    url: status["target_url"].as_str().map(str::to_owned),
                },
            ),
        );
    }
    by_name.into_values().map(|(_, check)| check).collect()
}

/// Whether a finished check passed: a success, or a check GitHub skipped or found neutral.
fn passed(state: &str) -> bool {
    matches!(state, "success" | "neutral" | "skipped")
}

/// Where the checks on a head stand against the ones expected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Every expected check is there and finished, and every check there passed.
    Green,
    /// A check finished and didn't pass: the first such is enough to end the wait.
    Red(Vec<Check>),
    /// Still running, or an expected check hasn't appeared yet: what's pending and what's missing.
    Waiting {
        pending: Vec<String>,
        missing: Vec<String>,
    },
}

/// Judges `checks` against `expected` (names). With nothing expected, at least one check must be
/// there, so a head nothing has started on yet isn't called green.
pub fn judge(checks: &[Check], expected: &[String]) -> Standing {
    let red: Vec<Check> = checks
        .iter()
        .filter(|check| check.state != "pending" && !passed(&check.state))
        .cloned()
        .collect();
    if !red.is_empty() {
        return Standing::Red(red);
    }
    let pending: Vec<String> = checks
        .iter()
        .filter(|check| check.state == "pending")
        .map(|check| check.name.clone())
        .collect();
    let mut missing: Vec<String> = expected
        .iter()
        .filter(|name| !checks.iter().any(|check| &check.name == *name))
        .cloned()
        .collect();
    if checks.is_empty() && missing.is_empty() {
        missing.push("any check".to_owned());
    }
    if pending.is_empty() && missing.is_empty() {
        Standing::Green
    } else {
        Standing::Waiting { pending, missing }
    }
}

/// How a wait ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    Green,
    Red,
    /// The pull request's head moved off the pinned commit: to this one.
    Moved(String),
    TimedOut,
}

impl Ended {
    /// The process's exit code: 0 green, 1 red, 3 moved, 4 timed out (2 is a usage error).
    pub fn code(&self) -> u8 {
        match self {
            Self::Green => 0,
            Self::Red => 1,
            Self::Moved(_) => 3,
            Self::TimedOut => 4,
        }
    }

    fn word(&self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Red => "red",
            Self::Moved(_) => "moved",
            Self::TimedOut => "timed_out",
        }
    }
}

/// What the wait reads, given so a test can stand in for GitHub.
pub struct Reads<'a> {
    /// The pull request's head now, for a wait on a pull request; `None` for a commit.
    pub head_now: Option<&'a dyn Fn() -> Result<String, String>>,
    /// The checks on the pinned head.
    pub checks: &'a dyn Fn() -> Result<Vec<Check>, String>,
    pub sleep: &'a dyn Fn(Duration),
}

/// Waits on `head` until its checks are green or red, the pull request moves off it, or `timeout`
/// has passed, reading every `every`. A read that fails is tried again until the timeout, and the
/// last such failure is said. Gives how it ended and the JSON to print.
pub fn wait(
    head: &str,
    expected: &[String],
    reads: &Reads,
    timeout: Duration,
    every: Duration,
) -> (Ended, Value) {
    let mut waited = Duration::ZERO;
    let mut last_error: Option<String> = None;
    // A pull request's head that couldn't be read: the checks on the pinned head still count, but
    // a move can't be seen meanwhile, so it's said.
    let mut head_error: Option<String> = None;
    let mut last: Vec<Check> = Vec::new();
    let mut last_standing = Standing::Waiting {
        pending: Vec::new(),
        missing: Vec::new(),
    };
    let ended = loop {
        if let Some(head_now) = reads.head_now {
            match head_now() {
                Ok(now) if now != head => break Ended::Moved(now),
                Ok(_) => head_error = None,
                Err(error) => head_error = Some(error),
            }
        }
        match (reads.checks)() {
            Ok(checks) => {
                last_error = None;
                last_standing = judge(&checks, expected);
                last = checks;
                match last_standing {
                    Standing::Green => break Ended::Green,
                    Standing::Red(_) => break Ended::Red,
                    Standing::Waiting { .. } => {}
                }
            }
            Err(error) => last_error = Some(error),
        }
        if waited >= timeout {
            break Ended::TimedOut;
        }
        let step = every.min(timeout - waited).max(Duration::from_millis(1));
        (reads.sleep)(step);
        waited += step;
    };
    let mut out = json!({
        "kind": "observed",
        "recorded": false,
        "verdict": ended.word(),
        "head": head,
        "waited_s": waited.as_secs(),
        "checks": last.iter().map(|check| json!({
            "name": check.name, "state": check.state, "url": check.url,
        })).collect::<Vec<_>>(),
    });
    match &ended {
        Ended::Green => {
            out["said"] = json!(format!(
                "every check passed at {head}: {} of them",
                last.len()
            ));
        }
        Ended::Red => {
            let red: Vec<String> = match &last_standing {
                Standing::Red(red) => red
                    .iter()
                    .map(|check| format!("{} {}", check.name, check.state))
                    .collect(),
                _ => Vec::new(),
            };
            out["said"] = json!(format!("not green at {head}: {}", red.join(", ")));
        }
        Ended::Moved(now) => {
            out["moved_to"] = json!(now);
            out["said"] = json!(format!(
                "the pull request's head moved from {head} to {now}: a green on the old head says \
                 nothing about the new one, so wait again on it"
            ));
        }
        Ended::TimedOut => {
            let (pending, missing) = match &last_standing {
                Standing::Waiting { pending, missing } => (pending.clone(), missing.clone()),
                _ => (Vec::new(), Vec::new()),
            };
            out["pending"] = json!(pending);
            out["missing"] = json!(missing);
            out["said"] = json!(format!(
                "still not finished at {head} after {} s: pending {}; not yet there {}",
                waited.as_secs(),
                list(&pending),
                list(&missing)
            ));
        }
    }
    if let Some(error) = last_error {
        out["last_read_failed"] = json!(error);
    }
    if let Some(error) = head_error {
        out["head_read_failed"] = json!(error);
    }
    (ended, out)
}

fn list(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn run(name: &str, id: u64, status: &str, conclusion: Option<&str>) -> Value {
        json!({"name": name, "id": id, "status": status, "conclusion": conclusion,
               "html_url": format!("https://github.com/x/y/runs/{id}")})
    }

    #[test]
    fn checks_take_the_newest_run_of_a_name_and_every_status() {
        let runs = json!({"check_runs": [
            run("check", 1, "completed", Some("failure")),
            run("check", 2, "completed", Some("success")),
            run("dependency-audit", 3, "in_progress", None),
        ]});
        let status = json!({"statuses": [
            {"context": "local/check", "state": "pending", "target_url": null},
            {"context": "local/ci-config", "state": "error"},
        ]});
        let got = checks(&runs, &status);
        let by = |name: &str| got.iter().find(|c| c.name == name).unwrap().state.clone();
        assert_eq!(got.len(), 4);
        assert_eq!(by("check"), "success", "the re-run is the one that counts");
        assert_eq!(by("dependency-audit"), "pending");
        assert_eq!(by("local/check"), "pending");
        assert_eq!(by("local/ci-config"), "error");
    }

    #[test]
    fn green_needs_every_expected_check_there_and_passed() {
        let c = |name: &str, state: &str| Check {
            name: name.into(),
            state: state.into(),
            url: None,
        };
        let expected = vec!["check".to_owned(), "local/check".to_owned()];
        // GitHub's checks finished, the local status not yet posted: waiting, not green.
        assert_eq!(
            judge(&[c("check", "success")], &expected),
            Standing::Waiting {
                pending: vec![],
                missing: vec!["local/check".into()]
            }
        );
        assert_eq!(
            judge(
                &[c("check", "success"), c("local/check", "pending")],
                &expected
            ),
            Standing::Waiting {
                pending: vec!["local/check".into()],
                missing: vec![]
            }
        );
        assert_eq!(
            judge(
                &[c("check", "success"), c("local/check", "success")],
                &expected
            ),
            Standing::Green
        );
        // A red ends it at once, even with another still running.
        assert!(matches!(
            judge(&[c("check", "failure"), c("local/check", "pending")], &expected),
            Standing::Red(red) if red.len() == 1 && red[0].name == "check"
        ));
        // Skipped and neutral pass; nothing at all isn't green.
        assert_eq!(judge(&[c("lint", "skipped")], &[]), Standing::Green);
        assert!(
            matches!(judge(&[], &[]), Standing::Waiting { missing, .. } if missing == ["any check"])
        );
    }

    // Three stand-in reads, each a closure over the test's own state.
    #[allow(clippy::type_complexity)]
    fn reads_from<'a>(
        steps: &'a RefCell<Vec<Vec<Check>>>,
        heads: &'a RefCell<Vec<String>>,
        slept: &'a Cell<u32>,
    ) -> (
        impl Fn() -> Result<Vec<Check>, String> + 'a,
        impl Fn() -> Result<String, String> + 'a,
        impl Fn(Duration) + 'a,
    ) {
        let checks = move || {
            let mut steps = steps.borrow_mut();
            Ok(if steps.len() > 1 {
                steps.remove(0)
            } else {
                steps[0].clone()
            })
        };
        let head = move || {
            let mut heads = heads.borrow_mut();
            Ok(if heads.len() > 1 {
                heads.remove(0)
            } else {
                heads[0].clone()
            })
        };
        let sleep = move |_: Duration| slept.set(slept.get() + 1);
        (checks, head, sleep)
    }

    #[test]
    fn a_wait_ends_green_red_moved_or_timed_out() {
        let c = |state: &str| {
            vec![Check {
                name: "check".into(),
                state: state.into(),
                url: None,
            }]
        };
        let expected = vec!["check".to_owned()];
        let every = Duration::from_secs(30);
        let timeout = Duration::from_secs(600);
        let h = "a".repeat(40);

        let steps = RefCell::new(vec![c("pending"), c("pending"), c("success")]);
        let heads = RefCell::new(vec![h.clone()]);
        let slept = Cell::new(0);
        let (checks, head, sleep) = reads_from(&steps, &heads, &slept);
        let reads = Reads {
            head_now: Some(&head),
            checks: &checks,
            sleep: &sleep,
        };
        let (ended, out) = wait(&h, &expected, &reads, timeout, every);
        assert_eq!((ended.code(), slept.get()), (0, 2), "{out}");
        assert_eq!(out["verdict"], "green");

        let steps = RefCell::new(vec![c("pending"), c("failure")]);
        let slept = Cell::new(0);
        let (checks, head, sleep) = reads_from(&steps, &heads, &slept);
        let reads = Reads {
            head_now: Some(&head),
            checks: &checks,
            sleep: &sleep,
        };
        let (ended, out) = wait(&h, &expected, &reads, timeout, every);
        assert_eq!(ended.code(), 1, "{out}");
        assert_eq!(out["said"], format!("not green at {h}: check failure"));

        // The pull request is pushed to while it waits: a green afterwards would be another head's.
        let steps = RefCell::new(vec![c("pending")]);
        let heads = RefCell::new(vec![h.clone(), "b".repeat(40)]);
        let slept = Cell::new(0);
        let (checks, head, sleep) = reads_from(&steps, &heads, &slept);
        let reads = Reads {
            head_now: Some(&head),
            checks: &checks,
            sleep: &sleep,
        };
        let (ended, out) = wait(&h, &expected, &reads, timeout, every);
        assert_eq!(ended, Ended::Moved("b".repeat(40)));
        assert_eq!(ended.code(), 3);
        assert_eq!(out["moved_to"], "b".repeat(40));

        // Never finishes: it stops at the timeout, never past it, and says what was outstanding.
        let steps = RefCell::new(vec![c("pending")]);
        let heads = RefCell::new(vec![h.clone()]);
        let slept = Cell::new(0);
        let (checks, head, sleep) = reads_from(&steps, &heads, &slept);
        let reads = Reads {
            head_now: Some(&head),
            checks: &checks,
            sleep: &sleep,
        };
        let (ended, out) = wait(&h, &expected, &reads, Duration::from_secs(90), every);
        assert_eq!((ended.code(), slept.get()), (4, 3), "{out}");
        assert_eq!(out["waited_s"], 90);
        assert_eq!(out["pending"], json!(["check"]));
    }

    #[test]
    fn a_failed_read_is_tried_again_and_said_if_it_never_recovers() {
        let h = "a".repeat(40);
        let tries = Cell::new(0);
        let checks = || {
            tries.set(tries.get() + 1);
            if tries.get() < 3 {
                Err("gh: offline".to_owned())
            } else {
                Ok(vec![Check {
                    name: "check".into(),
                    state: "success".into(),
                    url: None,
                }])
            }
        };
        let sleep = |_: Duration| {};
        let reads = Reads {
            head_now: None,
            checks: &checks,
            sleep: &sleep,
        };
        let (ended, out) = wait(
            &h,
            &[],
            &reads,
            Duration::from_secs(600),
            Duration::from_secs(30),
        );
        assert_eq!(ended, Ended::Green, "{out}");
        assert!(out.get("last_read_failed").is_none());

        let never = || Err::<Vec<Check>, _>("gh: offline".to_owned());
        let reads = Reads {
            head_now: None,
            checks: &never,
            sleep: &sleep,
        };
        let (ended, out) = wait(
            &h,
            &[],
            &reads,
            Duration::from_secs(60),
            Duration::from_secs(30),
        );
        assert_eq!(ended, Ended::TimedOut);
        assert_eq!(out["last_read_failed"], "gh: offline");
    }
}
