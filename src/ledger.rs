//! Story states and deterministic replay (RAL-3, RAL-12).
//!
//! The ledger's state is a pure function of its records: this module reads no clock, file or
//! outside system, and keeps its state in ordered maps, so the same records always give identical
//! state. A record's body holds one [`Change`]. A refused change leaves the state untouched.
//!
//! Stories stay in each project's roadmap, so a story here has only its ID, state and history.
//!
//! The check that a verifier isn't the submitter compares declared names, so it catches mistakes,
//! not forgery. See `docs/threat-model.md`.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::record::{Kind, Record, Source};

/// A story's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Ready,
    Claimed,
    InReview,
    Verified,
    SentBack,
}

impl State {
    /// Whether this state belongs to the verifier rather than the team.
    fn is_verifiers(self) -> bool {
        matches!(self, Self::Verified | Self::SentBack)
    }
}

/// What a record does to a story. This is the `body` of a record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
    /// A team marks a story as ready to claim.
    Ready { story: String },
    /// A team takes a ready or sent-back story.
    Claim { story: String, team: String },
    /// The team holding a claim gives it up.
    Release { story: String, team: String },
    /// The team holding a claim submits it, with evidence such as a commit.
    Submit {
        story: String,
        team: String,
        evidence: String,
    },
    /// Someone other than the submitter confirms the story.
    Verify { story: String, note: String },
    /// Someone other than the submitter returns the story, saying what's missing.
    SendBack { story: String, note: String },
    /// Annotates an earlier record of the same story, which stays in the history. Can also set the
    /// state: a team can only do that for a story it holds, naming itself in `team`, and moving a
    /// story into or out of a verifier's state takes a verifier. Without a state, it's only an
    /// annotation, which anyone can add, and `team` is ignored. Teams can record a disagreement
    /// this way, and it changes nothing else.
    Correct {
        story: String,
        corrects: u64,
        note: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<State>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        team: Option<String>,
    },
}

impl Change {
    fn story(&self) -> &str {
        match self {
            Self::Ready { story }
            | Self::Claim { story, .. }
            | Self::Release { story, .. }
            | Self::Submit { story, .. }
            | Self::Verify { story, .. }
            | Self::SendBack { story, .. }
            | Self::Correct { story, .. } => story,
        }
    }
}

/// One step in a story's history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HistoryEntry {
    pub seq: u64,
    pub kind: Kind,
    pub source: Source,
    pub at_ms: i64,
    pub change: Change,
    /// Later records that correct this one. The entry itself is never changed otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub corrected_by: Vec<u64>,
}

/// A story's current state and everything that happened to it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Story {
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submitted_by: Option<String>,
    pub history: Vec<HistoryEntry>,
}

/// Why a change was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum LedgerError {
    /// The record's body isn't a valid change. Holds the reason.
    InvalidChange(String),
    /// A story ID isn't in the `ABC-1` form.
    InvalidStoryId(String),
    /// A required text field is empty. Holds the field's name.
    EmptyField(&'static str),
    /// Records must be applied in order with no gaps.
    OutOfOrder {
        expected: u64,
        found: u64,
    },
    UnknownStory(String),
    AlreadyExists(String),
    /// This change can't happen from the story's current state.
    InvalidTransition {
        story: String,
        from: State,
    },
    /// A different team holds this story.
    WrongTeam {
        story: String,
        holder: String,
    },
    /// This change needs a record of another kind.
    WrongKind {
        expected: Kind,
        found: Kind,
    },
    /// A verifier's declared name matches the submitter's.
    SelfVerification {
        story: String,
        name: String,
    },
    /// A correction names a record that isn't an earlier record of this story.
    InvalidCorrectionTarget {
        story: String,
        corrects: u64,
    },
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidChange(reason) => write!(f, "invalid change: {reason}"),
            Self::InvalidStoryId(id) => write!(f, "story ID {id:?} isn't in the form ABC-1"),
            Self::EmptyField(field) => write!(f, "{field} is empty"),
            Self::OutOfOrder { expected, found } => {
                write!(f, "expected record {expected}, found record {found}")
            }
            Self::UnknownStory(story) => write!(f, "{story} isn't in the ledger"),
            Self::AlreadyExists(story) => write!(f, "{story} is already in the ledger"),
            Self::InvalidTransition { story, from } => {
                write!(f, "{story} can't make this change from {from:?}")
            }
            Self::WrongTeam { story, holder } => write!(f, "{story} is held by {holder}"),
            Self::WrongKind { expected, found } => {
                write!(f, "this change needs a {expected:?} record, not {found:?}")
            }
            Self::SelfVerification { story, name } => {
                write!(f, "{name} submitted {story}, so can't verify it")
            }
            Self::InvalidCorrectionTarget { story, corrects } => {
                write!(f, "record {corrects} isn't an earlier record of {story}")
            }
        }
    }
}

impl std::error::Error for LedgerError {}

/// The state rebuilt from a ledger's records.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Ledger {
    last_seq: u64,
    stories: BTreeMap<String, Story>,
}

impl Ledger {
    /// Rebuilds the state from records in order, numbering them from 1.
    pub fn replay<'a>(records: impl IntoIterator<Item = &'a Record>) -> Result<Self, LedgerError> {
        let mut ledger = Self::default();
        for (index, record) in records.into_iter().enumerate() {
            ledger.apply(index as u64 + 1, record)?;
        }
        Ok(ledger)
    }

    /// The last record applied, or 0 for an empty ledger.
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    pub fn story(&self, id: &str) -> Option<&Story> {
        self.stories.get(id)
    }

    pub fn stories(&self) -> &BTreeMap<String, Story> {
        &self.stories
    }

    /// Applies one record, or refuses it and leaves the state unchanged.
    pub fn apply(&mut self, seq: u64, record: &Record) -> Result<(), LedgerError> {
        let expected = self.last_seq + 1;
        if seq != expected {
            return Err(LedgerError::OutOfOrder {
                expected,
                found: seq,
            });
        }
        let change: Change = serde_json::from_value(record.body().clone())
            .map_err(|e| LedgerError::InvalidChange(e.to_string()))?;
        check_fields(&change)?;
        let id = change.story().to_owned();

        if matches!(change, Change::Ready { .. }) {
            if self.stories.contains_key(&id) {
                return Err(LedgerError::AlreadyExists(id));
            }
            require_kind(record, Kind::Declared)?;
            self.stories.insert(
                id.clone(),
                Story {
                    state: State::Ready,
                    team: None,
                    submitted_by: None,
                    history: Vec::new(),
                },
            );
        } else {
            let story = self
                .stories
                .get_mut(&id)
                .ok_or_else(|| LedgerError::UnknownStory(id.clone()))?;
            transition(story, &id, &change, record, seq)?;
        }

        let story = self.stories.get_mut(&id).expect("the story exists by now");
        story.history.push(HistoryEntry {
            seq,
            kind: record.kind(),
            source: record.source().clone(),
            at_ms: record.at_ms(),
            change,
            corrected_by: Vec::new(),
        });
        self.last_seq = seq;
        Ok(())
    }
}

/// Validates and applies a change to an existing story. Nothing is modified on refusal.
fn transition(
    story: &mut Story,
    id: &str,
    change: &Change,
    record: &Record,
    seq: u64,
) -> Result<(), LedgerError> {
    let refuse = |from| LedgerError::InvalidTransition {
        story: id.to_owned(),
        from,
    };
    match change {
        Change::Ready { .. } => unreachable!("handled by the caller"),
        Change::Claim { team, .. } => {
            require_kind(record, Kind::Declared)?;
            match story.state {
                State::Ready => {}
                // A sent-back story returns to the same team.
                State::SentBack => require_holder(story, id, team)?,
                from => return Err(refuse(from)),
            }
            story.state = State::Claimed;
            story.team = Some(team.clone());
            // A new claim needs its own submission.
            story.submitted_by = None;
        }
        Change::Release { team, .. } => {
            require_kind(record, Kind::Declared)?;
            if story.state != State::Claimed {
                return Err(refuse(story.state));
            }
            require_holder(story, id, team)?;
            story.state = State::Ready;
            story.team = None;
            story.submitted_by = None;
        }
        Change::Submit { team, .. } => {
            require_kind(record, Kind::Declared)?;
            if story.state != State::Claimed {
                return Err(refuse(story.state));
            }
            require_holder(story, id, team)?;
            story.state = State::InReview;
            story.submitted_by = record.source().declared_name.clone();
        }
        Change::Verify { .. } | Change::SendBack { .. } => {
            if story.state != State::InReview {
                return Err(refuse(story.state));
            }
            require_verifier(story, id, record)?;
            story.state = if matches!(change, Change::Verify { .. }) {
                State::Verified
            } else {
                State::SentBack
            };
        }
        Change::Correct {
            corrects,
            state,
            team,
            ..
        } => {
            let target = story.history.iter().position(|e| e.seq == *corrects);
            let Some(target) = target else {
                return Err(LedgerError::InvalidCorrectionTarget {
                    story: id.to_owned(),
                    corrects: *corrects,
                });
            };
            if record.kind() == Kind::Observed {
                return Err(LedgerError::WrongKind {
                    expected: Kind::Declared,
                    found: Kind::Observed,
                });
            }
            if let Some(new_state) = state {
                if story.state.is_verifiers()
                    || new_state.is_verifiers()
                    || record.kind() == Kind::Verified
                {
                    require_verifier(story, id, record)?;
                } else {
                    let team = team.as_deref().ok_or(LedgerError::EmptyField("team"))?;
                    require_holder(story, id, team)?;
                }
                // Every state past Ready needs a team to hold it, and every state past Claimed
                // needs a submission, or the check against self-verification has nothing to
                // compare with.
                let needs_submission = *new_state == State::InReview || new_state.is_verifiers();
                if (*new_state != State::Ready && story.team.is_none())
                    || (needs_submission && story.submitted_by.is_none())
                {
                    return Err(refuse(story.state));
                }
                story.state = *new_state;
                // Going back to Ready or Claimed means the work may change, so it needs a new
                // submission rather than reviving the old evidence.
                if matches!(new_state, State::Ready | State::Claimed) {
                    story.submitted_by = None;
                }
                if *new_state == State::Ready {
                    story.team = None;
                }
            }
            story.history[target].corrected_by.push(seq);
        }
    }
    Ok(())
}

fn require_kind(record: &Record, expected: Kind) -> Result<(), LedgerError> {
    if record.kind() == expected {
        Ok(())
    } else {
        Err(LedgerError::WrongKind {
            expected,
            found: record.kind(),
        })
    }
}

fn require_holder(story: &Story, id: &str, team: &str) -> Result<(), LedgerError> {
    match &story.team {
        Some(holder) if !same_name(holder, team) => Err(LedgerError::WrongTeam {
            story: id.to_owned(),
            holder: holder.clone(),
        }),
        _ => Ok(()),
    }
}

fn require_verifier(story: &Story, id: &str, record: &Record) -> Result<(), LedgerError> {
    require_kind(record, Kind::Verified)?;
    let Some(name) = record.source().declared_name.as_deref() else {
        return Ok(());
    };
    // Anyone who ever submitted this story counts as an author, as does the team holding it.
    let submitted = story.history.iter().any(|entry| {
        matches!(entry.change, Change::Submit { .. })
            && entry
                .source
                .declared_name
                .as_deref()
                .is_some_and(|submitter| same_name(submitter, name))
    });
    let holds = story
        .team
        .as_deref()
        .is_some_and(|team| same_name(team, name));
    if submitted || holds {
        return Err(LedgerError::SelfVerification {
            story: id.to_owned(),
            name: name.to_owned(),
        });
    }
    Ok(())
}

/// Compares declared names, ignoring case and surrounding space, to catch honest slips.
fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

fn check_fields(change: &Change) -> Result<(), LedgerError> {
    let id = change.story();
    if !is_story_id(id) {
        return Err(LedgerError::InvalidStoryId(id.to_owned()));
    }
    if let Change::Correct {
        team: Some(team), ..
    } = change
        && team.trim().is_empty()
    {
        return Err(LedgerError::EmptyField("team"));
    }
    let fields: &[(&'static str, &str)] = match change {
        Change::Ready { .. } => &[],
        Change::Claim { team, .. } | Change::Release { team, .. } => &[("team", team)],
        Change::Submit { team, evidence, .. } => &[("team", team), ("evidence", evidence)],
        Change::Verify { note, .. }
        | Change::SendBack { note, .. }
        | Change::Correct { note, .. } => &[("note", note)],
    };
    for (name, value) in fields {
        if value.trim().is_empty() {
            return Err(LedgerError::EmptyField(name));
        }
    }
    Ok(())
}

fn is_story_id(id: &str) -> bool {
    id.split_once('-').is_some_and(|(prefix, number)| {
        !prefix.is_empty()
            && prefix.chars().all(|c| c.is_ascii_uppercase())
            && !number.is_empty()
            && number.chars().all(|c| c.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const STORY: &str = "RAL-3";

    fn record(kind: Kind, name: &str, body: serde_json::Value) -> Record {
        let source = Source {
            credential: "a1".into(),
            declared_name: Some(name.into()),
        };
        Record::new(kind, source, 1_000, body).unwrap()
    }

    fn team(body: serde_json::Value) -> Record {
        record(Kind::Declared, "alice", body)
    }

    fn verifier(body: serde_json::Value) -> Record {
        record(Kind::Verified, "watch", body)
    }

    fn ready() -> Record {
        team(json!({"change": "ready", "story": STORY}))
    }

    fn claim() -> Record {
        team(json!({"change": "claim", "story": STORY, "team": "rally"}))
    }

    fn release() -> Record {
        team(json!({"change": "release", "story": STORY, "team": "rally"}))
    }

    fn submit() -> Record {
        team(json!({"change": "submit", "story": STORY, "team": "rally", "evidence": "abc123"}))
    }

    fn verify() -> Record {
        verifier(json!({"change": "verify", "story": STORY, "note": "read the diff"}))
    }

    fn send_back() -> Record {
        verifier(json!({"change": "send_back", "story": STORY, "note": "no test"}))
    }

    fn ledger_with(records: &[Record]) -> Ledger {
        Ledger::replay(records).unwrap()
    }

    fn apply_next(ledger: &mut Ledger, record: &Record) -> Result<(), LedgerError> {
        ledger.apply(ledger.last_seq() + 1, record)
    }

    fn state(ledger: &Ledger) -> State {
        ledger.story(STORY).unwrap().state
    }

    #[test]
    fn follows_the_normal_path() {
        let ledger = ledger_with(&[ready(), claim(), submit(), verify()]);
        let story = ledger.story(STORY).unwrap();
        assert_eq!(story.state, State::Verified);
        assert_eq!(story.team.as_deref(), Some("rally"));
        assert_eq!(story.history.len(), 4);
    }

    #[test]
    fn a_sent_back_story_returns_to_its_team() {
        let mut ledger = ledger_with(&[ready(), claim(), submit(), send_back()]);
        assert_eq!(state(&ledger), State::SentBack);
        let other = team(json!({"change": "claim", "story": STORY, "team": "muster"}));
        assert!(matches!(
            apply_next(&mut ledger, &other),
            Err(LedgerError::WrongTeam { .. })
        ));
        apply_next(&mut ledger, &claim()).unwrap();
        assert_eq!(state(&ledger), State::Claimed);
    }

    #[test]
    fn a_released_story_can_go_to_another_team() {
        let mut ledger = ledger_with(&[ready(), claim(), release()]);
        assert_eq!(state(&ledger), State::Ready);
        let other = team(json!({"change": "claim", "story": STORY, "team": "muster"}));
        apply_next(&mut ledger, &other).unwrap();
        assert_eq!(ledger.story(STORY).unwrap().team.as_deref(), Some("muster"));
    }

    #[test]
    fn refuses_each_invalid_transition_and_changes_nothing() {
        let ready_only = vec![ready()];
        let claimed = vec![ready(), claim()];
        let in_review = vec![ready(), claim(), submit()];
        let verified = vec![ready(), claim(), submit(), verify()];
        let sent_back = vec![ready(), claim(), submit(), send_back()];
        let cases: Vec<(&Vec<Record>, Vec<Record>)> = vec![
            (
                &ready_only,
                vec![release(), submit(), verify(), send_back()],
            ),
            (&claimed, vec![claim(), verify(), send_back()]),
            (&in_review, vec![claim(), release(), submit()]),
            (
                &verified,
                vec![claim(), release(), submit(), verify(), send_back()],
            ),
            (&sent_back, vec![release(), submit(), verify(), send_back()]),
        ];
        for (start, refused) in cases {
            for change in refused {
                let mut ledger = ledger_with(start);
                let before = ledger.clone();
                let result = apply_next(&mut ledger, &change);
                assert!(
                    matches!(result, Err(LedgerError::InvalidTransition { .. })),
                    "{change:?} from {:?} gave {result:?}",
                    before.story(STORY).unwrap().state
                );
                assert_eq!(ledger, before);
            }
        }
    }

    #[test]
    fn refuses_changes_to_an_unknown_story() {
        let mut ledger = Ledger::default();
        assert_eq!(
            apply_next(&mut ledger, &claim()),
            Err(LedgerError::UnknownStory(STORY.into()))
        );
    }

    #[test]
    fn refuses_a_story_added_twice() {
        let mut ledger = ledger_with(&[ready()]);
        assert_eq!(
            apply_next(&mut ledger, &ready()),
            Err(LedgerError::AlreadyExists(STORY.into()))
        );
    }

    #[test]
    fn only_the_holder_can_submit_or_release() {
        for change in ["submit", "release"] {
            let mut ledger = ledger_with(&[ready(), claim()]);
            let body = json!({"change": change, "story": STORY, "team": "muster", "evidence": "x"});
            let mut body = body;
            if change == "release" {
                body.as_object_mut().unwrap().remove("evidence");
            }
            assert!(matches!(
                apply_next(&mut ledger, &team(body)),
                Err(LedgerError::WrongTeam { .. })
            ));
        }
    }

    #[test]
    fn refuses_a_verifier_with_the_submitters_name() {
        for change in ["verify", "send_back"] {
            let mut ledger = ledger_with(&[ready(), claim(), submit()]);
            let own = record(
                Kind::Verified,
                "alice",
                json!({"change": change, "story": STORY, "note": "looks good to me"}),
            );
            assert!(matches!(
                apply_next(&mut ledger, &own),
                Err(LedgerError::SelfVerification { .. })
            ));
        }
    }

    #[test]
    fn verifying_needs_a_verified_record() {
        let mut ledger = ledger_with(&[ready(), claim(), submit()]);
        let declared = record(
            Kind::Declared,
            "watch",
            json!({"change": "verify", "story": STORY, "note": "fine"}),
        );
        assert!(matches!(
            apply_next(&mut ledger, &declared),
            Err(LedgerError::WrongKind { .. })
        ));
    }

    #[test]
    fn team_changes_need_a_declared_record() {
        let mut ledger = ledger_with(&[ready()]);
        let verified = record(
            Kind::Verified,
            "alice",
            json!({"change": "claim", "story": STORY, "team": "rally"}),
        );
        assert!(matches!(
            apply_next(&mut ledger, &verified),
            Err(LedgerError::WrongKind { .. })
        ));
    }

    #[test]
    fn refuses_bad_changes() {
        let mut ledger = ledger_with(&[ready(), claim()]);
        let cases = [
            (
                json!({"change": "promote", "story": STORY}),
                "unknown change",
            ),
            (json!({"story": STORY}), "no change"),
            (json!({"change": "claim", "story": STORY}), "no team"),
            (
                json!({"change": "ready", "story": STORY, "points": 10}),
                "unknown field",
            ),
        ];
        for (body, why) in cases {
            assert!(
                matches!(
                    apply_next(&mut ledger, &team(body)),
                    Err(LedgerError::InvalidChange(_))
                ),
                "{why}"
            );
        }
    }

    #[test]
    fn refuses_empty_fields_and_bad_ids() {
        let mut ledger = ledger_with(&[ready(), claim()]);
        let empty = json!({"change": "submit", "story": STORY, "team": "rally", "evidence": " "});
        assert_eq!(
            apply_next(&mut ledger, &team(empty)),
            Err(LedgerError::EmptyField("evidence"))
        );
        for id in ["", "RAL", "RAL-", "-3", "ral-3", "RAL-3a", "RAL-3-4"] {
            let body = json!({"change": "ready", "story": id});
            assert_eq!(
                apply_next(&mut ledger, &team(body)),
                Err(LedgerError::InvalidStoryId(id.into()))
            );
        }
    }

    #[test]
    fn a_correction_keeps_what_it_corrects() {
        let mut ledger = ledger_with(&[ready(), claim(), submit()]);
        let original = ledger.story(STORY).unwrap().history[2].change.clone();
        let correction = team(json!({
            "change": "correct", "story": STORY, "corrects": 3,
            "note": "wrong commit; it was def456", "state": "claimed", "team": "rally"
        }));
        apply_next(&mut ledger, &correction).unwrap();

        let story = ledger.story(STORY).unwrap();
        assert_eq!(story.state, State::Claimed);
        assert_eq!(story.history.len(), 4);
        assert_eq!(story.history[2].change, original);
        assert_eq!(story.history[2].corrected_by, vec![4]);
    }

    #[test]
    fn a_correction_can_leave_the_state_alone() {
        let mut ledger = ledger_with(&[ready(), claim()]);
        let correction = team(json!({
            "change": "correct", "story": STORY, "corrects": 2, "note": "typo in the note"
        }));
        apply_next(&mut ledger, &correction).unwrap();
        assert_eq!(state(&ledger), State::Claimed);
    }

    #[test]
    fn a_correction_must_name_an_earlier_record_of_the_same_story() {
        let other = team(json!({"change": "ready", "story": "RAL-4"}));
        let mut ledger = ledger_with(&[ready(), other]);
        for corrects in [2, 3, 99] {
            let correction = team(json!({
                "change": "correct", "story": STORY, "corrects": corrects, "note": "x"
            }));
            assert!(matches!(
                apply_next(&mut ledger, &correction),
                Err(LedgerError::InvalidCorrectionTarget { .. })
            ));
        }
    }

    #[test]
    fn a_team_cannot_correct_its_way_into_or_out_of_a_verifiers_state() {
        let into = team(json!({
            "change": "correct", "story": STORY, "corrects": 3, "note": "x", "state": "verified"
        }));
        let mut ledger = ledger_with(&[ready(), claim(), submit()]);
        assert!(matches!(
            apply_next(&mut ledger, &into),
            Err(LedgerError::WrongKind { .. })
        ));

        let out = team(json!({
            "change": "correct", "story": STORY, "corrects": 4, "note": "x", "state": "ready"
        }));
        let mut ledger = ledger_with(&[ready(), claim(), submit(), send_back()]);
        assert!(matches!(
            apply_next(&mut ledger, &out),
            Err(LedgerError::WrongKind { .. })
        ));
    }

    #[test]
    fn a_correction_to_ready_clears_the_team_and_past_ready_needs_one() {
        let mut ledger = ledger_with(&[ready(), claim()]);
        let to_ready = team(json!({
            "change": "correct", "story": STORY, "corrects": 2, "note": "x", "state": "ready",
            "team": "rally"
        }));
        apply_next(&mut ledger, &to_ready).unwrap();
        assert_eq!(ledger.story(STORY).unwrap().team, None);

        let to_claimed = team(json!({
            "change": "correct", "story": STORY, "corrects": 2, "note": "x", "state": "claimed",
            "team": "rally"
        }));
        assert!(matches!(
            apply_next(&mut ledger, &to_claimed),
            Err(LedgerError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn an_observed_record_cannot_correct() {
        let mut ledger = ledger_with(&[ready(), claim()]);
        let source = Source {
            credential: "github".into(),
            declared_name: None,
        };
        for body in [
            json!({"change": "correct", "story": STORY, "corrects": 2, "note": "x"}),
            json!({"change": "correct", "story": STORY, "corrects": 2, "note": "x",
                   "state": "ready"}),
        ] {
            let observed = Record::new(Kind::Observed, source.clone(), 1_000, body).unwrap();
            assert!(matches!(
                apply_next(&mut ledger, &observed),
                Err(LedgerError::WrongKind { .. })
            ));
        }
    }

    #[test]
    fn a_correction_cannot_skip_submission_to_self_verify() {
        let mut ledger = ledger_with(&[ready(), claim()]);
        let skip = team(json!({
            "change": "correct", "story": STORY, "corrects": 2, "note": "x", "state": "in_review"
        }));
        let _ = apply_next(&mut ledger, &skip);
        let own = record(
            Kind::Verified,
            "alice",
            json!({"change": "verify", "story": STORY, "note": "fine"}),
        );
        assert!(apply_next(&mut ledger, &own).is_err());
        assert_eq!(state(&ledger), State::Claimed);
    }

    #[test]
    fn another_team_cannot_move_a_claim_by_correcting_it() {
        for target in ["ready", "in_review", "claimed"] {
            let mut ledger = ledger_with(&[ready(), claim()]);
            let before = ledger.clone();
            let body = json!({
                "change": "correct", "story": STORY, "corrects": 2, "note": "x",
                "state": target
            });
            assert!(apply_next(&mut ledger, &team(body)).is_err(), "{target}");
            assert_eq!(ledger, before);
        }
    }

    #[test]
    fn a_correction_with_a_state_must_name_the_holding_team() {
        for (extra, missing) in [(None, true), (Some("muster"), false)] {
            let mut ledger = ledger_with(&[ready(), claim()]);
            let mut body = json!({
                "change": "correct", "story": STORY, "corrects": 2, "note": "x", "state": "ready"
            });
            if let Some(extra) = extra {
                body["team"] = json!(extra);
            }
            let result = apply_next(&mut ledger, &team(body));
            if missing {
                assert_eq!(result, Err(LedgerError::EmptyField("team")));
            } else {
                assert!(matches!(result, Err(LedgerError::WrongTeam { .. })));
            }
        }
    }

    #[test]
    fn the_holder_cannot_correct_into_review_without_a_submission() {
        let mut ledger = ledger_with(&[ready(), claim()]);
        let body = json!({
            "change": "correct", "story": STORY, "corrects": 2, "note": "x",
            "state": "in_review", "team": "rally"
        });
        assert!(matches!(
            apply_next(&mut ledger, &team(body)),
            Err(LedgerError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn a_verifier_cannot_correct_an_unsubmitted_story_into_a_verifiers_state() {
        for target in ["verified", "sent_back", "in_review"] {
            let mut ledger = ledger_with(&[ready(), claim()]);
            let body = json!({
                "change": "correct", "story": STORY, "corrects": 2, "note": "x", "state": target
            });
            assert!(matches!(
                apply_next(&mut ledger, &verifier(body)),
                Err(LedgerError::InvalidTransition { .. })
            ));
        }
    }

    #[test]
    fn a_verifier_can_reassign_a_claim() {
        let mut ledger = ledger_with(&[ready(), claim()]);
        let body = json!({
            "change": "correct", "story": STORY, "corrects": 2, "note": "session ended",
            "state": "ready"
        });
        apply_next(&mut ledger, &verifier(body)).unwrap();
        assert_eq!(state(&ledger), State::Ready);
    }

    #[test]
    fn a_new_claim_needs_its_own_submission() {
        let mut ledger = ledger_with(&[ready(), claim(), submit(), send_back(), claim()]);
        let body = json!({
            "change": "correct", "story": STORY, "corrects": 5, "note": "x",
            "state": "in_review", "team": "rally"
        });
        assert!(matches!(
            apply_next(&mut ledger, &team(body)),
            Err(LedgerError::InvalidTransition { .. })
        ));
    }

    fn verify_as(name: &str) -> Record {
        record(
            Kind::Verified,
            name,
            json!({"change": "verify", "story": STORY, "note": "fine"}),
        )
    }

    fn correct_to(target: &str, corrects: u64) -> serde_json::Value {
        json!({
            "change": "correct", "story": STORY, "corrects": corrects, "note": "x",
            "state": target, "team": "rally"
        })
    }

    #[test]
    fn a_team_cannot_verify_under_its_own_team_name() {
        let mut ledger = ledger_with(&[ready(), claim(), submit()]);
        assert!(matches!(
            apply_next(&mut ledger, &verify_as("rally")),
            Err(LedgerError::SelfVerification { .. })
        ));
    }

    #[test]
    fn going_back_to_claimed_needs_a_new_submission() {
        let mut ledger = ledger_with(&[ready(), claim(), submit()]);
        apply_next(&mut ledger, &team(correct_to("claimed", 3))).unwrap();
        assert!(matches!(
            apply_next(&mut ledger, &team(correct_to("in_review", 3))),
            Err(LedgerError::InvalidTransition { .. })
        ));
        let to_verified = verifier(json!({
            "change": "correct", "story": STORY, "corrects": 3, "note": "x", "state": "verified"
        }));
        assert!(matches!(
            apply_next(&mut ledger, &to_verified),
            Err(LedgerError::InvalidTransition { .. })
        ));
        apply_next(&mut ledger, &submit()).unwrap();
        assert_eq!(state(&ledger), State::InReview);
    }

    #[test]
    fn a_verifier_reopening_a_story_needs_a_new_submission() {
        let mut ledger = ledger_with(&[ready(), claim(), submit(), verify()]);
        let reopen = verifier(json!({
            "change": "correct", "story": STORY, "corrects": 4, "note": "x", "state": "claimed"
        }));
        apply_next(&mut ledger, &reopen).unwrap();
        assert!(matches!(
            apply_next(&mut ledger, &team(correct_to("in_review", 3))),
            Err(LedgerError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn anyone_who_ever_submitted_cannot_verify() {
        let mut ledger = ledger_with(&[ready(), claim(), submit(), send_back(), claim()]);
        let bob = record(
            Kind::Declared,
            "bob",
            json!({"change": "submit", "story": STORY, "team": "rally", "evidence": "def"}),
        );
        apply_next(&mut ledger, &bob).unwrap();
        assert!(matches!(
            apply_next(&mut ledger, &verify_as("alice")),
            Err(LedgerError::SelfVerification { .. })
        ));
    }

    #[test]
    fn names_compare_without_case_or_surrounding_space() {
        for name in ["Alice", "alice ", " ALICE", "Rally"] {
            let mut ledger = ledger_with(&[ready(), claim(), submit()]);
            assert!(
                matches!(
                    apply_next(&mut ledger, &verify_as(name)),
                    Err(LedgerError::SelfVerification { .. })
                ),
                "{name:?}"
            );
        }
        let mut ledger = ledger_with(&[ready(), claim()]);
        let body = json!({"change": "release", "story": STORY, "team": " RALLY "});
        apply_next(&mut ledger, &team(body)).unwrap();
        assert_eq!(state(&ledger), State::Ready);
    }

    #[test]
    fn a_correction_without_a_state_only_annotates() {
        let mut ledger = ledger_with(&[ready(), claim()]);
        let before = ledger.story(STORY).unwrap().clone();
        let note = record(
            Kind::Declared,
            "someone-else",
            json!({"change": "correct", "story": STORY, "corrects": 2, "note": "I disagree"}),
        );
        apply_next(&mut ledger, &note).unwrap();
        let story = ledger.story(STORY).unwrap();
        assert_eq!(story.state, before.state);
        assert_eq!(story.team, before.team);
        assert_eq!(story.history[1].corrected_by, vec![3]);
    }

    #[test]
    fn a_verifier_can_correct_a_verification() {
        let mut ledger = ledger_with(&[ready(), claim(), submit(), verify()]);
        let correction = verifier(json!({
            "change": "correct", "story": STORY, "corrects": 4,
            "note": "verified the wrong commit", "state": "in_review"
        }));
        apply_next(&mut ledger, &correction).unwrap();
        assert_eq!(state(&ledger), State::InReview);
    }

    #[test]
    fn the_submitter_cannot_correct_as_a_verifier() {
        let mut ledger = ledger_with(&[ready(), claim(), submit()]);
        let own = record(
            Kind::Verified,
            "alice",
            json!({"change": "correct", "story": STORY, "corrects": 3, "note": "x",
                   "state": "verified"}),
        );
        assert!(matches!(
            apply_next(&mut ledger, &own),
            Err(LedgerError::SelfVerification { .. })
        ));
    }

    #[test]
    fn refuses_records_out_of_order() {
        let mut ledger = ledger_with(&[ready()]);
        for seq in [0, 1, 3] {
            assert_eq!(
                ledger.apply(seq, &claim()),
                Err(LedgerError::OutOfOrder {
                    expected: 2,
                    found: seq
                })
            );
        }
    }

    #[test]
    fn replay_gives_identical_state_every_time() {
        let records = [
            team(json!({"change": "ready", "story": "RAL-9"})),
            ready(),
            team(json!({"change": "ready", "story": "CAT-1"})),
            claim(),
            submit(),
            send_back(),
            claim(),
        ];
        let first = serde_json::to_string(&ledger_with(&records)).unwrap();
        for _ in 0..20 {
            assert_eq!(
                serde_json::to_string(&ledger_with(&records)).unwrap(),
                first
            );
        }
    }

    #[test]
    fn state_as_of_any_past_record_can_be_rebuilt() {
        let records = [
            ready(),
            claim(),
            submit(),
            send_back(),
            claim(),
            submit(),
            verify(),
        ];
        let expected = [
            State::Ready,
            State::Claimed,
            State::InReview,
            State::SentBack,
            State::Claimed,
            State::InReview,
            State::Verified,
        ];
        let mut live = Ledger::default();
        for (index, record) in records.iter().enumerate() {
            apply_next(&mut live, record).unwrap();
            let rebuilt = ledger_with(&records[..=index]);
            assert_eq!(rebuilt, live, "as of record {}", index + 1);
            assert_eq!(state(&rebuilt), expected[index]);
        }
    }
}
