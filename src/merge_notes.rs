//! What `rally merge` and a team's session start say beside a story (RAL-171): the conditions in
//! force that name it, where they bite rather than only in the brief, and, at a merge, what the
//! merged commit's CI says. Both inform; neither blocks anything.

use std::time::Duration;

use serde_json::{Value, json};

use crate::digest::Ci;

/// The conditions in force that name `story`, from `/v1/standings`, each as its writer declared
/// it: the note, who, and its record number. Holds and grants aren't conditions and aren't given.
pub fn conditions_naming(standings: &Value, story: &str) -> Vec<Value> {
    standings["standings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["standing"] == "condition")
        .filter(|row| {
            row["story"]
                .as_str()
                .is_some_and(|named| crate::ledger::same_name(named, story))
        })
        .map(|row| {
            json!({
                "seq": row["seq"], "note": row["note"], "by": row["declared_by"],
                "kind": "declared",
                "said": format!(
                    "a condition in force names this story: {} (declared, #{} by {})",
                    crate::session::clean(row["note"].as_str().unwrap_or("")),
                    row["seq"].as_u64().unwrap_or(0),
                    crate::session::clean(row["declared_by"].as_str().unwrap_or("someone"))
                ),
            })
        })
        .collect()
}

/// The conditions a merge of `story` prints (RAL-175): those naming it, then those in force that
/// name no story at all, such as a hold on the order of every merge, which bear on each merge.
/// The second are said as naming no story; neither blocks the merge.
pub fn conditions_at_merge(standings: &Value, story: &str) -> Vec<Value> {
    let mut out = conditions_naming(standings, story);
    out.extend(
        standings["standings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| row["standing"] == "condition")
            .filter(|row| {
                row["story"]
                    .as_str()
                    .is_none_or(|named| named.trim().is_empty())
            })
            .map(|row| {
                json!({
                    "seq": row["seq"], "note": row["note"], "by": row["declared_by"],
                    "kind": "declared", "story": null,
                    "said": format!(
                        "a condition in force names no story, so it bears on every merge: {} \
                         (declared, #{} by {})",
                        crate::session::clean(row["note"].as_str().unwrap_or("")),
                        row["seq"].as_u64().unwrap_or(0),
                        crate::session::clean(row["declared_by"].as_str().unwrap_or("someone"))
                    ),
                })
            }),
    );
    out
}

/// How often `rally merge` reads again while no run has appeared (RAL-178).
pub const CI_POLL: Duration = Duration::from_secs(2);

/// Reads the merged commit's CI, and while no run has appeared reads again every `every`, for at
/// most `wait` in all (RAL-178): a run is usually started a few seconds after the merge's push.
/// Anything other than no run (passed, failed, running, or a read that failed) is given at once.
pub fn ci_waiting(
    read: impl Fn() -> Result<Ci, String>,
    wait: Duration,
    every: Duration,
    sleep: impl Fn(Duration),
) -> Result<Ci, String> {
    let mut waited = Duration::ZERO;
    loop {
        let got = read();
        if !matches!(got, Ok(Ci::NoRun)) || waited >= wait || every.is_zero() {
            return got;
        }
        let step = every.min(wait - waited);
        sleep(step);
        waited += step;
    }
}

/// What the merged commit's CI says, read through `gh` at the merge, observed (RAL-171). Still
/// running isn't a verdict: the digest's RAL-102 section checks it again.
pub fn ci_note(read: Result<Ci, String>) -> Value {
    let (state, said) = match &read {
        Ok(Ci::Passed) => ("passed", "every run on the merged commit passed".to_owned()),
        Ok(Ci::Failed { workflow, url }) => (
            "failed",
            format!(
                "{} failed on the merged commit: {}",
                crate::session::clean(workflow),
                crate::session::clean(url)
            ),
        ),
        Ok(Ci::Running) => (
            "running",
            "running; the digest's RAL-102 section checks again".to_owned(),
        ),
        Ok(Ci::NoRun) => (
            "not_started",
            "not started yet; the digest's RAL-102 section checks it".to_owned(),
        ),
        Err(error) => (
            "unread",
            format!("couldn't be read: {}", crate::session::clean(error)),
        ),
    };
    json!({"kind": "observed", "read_with": "gh, at the merge", "state": state, "said": said,
           "recorded": false})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_conditions_naming_the_story_are_given() {
        let standings = json!({"standings": [
            {"seq": 7, "standing": "condition", "story": "RAL-160", "note": "merges next",
             "declared_by": "mellow-weevil"},
            {"seq": 8, "standing": "hold", "story": "RAL-160", "note": "a hold",
             "declared_by": "mellow-weevil"},
            {"seq": 9, "standing": "condition", "story": "RAL-161", "note": "another story",
             "declared_by": "mellow-weevil"},
            {"seq": 10, "standing": "condition", "note": "no story", "declared_by": "w"},
        ]});
        let named = conditions_naming(&standings, "ral-160");
        assert_eq!(named.len(), 1, "{named:?}");
        assert_eq!(
            named[0]["said"],
            "a condition in force names this story: merges next (declared, #7 by mellow-weevil)"
        );
        assert!(conditions_naming(&json!({}), "RAL-160").is_empty());
    }

    #[test]
    fn a_condition_naming_no_story_is_given_at_every_merge() {
        let standings = json!({"standings": [
            {"seq": 7, "standing": "condition", "story": "RAL-160", "note": "merges next",
             "declared_by": "mellow-weevil"},
            {"seq": 9, "standing": "condition", "story": "RAL-161", "note": "another story",
             "declared_by": "mellow-weevil"},
            {"seq": 10, "standing": "condition", "note": "one merge at a time",
             "declared_by": "mellow-weevil"},
            {"seq": 11, "standing": "hold", "note": "a hold, not a condition", "declared_by": "w"},
        ]});
        let at = conditions_at_merge(&standings, "RAL-160");
        assert_eq!(at.len(), 2, "{at:?}");
        assert_eq!(at[0]["seq"], 7);
        assert_eq!(
            at[1]["said"],
            "a condition in force names no story, so it bears on every merge: one merge at a \
             time (declared, #10 by mellow-weevil)"
        );
        let other = conditions_at_merge(&standings, "RAL-999");
        assert_eq!(other.len(), 1, "{other:?}");
        assert_eq!(other[0]["seq"], 10);
        // A team's session start still lists only what names its stories.
        assert!(conditions_naming(&standings, "RAL-999").is_empty());
    }

    #[test]
    fn a_merge_waits_a_bounded_while_for_a_run_to_appear() {
        use std::cell::{Cell, RefCell};
        let reads = Cell::new(0);
        let slept = RefCell::new(Vec::new());
        // A run appears on the third read: it's given, after two waits.
        let got = ci_waiting(
            || {
                reads.set(reads.get() + 1);
                Ok(if reads.get() < 3 {
                    Ci::NoRun
                } else {
                    Ci::Running
                })
            },
            Duration::from_secs(8),
            CI_POLL,
            |d| slept.borrow_mut().push(d),
        );
        assert_eq!(got, Ok(Ci::Running));
        assert_eq!(*slept.borrow(), [CI_POLL, CI_POLL]);
        // None ever appears: it stops at the bound, never past it, and says not started.
        reads.set(0);
        slept.borrow_mut().clear();
        let got = ci_waiting(
            || {
                reads.set(reads.get() + 1);
                Ok(Ci::NoRun)
            },
            Duration::from_secs(5),
            CI_POLL,
            |d| slept.borrow_mut().push(d),
        );
        assert_eq!(got, Ok(Ci::NoRun));
        assert_eq!(
            slept.borrow().iter().sum::<Duration>(),
            Duration::from_secs(5)
        );
        assert_eq!(reads.get(), 4);
        assert_eq!(ci_note(got)["state"], "not_started");
        assert_eq!(
            ci_note(Ok(Ci::NoRun))["said"],
            "not started yet; the digest's RAL-102 section checks it"
        );
        // A failed read isn't waited on, and no wait reads once.
        reads.set(0);
        let failed = ci_waiting(
            || {
                reads.set(reads.get() + 1);
                Err("gh: offline".to_owned())
            },
            Duration::from_secs(8),
            CI_POLL,
            |_| panic!("no wait after a failed read"),
        );
        assert!(failed.is_err());
        assert_eq!(reads.get(), 1);
        let once = ci_waiting(
            || Ok(Ci::NoRun),
            Duration::ZERO,
            CI_POLL,
            |_| panic!("no wait"),
        );
        assert_eq!(once, Ok(Ci::NoRun));
    }

    #[test]
    fn a_running_ci_is_no_verdict() {
        assert_eq!(ci_note(Ok(Ci::Passed))["state"], "passed");
        let running = ci_note(Ok(Ci::Running));
        assert_eq!(
            running["said"],
            "running; the digest's RAL-102 section checks again"
        );
        assert_eq!(running["kind"], "observed");
        assert_eq!(running["recorded"], false);
        let failed = ci_note(Ok(Ci::Failed {
            workflow: "CI".into(),
            url: "https://github.com/x/y/actions/runs/1".into(),
        }));
        assert_eq!(failed["state"], "failed");
        assert!(failed["said"].as_str().unwrap().starts_with("CI failed"));
        assert_eq!(ci_note(Err("gh: offline".into()))["state"], "unread");
    }
}
