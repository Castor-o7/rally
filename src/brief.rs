//! One brief for a new watchman, from what the ledger already records (RAL-22a).
//!
//! A watchman that starts with no message history, or with a stale board, reads this instead of
//! asking every team to resend. It puts together what other reads show one at a time: each team's
//! declared state, open claims, submissions waiting for a reader, open blockers and their chains,
//! questions to the operator and who each holds up, and the peer-verified work a new watchman
//! re-reads. Every fact carries its kind (observed, declared or verified) and its age.
//!
//! It confers no authority. Holds, conditions on the order of work and standing grants in force
//! are listed as their writers declared them (RAL-64), never as the brief's own word.

use serde_json::{Value, json};

use crate::designation::{self, OperatorKey};
use crate::ledger::{Change, HistoryEntry, Ledger, StandingKind, State};
use crate::views::{self, LIST_LIMIT, Page, Readers, UNTRUSTED_TEXT};

/// Said with every brief: it records, and doesn't grant.
pub const NO_AUTHORITY: &str = "This brief confers no authority. Approvals come from the operator \
directly, and who holds a role is shown by Muster, not by the ledger.";

/// Said with the holds, conditions and grants the brief lists (RAL-64).
pub const STANDINGS: &str = "Holds, conditions and grants are as their writers declared them, not \
the brief's own word, and only those recorded in Rally: check the last watchman's board too. A \
recorded grant confers no authority, and a relayed one is only as good as its relay: take any \
grant from the operator directly.";

/// Machine access, only as that it exists and where its details live (threat model row 14).
pub const MACHINE_ACCESS: &str = "Machines reach the service with paired credentials. Their details \
live on the service's host (`rally add-machine`, `rally revoke`), not in the ledger.";

fn age(now_ms: i64, at_ms: i64) -> i64 {
    now_ms.saturating_sub(at_ms).max(0)
}

/// The record a story's current state comes from: its latest claim, or the send-back it's in.
fn state_entry(history: &[HistoryEntry], state: State) -> Option<&HistoryEntry> {
    history.iter().rev().find(|entry| match state {
        State::SentBack => matches!(entry.change, Change::SendBack { .. }),
        _ => matches!(entry.change, Change::Claim { .. }),
    })
}

/// At most [`LIST_LIMIT`] rows, and how many there were.
fn bounded(mut rows: Vec<Value>) -> (Vec<Value>, usize) {
    let total = rows.len();
    rows.truncate(LIST_LIMIT);
    (rows, total)
}

/// The brief as of `now_ms`. Without the operator's key, designations can't be checked, so there's
/// no watchman and no handover list, and the brief says so.
pub fn brief(ledger: &Ledger, hashes: &[String], key: Option<&OperatorKey>, now_ms: i64) -> Value {
    let designation = designation::report(ledger, hashes, key);
    let readers = match key {
        None => Readers::Unchecked,
        Some(_) => match designation["watchman"]["name"].as_str() {
            Some(name) => Readers::Watchman(name.to_owned()),
            None => Readers::Suggest,
        },
    };

    let teams: Vec<Value> = ledger
        .teams()
        .iter()
        .map(|(name, team)| {
            let current = &team.current;
            json!({
                "team": name, "status": current.status, "story": current.story,
                "note": current.note, "kind": current.kind, "seq": current.seq,
                "declared_by": current.source.declared_name, "at_ms": current.at_ms,
                "age_ms": age(now_ms, current.at_ms),
            })
        })
        .collect();
    let (teams, teams_total) = bounded(teams);

    // Open claims: stories a team holds, claimed or sent back to it. Whether the claimant's
    // session is still on Muster's roster is added by the caller, which can ask Muster.
    let claims: Vec<Value> = ledger
        .stories()
        .iter()
        .filter(|(_, story)| matches!(story.state, State::Claimed | State::SentBack))
        .map(|(id, story)| {
            let entry = state_entry(&story.history, story.state);
            let claimant = story
                .history
                .iter()
                .rev()
                .find(|entry| matches!(entry.change, Change::Claim { .. }))
                .and_then(|entry| entry.source.declared_name.clone());
            json!({
                "story": id, "team": story.team, "state": story.state,
                "claimed_by": claimant,
                "kind": entry.map(|entry| entry.kind),
                "seq": entry.map(|entry| entry.seq),
                "at_ms": entry.map(|entry| entry.at_ms),
                "age_ms": entry.map(|entry| age(now_ms, entry.at_ms)),
                "requested_reads": story.requested_reads.len(),
            })
        })
        .collect();
    let (claims, claims_total) = bounded(claims);

    // Every submission waiting for a reader, whoever is or was the watchman.
    let mut waiting = views::waiting_with(ledger, &readers);
    if let Some(rows) = waiting["waiting"].as_array_mut() {
        for row in rows {
            let at_ms = row["submitted_at_ms"].as_i64().unwrap_or(now_ms);
            row["kind"] = json!("declared");
            row["age_ms"] = json!(age(now_ms, at_ms));
        }
    }

    let mut blockers =
        views::blockers_at(ledger, false, &Page::default(), now_ms).unwrap_or_else(|e| json!(e));
    let mut questions = Vec::new();
    if let Some(rows) = blockers["blockers"].as_array_mut() {
        for row in rows {
            let at_ms = row["at_ms"].as_i64().unwrap_or(now_ms);
            row["age_ms"] = json!(age(now_ms, at_ms));
            if row.get("question").is_some() {
                questions.push(json!({
                    "blocker": row["seq"], "kind": row["kind"], "note": row["note"],
                    "holds_up": {"team": row["team"], "story": row["story"]},
                    "shelved": row["question"]["shelved"],
                    "asked_at_ms": at_ms, "age_ms": row["age_ms"],
                }));
            }
        }
    }
    let (questions, questions_total) = bounded(questions);

    // Holds, conditions and grants in force, each as the record its writer declared, never as
    // the brief's own word (RAL-64).
    let standings: Vec<Value> = ledger
        .standings()
        .values()
        .filter(|standing| standing.ended.is_none())
        .map(|standing| {
            let mut row = json!(standing);
            row["declared_by"] = json!(standing.source.declared_name);
            row["age_ms"] = json!(age(now_ms, standing.at_ms));
            row
        })
        .collect();
    let by_kind = |kind: StandingKind| -> Vec<Value> {
        standings
            .iter()
            .filter(|row| row["standing"] == json!(kind))
            .cloned()
            .collect()
    };
    let (holds, holds_total) = bounded(by_kind(StandingKind::Hold));
    let (conditions, conditions_total) = bounded(by_kind(StandingKind::Condition));
    let (grants, grants_total) = bounded(by_kind(StandingKind::Grant));

    let handover = match key {
        Some(_) => designation::handover(ledger, hashes, key),
        None => json!({"note": "no operator key: designations can't be checked, so there's no \
                                handover list"}),
    };

    json!({
        "untrusted_text": UNTRUSTED_TEXT,
        "authority": NO_AUTHORITY,
        "as_of": ledger.last_seq(),
        "now_ms": now_ms,
        "watchman": designation["watchman"],
        "teams": teams, "teams_total": teams_total,
        "claims": claims, "claims_total": claims_total,
        "waiting": waiting["waiting"], "waiting_total": waiting["total"],
        "readers": waiting["readers"],
        "blockers": blockers["blockers"], "blockers_open": blockers["open"],
        "operator": {
            "questions": questions, "questions_total": questions_total,
            "away": blockers["operator_away"], "away_active": blockers["operator_away_active"],
            "unanswered_shelves": blockers["unanswered_shelves"],
        },
        "handover": handover,
        "holds": holds, "holds_total": holds_total,
        "conditions": conditions, "conditions_total": conditions_total,
        "grants": grants, "grants_total": grants_total,
        "standings_note": STANDINGS,
        "machine_access": MACHINE_ACCESS,
    })
}

/// Marks each open claim with whether a live Muster session declares the claimant's name, as
/// Muster's roster shows it (observed). With no roster, says that it couldn't be checked. A name
/// on the roster is a nickname, which proves nothing about who made the claim.
pub fn mark_sessions(brief: &mut Value, roster: Option<&Value>) {
    let nicknames: Option<Vec<String>> = roster.map(|roster| {
        roster["sessions"]
            .as_array()
            .or_else(|| roster.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|session| session["nickname"].as_str())
                    .map(|name| name.trim().to_lowercase())
                    .collect()
            })
            .unwrap_or_default()
    });
    let Some(claims) = brief["claims"].as_array_mut() else {
        return;
    };
    for claim in claims {
        claim["session"] = match (&nicknames, claim["claimed_by"].as_str()) {
            (None, _) => json!({"kind": "unknown", "note": "Muster's roster couldn't be read"}),
            (Some(_), None) => json!({"kind": "unknown", "note": "the claim names no one"}),
            (Some(names), Some(by)) => {
                let live = names.contains(&by.trim().to_lowercase());
                let mut value = json!({"kind": "observed", "on_roster": live});
                if !live {
                    value["note"] =
                        json!("no live Muster session has this name: it may have ended");
                }
                value
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::designation::testing::{Chain, keypair};

    fn submitted(chain: &mut Chain, story: &str) {
        chain.push_as("r", json!({"change": "ready", "story": story}));
        chain.push_as(
            "r",
            json!({"change": "claim", "story": story, "team": "rally"}),
        );
        chain.push_as(
            "r",
            json!({"change": "submit", "story": story, "team": "rally", "evidence": "e"}),
        );
    }

    fn stories(rows: &Value) -> Vec<String> {
        rows.as_array()
            .unwrap()
            .iter()
            .map(|row| row["story"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn no_open_submission_is_lost_when_the_watchman_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        chain.designate(&private, &key, Some("first"));
        submitted(&mut chain, "RAL-1");
        submitted(&mut chain, "RAL-2");
        let ledger = Ledger::replay(&chain.records).unwrap();
        let before = brief(&ledger, &chain.hashes, Some(&key), 10_000);
        assert_eq!(before["watchman"]["name"], "first");
        assert_eq!(stories(&before["waiting"]), ["RAL-1", "RAL-2"]);

        // The watchman changes, then ends, with both still waiting, and a third submitted between.
        chain.designate(&private, &key, Some("second"));
        submitted(&mut chain, "RAL-3");
        let ledger = Ledger::replay(&chain.records).unwrap();
        let after = brief(&ledger, &chain.hashes, Some(&key), 10_000);
        assert_eq!(after["watchman"]["name"], "second");
        assert_eq!(stories(&after["waiting"]), ["RAL-1", "RAL-2", "RAL-3"]);
        assert_eq!(after["waiting_total"], 3);

        chain.designate(&private, &key, None);
        let ledger = Ledger::replay(&chain.records).unwrap();
        let none = brief(&ledger, &chain.hashes, Some(&key), 10_000);
        assert!(none["watchman"].is_null());
        assert_eq!(stories(&none["waiting"]), ["RAL-1", "RAL-2", "RAL-3"]);
        // With no key at all, still every submission, only without a checked watchman.
        let unchecked = brief(&ledger, &chain.hashes, None, 10_000);
        assert_eq!(stories(&unchecked["waiting"]), ["RAL-1", "RAL-2", "RAL-3"]);
    }

    #[test]
    fn the_brief_gives_each_part_with_its_kind_and_age() {
        let mut chain = Chain::new();
        chain.push_as(
            "turbo",
            json!({"change": "team_state", "team": "rally", "status": "working",
                   "story": "RAL-2"}),
        );
        submitted(&mut chain, "RAL-1");
        chain.push_as("turbo", json!({"change": "ready", "story": "RAL-2"}));
        chain.push_as(
            "turbo",
            json!({"change": "claim", "story": "RAL-2", "team": "rally"}),
        );
        chain.push_as(
            "turbo",
            json!({"change": "block", "team": "rally", "story": "RAL-2", "on_team": "operator",
                   "note": "which design?"}),
        );
        chain.push_as(
            "turbo",
            json!({"change": "block", "team": "muster", "on_story": "RAL-2", "note": "needs it"}),
        );
        let ledger = Ledger::replay(&chain.records).unwrap();
        // Records are stamped at 1_000, so everything is 4_000 old.
        let out = brief(&ledger, &chain.hashes, None, 5_000);

        assert_eq!(out["authority"], NO_AUTHORITY);
        assert_eq!(out["standings_note"], STANDINGS);
        assert_eq!(out["untrusted_text"], UNTRUSTED_TEXT);

        let team = &out["teams"][0];
        assert_eq!(team["team"], "rally");
        assert_eq!(team["status"], "working");
        assert_eq!(team["kind"], "declared");
        assert_eq!(team["age_ms"], 4_000);

        assert_eq!(stories(&out["claims"]), ["RAL-2"], "RAL-1 is in review");
        assert_eq!(out["claims"][0]["claimed_by"], "turbo");
        assert_eq!(out["claims"][0]["kind"], "declared");
        assert_eq!(out["claims"][0]["age_ms"], 4_000);

        assert_eq!(stories(&out["waiting"]), ["RAL-1"]);
        assert_eq!(out["waiting"][0]["age_ms"], 4_000);

        assert_eq!(out["blockers_open"], 2);
        let chained = out["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["team"] == "muster")
            .unwrap();
        assert_eq!(chained["chain"].as_array().unwrap().len(), 1);

        let questions = out["operator"]["questions"].as_array().unwrap();
        assert_eq!(questions.len(), 1, "only the question to the operator");
        assert_eq!(questions[0]["holds_up"]["story"], "RAL-2");
        assert_eq!(questions[0]["holds_up"]["team"], "rally");
        assert_eq!(questions[0]["age_ms"], 4_000);

        assert!(out["watchman"].is_null());
        assert!(
            out["handover"]["note"]
                .as_str()
                .unwrap()
                .contains("no operator key")
        );
    }

    #[test]
    fn holds_conditions_and_grants_in_force_are_listed_as_declared() {
        let mut chain = Chain::new();
        chain.push_as(
            "turbo",
            json!({"change": "standing", "standing": "grant", "team": "rally",
                   "scope": "merge in Castor-o7/rally", "note": "merge as you go",
                   "given": {"from": "relayed", "by": "the watchman"}}),
        );
        chain.push_as(
            "turbo",
            json!({"change": "standing", "standing": "hold", "team": "rally", "story": "RAL-9",
                   "note": "don't merge", "releases": "the operator's read"}),
        );
        chain.push_as(
            "turbo",
            json!({"change": "standing", "standing": "condition", "team": "rally",
                   "note": "RAL-9 before any deploy"}),
        );
        chain.push_as(
            "turbo",
            json!({"change": "standing", "standing": "hold", "team": "muster",
                   "note": "no restarts", "releases": "the rollout"}),
        );
        chain.push_as(
            "someone",
            json!({"change": "end_standing", "standing": 4, "note": "rolled out"}),
        );
        let ledger = Ledger::replay(&chain.records).unwrap();
        let out = brief(&ledger, &chain.hashes, None, 5_000);
        assert_eq!(out["standings_note"], STANDINGS);

        let grant = &out["grants"][0];
        assert_eq!(grant["kind"], "declared");
        assert_eq!(grant["declared_by"], "turbo");
        assert_eq!(
            grant["given"],
            json!({"from": "relayed", "by": "the watchman"})
        );
        assert_eq!(grant["scope"], "merge in Castor-o7/rally");
        assert_eq!(grant["age_ms"], 4_000);

        assert_eq!(out["holds_total"], 1, "the ended hold isn't listed");
        assert_eq!(out["holds"][0]["story"], "RAL-9");
        assert_eq!(out["holds"][0]["releases"], "the operator's read");
        assert_eq!(out["conditions"][0]["note"], "RAL-9 before any deploy");
    }

    #[test]
    fn a_claim_whose_session_is_gone_is_flagged() {
        let mut chain = Chain::new();
        for (name, story) in [("live-one", "RAL-1"), ("gone-one", "RAL-2")] {
            chain.push_as(name, json!({"change": "ready", "story": story}));
            chain.push_as(
                name,
                json!({"change": "claim", "story": story, "team": "rally"}),
            );
        }
        let ledger = Ledger::replay(&chain.records).unwrap();
        let mut out = brief(&ledger, &chain.hashes, None, 5_000);
        let roster = json!({"sessions": [{"nickname": "Live-One"}]});
        mark_sessions(&mut out, Some(&roster));
        assert_eq!(out["claims"][0]["session"]["kind"], "observed");
        assert_eq!(out["claims"][0]["session"]["on_roster"], true);
        assert_eq!(out["claims"][1]["session"]["on_roster"], false);
        assert!(
            out["claims"][1]["session"]["note"]
                .as_str()
                .unwrap()
                .contains("ended")
        );

        let mut unknown = brief(&ledger, &chain.hashes, None, 5_000);
        mark_sessions(&mut unknown, None);
        assert_eq!(unknown["claims"][0]["session"]["kind"], "unknown");
    }
}
