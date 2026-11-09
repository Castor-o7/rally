//! `rally stalls`: prompts for the watchman, from Muster's roster and Rally's records (RAL-7).
//!
//! Each row of the design's stall table (`docs/design.md`, "Stalls") is one rule here, and the
//! stall of 2026-10-30 is one more: every team waiting at once, each correctly, with nothing
//! recorded for a while. These are prompts, never conclusions. Muster's facts keep Muster's own
//! labels: `wake: waiting` is observed and "probably wakeable", everything else on a roster row is
//! declared, and `wake: none` is no sign that a message would wake a session, not proof it's gone.
//!
//! The decisions are pure functions of what was read, so they're tested without Muster, the
//! roadmaps or the ledger. How long a check-in lasts hasn't been agreed with the operator; the
//! caller passes `rally wake`'s `--quiet-after`, and without it the rules that need it say so.

use serde_json::{Value, json};

use crate::wake::Session;

/// An open blocker a team raised, as the ledger has it (declared).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Waiting {
    pub seq: u64,
    pub story: Option<String>,
    /// What it waits on: a team's name, such as `operator`, or a story.
    pub on: Option<String>,
    pub note: String,
}

/// What's known about one team.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub name: String,
    /// The team's sessions on Muster's roster.
    pub sessions: Vec<Session>,
    /// The team's declared state, such as `resting` or `blocked`.
    pub status: Option<String>,
    /// Each story the team holds, with its newest record's time.
    pub claims: Vec<(String, i64)>,
    /// The team's next ready story by `rally next`'s rule, if it has one.
    pub ready: Option<String>,
    /// Open blockers the team raised.
    pub blockers: Vec<Waiting>,
}

/// Whether a session is idle: by its declared activity, as `rally wake` judges it (RAL-66).
fn idle(session: &Session) -> bool {
    session.idle()
}

impl Facts {
    fn resting(&self) -> bool {
        self.status.as_deref() == Some("resting")
    }

    fn all_idle(&self) -> bool {
        !self.sessions.is_empty() && self.sessions.iter().all(idle)
    }

    fn wakeable(&self) -> bool {
        self.sessions.iter().any(Session::wakeable)
    }

    /// Whether every claim has gone without a record for `quiet_after_ms`.
    fn claims_quiet(&self, quiet_after_ms: u64, now_ms: i64) -> bool {
        !self.claims.is_empty()
            && self
                .claims
                .iter()
                .all(|(_, last)| now_ms.saturating_sub(*last) >= quiet_after_ms as i64)
    }

    /// How Muster labels what it says about this team, for the row.
    fn muster_says(&self) -> Value {
        if self.sessions.is_empty() {
            return json!({"sessions": 0, "note": "no session on Muster's roster for this team"});
        }
        let rows: Vec<Value> = self
            .sessions
            .iter()
            .map(|session| {
                json!({
                    // The roster it came from (RAL-126).
                    "source": session.source,
                    "wake": session.wake,
                    "wake_kind": if session.wake.as_deref() == Some("waiting") { "observed" }
                                 else { "declared" },
                    "activity": session.activity, "activity_kind": "declared",
                })
            })
            .collect();
        json!({"sessions": rows.len(), "rows": rows})
    }
}

fn prompt(team: &Facts, row: &str, meaning: &str, action: &str, rally_says: Value) -> Value {
    json!({
        "team": team.name, "row": row, "likely_meaning": meaning, "action": action,
        "muster_says": team.muster_says(), "rally_says": rally_says,
        "note": "A prompt for the watchman, not a conclusion.",
    })
}

/// The stall table's rows that hold for one team, in the table's order.
pub fn flags(team: &Facts, quiet_after_ms: Option<u64>, now_ms: i64) -> Vec<Value> {
    let mut out = Vec::new();
    let claims: Vec<&str> = team
        .claims
        .iter()
        .map(|(story, _)| story.as_str())
        .collect();
    // Can't be woken, with an open claim.
    if !team.claims.is_empty() && !team.wakeable() {
        out.push(prompt(
            team,
            "cant_be_woken_with_claim",
            "Agent may be gone",
            "Ask the team or the operator, then reassign the work if it's gone. `wake: none` \
             isn't proof a session is gone.",
            json!({"claims": claims, "kind": "declared"}),
        ));
    }
    // Idle and wakeable, with a claim that missed its check-in.
    if team.all_idle()
        && team.wakeable()
        && quiet_after_ms.is_some_and(|after| team.claims_quiet(after, now_ms))
    {
        let quiet: Vec<Value> = team
            .claims
            .iter()
            .map(|(story, last)| json!({"story": story, "quiet_ms": now_ms.saturating_sub(*last)}))
            .collect();
        out.push(prompt(
            team,
            "missed_check_in",
            "Possibly stuck",
            "Ask the team",
            json!({"claims": quiet, "kind": "declared"}),
        ));
    }
    // Any, declared blocked.
    if team.status.as_deref() == Some("blocked") || !team.blockers.is_empty() {
        let on: Vec<Value> = team
            .blockers
            .iter()
            .map(|b| json!({"blocker": b.seq, "story": b.story, "on": b.on, "note": b.note}))
            .collect();
        out.push(prompt(
            team,
            "declared_blocked",
            "Real blocker",
            "Help or escalate",
            json!({"status": team.status, "blockers": on, "kind": "declared"}),
        ));
    }
    // Idle and wakeable, with no claim and a story ready. Resting after a story is normal.
    if team.claims.is_empty()
        && !team.resting()
        && team.all_idle()
        && team.wakeable()
        && let Some(story) = &team.ready
    {
        out.push(prompt(
            team,
            "out_of_work",
            "Out of assigned work",
            "Offer more",
            json!({"ready": story, "kind": "declared"}),
        ));
    }
    out
}

/// What a team is waiting on, or `None` if it's working: a session that isn't idle, or a claim
/// with a record inside the check-in.
fn waits_on(team: &Facts, quiet_after_ms: u64, now_ms: i64) -> Option<Value> {
    let busy = team.sessions.iter().any(|session| !idle(session));
    let live_claim = team
        .claims
        .iter()
        .any(|(_, last)| now_ms.saturating_sub(*last) < quiet_after_ms as i64);
    if busy || live_claim {
        return None;
    }
    let mut reasons = Vec::new();
    for blocker in &team.blockers {
        reasons.push(
            json!({"on": blocker.on, "story": blocker.story, "blocker": blocker.seq,
                            "note": blocker.note}),
        );
    }
    if reasons.is_empty() {
        let why = if team.resting() {
            "resting"
        } else if !team.claims.is_empty() {
            "a claim gone quiet"
        } else if team.ready.is_none() {
            "no ready story"
        } else {
            "a ready story nobody has taken"
        };
        reasons.push(json!({"on": null, "why": why}));
    }
    Some(json!({"team": team.name, "waiting_on": reasons}))
}

/// The stall of 2026-10-30: every team waiting at once, each for a good reason, and nothing
/// recorded for the check-in. No one team is a stall on its own, so this looks at all of them,
/// and names what each waits on. `last_record_ms` is the newest record in the ledger.
pub fn all_waiting(
    teams: &[Facts],
    last_record_ms: i64,
    quiet_after_ms: u64,
    now_ms: i64,
) -> Option<Value> {
    if teams.is_empty() || now_ms.saturating_sub(last_record_ms) < quiet_after_ms as i64 {
        return None;
    }
    let mut each = Vec::new();
    for team in teams {
        each.push(waits_on(team, quiet_after_ms, now_ms)?);
    }
    Some(json!({
        "row": "every_team_waiting",
        "likely_meaning": "Everyone is waiting, each correctly, so nothing moves",
        "action": "Say so to the operator, naming what each team waits on",
        "quiet_ms": now_ms.saturating_sub(last_record_ms),
        "teams": each,
        "note": "A prompt for the watchman, not a conclusion.",
    }))
}

/// The full report: each team's prompts, and the every-team-waiting check if a check-in length
/// was given.
pub fn report(
    teams: &[Facts],
    last_record_ms: i64,
    quiet_after_ms: Option<u64>,
    now_ms: i64,
) -> Value {
    let prompts: Vec<Value> = teams
        .iter()
        .flat_map(|team| flags(team, quiet_after_ms, now_ms))
        .collect();
    let everyone = match quiet_after_ms {
        Some(after) => all_waiting(teams, last_record_ms, after, now_ms).unwrap_or(Value::Null),
        None => Value::Null,
    };
    let check_in = match quiet_after_ms {
        Some(after) => json!({"quiet_after_ms": after}),
        None => json!({
            "quiet_after_ms": null,
            "note": "No check-in length was given (--quiet-after, as for `rally wake`), and none \
                     has been agreed with the operator, so missed check-ins and the \
                     every-team-waiting check aren't looked for.",
        }),
    };
    json!({
        "untrusted_text": crate::views::UNTRUSTED_TEXT,
        "now_ms": now_ms,
        "check_in": check_in,
        "prompts": prompts,
        "every_team_waiting": everyone,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;
    const NOW: i64 = 1_000 * MIN;
    const QUIET: u64 = 20 * MIN as u64;

    fn session(activity: &str, wake: &str) -> Session {
        Session {
            activity: Some(activity.into()),
            wake: Some(wake.into()),
            source: None,
        }
    }

    fn idle() -> Vec<Session> {
        vec![session("idle", "waiting")]
    }

    fn rows(team: &Facts) -> Vec<String> {
        flags(team, Some(QUIET), NOW)
            .iter()
            .map(|row| row["row"].as_str().unwrap().to_owned())
            .collect()
    }

    fn team(name: &str) -> Facts {
        Facts {
            name: name.into(),
            sessions: idle(),
            ..Facts::default()
        }
    }

    #[test]
    fn row_one_a_claim_no_session_can_be_woken_for() {
        let mut gone = team("rally");
        gone.sessions = vec![session("idle", "none")];
        gone.claims = vec![("RAL-1".into(), NOW - MIN)];
        assert_eq!(rows(&gone), ["cant_be_woken_with_claim"]);
        let row = &flags(&gone, Some(QUIET), NOW)[0];
        assert_eq!(row["muster_says"]["rows"][0]["wake_kind"], "declared");
        assert!(row["action"].as_str().unwrap().contains("isn't proof"));
        // No session on the roster at all is the same prompt.
        gone.sessions.clear();
        assert_eq!(rows(&gone), ["cant_be_woken_with_claim"]);
        // A wakeable session with the same claim isn't.
        gone.sessions = idle();
        assert!(rows(&gone).is_empty());
    }

    #[test]
    fn row_two_an_idle_team_whose_claim_missed_its_check_in() {
        let mut quiet = team("rally");
        quiet.claims = vec![("RAL-1".into(), NOW - 21 * MIN)];
        assert_eq!(rows(&quiet), ["missed_check_in"]);
        let row = &flags(&quiet, Some(QUIET), NOW)[0];
        assert_eq!(row["muster_says"]["rows"][0]["wake_kind"], "observed");
        assert_eq!(row["rally_says"]["claims"][0]["quiet_ms"], 21 * MIN);
        // Inside the check-in, or busy, or with no check-in agreed: nothing.
        quiet.claims = vec![("RAL-1".into(), NOW - 5 * MIN)];
        assert!(rows(&quiet).is_empty());
        quiet.claims = vec![("RAL-1".into(), NOW - 21 * MIN)];
        quiet.sessions = vec![session("busy", "waiting")];
        assert!(rows(&quiet).is_empty());
        quiet.sessions = idle();
        assert!(flags(&quiet, None, NOW).is_empty());
    }

    #[test]
    fn row_three_any_team_declared_blocked() {
        let mut blocked = team("rally");
        blocked.sessions = vec![session("busy", "none")];
        blocked.blockers = vec![Waiting {
            seq: 7,
            story: Some("RAL-1".into()),
            on: Some("operator".into()),
            note: "which design?".into(),
        }];
        assert_eq!(rows(&blocked), ["declared_blocked"]);
        assert_eq!(
            flags(&blocked, Some(QUIET), NOW)[0]["rally_says"]["blockers"][0]["on"],
            "operator"
        );
        // A declared state of blocked is enough on its own.
        let mut said = team("muster");
        said.status = Some("blocked".into());
        assert_eq!(rows(&said), ["declared_blocked"]);
    }

    #[test]
    fn row_four_an_idle_team_with_no_claim_and_a_story_ready() {
        let mut free = team("catapp");
        free.ready = Some("CAT-9".into());
        assert_eq!(rows(&free), ["out_of_work"]);
        // Busy, or nothing ready: nothing.
        free.sessions = vec![session("busy", "waiting")];
        assert!(rows(&free).is_empty());
        free.sessions = idle();
        free.ready = None;
        assert!(rows(&free).is_empty());
    }

    #[test]
    fn a_team_resting_after_a_story_isnt_flagged() {
        let mut resting = team("rally");
        resting.status = Some("resting".into());
        resting.ready = Some("RAL-2".into());
        assert!(rows(&resting).is_empty());
        let report = report(&[resting], NOW - MIN, Some(QUIET), NOW);
        assert_eq!(report["prompts"], json!([]));
    }

    /// Finding 21: three teams idle with no live claim, one blocked on the operator, one on the
    /// watchman, and one with no ready story, and 20 minutes with no record.
    fn october_30() -> Vec<Facts> {
        let mut paradox = team("ParadoxDetector");
        paradox.blockers = vec![Waiting {
            seq: 1,
            story: Some("PD-3".into()),
            on: Some("operator".into()),
            note: "a decision".into(),
        }];
        let mut muster = team("Muster");
        muster.blockers = vec![Waiting {
            seq: 2,
            story: None,
            on: Some("watchman".into()),
            note: "the quiet-point signal".into(),
        }];
        let catapp = team("catapp");
        vec![paradox, muster, catapp]
    }

    #[test]
    fn the_stall_of_2026_10_30_would_have_been_flagged() {
        let teams = october_30();
        let last = NOW - 20 * MIN;
        let report = report(&teams, last, Some(QUIET), NOW);
        let everyone = &report["every_team_waiting"];
        assert_eq!(everyone["row"], "every_team_waiting", "{report}");
        let on: Vec<Value> = everyone["teams"]
            .as_array()
            .unwrap()
            .iter()
            .map(|team| team["waiting_on"][0].clone())
            .collect();
        assert_eq!(on[0]["on"], "operator");
        assert_eq!(on[1]["on"], "watchman");
        assert_eq!(on[2]["why"], "no ready story");

        // One team working, and it isn't.
        let mut working = october_30();
        working[2].sessions = vec![session("busy", "waiting")];
        assert!(all_waiting(&working, last, QUIET, NOW).is_none());
        // A team with a fresh claim counts as working too.
        let mut claimed = october_30();
        claimed[2].claims = vec![("CAT-1".into(), NOW - 2 * MIN)];
        assert!(all_waiting(&claimed, NOW - 21 * MIN, QUIET, NOW).is_none());
        // Before the check-in has passed with no record, it isn't either.
        assert!(all_waiting(&teams, NOW - 10 * MIN, QUIET, NOW).is_none());
        // And with no check-in length agreed, it isn't looked for, and the report says why.
        let unagreed = super::report(&teams, last, None, NOW);
        assert!(unagreed["every_team_waiting"].is_null());
        assert!(
            unagreed["check_in"]["note"]
                .as_str()
                .unwrap()
                .contains("agreed")
        );
    }
}
