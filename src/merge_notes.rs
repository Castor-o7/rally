//! What `rally merge` and a team's session start say beside a story (RAL-171): the conditions in
//! force that name it, where they bite rather than only in the brief, and, at a merge, what the
//! merged commit's CI says. Both inform; neither blocks anything.

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
        Ok(Ci::NoRun) => ("none", "no run on the merged commit yet".to_owned()),
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
