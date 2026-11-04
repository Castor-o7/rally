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

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::record::{Kind, Record, Source};

const MAX_TEAM_CHARS: usize = 64;
const MAX_NOTE_CHARS: usize = 4_000;
const MAX_EVIDENCE_CHARS: usize = 1_000;
const MAX_STORY_ID_BYTES: usize = 32;

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

/// What a team says about itself (RAL-2). Declared, so it's the team's account and not a fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamStatus {
    Available,
    Working,
    Blocked,
}

/// What a record does to a story or team. This is the `body` of a record.
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
    /// Someone other than the submitter confirms the story, naming the commit they read (RAL-31).
    /// A new record must name it; records written before RAL-31 replay without one.
    Verify {
        story: String,
        note: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
    /// Someone other than the submitter returns the story, saying what's missing, and may name the
    /// commit they read.
    SendBack {
        story: String,
        note: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
    /// The story's work was merged at a commit (RAL-33). Merged is independent of Verified: it's
    /// allowed in any state and changes none. It names no merger, since every session shares one
    /// GitHub account, so the record's source says only who recorded it.
    Merge { story: String, commit: String },
    /// Someone asks for a read of the story at a commit before it's merged (RAL-32). Allowed in any
    /// state, since the ask can come before the submission. It stays open until a verify or
    /// send-back answers it, or it's withdrawn.
    RequestRead {
        story: String,
        commit: String,
        note: String,
    },
    /// The requester, by the same declared name, or a verifier withdraws a requested read.
    WithdrawRead {
        story: String,
        request: u64,
        note: String,
    },
    /// Annotates an earlier record of the same story, which stays in the history. Can also set the
    /// state: a team can only do that for a story it holds, naming itself in `team`, and moving a
    /// story into or out of a verifier's state takes a verifier. Without a state, it's only an
    /// annotation, which anyone can add, and `team` is ignored. Teams can record a disagreement
    /// this way, and it changes nothing else. Setting the state to Verified names the commit read,
    /// as a verification does (RAL-31); `commit` is refused with any other state.
    Correct {
        story: String,
        corrects: u64,
        note: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<State>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        team: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
    /// A team declares its own state. Being blocked needs a note saying on what.
    TeamState {
        team: String,
        status: TeamStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        story: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
}

impl Change {
    /// The story this change is about. A team's state may only mention one.
    fn story(&self) -> &str {
        match self {
            Self::TeamState { story, .. } => story.as_deref().unwrap_or_default(),
            Self::Ready { story }
            | Self::Claim { story, .. }
            | Self::Release { story, .. }
            | Self::Submit { story, .. }
            | Self::Verify { story, .. }
            | Self::SendBack { story, .. }
            | Self::Merge { story, .. }
            | Self::RequestRead { story, .. }
            | Self::WithdrawRead { story, .. }
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
    /// The commit the verifier read, while the story is Verified and its record named one (RAL-31).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_commit: Option<String>,
    /// The commit last recorded as merged (RAL-33), whatever the story's state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_commit: Option<String>,
    /// Reads asked for and not yet answered or withdrawn (RAL-32), oldest first. Visible to
    /// whoever merges, until a verify or send-back answers them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requested_reads: Vec<RequestedRead>,
    pub history: Vec<HistoryEntry>,
    /// The declared names of everyone who ever claimed or submitted it, compared as
    /// [`same_name`] does, so that checking a verifier doesn't search the whole history.
    ///
    /// Not serialized, so a `Story` must only ever be built by [`Ledger::apply`], never read back
    /// from JSON: one rebuilt without it would let an author verify their own story. The state the
    /// service keeps between requests is this same in-memory `Ledger`, built by replay and apply.
    #[serde(skip)]
    authors: BTreeSet<String>,
}

/// A read someone asked for before a merge (RAL-32).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RequestedRead {
    pub seq: u64,
    pub at_ms: i64,
    pub requested_by: String,
    pub commit: String,
    pub note: String,
    /// The commit recorded as merged while this request was still open, if any: the story was
    /// merged before its read, as catapp#78 was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_before_read: Option<String>,
}

impl Story {
    /// The verified and merged commits when the story has both and they differ (RAL-31). This is
    /// worked out from the ledger alone, so it's observed, not anyone's claim.
    pub fn commit_mismatch(&self) -> Option<(&str, &str)> {
        match (&self.verified_commit, &self.merged_commit) {
            (Some(verified), Some(merged)) if !verified.eq_ignore_ascii_case(merged) => {
                Some((verified, merged))
            }
            _ => None,
        }
    }
}

/// One declared state of a team.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TeamEntry {
    pub seq: u64,
    pub kind: Kind,
    pub source: Source,
    pub at_ms: i64,
    pub status: TeamStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub story: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A team's latest declared state and those before it. The record's kind, source and time travel
/// with it, so a reader can see it's declared and how old it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Team {
    pub current: TeamEntry,
    pub history: Vec<TeamEntry>,
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
    /// Text is over its limit. Holds the field's name and limit in characters.
    TooLong(&'static str, usize),
    /// A team's name has a control character.
    ControlCharacter(&'static str),
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
    /// Text looks like a secret (RAL-17). Holds the field and the format, never the text.
    SecretLike {
        field: String,
        format: &'static str,
    },
    /// A correction names a record that isn't an earlier record of this story.
    InvalidCorrectionTarget {
        story: String,
        corrects: u64,
    },
    /// A commit isn't a full hash (RAL-31). Doesn't hold the text, which a reader would see.
    InvalidCommit,
    /// A verification, or a correction into Verified, names no commit (RAL-31).
    MissingCommit,
    /// A withdrawal names a record that isn't an open requested read of this story (RAL-32).
    InvalidRequestTarget {
        story: String,
        request: u64,
    },
    /// Someone other than the requester, and not a verifier, tried to withdraw a read.
    NotTheRequester {
        story: String,
        name: String,
    },
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidChange(reason) => write!(f, "invalid change: {reason}"),
            Self::InvalidStoryId(id) => write!(f, "story ID {id:?} isn't in the form ABC-1"),
            Self::EmptyField(field) => write!(f, "{field} is empty"),
            Self::TooLong(field, limit) => write!(f, "{field} is over {limit} characters"),
            Self::ControlCharacter(field) => write!(f, "{field} has a control character"),
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
            Self::SecretLike { field, format } => write!(
                f,
                "{field} looks like it holds a {format}, so it wasn't recorded; if it's real, revoke it"
            ),
            Self::InvalidCorrectionTarget { story, corrects } => {
                write!(f, "record {corrects} isn't an earlier record of {story}")
            }
            Self::InvalidCommit => write!(f, "commit must be a full hash: 40 or 64 hex digits"),
            Self::MissingCommit => write!(f, "a verification names the commit it read"),
            Self::InvalidRequestTarget { story, request } => {
                write!(
                    f,
                    "record {request} isn't an open requested read of {story}"
                )
            }
            Self::NotTheRequester { story, name } => write!(
                f,
                "{name} didn't request this read of {story}; only the requester or a verifier can withdraw it"
            ),
        }
    }
}

impl std::error::Error for LedgerError {}

/// The state rebuilt from a ledger's records.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Ledger {
    last_seq: u64,
    stories: BTreeMap<String, Story>,
    teams: BTreeMap<String, Team>,
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

    /// Teams by name, in lower case without surrounding space, so `Rally` and `rally` are one team.
    pub fn teams(&self) -> &BTreeMap<String, Team> {
        &self.teams
    }

    /// Says whether a new record may be appended next, without changing anything: whether
    /// [`apply`](Self::apply) would accept it, and whether it meets the rules added since records
    /// were first written. Those apply to new records only, so a ledger written before them still
    /// replays (RAL-41). It copies only the one story the record is about, so it costs that
    /// story's size, not the ledger's.
    pub fn check(&self, seq: u64, record: &Record) -> Result<(), LedgerError> {
        self.check_new_rules(record)?;
        let mut trial = Self {
            last_seq: self.last_seq,
            ..Self::default()
        };
        // A team's state never depends on what came before, and a change that doesn't parse is
        // refused the same way with or without its story.
        if let Ok(change) = serde_json::from_value::<Change>(record.body().clone())
            && !matches!(change, Change::TeamState { .. })
            && let Some(story) = self.stories.get(change.story())
        {
            trial
                .stories
                .insert(change.story().to_owned(), story.clone());
        }
        trial.apply(seq, record)
    }

    /// Rules for new records that replay doesn't apply (RAL-42). A record they'd refuse isn't
    /// invalid in a ledger that already holds it.
    fn check_new_rules(&self, record: &Record) -> Result<(), LedgerError> {
        // Before anything else: a refused record must not reach the ledger, whatever else is wrong
        // with it, and the ledger can't forget one it took (RAL-17).
        check_secrets(record)?;
        // A body that isn't a change is refused by `apply`, with its own reason.
        let Ok(change) = serde_json::from_value::<Change>(record.body().clone()) else {
            return Ok(());
        };
        // So that `RAL-1` and `RAL-01` can't be two stories.
        let id = change.story();
        let names_a_story = !matches!(change, Change::TeamState { story: None, .. });
        if names_a_story && has_leading_zero(id) {
            return Err(LedgerError::InvalidStoryId(id.to_owned()));
        }
        // A verification names the commit it read (RAL-31). Heads move while work waits, so
        // Verified without a commit says nothing about what was read.
        if matches!(
            change,
            Change::Verify { commit: None, .. }
                | Change::Correct {
                    state: Some(State::Verified),
                    commit: None,
                    ..
                }
        ) {
            return Err(LedgerError::MissingCommit);
        }
        // A verified note, even one that changes no state, reads as a check by someone else, so
        // its author mustn't be one of the story's.
        if let Change::Correct { state: None, .. } = &change
            && record.kind() == Kind::Verified
            && let Some(story) = self.stories.get(id)
        {
            require_verifier(story, id, record)?;
        }
        Ok(())
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
        if let Change::TeamState {
            team,
            status,
            story,
            note,
        } = change
        {
            require_kind(record, Kind::Declared)?;
            let entry = TeamEntry {
                seq,
                kind: record.kind(),
                source: record.source().clone(),
                at_ms: record.at_ms(),
                status,
                story,
                note,
            };
            let key = team.trim().to_lowercase();
            match self.teams.get_mut(&key) {
                Some(team) => {
                    team.current = entry.clone();
                    team.history.push(entry);
                }
                None => {
                    let team = Team {
                        current: entry.clone(),
                        history: vec![entry],
                    };
                    self.teams.insert(key, team);
                }
            }
            self.last_seq = seq;
            return Ok(());
        }
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
                    verified_commit: None,
                    merged_commit: None,
                    requested_reads: Vec::new(),
                    history: Vec::new(),
                    authors: BTreeSet::new(),
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
        if matches!(change, Change::Claim { .. } | Change::Submit { .. })
            && let Some(name) = &record.source().declared_name
        {
            story.authors.insert(name_key(name));
        }
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
        Change::Ready { .. } | Change::TeamState { .. } => unreachable!("handled by the caller"),
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
        Change::Verify { commit, .. } => {
            if story.state != State::InReview {
                return Err(refuse(story.state));
            }
            require_verifier(story, id, record)?;
            story.state = State::Verified;
            story.verified_commit = commit.clone();
            // A read answers every open request for one.
            story.requested_reads.clear();
        }
        Change::SendBack { .. } => {
            if story.state != State::InReview {
                return Err(refuse(story.state));
            }
            require_verifier(story, id, record)?;
            story.state = State::SentBack;
            story.requested_reads.clear();
        }
        Change::Merge { commit, .. } => {
            // Any state, and the state stays: a merge is a fact about the repository, not a
            // judgement of the work.
            require_kind(record, Kind::Declared)?;
            story.merged_commit = Some(commit.clone());
            // The request stays open, and now shows the merge got there first.
            for read in &mut story.requested_reads {
                read.merged_before_read = Some(commit.clone());
            }
        }
        Change::RequestRead { commit, note, .. } => {
            require_kind(record, Kind::Declared)?;
            story.requested_reads.push(RequestedRead {
                seq,
                at_ms: record.at_ms(),
                requested_by: record.source().declared_name.clone().unwrap_or_default(),
                commit: commit.clone(),
                note: note.clone(),
                merged_before_read: None,
            });
        }
        Change::WithdrawRead { request, .. } => {
            let index = story
                .requested_reads
                .iter()
                .position(|read| read.seq == *request)
                .ok_or_else(|| LedgerError::InvalidRequestTarget {
                    story: id.to_owned(),
                    request: *request,
                })?;
            match record.kind() {
                Kind::Verified => {}
                Kind::Declared => {
                    let name = record.source().declared_name.as_deref().unwrap_or_default();
                    if !same_name(&story.requested_reads[index].requested_by, name) {
                        return Err(LedgerError::NotTheRequester {
                            story: id.to_owned(),
                            name: name.to_owned(),
                        });
                    }
                }
                Kind::Observed => {
                    return Err(LedgerError::WrongKind {
                        expected: Kind::Declared,
                        found: Kind::Observed,
                    });
                }
            }
            story.requested_reads.remove(index);
        }
        Change::Correct {
            corrects,
            state,
            team,
            commit,
            ..
        } => {
            // History is in record order, so the target can be found without a scan.
            let target = story
                .history
                .binary_search_by_key(corrects, |entry| entry.seq)
                .ok();
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
                if new_state.is_verifiers() {
                    story.requested_reads.clear();
                }
                // The verified commit is the one read for the current Verified state, so leaving
                // that state drops it, and entering it takes the correction's.
                story.verified_commit = if *new_state == State::Verified {
                    commit.clone()
                } else {
                    None
                };
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
    // Anyone who ever claimed or submitted this story counts as an author, as does the team
    // holding it.
    let authored = story.authors.contains(&name_key(name));
    let holds = story
        .team
        .as_deref()
        .is_some_and(|team| same_name(team, name));
    if authored || holds {
        return Err(LedgerError::SelfVerification {
            story: id.to_owned(),
            name: name.to_owned(),
        });
    }
    Ok(())
}

/// Compares declared names, ignoring case and surrounding space, to catch honest slips.
fn same_name(a: &str, b: &str) -> bool {
    name_key(a) == name_key(b)
}

fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn check_fields(change: &Change) -> Result<(), LedgerError> {
    let id = change.story();
    let story_is_optional = matches!(change, Change::TeamState { story: None, .. });
    if !story_is_optional && !is_story_id(id) {
        return Err(LedgerError::InvalidStoryId(id.to_owned()));
    }
    if let Change::TeamState { status, note, .. } = change {
        let has_note = note.as_deref().is_some_and(|n| !n.trim().is_empty());
        if note.is_some() && !has_note || *status == TeamStatus::Blocked && !has_note {
            return Err(LedgerError::EmptyField("note"));
        }
    }
    if let Change::Correct {
        team: Some(team), ..
    } = change
        && team.trim().is_empty()
    {
        return Err(LedgerError::EmptyField("team"));
    }
    if let Change::Correct {
        commit: Some(_),
        state,
        ..
    } = change
        && *state != Some(State::Verified)
    {
        return Err(LedgerError::InvalidChange(
            "a correction names a commit only when it sets the state to verified".into(),
        ));
    }
    let commit = match change {
        Change::Verify { commit, .. }
        | Change::SendBack { commit, .. }
        | Change::Correct { commit, .. } => commit.as_deref(),
        Change::Merge { commit, .. } | Change::RequestRead { commit, .. } => Some(commit.as_str()),
        _ => None,
    };
    if commit.is_some_and(|commit| !is_commit(commit)) {
        return Err(LedgerError::InvalidCommit);
    }
    let fields: &[(&'static str, &str)] = match change {
        Change::Ready { .. } | Change::Merge { .. } => &[],
        Change::Claim { team, .. }
        | Change::Release { team, .. }
        | Change::TeamState { team, .. } => &[("team", team)],
        Change::Submit { team, evidence, .. } => &[("team", team), ("evidence", evidence)],
        Change::Verify { note, .. }
        | Change::SendBack { note, .. }
        | Change::RequestRead { note, .. }
        | Change::WithdrawRead { note, .. }
        | Change::Correct { note, .. } => &[("note", note)],
    };
    for (name, value) in fields {
        if value.trim().is_empty() {
            return Err(LedgerError::EmptyField(name));
        }
        check_limit(name, value)?;
    }
    // Optional fields aren't in the list above.
    match change {
        Change::Correct {
            team: Some(team), ..
        } => check_limit("team", team),
        Change::TeamState {
            note: Some(note), ..
        } => check_limit("note", note),
        _ => Ok(()),
    }
}

/// Refuses a record with secret-looking text in any string its body carries, or in its declared
/// name. Not the credential, which is how the service knows the writer, not text it wrote.
fn check_secrets(record: &Record) -> Result<(), LedgerError> {
    fn walk(field: &str, value: &serde_json::Value) -> Result<(), LedgerError> {
        match value {
            serde_json::Value::String(text) => match crate::secrets::find(text) {
                Some(format) => Err(LedgerError::SecretLike {
                    field: field.to_owned(),
                    format: format.name(),
                }),
                None => Ok(()),
            },
            serde_json::Value::Array(items) => items.iter().try_for_each(|v| walk(field, v)),
            serde_json::Value::Object(map) => map.iter().try_for_each(|(k, v)| walk(k, v)),
            _ => Ok(()),
        }
    }
    walk("body", record.body())?;
    if let Some(name) = &record.source().declared_name {
        walk("declared name", &serde_json::Value::String(name.clone()))?;
    }
    Ok(())
}

/// Limits on text that teams write and others read (RAL-16). Names have to fit on a line.
fn check_limit(name: &'static str, value: &str) -> Result<(), LedgerError> {
    let limit = match name {
        "team" => MAX_TEAM_CHARS,
        "evidence" => MAX_EVIDENCE_CHARS,
        _ => MAX_NOTE_CHARS,
    };
    if value.chars().count() > limit {
        return Err(LedgerError::TooLong(name, limit));
    }
    if name == "team" && value.chars().any(char::is_control) {
        return Err(LedgerError::ControlCharacter(name));
    }
    Ok(())
}

/// Whether a story ID's number starts with a zero, other than the number 0 itself.
fn has_leading_zero(id: &str) -> bool {
    id.split_once('-')
        .is_some_and(|(_, number)| number.len() > 1 && number.starts_with('0'))
}

/// A full commit hash: 40 hex digits, or 64 for a SHA-256 repository. Short hashes are refused,
/// since one can come to name two commits.
fn is_commit(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64) && commit.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_story_id(id: &str) -> bool {
    id.len() <= MAX_STORY_ID_BYTES
        && id.split_once('-').is_some_and(|(prefix, number)| {
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
    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
    const OTHER_COMMIT: &str = "fedcba9876543210fedcba9876543210fedcba98";

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
        verifier(json!({
            "change": "verify", "story": STORY, "note": "read the diff", "commit": COMMIT
        }))
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
    fn a_verification_records_the_commit_it_read() {
        let ledger = ledger_with(&[ready(), claim(), submit(), verify()]);
        let story = ledger.story(STORY).unwrap();
        assert_eq!(story.verified_commit.as_deref(), Some(COMMIT));
        assert_eq!(story.merged_commit, None);
        assert_eq!(story.commit_mismatch(), None);
    }

    #[test]
    fn a_new_verification_without_a_commit_is_refused_but_an_old_one_replays() {
        let old = verifier(json!({"change": "verify", "story": STORY, "note": "read the diff"}));
        let ledger = ledger_with(&[ready(), claim(), submit()]);
        assert_eq!(
            ledger.check(ledger.last_seq() + 1, &old),
            Err(LedgerError::MissingCommit)
        );
        // Written before RAL-31, so it's in the ledger and still replays.
        let replayed = ledger_with(&[ready(), claim(), submit(), old]);
        let story = replayed.story(STORY).unwrap();
        assert_eq!(story.state, State::Verified);
        assert_eq!(story.verified_commit, None);
    }

    #[test]
    fn a_commit_must_be_a_full_hash() {
        for bad in [
            "abc123",
            "",
            " ",
            &COMMIT[..39],
            &format!("{COMMIT}0"),
            &format!("g{}", &COMMIT[1..]),
        ] {
            let mut ledger = ledger_with(&[ready(), claim(), submit()]);
            let short = verifier(json!({
                "change": "verify", "story": STORY, "note": "fine", "commit": bad
            }));
            assert_eq!(
                apply_next(&mut ledger, &short),
                Err(LedgerError::InvalidCommit),
                "{bad:?}"
            );
            let merge = team(json!({"change": "merge", "story": STORY, "commit": bad}));
            assert_eq!(
                apply_next(&mut ledger, &merge),
                Err(LedgerError::InvalidCommit),
                "{bad:?}"
            );
            assert_eq!(state(&ledger), State::InReview);
        }
        // Upper case and a SHA-256 hash are hashes too.
        for good in [COMMIT.to_uppercase(), "ab".repeat(32)] {
            let mut ledger = ledger_with(&[ready()]);
            let merge = team(json!({"change": "merge", "story": STORY, "commit": good}));
            assert!(apply_next(&mut ledger, &merge).is_ok());
        }
    }

    #[test]
    fn a_merge_is_recorded_in_any_state_and_changes_none() {
        let paths: [&[Record]; 5] = [
            &[ready()],
            &[ready(), claim()],
            &[ready(), claim(), submit()],
            &[ready(), claim(), submit(), verify()],
            &[ready(), claim(), submit(), send_back()],
        ];
        for path in paths {
            let mut ledger = ledger_with(path);
            let before = ledger.story(STORY).unwrap().clone();
            let merge = team(json!({"change": "merge", "story": STORY, "commit": OTHER_COMMIT}));
            apply_next(&mut ledger, &merge).unwrap();
            let after = ledger.story(STORY).unwrap();
            assert_eq!(after.state, before.state);
            assert_eq!(after.team, before.team);
            assert_eq!(after.submitted_by, before.submitted_by);
            assert_eq!(after.verified_commit, before.verified_commit);
            assert_eq!(after.merged_commit.as_deref(), Some(OTHER_COMMIT));
            assert_eq!(after.history.len(), before.history.len() + 1);
            // The record says who recorded the merge, never who merged.
            let entry = after.history.last().unwrap();
            assert_eq!(entry.kind, Kind::Declared);
            assert_eq!(
                entry.change,
                Change::Merge {
                    story: STORY.into(),
                    commit: OTHER_COMMIT.into()
                }
            );
        }
    }

    #[test]
    fn a_merge_needs_a_declared_record() {
        let mut ledger = ledger_with(&[ready()]);
        let merge = verifier(json!({"change": "merge", "story": STORY, "commit": COMMIT}));
        assert!(matches!(
            apply_next(&mut ledger, &merge),
            Err(LedgerError::WrongKind { .. })
        ));
    }

    #[test]
    fn a_merged_commit_that_differs_from_the_verified_one_is_observed() {
        let merge =
            |commit: &str| team(json!({"change": "merge", "story": STORY, "commit": commit}));
        // Merged before verification, as catapp#78 was: nothing to compare yet.
        let mut ledger = ledger_with(&[ready(), claim(), submit(), merge(OTHER_COMMIT)]);
        assert_eq!(ledger.story(STORY).unwrap().commit_mismatch(), None);
        apply_next(&mut ledger, &verify()).unwrap();
        assert_eq!(
            ledger.story(STORY).unwrap().commit_mismatch(),
            Some((COMMIT, OTHER_COMMIT))
        );
        // A later merge of the verified commit, in either case, clears it.
        apply_next(&mut ledger, &merge(&COMMIT.to_uppercase())).unwrap();
        assert_eq!(ledger.story(STORY).unwrap().commit_mismatch(), None);
        // Sending the story back and verifying another commit compares that one.
        let ledger = ledger_with(&[
            ready(),
            claim(),
            submit(),
            send_back(),
            merge(COMMIT),
            claim(),
            submit(),
        ]);
        assert_eq!(ledger.story(STORY).unwrap().commit_mismatch(), None);
    }

    #[test]
    fn a_correction_into_verified_names_its_commit_and_leaving_drops_it() {
        let correction = |state: &str, commit: Option<&str>| {
            let mut body = json!({
                "change": "correct", "story": STORY, "corrects": 4, "note": "x", "state": state
            });
            if let Some(commit) = commit {
                body["commit"] = json!(commit);
            }
            verifier(body)
        };
        let mut ledger = ledger_with(&[ready(), claim(), submit(), verify()]);
        apply_next(&mut ledger, &correction("in_review", None)).unwrap();
        assert_eq!(ledger.story(STORY).unwrap().verified_commit, None);
        assert_eq!(
            ledger.check(ledger.last_seq() + 1, &correction("verified", None)),
            Err(LedgerError::MissingCommit)
        );
        assert!(matches!(
            apply_next(&mut ledger, &correction("in_review", Some(OTHER_COMMIT))),
            Err(LedgerError::InvalidChange(_))
        ));
        apply_next(&mut ledger, &correction("verified", Some(OTHER_COMMIT))).unwrap();
        let story = ledger.story(STORY).unwrap();
        assert_eq!(story.state, State::Verified);
        assert_eq!(story.verified_commit.as_deref(), Some(OTHER_COMMIT));
    }

    fn request_read() -> Record {
        team(json!({
            "change": "request_read", "story": STORY, "commit": COMMIT, "note": "read before merge"
        }))
    }

    fn withdraw_as(name: &str, kind: Kind, request: u64) -> Record {
        record(
            kind,
            name,
            json!({"change": "withdraw_read", "story": STORY, "request": request, "note": "no need"}),
        )
    }

    #[test]
    fn a_requested_read_is_open_until_a_read_answers_it() {
        // Asked for while still Claimed, before the submission.
        let mut ledger = ledger_with(&[ready(), claim(), request_read()]);
        let story = ledger.story(STORY).unwrap();
        assert_eq!(story.state, State::Claimed);
        let read = &story.requested_reads[0];
        assert_eq!(
            (read.seq, read.commit.as_str(), read.requested_by.as_str()),
            (3, COMMIT, "alice")
        );
        assert_eq!(read.merged_before_read, None);
        apply_next(&mut ledger, &submit()).unwrap();
        assert_eq!(ledger.story(STORY).unwrap().requested_reads.len(), 1);
        apply_next(&mut ledger, &verify()).unwrap();
        assert!(ledger.story(STORY).unwrap().requested_reads.is_empty());
        // A send-back is a read too.
        let ledger = ledger_with(&[ready(), claim(), submit(), request_read(), send_back()]);
        assert!(ledger.story(STORY).unwrap().requested_reads.is_empty());
        // So is a verifier's correction into a verifier's state.
        let mut ledger = ledger_with(&[ready(), claim(), submit(), verify(), request_read()]);
        let correction = verifier(json!({
            "change": "correct", "story": STORY, "corrects": 4, "note": "x", "state": "sent_back"
        }));
        apply_next(&mut ledger, &correction).unwrap();
        assert!(ledger.story(STORY).unwrap().requested_reads.is_empty());
    }

    #[test]
    fn a_requested_read_is_withdrawn_by_its_requester_or_a_verifier() {
        let mut ledger = ledger_with(&[ready(), claim(), request_read()]);
        assert!(matches!(
            apply_next(&mut ledger, &withdraw_as("bob", Kind::Declared, 3)),
            Err(LedgerError::NotTheRequester { .. })
        ));
        assert!(matches!(
            apply_next(&mut ledger, &withdraw_as("alice", Kind::Declared, 2)),
            Err(LedgerError::InvalidRequestTarget { request: 2, .. })
        ));
        assert_eq!(ledger.story(STORY).unwrap().requested_reads.len(), 1);
        apply_next(&mut ledger, &withdraw_as(" Alice ", Kind::Declared, 3)).unwrap();
        assert!(ledger.story(STORY).unwrap().requested_reads.is_empty());
        // Withdrawn once; a second withdrawal has nothing to name.
        assert!(matches!(
            apply_next(&mut ledger, &withdraw_as("alice", Kind::Declared, 3)),
            Err(LedgerError::InvalidRequestTarget { request: 3, .. })
        ));
        let mut ledger = ledger_with(&[ready(), claim(), request_read()]);
        apply_next(&mut ledger, &withdraw_as("watch", Kind::Verified, 3)).unwrap();
        assert!(ledger.story(STORY).unwrap().requested_reads.is_empty());
    }

    #[test]
    fn a_merge_while_a_read_is_open_is_marked_on_the_request() {
        let merge = team(json!({"change": "merge", "story": STORY, "commit": OTHER_COMMIT}));
        let mut ledger = ledger_with(&[ready(), claim(), submit(), request_read(), merge]);
        let story = ledger.story(STORY).unwrap();
        assert_eq!(story.state, State::InReview);
        assert_eq!(
            story.requested_reads[0].merged_before_read.as_deref(),
            Some(OTHER_COMMIT)
        );
        apply_next(&mut ledger, &verify()).unwrap();
        let story = ledger.story(STORY).unwrap();
        assert!(story.requested_reads.is_empty());
        assert_eq!(story.commit_mismatch(), Some((COMMIT, OTHER_COMMIT)));
    }

    #[test]
    fn a_request_needs_a_declared_record_a_full_commit_and_a_note() {
        let mut ledger = ledger_with(&[ready()]);
        let short = team(json!({
            "change": "request_read", "story": STORY, "commit": "abc123", "note": "x"
        }));
        assert_eq!(
            apply_next(&mut ledger, &short),
            Err(LedgerError::InvalidCommit)
        );
        let silent = team(json!({
            "change": "request_read", "story": STORY, "commit": COMMIT, "note": " "
        }));
        assert_eq!(
            apply_next(&mut ledger, &silent),
            Err(LedgerError::EmptyField("note"))
        );
        let verified = verifier(json!({
            "change": "request_read", "story": STORY, "commit": COMMIT, "note": "x"
        }));
        assert!(matches!(
            apply_next(&mut ledger, &verified),
            Err(LedgerError::WrongKind { .. })
        ));
        assert!(ledger.story(STORY).unwrap().requested_reads.is_empty());
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
                json!({
                    "change": change, "story": STORY, "note": "looks good to me", "commit": COMMIT
                }),
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
            json!({"change": "verify", "story": STORY, "note": "fine", "commit": COMMIT}),
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
            json!({"change": "verify", "story": STORY, "note": "fine", "commit": COMMIT}),
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
            json!({"change": "verify", "story": STORY, "note": "fine", "commit": COMMIT}),
        )
    }

    fn correct_to(target: &str, corrects: u64) -> serde_json::Value {
        json!({
            "change": "correct", "story": STORY, "corrects": corrects, "note": "x",
            "state": target, "team": "rally"
        })
    }

    /// Applies `n` corrections to one story, each naming the newest record, which is the slowest
    /// one to find by searching from the start. Verified ones with a state also check authorship.
    fn time_corrections(n: u64) -> std::time::Duration {
        let mut ledger = ledger_with(&[ready(), claim(), submit()]);
        let started = std::time::Instant::now();
        for _ in 0..n {
            let newest = ledger.last_seq();
            let body = if newest.is_multiple_of(2) {
                json!({"change": "correct", "story": STORY, "corrects": newest, "note": "x"})
            } else {
                json!({"change": "correct", "story": STORY, "corrects": newest, "note": "x",
                       "state": "in_review"})
            };
            apply_next(&mut ledger, &verifier(body)).unwrap();
        }
        started.elapsed()
    }

    #[test]
    fn corrections_replay_in_linear_time() {
        // Measured before the fix, in a release build: 4 times the corrections took 9 times as long.
        let time = |n| (0..3).map(|_| time_corrections(n)).min().unwrap();
        let (small, large) = (time(3_000), time(12_000));
        let ratio = large.as_secs_f64() / small.as_secs_f64();
        assert!(
            ratio < 8.0,
            "4 times the corrections took {ratio:.1} times as long ({small:?}, {large:?})"
        );
    }

    #[test]
    fn a_submitter_cannot_add_a_verified_annotation_to_their_own_story() {
        let ledger = ledger_with(&[ready(), claim(), submit()]);
        let note =
            json!({"change": "correct", "story": STORY, "corrects": 3, "note": "looks good"});
        for author in ["alice", "rally"] {
            let own = record(Kind::Verified, author, note.clone());
            assert!(
                matches!(
                    ledger.check(4, &own),
                    Err(LedgerError::SelfVerification { .. })
                ),
                "{author}"
            );
        }
        // Someone else's verified note, and the author's declared one, are fine.
        assert!(ledger.check(4, &verifier(note.clone())).is_ok());
        assert!(ledger.check(4, &team(note)).is_ok());
    }

    #[test]
    fn a_new_story_id_with_a_leading_zero_is_refused_but_an_old_one_still_replays() {
        let padded = [
            team(json!({"change": "ready", "story": "RAL-01"})),
            team(
                json!({"change": "team_state", "team": "rally", "status": "working",
                        "story": "RAL-007"}),
            ),
        ];
        for record in &padded {
            assert!(
                matches!(
                    Ledger::default().check(1, record),
                    Err(LedgerError::InvalidStoryId(_))
                ),
                "{record:?}"
            );
            // A ledger written before the rule still replays.
            assert!(Ledger::replay([record]).is_ok());
        }
        for id in ["RAL-1", "RAL-10", "RAL-0"] {
            let record = team(json!({"change": "ready", "story": id}));
            assert!(Ledger::default().check(1, &record).is_ok(), "{id}");
        }
    }

    /// `check` agrees with `apply` on every record outside the rules for new records only, which
    /// `check_new_rules` adds; the two shapes those refuse are tested at the end, where `check`
    /// refuses what `apply` would still accept on replay.
    #[test]
    fn checking_a_record_agrees_with_applying_it_and_changes_nothing() {
        let candidates = [
            ready(),
            claim(),
            release(),
            submit(),
            verify(),
            send_back(),
            verify_as("alice"),
            verify_as("rally"),
            verifier(correct_to("claimed", 2)),
            team(correct_to("in_review", 3)),
            team(json!({"change": "correct", "story": STORY, "corrects": 99, "note": "x"})),
            team(json!({"change": "claim", "story": "RAL-9", "team": "rally"})),
            team(json!({"change": "team_state", "team": "rally", "status": "working"})),
            team(json!({"change": "nonsense"})),
        ];
        let prefixes: [&[Record]; 5] = [
            &[],
            &[ready()],
            &[ready(), claim()],
            &[ready(), claim(), submit()],
            &[ready(), claim(), submit(), send_back()],
        ];
        for prefix in prefixes {
            let ledger = ledger_with(prefix);
            for record in &candidates {
                let before = ledger.clone();
                let checked = ledger.check(ledger.last_seq() + 1, record);
                assert_eq!(ledger, before);
                let mut applied = ledger.clone();
                let result = apply_next(&mut applied, record);
                assert_eq!(
                    checked,
                    result,
                    "after {} records: {record:?}",
                    prefix.len()
                );
            }
            assert!(ledger.check(ledger.last_seq() + 2, &ready()).is_err());
        }
        // Where they're meant to disagree: rules for new records only.
        let submitted = ledger_with(&[ready(), claim(), submit()]);
        let own_note = record(
            Kind::Verified,
            "alice",
            json!({"change": "correct", "story": STORY, "corrects": 3, "note": "fine"}),
        );
        let padded = team(json!({"change": "ready", "story": "RAL-01"}));
        let leaked = team(json!({
            "change": "team_state", "team": "rally", "status": "working",
            "note": format!("pasted ghp_{}", "a".repeat(36)),
        }));
        for (ledger, record) in [
            (&submitted, &own_note),
            (&Ledger::default(), &padded),
            (&Ledger::default(), &leaked),
        ] {
            assert!(ledger.check(ledger.last_seq() + 1, record).is_err());
            assert!(ledger.clone().apply(ledger.last_seq() + 1, record).is_ok());
        }
    }

    #[test]
    fn a_secret_in_any_text_is_refused_by_name_not_by_value() {
        let token = format!("AKIA{}", "Q".repeat(16));
        let cases = [
            (
                team(
                    json!({"change": "team_state", "team": "rally", "status": "working", "note": token}),
                ),
                "note",
            ),
            (
                team(json!({"change": "claim", "story": STORY, "team": token})),
                "team",
            ),
            (
                record(
                    Kind::Declared,
                    &token,
                    json!({"change": "ready", "story": STORY}),
                ),
                "declared name",
            ),
        ];
        for (rec, field) in cases {
            let err = Ledger::default().check(1, &rec).unwrap_err();
            assert!(
                matches!(&err, LedgerError::SecretLike { field: f, format: "AWS access key" } if f == field),
                "{field}: {err:?}"
            );
            assert!(
                !err.to_string().contains(&token),
                "the refusal mustn't echo the secret"
            );
        }
        let mut ledger = ledger_with(&[ready(), claim()]);
        let evidence = team(json!({
            "change": "submit", "story": STORY, "team": "rally",
            "evidence": format!("token: {}", "0123456789abcdef".repeat(3)),
        }));
        assert!(matches!(
            ledger.check(ledger.last_seq() + 1, &evidence),
            Err(LedgerError::SecretLike { .. })
        ));
        // A plain commit hash is evidence, not a secret.
        let commit = team(json!({
            "change": "submit", "story": STORY, "team": "rally",
            "evidence": format!("Castor-o7/rally {}", "0123456789abcdef".repeat(3)),
        }));
        assert!(ledger.check(ledger.last_seq() + 1, &commit).is_ok());
        assert!(apply_next(&mut ledger, &commit).is_ok());
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
    fn whoever_claimed_a_story_cannot_verify_it() {
        let bob_claims = record(
            Kind::Declared,
            "bob",
            json!({"change": "claim", "story": STORY, "team": "rally"}),
        );
        let mut ledger = ledger_with(&[ready(), bob_claims, submit()]);
        assert!(matches!(
            apply_next(&mut ledger, &verify_as("Bob")),
            Err(LedgerError::SelfVerification { .. })
        ));
    }

    fn team_state(body: serde_json::Value) -> Record {
        team(body)
    }

    #[test]
    fn a_team_sets_its_state_and_keeps_its_history() {
        let mut ledger = Ledger::default();
        let bodies = [
            json!({"change": "team_state", "team": "Rally", "status": "working", "story": "RAL-2"}),
            json!({"change": "team_state", "team": "rally ", "status": "blocked",
                   "story": "RAL-2", "note": "waiting on MUS-44"}),
        ];
        for body in bodies {
            apply_next(&mut ledger, &team_state(body)).unwrap();
        }
        assert_eq!(ledger.teams().len(), 1);
        let rally = &ledger.teams()["rally"];
        assert_eq!(rally.current.status, TeamStatus::Blocked);
        assert_eq!(rally.current.kind, Kind::Declared);
        assert_eq!(rally.current.at_ms, 1_000);
        assert_eq!(rally.current.source.credential, "a1");
        assert_eq!(rally.history.len(), 2);
        assert_eq!(rally.history[0].status, TeamStatus::Working);
    }

    #[test]
    fn team_state_is_labelled_declared_when_read() {
        let body = json!({"change": "team_state", "team": "rally", "status": "available"});
        let ledger = ledger_with(&[team_state(body)]);
        let shown = serde_json::to_value(&ledger.teams()["rally"].current).unwrap();
        assert_eq!(shown["kind"], "declared");
    }

    #[test]
    fn team_state_can_only_be_declared() {
        let mut ledger = Ledger::default();
        let body = json!({"change": "team_state", "team": "rally", "status": "working"});
        let verified = record(Kind::Verified, "watch", body.clone());
        assert!(matches!(
            apply_next(&mut ledger, &verified),
            Err(LedgerError::WrongKind { .. })
        ));
        let source = Source {
            credential: "muster".into(),
            declared_name: None,
        };
        let observed = Record::new(Kind::Observed, source, 1_000, body).unwrap();
        assert!(matches!(
            apply_next(&mut ledger, &observed),
            Err(LedgerError::WrongKind { .. })
        ));
    }

    #[test]
    fn being_blocked_needs_a_note() {
        let mut ledger = Ledger::default();
        for note in [None, Some(" ")] {
            let mut body = json!({"change": "team_state", "team": "rally", "status": "blocked"});
            if let Some(note) = note {
                body["note"] = json!(note);
            }
            assert_eq!(
                apply_next(&mut ledger, &team_state(body)),
                Err(LedgerError::EmptyField("note"))
            );
        }
    }

    #[test]
    fn refuses_a_bad_team_state() {
        let mut ledger = Ledger::default();
        let bad = [
            (
                json!({"change": "team_state", "team": " ", "status": "working"}),
                LedgerError::EmptyField("team"),
            ),
            (
                json!({"change": "team_state", "team": "rally", "status": "working", "story": "x"}),
                LedgerError::InvalidStoryId("x".into()),
            ),
        ];
        for (body, error) in bad {
            assert_eq!(apply_next(&mut ledger, &team_state(body)), Err(error));
        }
        let unknown = json!({"change": "team_state", "team": "rally", "status": "resting"});
        assert!(matches!(
            apply_next(&mut ledger, &team_state(unknown)),
            Err(LedgerError::InvalidChange(_))
        ));
        assert_eq!(ledger, Ledger::default());
    }

    #[test]
    fn replay_rebuilds_team_state_as_of_a_past_record() {
        let records = [
            team_state(json!({"change": "team_state", "team": "rally", "status": "working"})),
            team_state(json!({"change": "team_state", "team": "rally", "status": "available"})),
        ];
        let then = ledger_with(&records[..1]);
        assert_eq!(then.teams()["rally"].current.status, TeamStatus::Working);
        let now = ledger_with(&records);
        assert_eq!(now.teams()["rally"].current.status, TeamStatus::Available);
    }

    #[test]
    fn refuses_text_over_its_limit() {
        let mut ledger = ledger_with(&[ready(), claim()]);
        let long = "x".repeat(4_001);
        let body = json!({"change": "correct", "story": STORY, "corrects": 2, "note": long});
        assert!(apply_next(&mut ledger, &team(body)).is_err());
        let long = "x".repeat(1_001);
        let body = json!({"change": "submit", "story": STORY, "team": "rally", "evidence": long});
        assert!(apply_next(&mut ledger, &team(body)).is_err());
        let long = "x".repeat(65);
        let body = json!({"change": "team_state", "team": long, "status": "working"});
        assert!(apply_next(&mut ledger, &team(body)).is_err());
    }

    #[test]
    fn refuses_control_characters_in_team_names() {
        let mut ledger = Ledger::default();
        let body = json!({"change": "team_state", "team": "ral\nly", "status": "working"});
        assert!(apply_next(&mut ledger, &team(body)).is_err());
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
