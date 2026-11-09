//! `rally wake`: wake an idle team with its next story (RAL-54).
//!
//! The decision is a pure function of what was read: each team's sessions on the roster, its
//! pick from `rally next`'s rule, its declared state, when it was last woken, the times agreed with
//! the operator, and now. Reading the roster, the roadmaps and the ledger, and sending through
//! `marshal-shim` or `muster-shim tell` ([`crate::shims`]), happen around it, so it can be tested
//! without any of them.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// The times agreed with the operator. Neither has a default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Times {
    /// How long a claim can go without a record before its team counts as quiet. Without it, only
    /// idle teams are woken.
    pub quiet_after_ms: Option<u64>,
    /// The least time between two digests to one team. Without it, nothing is sent: teams are
    /// only reported.
    pub interval_ms: Option<u64>,
}

/// What's known about one team for a pass.
#[derive(Clone, Debug, Default)]
pub struct Team {
    pub name: String,
    /// The team's sessions on the roster: each one's declared `activity` and its `wake`.
    pub sessions: Vec<Session>,
    /// The team's declared state, such as `resting`.
    pub status: Option<String>,
    /// `rally next`'s answer for the team.
    pub pick: Value,
    /// The newest record time on each story the team holds, for quiet claims.
    pub claims: Vec<(String, i64)>,
    /// When the team was last woken, from the state file.
    pub last_woken_ms: Option<i64>,
    /// Whether the operator has answered a question one of its shelved stories waits on since
    /// the last pass, which wakes it at once.
    pub answered: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    pub activity: Option<String>,
    pub wake: Option<String>,
    /// The roster it came from, such as `marshal-shim` (RAL-126).
    pub source: Option<String>,
}

impl Session {
    /// Whether the session is idle, by its declared activity (RAL-66). Only when it declares none
    /// does an observed `wake: waiting` count, since a busy session can be waiting for messages
    /// too: the roster shows sessions that are busy and `wake: waiting` at once.
    pub fn idle(&self) -> bool {
        match self.activity.as_deref() {
            Some(activity) => activity == "idle",
            None => self.wake.as_deref() == Some("waiting"),
        }
    }

    pub fn wakeable(&self) -> bool {
        self.wake.as_deref().is_some_and(|wake| wake != "none")
    }
}

/// What to do for one team.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Send this digest.
    Wake(String),
    /// Nothing to send, and why.
    Leave(&'static str),
    /// It needs waking but no session shows a way to wake it: report it, as a prompt to ask,
    /// never as a sign the team is gone.
    CantWake(String),
}

/// The digest a team is sent, naming its next story.
pub fn digest(team: &str, pick: &Value) -> Option<String> {
    let next = pick.get("next").filter(|next| !next.is_null())?;
    let story = next["story"].as_str()?;
    let title = next["title"]
        .as_str()
        .map(|title| format!(": {title}"))
        .unwrap_or_default();
    let why = next["why"].as_str().unwrap_or("next in roadmap order");
    Some(format!(
        "Rally suggests your next story, team {team}: {story}{title} ({why}). `rally next --team \
         {team}` shows it, with anything skipped. This is coordination, not an order. {}",
        crate::views::UNTRUSTED_TEXT
    ))
}

pub fn decide(team: &Team, times: Times, now_ms: i64) -> Decision {
    if team
        .status
        .as_deref()
        .is_some_and(|status| status == "resting")
    {
        return Decision::Leave("the team said it's resting");
    }
    let Some(digest) = digest(&team.name, &team.pick) else {
        return Decision::Leave("no ready story for it");
    };
    // Never idle for want of sessions: a team with none known is only that.
    if team.sessions.is_empty() {
        return Decision::Leave("no sessions known for it");
    }
    let idle = team.claims.is_empty() && team.sessions.iter().all(Session::idle);
    let quiet = times.quiet_after_ms.is_some_and(|after| {
        !team.claims.is_empty()
            && team
                .claims
                .iter()
                .all(|(_, last)| now_ms.saturating_sub(*last) >= after as i64)
    });
    if !team.answered && !idle && !quiet {
        return Decision::Leave("it's working");
    }
    let Some(interval_ms) = times.interval_ms else {
        return Decision::Leave(
            "no interval has been agreed with the operator, so nothing is sent",
        );
    };
    let held = team
        .last_woken_ms
        .is_some_and(|last| now_ms.saturating_sub(last) < interval_ms as i64);
    if held && !team.answered {
        return Decision::Leave("woken within the interval");
    }
    if !team.sessions.iter().any(Session::wakeable) {
        return Decision::CantWake(format!(
            "team {} has a ready story but no session shows a way to wake it (wake: none); ask \
             the team or the operator",
            team.name
        ));
    }
    Decision::Wake(digest)
}

/// Sends a team its digest. Rally sends through Muster or Marshal ([`Shims`]); without one, it
/// only reports. Another harness's messaging, such as Prime Agent's, can be another sender.
pub trait Sender {
    /// A short name for the report.
    fn name(&self) -> String;
    /// Sends `message` to `team`, and says what it was sent through.
    fn send(&self, team: &str, message: &str) -> Result<String, String>;
}

/// Sends through the first shim in the list that knows the team: `<shim> tell <team> <message>`,
/// as [`crate::shims::tell`] rules (RAL-120).
pub struct Shims(pub Vec<String>);

impl Sender for Shims {
    fn name(&self) -> String {
        self.0.join(", then ")
    }

    fn send(&self, team: &str, message: &str) -> Result<String, String> {
        crate::shims::tell(&self.0, team, message)
    }
}

/// A team's sessions on Muster's roster, by an explicit list of Muster project names or session
/// nicknames, never by guessing from the team's name.
pub fn sessions_of(roster: &Value, names: &[String]) -> Vec<Session> {
    let list = roster["sessions"]
        .as_array()
        .or_else(|| roster.as_array())
        .cloned()
        .unwrap_or_default();
    list.iter()
        .filter(|session| {
            let project = session["declared"]["project"].as_str();
            let nickname = session["nickname"].as_str();
            names
                .iter()
                .any(|name| Some(name.as_str()) == project || Some(name.as_str()) == nickname)
        })
        .map(|session| Session {
            activity: session["declared"]["activity"].as_str().map(str::to_owned),
            wake: session["wake"].as_str().map(str::to_owned),
            source: session["source"].as_str().map(str::to_owned),
        })
        .collect()
}

/// Each team's decision, as JSON for `rally wake`'s output and the status file.
pub fn report(decisions: &BTreeMap<String, Decision>, now_ms: i64) -> Value {
    let rows: Vec<Value> = decisions
        .iter()
        .map(|(team, decision)| match decision {
            Decision::Wake(digest) => json!({"team": team, "woken": true, "digest": digest}),
            Decision::Leave(why) => json!({"team": team, "woken": false, "why": why}),
            Decision::CantWake(why) => {
                json!({"team": team, "woken": false, "cant_wake": true, "why": why})
            }
        })
        .collect();
    json!({"at_ms": now_ms, "teams": rows})
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    fn times() -> Times {
        Times {
            quiet_after_ms: Some(30 * MIN),
            interval_ms: Some(10 * MIN),
        }
    }

    fn idle_session() -> Session {
        Session {
            activity: Some("idle".into()),
            wake: Some("waiting".into()),
            source: None,
        }
    }

    fn team() -> Team {
        Team {
            name: "rally".into(),
            sessions: vec![idle_session()],
            pick: json!({"next": {"story": "RAL-55", "title": "Story IDs", "why": "next in roadmap order"}}),
            ..Team::default()
        }
    }

    #[test]
    fn an_idle_team_with_a_ready_story_is_woken_with_it() {
        let Decision::Wake(digest) = decide(&team(), times(), 0) else {
            panic!("not woken");
        };
        assert!(
            digest.starts_with("Rally suggests your next story"),
            "{digest}"
        );
        assert!(digest.contains("RAL-55: Story IDs"), "{digest}");
        assert!(digest.contains("not instructions"), "{digest}");
        assert!(digest.contains("rally next --team rally"));
    }

    #[test]
    fn a_busy_session_waiting_for_messages_is_working_not_idle() {
        // As the roster showed for oceanic-gemsbok on 2026-11-05: busy, and wake: waiting.
        let busy = Session {
            activity: Some("busy".into()),
            wake: Some("waiting".into()),
            source: None,
        };
        assert!(!busy.idle());
        let mut working = team();
        working.sessions = vec![busy];
        assert_eq!(
            decide(&working, times(), 0),
            Decision::Leave("it's working")
        );
        // With no declared activity, wake: waiting still counts as idle.
        let unknown = Session {
            activity: None,
            wake: Some("waiting".into()),
            source: None,
        };
        assert!(unknown.idle());
        let mut quiet = team();
        quiet.sessions = vec![unknown];
        assert!(matches!(decide(&quiet, times(), 0), Decision::Wake(_)));
    }

    #[test]
    fn a_team_with_nothing_ready_or_resting_gets_nothing() {
        let mut empty = team();
        empty.pick = json!({"next": null, "nothing_left": true});
        assert_eq!(
            decide(&empty, times(), 0),
            Decision::Leave("no ready story for it")
        );
        let mut resting = team();
        resting.status = Some("resting".into());
        assert_eq!(
            decide(&resting, times(), 0),
            Decision::Leave("the team said it's resting")
        );
    }

    #[test]
    fn a_working_team_is_left_until_its_claim_goes_quiet() {
        let mut working = team();
        working.sessions = vec![Session {
            activity: Some("busy".into()),
            wake: Some("busy".into()),
            source: None,
        }];
        working.claims = vec![("RAL-54".into(), 0)];
        let before = (30 * MIN) as i64 - 1;
        assert_eq!(
            decide(&working, times(), before),
            Decision::Leave("it's working")
        );
        assert!(matches!(
            decide(&working, times(), before + 1),
            Decision::Wake(_)
        ));
        // Without an agreed time, a claim never counts as quiet.
        let unagreed = Times {
            quiet_after_ms: None,
            ..times()
        };
        assert_eq!(
            decide(&working, unagreed, i64::MAX),
            Decision::Leave("it's working")
        );
    }

    #[test]
    fn at_most_once_per_interval_unless_the_operator_answered() {
        let mut woken = team();
        woken.last_woken_ms = Some(0);
        let within = (10 * MIN) as i64 - 1;
        assert_eq!(
            decide(&woken, times(), within),
            Decision::Leave("woken within the interval")
        );
        assert!(matches!(
            decide(&woken, times(), within + 1),
            Decision::Wake(_)
        ));
        // An answer to a question its shelved story waits on wakes it at once.
        woken.answered = true;
        assert!(matches!(decide(&woken, times(), 1), Decision::Wake(_)));
    }

    #[test]
    fn a_team_asleep_with_wake_none_is_reported_not_messaged() {
        // 2026-11-05: tangerine-wombat slept about 2 h with wake `none` and ready work.
        let mut asleep = team();
        asleep.sessions = vec![Session {
            activity: Some("idle".into()),
            wake: Some("none".into()),
            source: None,
        }];
        let decision = decide(&asleep, times(), 0);
        assert!(matches!(&decision, Decision::CantWake(why) if why.contains("wake: none")));
        let report = report(&BTreeMap::from([("rally".to_owned(), decision)]), 5);
        assert_eq!(report["teams"][0]["cant_wake"], true);
        assert_eq!(report["teams"][0]["woken"], false);
    }

    #[test]
    fn a_teams_sessions_are_the_ones_listed_for_it_never_guessed() {
        let roster = json!([
            {"nickname": "giddy-geyser", "wake": "busy",
             "declared": {"project": "src", "activity": "busy"}},
            {"nickname": "tangerine-wombat", "wake": "none",
             "declared": {"project": "muster", "activity": "idle"}},
        ]);
        // A session whose project is `src` is nobody's unless it's listed.
        assert!(sessions_of(&roster, &["rally".into()]).is_empty());
        let by_name = sessions_of(&roster, &["giddy-geyser".into()]);
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].activity.as_deref(), Some("busy"));
        let by_project = sessions_of(&json!({"sessions": roster}), &["muster".into()]);
        assert_eq!(by_project[0].wake.as_deref(), Some("none"));
    }

    #[test]
    fn without_an_agreed_interval_nothing_is_sent() {
        let unagreed = Times {
            interval_ms: None,
            ..times()
        };
        assert!(
            matches!(decide(&team(), unagreed, 0), Decision::Leave(why) if why.contains("interval"))
        );
    }

    #[test]
    fn a_team_with_no_session_on_the_roster_is_left() {
        let mut gone = team();
        gone.sessions.clear();
        assert_eq!(
            decide(&gone, times(), 0),
            Decision::Leave("no sessions known for it")
        );
    }
}
