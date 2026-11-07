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
/// A new story's title (RAL-55), which a roadmap line repeats.
const MAX_TITLE_CHARS: usize = 200;
const MAX_STORY_ID_BYTES: usize = 32;
/// The most parts a story can have (RAL-79).
pub const MAX_PARTS: u32 = 64;

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
    /// Resting by choice: `rally wake` leaves it alone (RAL-54).
    Resting,
}

/// What a v3 designation does to the set of watchmen in force (RAL-63).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesignateAction {
    /// Adds the watchman, or replaces its earlier designation, leaving the others in force.
    Add,
    /// Ends this watchman's designation, leaving the others in force.
    End,
}

impl DesignateAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::End => "end",
        }
    }
}

/// What a record does to a story or team. This is the `body` of a record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
    /// A team marks a story as ready to claim.
    Ready { story: String },
    /// A team takes the next free number for a prefix, as a new ready story with a title
    /// (RAL-55). Refused unless the number is above every one that prefix has in the ledger, so
    /// two teams can't take one number: the writer's lock puts appends in order, and the loser
    /// tries the next.
    NewStory { story: String, title: String },
    /// A team takes a ready or sent-back story.
    Claim { story: String, team: String },
    /// The team holding a claim gives it up.
    Release { story: String, team: String },
    /// The team holding a claim submits it, with evidence such as a commit.
    Submit {
        story: String,
        team: String,
        evidence: String,
        /// What the team didn't test, in its own field rather than mixed into the evidence
        /// (RAL-5). Declared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        untested: Option<String>,
        /// An amendment (RAL-71): the submitter corrects the evidence of a story still in review,
        /// without a send-back. The state stays In review, and this becomes the latest submission,
        /// so Rally checks the new evidence and readers see it. The earlier submission stays in
        /// the history.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        amend: bool,
        /// The part of a multi-part story this is for (RAL-79). A part has its own submit, read
        /// and amendment, and leaves the story's own state alone. Records without it replay as
        /// before; a binary older than RAL-79 can't replay one with it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<u32>,
        /// How many parts the story has, as the team declares it with a part (RAL-79).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        of: Option<u32>,
    },
    /// Rally's own check of a submission's evidence against GitHub (RAL-5), observed. It's
    /// evidence, not a verdict: `ci: passed` is GitHub's word on that commit, and RAL-31 still
    /// compares the merged commit with the one read. One per submission, and it changes no state.
    EvidenceCheck {
        story: String,
        /// The submit record it checks.
        submit: u64,
        outcome: crate::evidence::Outcome,
    },
    /// Someone other than the submitter confirms the story, naming the commit they read (RAL-31).
    /// A new record must name it; records written before RAL-31 replay without one.
    Verify {
        story: String,
        note: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
        /// The reader's own team, declared, so reads can be spread across teams (RAL-57).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        team: Option<String>,
        /// The part of a multi-part story this is for (RAL-79). A part has its own submit, read
        /// and amendment, and leaves the story's own state alone. Records without it replay as
        /// before; a binary older than RAL-79 can't replay one with it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<u32>,
    },
    /// Someone other than the submitter returns the story, saying what's missing, and may name the
    /// commit they read.
    SendBack {
        story: String,
        note: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
        /// The reader's own team, declared (RAL-57).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        team: Option<String>,
        /// The part of a multi-part story this is for (RAL-79). A part has its own submit, read
        /// and amendment, and leaves the story's own state alone. Records without it replay as
        /// before; a binary older than RAL-79 can't replay one with it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<u32>,
    },
    /// The story's work was merged at a commit (RAL-33). Merged is independent of Verified: it's
    /// allowed in any state and changes none. It names no merger, since every session shares one
    /// GitHub account, so the record's source says only who recorded it.
    Merge { story: String, commit: String },
    /// An entry made on the fallback board during an outage, backfilled afterwards (RAL-30):
    /// `entry` is the change as it was made, by `entered_by` at `entered_at_ms`. The record's own
    /// source and time stay those of whoever backfilled it and when, so the gap stays visible and a
    /// backfilled record can't pass for a live one. The entry's author checks use `entered_by`.
    Backfill {
        entered_at_ms: i64,
        entered_by: String,
        entry: Box<Change>,
    },
    /// A team says what it's waiting on (RAL-20): with `story`, one of its stories is blocked, and
    /// without, the whole team is. It names what it waits on in `note`, and may name another story
    /// or team in `on_story` and `on_team`, which can be outside the ledger. The blocker is the
    /// record's number, and stays open until an unblock closes it.
    Block {
        team: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        story: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_story: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_team: Option<String>,
        note: String,
    },
    /// Anyone closes an open blocker, saying why. Who closed it is the record's source. Closing a
    /// question to the operator says where the answer came from (RAL-56); records written before
    /// that replay without it.
    Unblock {
        blocker: u64,
        note: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer: Option<Answer>,
    },
    /// A team shelves a story whose question to the operator has had no answer within the window
    /// agreed with the operator, so it can take its next story (RAL-56). It names the open blocker
    /// on the operator that holds the question; closing that blocker answers it, and the story
    /// comes back first. The service sets `window_ms` from its own setting and refuses any other,
    /// and the record's time, which the service stamps, must be at least that long after the
    /// question's.
    Shelve {
        blocker: u64,
        team: String,
        window_ms: u64,
    },
    /// Rally itself records the operator as away, when a shelve makes a run of unanswered shelves
    /// as long as the service's setting (RAL-56). Observed, and only Rally writes it.
    OperatorAway {
        entered_at_ms: i64,
        ends_at_ms: i64,
        shelves: Vec<u64>,
    },
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
    /// The operator designates a watchman, or with no watchman ends a designation (RAL-51). It
    /// carries the operator's SSH signature over [`designation_statement`], which names the record
    /// just before it. The ledger checks only its form and place; whether the signature is the
    /// operator's is checked by the service, against the operator's key, each time it reports a
    /// designation (see `designation.rs`), because the database can be written past the service.
    Designate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        watchman: Option<String>,
        /// The credential only the watchman's session holds (RAL-53). With it, the statement is
        /// v2 and a read counts as the watchman's only when made with this credential.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credential: Option<String>,
        /// Under v3 (RAL-63), whether this adds the watchman to the set in force or ends its
        /// designation, leaving the others. Without it, as under v1 and v2, the designation
        /// replaces the whole set.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<DesignateAction>,
        /// Under a v3 add, whose reads a merge waits for: `all`, `team:<team>` or
        /// `machine:<credential>` (RAL-63).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        after_seq: u64,
        after_hash: String,
        key_fingerprint: String,
        signature: String,
    },
    /// A team annotates an earlier record that names it, to correct it or disagree (RAL-19): one of
    /// its declared states, a blocker it raised or that waits on it, or a record of a story it
    /// held. It changes nothing else, so a verification's outcome stays with the verifier, and it
    /// can't annotate another annotation, so threads stay one deep.
    Annotate {
        corrects: u64,
        team: String,
        note: String,
        /// Commits the verifier also read, added to their own verification in force (RAL-67), for
        /// a story merged in several pull requests. Only from the verification's own credential.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        also_read: Vec<String>,
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
    /// A team records something standing in force (RAL-64): a hold and what releases it, a
    /// condition on the order of work, or a grant with its scope and whether it came directly from
    /// the operator or was relayed, and by whom. Declared, and only declared: a grant's record
    /// confers no authority, since approvals come from the operator directly. It stays in force
    /// until an `end_standing` ends it.
    Standing {
        standing: StandingKind,
        team: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        story: Option<String>,
        note: String,
        /// For a hold: what releases it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        releases: Option<String>,
        /// For a grant: what it allows, and where.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        /// For a grant: whether the operator gave it directly, or it was relayed, and by whom.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        given: Option<Answer>,
    },
    /// Anyone ends a hold, condition or grant in force, saying why (RAL-64).
    EndStanding { standing: u64, note: String },
    /// Someone opens a rally (RAL-76): its name, the projects it works on by story prefix, and the
    /// teams in it. The board the viewer shows starts clean here; nothing before it is removed
    /// from the ledger. Declared. Refused while another rally is open.
    OpenRally {
        name: String,
        projects: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        teams: Vec<String>,
    },
    /// Someone closes the open rally, saying why (RAL-76). It becomes a past rally, kept whole.
    CloseRally { note: String },
}

impl Change {
    /// The story this change is about. A team's state may only mention one.
    fn story(&self) -> &str {
        match self {
            Self::TeamState { story, .. } | Self::Block { story, .. } => {
                story.as_deref().unwrap_or_default()
            }
            Self::Standing { story, .. } => story.as_deref().unwrap_or_default(),
            Self::Unblock { .. }
            | Self::EndStanding { .. }
            | Self::OpenRally { .. }
            | Self::CloseRally { .. }
            | Self::Shelve { .. }
            | Self::OperatorAway { .. }
            | Self::Designate { .. }
            | Self::Annotate { .. } => "",
            Self::Backfill { entry, .. } => entry.story(),
            Self::Ready { story }
            | Self::NewStory { story, .. }
            | Self::Claim { story, .. }
            | Self::Release { story, .. }
            | Self::Submit { story, .. }
            | Self::EvidenceCheck { story, .. }
            | Self::Verify { story, .. }
            | Self::SendBack { story, .. }
            | Self::Merge { story, .. }
            | Self::RequestRead { story, .. }
            | Self::WithdrawRead { story, .. }
            | Self::Correct { story, .. } => story,
        }
    }
}

/// That an entry was made on the fallback board during an outage and backfilled afterwards
/// (RAL-30), as the backfiller declares. The entry's own `at_ms` and `source` are the backfill's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Backfill {
    pub entered_at_ms: i64,
    pub entered_by: String,
}

/// One step in a story's history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HistoryEntry {
    pub seq: u64,
    pub kind: Kind,
    pub source: Source,
    pub at_ms: i64,
    /// The change made: for a backfilled entry, the entry itself, with `backfilled` saying so.
    pub change: Change,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backfilled: Option<Backfill>,
    /// Later records that correct this one. The entry itself is never changed otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub corrected_by: Vec<u64>,
}

impl Story {
    /// The records of this story made while `team` held it, from its claim to its release or a
    /// correction back to Ready, the release included (RAL-19). The team record shows these, and a
    /// team may annotate only these, so the two match.
    pub fn held_by(&self, team: &str) -> BTreeSet<u64> {
        let mut holding = false;
        let mut seqs = BTreeSet::new();
        for entry in &self.history {
            match &entry.change {
                Change::Claim { team: by, .. } => holding = same_name(by, team),
                Change::Correct {
                    state: Some(State::Ready),
                    ..
                } => holding = false,
                _ => {}
            }
            let releasing =
                matches!(&entry.change, Change::Release { team: by, .. } if same_name(by, team));
            if holding || releasing {
                seqs.insert(entry.seq);
            }
            if releasing {
                holding = false;
            }
        }
        seqs
    }
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
    /// Commits the verifier added as also read to the verification in force (RAL-67), dropped
    /// with it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub also_read: Vec<String>,
    /// The commit last recorded as merged (RAL-33), whatever the story's state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_commit: Option<String>,
    /// Reads asked for and not yet answered or withdrawn (RAL-32), oldest first. Visible to
    /// whoever merges, until a verify or send-back answers them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requested_reads: Vec<RequestedRead>,
    /// Its parts, if it's a multi-part story (RAL-79), by number.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub parts: BTreeMap<u32, Part>,
    /// How many parts the team last said the story has (RAL-79), declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parts_of: Option<u32>,
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

/// One part of a multi-part story (RAL-79): submitted, then verified or sent back by someone
/// other than its submitter, while the story itself stays claimed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Part {
    /// In review, Verified or Sent back.
    pub state: State,
    /// The record of its latest submission.
    pub submitted_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submitted_by: Option<String>,
    pub evidence: String,
    /// The commit its reader read, while it's Verified and the read named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_commit: Option<String>,
}

impl Story {
    /// How many parts it has: the number the team last declared, else the highest submitted.
    pub fn parts_total(&self) -> u32 {
        let highest = self.parts.keys().next_back().copied().unwrap_or(0);
        self.parts_of.unwrap_or(0).max(highest)
    }

    /// How many of its parts are verified.
    pub fn parts_verified(&self) -> usize {
        self.parts
            .values()
            .filter(|part| part.state == State::Verified)
            .count()
    }

    /// Parts up to the declared total that were never submitted (RAL-79).
    fn missing_parts(&self) -> Vec<u32> {
        (1..=self.parts_total())
            .filter(|k| !self.parts.contains_key(k))
            .collect()
    }

    /// The parts still open: in review or sent back.
    fn open_parts(&self) -> Vec<u32> {
        self.parts
            .iter()
            .filter(|(_, part)| matches!(part.state, State::InReview | State::SentBack))
            .map(|(k, _)| *k)
            .collect()
    }
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

/// Something a team or story is waiting on (RAL-20), and, once closed, who closed it and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Blocker {
    pub seq: u64,
    pub kind: Kind,
    pub source: Source,
    pub at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backfilled: Option<Backfill>,
    /// The team that's waiting.
    pub team: String,
    /// The story that's waiting, when it's a story that's blocked rather than the whole team.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub story: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_story: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_team: Option<String>,
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed: Option<Closure>,
    /// Each time the waiting team shelved the story because the operator hadn't answered
    /// (RAL-56), oldest first. More than one only when an away hour ended and it was asked again.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shelves: Vec<Shelf>,
}

impl Blocker {
    pub fn waits_on_operator(&self) -> bool {
        self.on_team
            .as_deref()
            .is_some_and(|on| same_name(on, OPERATOR))
    }
}

/// The operator treated as away, after a run of unanswered shelves (RAL-56). Recorded by Rally
/// itself, as observed. It lasts until `ends_at_ms`, or until an answer from the operator ends it
/// early, by the service's time on that answer's record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Away {
    pub seq: u64,
    pub entered_at_ms: i64,
    pub ends_at_ms: i64,
    /// The shelves, by record number, with no answer between them, that caused it.
    pub shelves: Vec<u64>,
    /// The answer that ended it before `ends_at_ms`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_early_by: Option<u64>,
    /// When that answer was recorded, by the service's time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_early_at_ms: Option<i64>,
}

impl Away {
    /// Whether it's in force at `at_ms`.
    pub fn active_at(&self, at_ms: i64) -> bool {
        self.ended_early_by.is_none() && at_ms < self.ends_at_ms
    }

    /// When it ended, if it had by `at_ms`: when its hour ran out, or when an answer ended it
    /// early. Either way, what it held is asked again from then.
    pub fn ended_by(&self, at_ms: i64) -> Option<i64> {
        match self.ended_early_at_ms {
            Some(ended) => Some(ended),
            None => (at_ms >= self.ends_at_ms).then_some(self.ends_at_ms),
        }
    }
}

/// Where an answer from the operator came from (RAL-56): straight from the operator, or relayed,
/// and by whom. Declared, like everything an agent records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case", deny_unknown_fields)]
pub enum Answer {
    Direct,
    Relayed { by: String },
}

/// What a standing record is (RAL-64).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StandingKind {
    /// Work held until something releases it.
    Hold,
    /// A condition on the order of work.
    Condition,
    /// Something the operator allowed, with its scope. Recorded, never conferred.
    Grant,
}

/// A hold, condition or grant, as a team declared it, and its end if it has one (RAL-64).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Standing {
    pub seq: u64,
    pub kind: Kind,
    pub source: Source,
    pub at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backfilled: Option<Backfill>,
    pub standing: StandingKind,
    pub team: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub story: Option<String>,
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub releases: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub given: Option<Answer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended: Option<Closure>,
}

/// Who shelved a story waiting on the operator, and when (RAL-56).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Shelf {
    pub seq: u64,
    pub source: Source,
    pub at_ms: i64,
    pub team: String,
    pub window_ms: u64,
}

/// The team name a blocker uses to wait on the operator (RAL-56).
pub const OPERATOR: &str = "operator";

/// Who closed a blocker, when, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Closure {
    pub seq: u64,
    pub kind: Kind,
    pub source: Source,
    pub at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backfilled: Option<Backfill>,
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<Answer>,
}

/// A rally (RAL-76): what it works on and who's in it, as declared when it was opened, and how it
/// was closed, if it has been. The board shows only the open rally's work since it opened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Rally {
    pub seq: u64,
    pub kind: Kind,
    pub source: Source,
    pub at_ms: i64,
    pub name: String,
    pub projects: Vec<String>,
    pub teams: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed: Option<Closure>,
}

/// One declared state of a team.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TeamEntry {
    pub seq: u64,
    pub kind: Kind,
    pub source: Source,
    pub at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backfilled: Option<Backfill>,
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
    /// A submit of a story nobody holds a claim on: it was sent back, released or never claimed,
    /// so it needs a claim first (RAL-72).
    ClaimLapsed {
        story: String,
        from: State,
    },
    /// A story ID isn't in the `ABC-1` form.
    InvalidStoryId(String),
    /// A required text field is empty. Holds the field's name.
    EmptyField(&'static str),
    /// Text is over its limit. Holds the field's name and limit in characters.
    TooLong(&'static str, usize),
    /// A team's name has a control character.
    ControlCharacter(&'static str),
    /// A name has a character that shows nothing or changes the text's direction (RAL-41).
    InvisibleCharacter(&'static str),
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
    /// An unblock names a record that isn't an open blocker (RAL-20).
    InvalidBlockerTarget(u64),
    /// An `end_standing` names a record that isn't a hold, condition or grant in force (RAL-64).
    InvalidStandingTarget(u64),
    /// A rally is opened while another is open, which names the open one's record (RAL-76).
    RallyAlreadyOpen(u64),
    /// A rally is closed while none is open (RAL-76).
    NoRallyOpen,
    /// A rally's project isn't a story prefix, is named twice, or it names none (RAL-76).
    InvalidRallyProjects,
    /// A rally names a team twice (RAL-76).
    InvalidRallyTeams,
    /// A shelve names a blocker that isn't an open question to the operator about a story, held
    /// by the shelving team, or that's already shelved (RAL-56).
    InvalidShelf(u64, &'static str),
    /// A new record uses `operator` as its own team's name, which is kept for waiting on the
    /// operator, so no team can look like the operator (RAL-56).
    ReservedTeam,
    /// An evidence check that doesn't fit the story's submissions (RAL-5).
    InvalidEvidenceCheck(u64, &'static str),
    /// A submit of a story already in review, which an amendment does instead (RAL-71).
    AlreadyInReview(String),
    /// Something a part can't do in its state (RAL-79), with what to do instead.
    Part {
        story: String,
        part: u32,
        why: String,
    },
    /// A final verify of a story with parts still in review or sent back (RAL-79).
    PartsOpen {
        story: String,
        parts: Vec<u32>,
        /// Declared with `--of` and never submitted.
        missing: Vec<u32>,
    },
    /// An amendment of a story that isn't in review (RAL-71).
    NotInReview {
        story: String,
        from: State,
    },
    /// An amendment that changes neither the evidence nor what wasn't tested (RAL-71).
    NothingAmended(String),
    /// A read names a commit other than the amended evidence's (RAL-71).
    ReadBeforeAmendment {
        story: String,
        amended_at: u64,
        commit: String,
    },
    /// Someone other than the submitter tried to amend a submission (RAL-71).
    NotTheSubmitter {
        story: String,
        submitter: String,
    },
    /// A new story's number isn't above every one its prefix has in the ledger (RAL-55).
    NumberTaken {
        story: String,
        highest: u64,
    },
    /// A read names the story's own team as the reader's (RAL-57).
    ReaderIsAuthor(String),
    /// An away record that doesn't follow from the ledger (RAL-56).
    InvalidAway(&'static str),
    /// Closing a question to the operator must say where the answer came from, and closing any
    /// other blocker mustn't (RAL-56).
    AnswerSource(&'static str),
    /// A story is named as waiting on itself.
    BlockedOnItself(String),
    /// An annotation names a record that isn't an earlier record naming its team (RAL-19).
    InvalidAnnotationTarget {
        corrects: u64,
        team: String,
    },
    /// An annotation names another annotation; annotate the original (RAL-19).
    AnnotationOfAnnotation(u64),
    /// A designation doesn't name the record just before it (RAL-51).
    DesignationOutOfPlace {
        seq: u64,
        after_seq: u64,
    },
    /// A designation's fields aren't in their form (RAL-51). Holds which.
    InvalidDesignation(&'static str),
    /// The service refused a designation: no operator key, the wrong place, or a signature that
    /// isn't the operator's (RAL-51). Holds why.
    DesignationRefused(String),
    /// A backfilled entry's time isn't before the record's own (RAL-30).
    BackfillNotEarlier {
        entered_at_ms: i64,
        at_ms: i64,
    },
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidChange(reason) => write!(f, "invalid change: {reason}"),
            Self::InvalidStoryId(id) => write!(
                f,
                "story ID {id:?} isn't in the form ABC-123: capital letters, a hyphen, then a \
                 number with no leading zero and nothing after it, so DSM-45b and DSM-045 are \
                 refused; for a new story, take the next number with rally new-story <PREFIX>"
            ),
            Self::EmptyField(field) => write!(f, "{field} is empty"),
            Self::TooLong(field, limit) => write!(f, "{field} is over {limit} characters"),
            Self::ControlCharacter(field) => write!(f, "{field} has a control character"),
            Self::InvisibleCharacter(field) => write!(
                f,
                "{field} has an invisible or direction-changing character, so it could look like \
                 another name"
            ),
            Self::OutOfOrder { expected, found } => {
                write!(f, "expected record {expected}, found record {found}")
            }
            Self::UnknownStory(story) => write!(f, "{story} isn't in the ledger"),
            Self::AlreadyExists(story) => write!(f, "{story} is already in the ledger"),
            Self::InvalidTransition { story, from } => {
                write!(f, "{story} can't make this change from {from:?}")
            }
            Self::ClaimLapsed { story, from } => match from {
                State::SentBack => write!(
                    f,
                    "{story} was sent back, so its claim has lapsed: re-claim first with \
                     rally claim {story} --team <team>, then submit"
                ),
                _ => write!(
                    f,
                    "{story} isn't claimed (it's {from:?}): claim it first with \
                     rally claim {story} --team <team>, then submit"
                ),
            },
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
            Self::InvalidBlockerTarget(seq) => write!(f, "record {seq} isn't an open blocker"),
            Self::InvalidStandingTarget(seq) => {
                write!(f, "record {seq} isn't a hold, condition or grant in force")
            }
            Self::RallyAlreadyOpen(seq) => write!(
                f,
                "the rally opened at record {seq} is still open: close it first"
            ),
            Self::NoRallyOpen => write!(f, "no rally is open"),
            Self::InvalidRallyProjects => write!(
                f,
                "a rally names at least one project, each once and a story prefix such as RAL"
            ),
            Self::InvalidRallyTeams => write!(f, "a rally names each team once"),
            Self::InvalidShelf(seq, why) => write!(f, "can't shelve on record {seq}: {why}"),
            Self::ReservedTeam => write!(
                f,
                "\"{OPERATOR}\" is kept for waiting on the operator, so no team can use it as its name"
            ),
            Self::AnswerSource(why) => write!(f, "{why}"),
            Self::InvalidEvidenceCheck(seq, why) => {
                write!(f, "can't record an evidence check of record {seq}: {why}")
            }
            Self::AlreadyInReview(story) => write!(
                f,
                "{story} is already in review; to correct its evidence, amend it with \
                 rally amend {story} --team <team> --evidence <owner/repo@full commit>"
            ),
            Self::Part { story, part, why } => write!(f, "{story} part {part}: {why}"),
            Self::PartsOpen {
                story,
                parts,
                missing,
            } => {
                let list = |parts: &[u32]| {
                    parts
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let mut why = Vec::new();
                if !parts.is_empty() {
                    why.push(format!(
                        "parts in review or sent back ({}): read those first",
                        list(parts)
                    ));
                }
                if !missing.is_empty() {
                    why.push(format!(
                        "parts declared with --of and never submitted ({}): submit them, or \
                         the team amends the close with a lower --of",
                        list(missing)
                    ));
                }
                write!(f, "{story} can't be verified yet: {}", why.join("; "))
            }
            Self::NotInReview { story, from } => write!(
                f,
                "{story} is {from:?}, and only a submission still in review can be amended"
            ),
            Self::NothingAmended(story) => write!(
                f,
                "that's {story}'s evidence already, so there's nothing to amend"
            ),
            Self::ReadBeforeAmendment {
                story,
                amended_at,
                commit,
            } => write!(
                f,
                "{story}'s evidence was amended at record {amended_at} to commit {commit}: read \
                 that commit, then name it with --commit"
            ),
            Self::NotTheSubmitter { story, submitter } => write!(
                f,
                "{submitter} submitted {story}, so only {submitter} can amend its evidence"
            ),
            Self::NumberTaken { story, highest } => write!(
                f,
                "{story} isn't the next free number: its prefix already reaches {highest}"
            ),
            Self::ReaderIsAuthor(team) => {
                write!(
                    f,
                    "{team} holds this story, so it can't read it as another team"
                )
            }
            Self::InvalidAway(why) => write!(f, "invalid away record: {why}"),
            Self::BlockedOnItself(story) => write!(f, "{story} can't wait on itself"),
            Self::DesignationOutOfPlace { seq, after_seq } => write!(
                f,
                "a designation at record {seq} must name record {}, not {after_seq}; read the head again and sign again",
                seq - 1
            ),
            Self::InvalidDesignation(what) => write!(f, "a designation's {what}"),
            Self::InvalidAnnotationTarget { corrects, team } => write!(
                f,
                "record {corrects} isn't an earlier record that names {team}: one of its states, a \
                 blocker by or on it, or a record of a story it held"
            ),
            Self::AnnotationOfAnnotation(seq) => write!(
                f,
                "record {seq} is itself an annotation; annotate the record it annotates"
            ),
            Self::DesignationRefused(why) => write!(f, "the designation was refused: {why}"),
            Self::BackfillNotEarlier {
                entered_at_ms,
                at_ms,
            } => write!(
                f,
                "a backfilled entry's time, {entered_at_ms}, must be after the epoch and before the record's own, {at_ms}"
            ),
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
    /// The time of the last record applied, or 0 for an empty ledger: the moment a past state
    /// stands at (RAL-28).
    last_at_ms: i64,
    stories: BTreeMap<String, Story>,
    teams: BTreeMap<String, Team>,
    /// Every blocker by its record number, open and closed (RAL-20).
    blockers: BTreeMap<u64, Blocker>,
    /// Every hold, condition and grant by its record number, in force and ended (RAL-64).
    standings: BTreeMap<u64, Standing>,
    /// Every designation record, in order, whether or not its signature holds (RAL-51).
    designations: Vec<Designation>,
    /// Teams' annotations, by the record each annotates (RAL-19).
    annotations: BTreeMap<u64, Vec<Annotation>>,
    /// The records that are annotations, so that none is annotated in turn.
    annotation_seqs: BTreeSet<u64>,
    /// Shelves, by record number and time, since the last answer from the operator or away
    /// record (RAL-56).
    unanswered_shelves: Vec<(u64, i64)>,
    /// Every time the operator was treated as away, oldest first (RAL-56).
    aways: Vec<Away>,
    /// Every rally, oldest first; only the last can be open (RAL-76).
    rallies: Vec<Rally>,
}

/// A team's annotation of an earlier record (RAL-19). Declared, like the team's name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Annotation {
    pub seq: u64,
    pub kind: Kind,
    pub source: Source,
    pub at_ms: i64,
    pub team: String,
    pub note: String,
}

/// The fixed text the operator signs to designate a watchman, or with none to end a designation.
/// It names the record just before it by number and chain hash, so a signature can't be used
/// anywhere else in this ledger or in another.
/// With a credential, the statement is v2 and binds the designation to it (RAL-53); without one
/// it's v1, as every designation before RAL-53 was signed.
pub fn designation_statement(
    watchman: Option<&str>,
    credential: Option<&str>,
    after_seq: u64,
    after_hash: &str,
) -> String {
    let watchman = watchman.unwrap_or("none");
    match credential {
        Some(credential) => format!(
            "rally-designation v2\nwatchman: {watchman}\ncredential: {credential}\n\
             after: {after_seq}:{after_hash}\n"
        ),
        None => {
            format!("rally-designation v1\nwatchman: {watchman}\nafter: {after_seq}:{after_hash}\n")
        }
    }
}

/// The fixed text the operator signs for a v3 designation (RAL-63): one watchman added to the set
/// in force, bound to its credential and given a scope, or one ended. Like v1 and v2 it names the
/// record just before it, and its first line keeps a v3 signature from verifying as v1 or v2.
pub fn designation_statement_v3(
    action: DesignateAction,
    watchman: &str,
    credential: Option<&str>,
    scope: Option<&str>,
    after_seq: u64,
    after_hash: &str,
) -> String {
    let mut text = format!(
        "rally-designation v3\naction: {}\nwatchman: {watchman}\n",
        action.as_str()
    );
    if let Some(credential) = credential {
        text.push_str(&format!("credential: {credential}\n"));
    }
    if let Some(scope) = scope {
        text.push_str(&format!("scope: {scope}\n"));
    }
    text.push_str(&format!("after: {after_seq}:{after_hash}\n"));
    text
}

/// What a designation says, whichever version it is: the parts its statement is made from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Terms<'a> {
    pub watchman: Option<&'a str>,
    pub credential: Option<&'a str>,
    pub action: Option<DesignateAction>,
    pub scope: Option<&'a str>,
}

impl Terms<'_> {
    /// The statement the operator signs for these terms, after record `after_seq`.
    pub fn statement(&self, after_seq: u64, after_hash: &str) -> String {
        match (self.action, self.watchman) {
            (Some(action), Some(watchman)) => designation_statement_v3(
                action,
                watchman,
                self.credential,
                self.scope,
                after_seq,
                after_hash,
            ),
            _ => designation_statement(self.watchman, self.credential, after_seq, after_hash),
        }
    }
}

/// One designation record as the ledger holds it. Whether it's in force is for
/// `designation::report` to say.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Designation {
    pub seq: u64,
    pub source: Source,
    pub at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watchman: Option<String>,
    /// The credential the designation is bound to, under v2 (RAL-53).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    /// Under v3, an add or an end of one watchman (RAL-63).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<DesignateAction>,
    /// Under a v3 add, the watchman's scope (RAL-63).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub after_seq: u64,
    pub after_hash: String,
    pub key_fingerprint: String,
    pub signature: String,
}

impl Designation {
    pub fn statement(&self) -> String {
        self.terms().statement(self.after_seq, &self.after_hash)
    }

    pub fn terms(&self) -> Terms<'_> {
        Terms {
            watchman: self.watchman.as_deref(),
            credential: self.credential.as_deref(),
            action: self.action,
            scope: self.scope.as_deref(),
        }
    }
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

    /// The time of the last record applied, or 0 for an empty ledger (RAL-28).
    pub fn last_at_ms(&self) -> i64 {
        self.last_at_ms
    }

    pub fn story(&self, id: &str) -> Option<&Story> {
        self.stories.get(id)
    }

    pub fn stories(&self) -> &BTreeMap<String, Story> {
        &self.stories
    }

    /// Every designation record, in order (RAL-51).
    pub fn designations(&self) -> &[Designation] {
        &self.designations
    }

    /// Teams' annotations, by the record each annotates (RAL-19).
    pub fn annotations(&self) -> &BTreeMap<u64, Vec<Annotation>> {
        &self.annotations
    }

    /// The shelves since the last answer from the operator or away hour, across the whole rally,
    /// as of `at_ms`: once an away hour has run out, the run starts again (RAL-56).
    pub fn unanswered_shelves(&self, at_ms: i64) -> Vec<u64> {
        let since = self
            .aways
            .last()
            .and_then(|away| away.ended_by(at_ms))
            .unwrap_or(i64::MIN);
        self.unanswered_shelves
            .iter()
            .filter(|(_, shelved_at)| *shelved_at >= since)
            .map(|(seq, _)| *seq)
            .collect()
    }

    /// The highest number any story with `prefix` has in the ledger, or 0 (RAL-55).
    pub fn highest_number(&self, prefix: &str) -> u64 {
        let lead = format!("{prefix}-");
        self.stories
            .range(lead.clone()..)
            .take_while(|(id, _)| id.starts_with(&lead))
            .filter_map(|(id, _)| id[lead.len()..].parse::<u64>().ok())
            .max()
            .unwrap_or(0)
    }

    /// The latest time the operator was treated as away, if ever (RAL-56).
    pub fn operator_away(&self) -> Option<&Away> {
        self.aways.last()
    }

    /// Whether an open question to the operator is shelved as of `at_ms`: its last shelf came
    /// after its window last started. A shelf from before an away hour ended, by running out or by
    /// an answer, has been revisited, so the question is asked again (RAL-56).
    pub fn is_shelved(&self, blocker: &Blocker, at_ms: i64) -> bool {
        let start = self.question_start(blocker, at_ms);
        blocker
            .shelves
            .last()
            .is_some_and(|shelf| shelf.at_ms >= start)
    }

    /// When a question's window starts: when it was asked, or when an away hour that held it
    /// ended, whichever is later, as of `at_ms`.
    pub fn question_start(&self, blocker: &Blocker, at_ms: i64) -> i64 {
        self.aways
            .iter()
            .filter_map(|away| away.ended_by(at_ms))
            .filter(|ended| *ended > blocker.at_ms)
            .max()
            .unwrap_or(blocker.at_ms)
            .max(blocker.at_ms)
    }

    /// Whether earlier record `seq` names `team`: one of its declared states, a blocker it raised
    /// or that waits on it, or a record of a story it ever claimed.
    /// RAL-67: add commits the verifier also read to the verification at `corrects`, which must
    /// be the one in force on its story, from that verification's own credential and name.
    fn add_also_read(
        &mut self,
        corrects: u64,
        record: &Record,
        commits: &[String],
    ) -> Result<(), LedgerError> {
        let refuse = |why: &str| LedgerError::InvalidChange(format!("also-read: {why}"));
        if !commits.iter().all(|commit| is_commit(commit)) {
            return Err(refuse("each commit must be a full hash"));
        }
        let story = self
            .stories
            .values_mut()
            .find(|story| story.history.iter().any(|entry| entry.seq == corrects))
            .ok_or_else(|| refuse("the record annotated isn't a story's"))?;
        if story.state != State::Verified {
            return Err(refuse("the story isn't Verified"));
        }
        // The verification in force is the story's last record that made it Verified.
        let current = story.history.iter().rev().find(|entry| {
            matches!(
                entry.change,
                Change::Verify { part: None, .. }
                    | Change::Correct {
                        state: Some(State::Verified),
                        ..
                    }
            )
        });
        let Some(verify) = current.filter(|entry| entry.seq == corrects) else {
            return Err(refuse(
                "only the verification in force can have commits added",
            ));
        };
        let same_name = match (&verify.source.declared_name, &record.source().declared_name) {
            (Some(a), Some(b)) => same_name(a, b),
            _ => false,
        };
        if verify.source.credential != record.source().credential || !same_name {
            return Err(refuse(
                "only the verifier, with the verification's own credential",
            ));
        }
        for commit in commits {
            let commit = commit.to_ascii_lowercase();
            if !story.also_read.contains(&commit) {
                story.also_read.push(commit);
            }
        }
        Ok(())
    }

    fn names_team(&self, seq: u64, team: &str) -> bool {
        if self
            .teams
            .get(&name_key(team))
            .is_some_and(|t| t.history.iter().any(|entry| entry.seq == seq))
        {
            return true;
        }
        if self.blockers.get(&seq).is_some_and(|b| {
            same_name(&b.team, team) || b.on_team.as_deref().is_some_and(|t| same_name(t, team))
        }) {
            return true;
        }
        if self
            .standings
            .get(&seq)
            .is_some_and(|standing| same_name(&standing.team, team))
        {
            return true;
        }
        self.stories
            .values()
            .any(|story| story.held_by(team).contains(&seq))
    }

    /// Teams by name, in lower case without surrounding space, so `Rally` and `rally` are one team.
    pub fn teams(&self) -> &BTreeMap<String, Team> {
        &self.teams
    }

    /// Every blocker by its record number, open and closed (RAL-20).
    pub fn blockers(&self) -> &BTreeMap<u64, Blocker> {
        &self.blockers
    }

    /// Every hold, condition and grant by its record number, in force and ended (RAL-64).
    pub fn standings(&self) -> &BTreeMap<u64, Standing> {
        &self.standings
    }

    /// Every rally, oldest first (RAL-76).
    pub fn rallies(&self) -> &[Rally] {
        &self.rallies
    }

    /// The rally that's open, if one is (RAL-76).
    pub fn open_rally(&self) -> Option<&Rally> {
        self.rallies.last().filter(|rally| rally.closed.is_none())
    }

    /// Says whether a new record may be appended next, without changing anything: whether
    /// [`apply`](Self::apply) would accept it, and whether it meets the rules added since records
    /// were first written. Those apply to new records only, so a ledger written before them still
    /// replays (RAL-41). It copies only the one story the record is about, so it costs that
    /// story's size, not the ledger's.
    pub fn check(&self, seq: u64, record: &Record) -> Result<(), LedgerError> {
        self.check_new_rules(record)?;
        // An annotation's target can be anywhere, so it's checked against everything. They're rare.
        if matches!(
            serde_json::from_value::<Change>(record.body().clone()),
            Ok(Change::Annotate { .. })
        ) {
            return self.clone().apply(seq, record);
        }
        // The away hours and the run of unanswered shelves are small, and a shelve, an answer
        // or an away record depends on them (RAL-56).
        let mut trial = Self {
            last_seq: self.last_seq,
            unanswered_shelves: self.unanswered_shelves.clone(),
            aways: self.aways.clone(),
            // Opening or closing a rally depends only on the last one (RAL-76).
            rallies: self.rallies.last().cloned().into_iter().collect(),
            ..Self::default()
        };
        // A team's state never depends on what came before, and a change that doesn't parse is
        // refused the same way with or without its story.
        if let Ok(change) = serde_json::from_value::<Change>(record.body().clone()) {
            let change = match change {
                Change::Backfill { entry, .. } => *entry,
                change => change,
            };
            if let Change::Unblock { blocker, .. } | Change::Shelve { blocker, .. } = change {
                // Closing a blocker depends only on that blocker.
                if let Some(open) = self.blockers.get(&blocker) {
                    trial.blockers.insert(blocker, open.clone());
                }
            } else if let Change::EndStanding { standing, .. } = change {
                // Ending a standing record depends only on that record.
                if let Some(open) = self.standings.get(&standing) {
                    trial.standings.insert(standing, open.clone());
                }
            } else if !matches!(
                change,
                Change::TeamState { .. } | Change::Block { .. } | Change::Standing { .. }
            ) && let Some(story) = self.stories.get(change.story())
            {
                trial
                    .stories
                    .insert(change.story().to_owned(), story.clone());
            }
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
        let Ok(parsed) = serde_json::from_value::<Change>(record.body().clone()) else {
            return Ok(());
        };
        // A backfilled entry is checked as the entry, by whoever entered it.
        let (change, actor) = match &parsed {
            Change::Backfill {
                entry, entered_by, ..
            } => (entry.as_ref(), Some(entered_by.as_str())),
            change => (change, record.source().declared_name.as_deref()),
        };
        check_text(record, &parsed, change)?;
        if let Change::NewStory { story, .. } = change
            && let Some((prefix, number)) = story.split_once('-')
            && let Ok(number) = number.parse::<u64>()
        {
            let highest = self.highest_number(prefix);
            if number <= highest {
                return Err(LedgerError::NumberTaken {
                    story: story.clone(),
                    highest,
                });
            }
        }
        if own_team(change).is_some_and(|team| same_name(team, OPERATOR)) {
            return Err(LedgerError::ReservedTeam);
        }
        // A team doesn't read its own story (RAL-57).
        if let Change::Verify {
            team: Some(reader), ..
        }
        | Change::SendBack {
            team: Some(reader), ..
        } = change
            && reader.trim().is_empty()
        {
            return Err(LedgerError::EmptyField("team"));
        }
        if let Change::Verify {
            team: Some(reader), ..
        }
        | Change::SendBack {
            team: Some(reader), ..
        } = change
            && let Some(story) = self.stories.get(change.story())
            && story
                .team
                .as_deref()
                .is_some_and(|author| same_name(author, reader))
        {
            return Err(LedgerError::ReaderIsAuthor(reader.clone()));
        }
        if let Change::Unblock {
            blocker, answer, ..
        } = change
            && let Some(open) = self.blockers.get(blocker)
        {
            let on_operator = open
                .on_team
                .as_deref()
                .is_some_and(|on| same_name(on, OPERATOR));
            match (on_operator, answer) {
                (true, None) => {
                    return Err(LedgerError::AnswerSource(
                        "closing a question to the operator says where the answer came from: \
                         direct from the operator, or relayed and by whom",
                    ));
                }
                (false, Some(_)) => {
                    return Err(LedgerError::AnswerSource(
                        "only closing a question to the operator says where an answer came from",
                    ));
                }
                (true, Some(Answer::Relayed { by })) => {
                    if by.trim().is_empty() {
                        return Err(LedgerError::EmptyField("relayed by"));
                    }
                    if !crate::record::is_clear_name(by) {
                        return Err(LedgerError::InvisibleCharacter("relayed by"));
                    }
                }
                _ => {}
            }
        }
        if let Change::Standing {
            given: Some(Answer::Relayed { by }),
            ..
        } = change
            && !crate::record::is_clear_name(by)
        {
            return Err(LedgerError::InvisibleCharacter("relayed by"));
        }
        // So that `RAL-1` and `RAL-01` can't be two stories.
        let id = change.story();
        if !id.is_empty() && has_leading_zero(id) {
            return Err(LedgerError::InvalidStoryId(id.to_owned()));
        }
        if let Change::Block {
            on_story: Some(on), ..
        } = change
            && has_leading_zero(on)
        {
            return Err(LedgerError::InvalidStoryId(on.clone()));
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
        if let Change::Correct { state: None, .. } = change
            && record.kind() == Kind::Verified
            && let Some(story) = self.stories.get(id)
        {
            require_verifier(story, id, record.kind(), actor)?;
        }
        Ok(())
    }

    /// Applies one record, or refuses it and leaves the state unchanged.
    pub fn apply(&mut self, seq: u64, record: &Record) -> Result<(), LedgerError> {
        self.apply_record(seq, record)?;
        self.last_at_ms = record.at_ms();
        Ok(())
    }

    fn apply_record(&mut self, seq: u64, record: &Record) -> Result<(), LedgerError> {
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
        // A backfilled entry is applied as the entry, acting as whoever entered it, and keeps the
        // record's own source and time, which are the backfill's (RAL-30).
        let (change, backfill) = match change {
            Change::Backfill {
                entered_at_ms,
                entered_by,
                entry,
            } => {
                if entered_at_ms <= 0 || entered_at_ms >= record.at_ms() {
                    return Err(LedgerError::BackfillNotEarlier {
                        entered_at_ms,
                        at_ms: record.at_ms(),
                    });
                }
                let backfill = Backfill {
                    entered_at_ms,
                    entered_by,
                };
                (*entry, Some(backfill))
            }
            change => (change, None),
        };
        let actor = backfill
            .as_ref()
            .map(|backfill| backfill.entered_by.as_str())
            .or(record.source().declared_name.as_deref());
        match change {
            Change::Annotate {
                corrects,
                team,
                note,
                also_read,
            } => {
                require_kind(record, Kind::Declared)?;
                if backfill.is_some() {
                    return Err(LedgerError::InvalidChange(
                        "an annotation can't be backfilled".into(),
                    ));
                }
                if self.annotation_seqs.contains(&corrects) {
                    return Err(LedgerError::AnnotationOfAnnotation(corrects));
                }
                if also_read.is_empty() {
                    if corrects >= seq || !self.names_team(corrects, &team) {
                        return Err(LedgerError::InvalidAnnotationTarget { corrects, team });
                    }
                } else {
                    self.add_also_read(corrects, record, &also_read)?;
                }
                self.annotations
                    .entry(corrects)
                    .or_default()
                    .push(Annotation {
                        seq,
                        kind: record.kind(),
                        source: record.source().clone(),
                        at_ms: record.at_ms(),
                        team,
                        note,
                    });
                self.annotation_seqs.insert(seq);
                self.last_seq = seq;
                return Ok(());
            }
            Change::Designate {
                watchman,
                credential,
                action,
                scope,
                after_seq,
                after_hash,
                key_fingerprint,
                signature,
            } => {
                require_kind(record, Kind::Declared)?;
                if backfill.is_some() {
                    return Err(LedgerError::InvalidChange(
                        "a designation can't be backfilled".into(),
                    ));
                }
                if after_seq + 1 != seq {
                    return Err(LedgerError::DesignationOutOfPlace { seq, after_seq });
                }
                self.designations.push(Designation {
                    seq,
                    source: record.source().clone(),
                    at_ms: record.at_ms(),
                    watchman,
                    credential,
                    action,
                    scope,
                    after_seq,
                    after_hash,
                    key_fingerprint,
                    signature,
                });
                self.last_seq = seq;
                return Ok(());
            }
            Change::Block {
                team,
                story,
                on_story,
                on_team,
                note,
            } => {
                require_kind(record, Kind::Declared)?;
                self.blockers.insert(
                    seq,
                    Blocker {
                        seq,
                        kind: record.kind(),
                        source: record.source().clone(),
                        at_ms: record.at_ms(),
                        backfilled: backfill,
                        team,
                        story,
                        on_story,
                        on_team,
                        note,
                        closed: None,
                        shelves: Vec::new(),
                    },
                );
                self.last_seq = seq;
                return Ok(());
            }
            Change::Standing {
                standing,
                team,
                story,
                note,
                releases,
                scope,
                given,
            } => {
                require_kind(record, Kind::Declared)?;
                self.standings.insert(
                    seq,
                    Standing {
                        seq,
                        kind: record.kind(),
                        source: record.source().clone(),
                        at_ms: record.at_ms(),
                        backfilled: backfill,
                        standing,
                        team,
                        story,
                        note,
                        releases,
                        scope,
                        given,
                        ended: None,
                    },
                );
                self.last_seq = seq;
                return Ok(());
            }
            Change::OpenRally {
                name,
                projects,
                teams,
            } => {
                require_kind(record, Kind::Declared)?;
                if let Some(open) = self.open_rally() {
                    return Err(LedgerError::RallyAlreadyOpen(open.seq));
                }
                self.rallies.push(Rally {
                    seq,
                    kind: record.kind(),
                    source: record.source().clone(),
                    at_ms: record.at_ms(),
                    name,
                    projects,
                    teams,
                    closed: None,
                });
                self.last_seq = seq;
                return Ok(());
            }
            Change::CloseRally { note } => {
                require_kind(record, Kind::Declared)?;
                let open = self
                    .rallies
                    .last_mut()
                    .filter(|rally| rally.closed.is_none())
                    .ok_or(LedgerError::NoRallyOpen)?;
                open.closed = Some(Closure {
                    seq,
                    kind: record.kind(),
                    source: record.source().clone(),
                    at_ms: record.at_ms(),
                    backfilled: backfill,
                    note,
                    answer: None,
                });
                self.last_seq = seq;
                return Ok(());
            }
            Change::EndStanding { standing, note } => {
                require_kind(record, Kind::Declared)?;
                let open = self
                    .standings
                    .get_mut(&standing)
                    .filter(|open| open.ended.is_none())
                    .ok_or(LedgerError::InvalidStandingTarget(standing))?;
                open.ended = Some(Closure {
                    seq,
                    kind: record.kind(),
                    source: record.source().clone(),
                    at_ms: record.at_ms(),
                    backfilled: backfill,
                    note,
                    answer: None,
                });
                self.last_seq = seq;
                return Ok(());
            }
            Change::Unblock {
                blocker,
                note,
                answer,
            } => {
                if record.kind() == Kind::Observed {
                    return Err(LedgerError::WrongKind {
                        expected: Kind::Declared,
                        found: Kind::Observed,
                    });
                }
                let open = self
                    .blockers
                    .get_mut(&blocker)
                    .filter(|open| open.closed.is_none())
                    .ok_or(LedgerError::InvalidBlockerTarget(blocker))?;
                // An answer from the operator restarts the run, and ends an away hour early, by
                // the service's time on this record, never a time the writer declared (RAL-56).
                if open.waits_on_operator() && answer.is_some() {
                    self.unanswered_shelves.clear();
                    if let Some(away) = self.aways.last_mut()
                        && away.active_at(record.at_ms())
                    {
                        away.ended_early_by = Some(seq);
                        away.ended_early_at_ms = Some(record.at_ms());
                    }
                }
                let open = self.blockers.get_mut(&blocker).expect("found above");
                open.closed = Some(Closure {
                    seq,
                    kind: record.kind(),
                    source: record.source().clone(),
                    at_ms: record.at_ms(),
                    backfilled: backfill,
                    note,
                    answer,
                });
                self.last_seq = seq;
                return Ok(());
            }
            Change::Shelve {
                blocker,
                team,
                window_ms,
            } => {
                require_kind(record, Kind::Declared)?;
                if backfill.is_some() {
                    return Err(LedgerError::InvalidChange(
                        "a shelve can't be backfilled".into(),
                    ));
                }
                let open = self
                    .blockers
                    .get(&blocker)
                    .filter(|open| open.closed.is_none())
                    .ok_or(LedgerError::InvalidBlockerTarget(blocker))?;
                let start = self.question_start(open, record.at_ms());
                let why = if open.story.is_none() {
                    Some("it blocks a team, not a story")
                } else if !open
                    .on_team
                    .as_deref()
                    .is_some_and(|on| same_name(on, OPERATOR))
                {
                    Some("it doesn't wait on the operator")
                } else if !same_name(&open.team, &team) {
                    Some("another team is waiting on it")
                } else if open
                    .shelves
                    .last()
                    .is_some_and(|shelf| shelf.at_ms >= start)
                {
                    Some("it's already shelved")
                } else if window_ms == 0 {
                    Some("the window must be longer than nothing")
                } else if record.at_ms().saturating_sub(start)
                    < i64::try_from(window_ms).unwrap_or(i64::MAX)
                {
                    Some("the operator's window hasn't passed since the question was asked")
                } else {
                    None
                };
                if let Some(why) = why {
                    return Err(LedgerError::InvalidShelf(blocker, why));
                }
                self.unanswered_shelves.push((seq, record.at_ms()));
                let open = self.blockers.get_mut(&blocker).expect("found above");
                open.shelves.push(Shelf {
                    seq,
                    source: record.source().clone(),
                    at_ms: record.at_ms(),
                    team,
                    window_ms,
                });
                self.last_seq = seq;
                return Ok(());
            }
            Change::OperatorAway {
                entered_at_ms,
                ends_at_ms,
                shelves,
            } => {
                require_kind(record, Kind::Observed)?;
                if backfill.is_some() {
                    return Err(LedgerError::InvalidChange(
                        "an away hour can't be backfilled".into(),
                    ));
                }
                let why = if entered_at_ms != record.at_ms() {
                    Some("it must start at its own record's time")
                } else if ends_at_ms <= entered_at_ms {
                    Some("it must end after it starts")
                } else if shelves.is_empty() || shelves != self.unanswered_shelves(record.at_ms()) {
                    Some("its shelves must be the run of unanswered shelves")
                } else if self
                    .aways
                    .last()
                    .is_some_and(|away| away.active_at(record.at_ms()))
                {
                    Some("the operator is already treated as away")
                } else {
                    None
                };
                if let Some(why) = why {
                    return Err(LedgerError::InvalidAway(why));
                }
                self.unanswered_shelves.clear();
                self.aways.push(Away {
                    seq,
                    entered_at_ms,
                    ends_at_ms,
                    shelves,
                    ended_early_by: None,
                    ended_early_at_ms: None,
                });
                self.last_seq = seq;
                return Ok(());
            }
            _ => {}
        }
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
                backfilled: backfill,
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

        if matches!(change, Change::Ready { .. } | Change::NewStory { .. }) {
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
                    also_read: Vec::new(),
                    merged_commit: None,
                    requested_reads: Vec::new(),
                    parts: BTreeMap::new(),
                    parts_of: None,
                    history: Vec::new(),
                    authors: BTreeSet::new(),
                },
            );
        } else {
            let story = self
                .stories
                .get_mut(&id)
                .ok_or_else(|| LedgerError::UnknownStory(id.clone()))?;
            transition(story, &id, &change, record, actor, seq)?;
        }

        let story = self.stories.get_mut(&id).expect("the story exists by now");
        if matches!(change, Change::Claim { .. } | Change::Submit { .. })
            && let Some(name) = actor
        {
            story.authors.insert(name_key(name));
        }
        story.history.push(HistoryEntry {
            seq,
            kind: record.kind(),
            source: record.source().clone(),
            at_ms: record.at_ms(),
            change,
            backfilled: backfill,
            corrected_by: Vec::new(),
        });
        self.last_seq = seq;
        Ok(())
    }
}

/// Validates and applies a change to an existing story. Nothing is modified on refusal.
/// `actor` is the declared name the change is checked as: the record's, or for a backfilled
/// entry, whoever entered it.
fn transition(
    story: &mut Story,
    id: &str,
    change: &Change,
    record: &Record,
    actor: Option<&str>,
    seq: u64,
) -> Result<(), LedgerError> {
    let refuse = |from| LedgerError::InvalidTransition {
        story: id.to_owned(),
        from,
    };
    match change {
        Change::Ready { .. }
        | Change::NewStory { .. }
        | Change::TeamState { .. }
        | Change::Backfill { .. }
        | Change::Block { .. }
        | Change::Unblock { .. }
        | Change::Standing { .. }
        | Change::EndStanding { .. }
        | Change::OpenRally { .. }
        | Change::CloseRally { .. }
        | Change::Shelve { .. }
        | Change::OperatorAway { .. }
        | Change::Designate { .. }
        | Change::Annotate { .. } => unreachable!("handled by the caller"),
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
        Change::EvidenceCheck { submit, .. } => {
            require_kind(record, Kind::Observed)?;
            let submitted = story
                .history
                .iter()
                .any(|entry| entry.seq == *submit && matches!(entry.change, Change::Submit { .. }));
            if !submitted {
                return Err(LedgerError::InvalidEvidenceCheck(
                    *submit,
                    "it isn't a submission of this story",
                ));
            }
            let checked = story.history.iter().any(|entry| {
                matches!(&entry.change, Change::EvidenceCheck { submit: s, .. } if s == submit)
            });
            if checked {
                return Err(LedgerError::InvalidEvidenceCheck(
                    *submit,
                    "it's already checked",
                ));
            }
        }
        Change::Submit {
            team,
            evidence,
            amend,
            part: Some(k),
            of,
            ..
        } => {
            require_kind(record, Kind::Declared)?;
            let part_error = |why: String| LedgerError::Part {
                story: id.to_owned(),
                part: *k,
                why,
            };
            if *k == 0 || *k > MAX_PARTS || of.is_some_and(|n| n < *k || n > MAX_PARTS) {
                return Err(part_error(format!(
                    "parts are numbered from 1 to {MAX_PARTS}, and no higher than --of"
                )));
            }
            let highest = story.parts.keys().next_back().copied().unwrap_or(0);
            if let Some(n) = of
                && *n < highest
            {
                return Err(part_error(format!(
                    "--of {n} is below part {highest}, which is already submitted"
                )));
            }
            // A part is the holding team's work in progress: the story stays claimed.
            if story.state != State::Claimed {
                return Err(part_error(format!(
                    "the story is {:?}; a part is submitted while the team holds its claim",
                    story.state
                )));
            }
            require_holder(story, id, team)?;
            let now = story.parts.get(k);
            if *amend {
                let Some(part) = now.filter(|part| part.state == State::InReview) else {
                    return Err(part_error(
                        "only a part still in review can be amended".into(),
                    ));
                };
                if part.evidence.trim() == evidence.trim() {
                    return Err(part_error(
                        "that's its evidence already, so there's nothing to amend".into(),
                    ));
                }
                if let (Some(submitter), Some(name)) = (&part.submitted_by, actor)
                    && !same_name(submitter, name)
                {
                    return Err(part_error(format!(
                        "{submitter} submitted it, so only {submitter} can amend its evidence"
                    )));
                }
            } else {
                match now.map(|part| part.state) {
                    None | Some(State::SentBack) => {}
                    Some(State::InReview) => {
                        return Err(part_error(format!(
                            "it's already in review; to correct its evidence, amend it with \
                             rally amend {id} --part {k} --team <team> --evidence \
                             <owner/repo@full commit>"
                        )));
                    }
                    Some(_) => return Err(part_error("it's already verified".into())),
                }
            }
            if let Some(n) = of {
                story.parts_of = Some(*n);
            }
            story.parts.insert(
                *k,
                Part {
                    state: State::InReview,
                    submitted_at: seq,
                    submitted_by: actor.map(str::to_owned),
                    evidence: evidence.clone(),
                    verified_commit: None,
                },
            );
        }
        Change::Verify {
            commit,
            part: Some(k),
            ..
        }
        | Change::SendBack {
            commit,
            part: Some(k),
            ..
        } => {
            let Some(part) = story.parts.get(k) else {
                return Err(LedgerError::Part {
                    story: id.to_owned(),
                    part: *k,
                    why: "it hasn't been submitted".into(),
                });
            };
            if part.state != State::InReview {
                return Err(LedgerError::Part {
                    story: id.to_owned(),
                    part: *k,
                    why: format!(
                        "it's {:?}, and only a part in review can be read",
                        part.state
                    ),
                });
            }
            require_amended_part_read(story, id, *k, commit.as_deref())?;
            require_verifier(story, id, record.kind(), actor)?;
            let part = story.parts.get_mut(k).expect("checked above");
            if matches!(change, Change::Verify { .. }) {
                part.state = State::Verified;
                part.verified_commit = commit.clone();
            } else {
                part.state = State::SentBack;
                part.verified_commit = None;
            }
        }
        Change::Submit {
            team,
            evidence,
            untested,
            amend: true,
            of,
            ..
        } => {
            require_kind(record, Kind::Declared)?;
            check_closing_of(story, *of)?;
            if story.state != State::InReview {
                return Err(LedgerError::NotInReview {
                    story: id.to_owned(),
                    from: story.state,
                });
            }
            require_holder(story, id, team)?;
            if let Some(Change::Submit {
                evidence: before,
                untested: untested_before,
                ..
            }) = latest_submission(story).map(|entry| &entry.change)
                && before.trim() == evidence.trim()
                && untested_before == untested
                && of.is_none_or(|n| story.parts_of == Some(n))
            {
                return Err(LedgerError::NothingAmended(id.to_owned()));
            }
            // Only whoever submitted it amends it: the evidence is theirs to correct.
            if let (Some(submitter), Some(name)) = (&story.submitted_by, actor)
                && !same_name(submitter, name)
            {
                return Err(LedgerError::NotTheSubmitter {
                    story: id.to_owned(),
                    submitter: submitter.clone(),
                });
            }
            if let Some(n) = of {
                story.parts_of = Some(*n);
            }
        }
        Change::Submit { team, of, .. } => {
            require_kind(record, Kind::Declared)?;
            check_closing_of(story, *of)?;
            match story.state {
                State::Claimed => {}
                // A send-back or release ends the claim, so a resubmit needs a new one (RAL-72).
                from @ (State::SentBack | State::Ready) => {
                    return Err(LedgerError::ClaimLapsed {
                        story: id.to_owned(),
                        from,
                    });
                }
                // A resubmit in review has its own error, saying to amend (RAL-71).
                State::InReview => return Err(LedgerError::AlreadyInReview(id.to_owned())),
                from => return Err(refuse(from)),
            }
            require_holder(story, id, team)?;
            story.state = State::InReview;
            story.submitted_by = actor.map(str::to_owned);
            if let Some(n) = of {
                story.parts_of = Some(*n);
            }
        }
        Change::Verify { commit, .. } => {
            if story.state != State::InReview {
                return Err(refuse(story.state));
            }
            // The final close of a multi-part story waits for every part's read (RAL-79).
            let open = story.open_parts();
            let missing = story.missing_parts();
            if !open.is_empty() || !missing.is_empty() {
                return Err(LedgerError::PartsOpen {
                    story: id.to_owned(),
                    parts: open,
                    missing,
                });
            }
            require_amended_read(story, id, commit.as_deref())?;
            require_verifier(story, id, record.kind(), actor)?;
            story.state = State::Verified;
            story.verified_commit = commit.clone();
            story.also_read.clear();
            // A read answers every open request for one.
            story.requested_reads.clear();
        }
        Change::SendBack { commit, .. } => {
            if story.state != State::InReview {
                return Err(refuse(story.state));
            }
            require_amended_read(story, id, commit.as_deref())?;
            require_verifier(story, id, record.kind(), actor)?;
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
                requested_by: actor.unwrap_or_default().to_owned(),
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
                    let name = actor.unwrap_or_default();
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
                    require_verifier(story, id, record.kind(), actor)?;
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
                story.also_read.clear();
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

/// `--of` without a part (RAL-79): the close re-declaring how many parts the story ended with. A
/// story with no parts can't give one, and it can't drop a part already submitted.
fn check_closing_of(story: &Story, of: Option<u32>) -> Result<(), LedgerError> {
    let Some(n) = of else {
        return Ok(());
    };
    match story.parts.keys().next_back().copied() {
        None => Err(LedgerError::InvalidChange(
            "--of is for a story in parts: give --part with it".into(),
        )),
        Some(highest) if n < highest || n > MAX_PARTS => Err(LedgerError::InvalidChange(format!(
            "--of {n} must be from part {highest}, the highest submitted, to {MAX_PARTS}"
        ))),
        Some(_) => Ok(()),
    }
}

/// The story's latest submission, amended or not.
fn latest_submission(story: &Story) -> Option<&HistoryEntry> {
    story
        .history
        .iter()
        .rev()
        .find(|entry| matches!(entry.change, Change::Submit { part: None, .. }))
}

/// [`require_amended_read`] for one part (RAL-79): once a part's submission is amended, a read of
/// it that names a commit must name the amended evidence's.
fn require_amended_part_read(
    story: &Story,
    id: &str,
    k: u32,
    commit: Option<&str>,
) -> Result<(), LedgerError> {
    let Some(part) = story.parts.get(&k) else {
        return Ok(());
    };
    let amended = story.history.iter().any(|entry| {
        entry.seq == part.submitted_at && matches!(entry.change, Change::Submit { amend: true, .. })
    });
    // The part being applied is the latest, and isn't in the history yet when it's this record.
    if !amended {
        return Ok(());
    }
    let (Some(read), Ok((_, evidence))) = (commit, crate::evidence::parse(&part.evidence)) else {
        return Ok(());
    };
    if read.eq_ignore_ascii_case(&evidence) {
        Ok(())
    } else {
        Err(LedgerError::Part {
            story: id.to_owned(),
            part: k,
            why: format!(
                "its evidence was amended at record {} to commit {evidence}: read that commit, \
                 then name it with --commit",
                part.submitted_at
            ),
        })
    }
}

/// Once a submission is amended (RAL-71), a read that names a commit must name the amended
/// evidence's, so a verify of the commit read before the amendment can't stand for the evidence
/// after it. Evidence Rally can't parse has no commit to compare, and a send-back may name none.
fn require_amended_read(story: &Story, id: &str, commit: Option<&str>) -> Result<(), LedgerError> {
    let Some(entry) = latest_submission(story) else {
        return Ok(());
    };
    let Change::Submit {
        evidence,
        amend: true,
        ..
    } = &entry.change
    else {
        return Ok(());
    };
    let (Some(read), Ok((_, amended))) = (commit, crate::evidence::parse(evidence)) else {
        return Ok(());
    };
    if read.eq_ignore_ascii_case(&amended) {
        Ok(())
    } else {
        Err(LedgerError::ReadBeforeAmendment {
            story: id.to_owned(),
            amended_at: entry.seq,
            commit: amended,
        })
    }
}

fn require_verifier(
    story: &Story,
    id: &str,
    kind: Kind,
    actor: Option<&str>,
) -> Result<(), LedgerError> {
    if kind != Kind::Verified {
        return Err(LedgerError::WrongKind {
            expected: Kind::Verified,
            found: kind,
        });
    }
    let Some(name) = actor else {
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
pub fn same_name(a: &str, b: &str) -> bool {
    name_key(a) == name_key(b)
}

fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn check_fields(change: &Change) -> Result<(), LedgerError> {
    if let Change::Backfill {
        entered_by, entry, ..
    } = change
    {
        if entered_by.trim().is_empty() {
            return Err(LedgerError::EmptyField("entered_by"));
        }
        if matches!(**entry, Change::Backfill { .. }) {
            return Err(LedgerError::InvalidChange(
                "a backfilled entry can't itself be a backfill".into(),
            ));
        }
        return check_fields(entry);
    }
    let id = change.story();
    let story_is_optional = matches!(
        change,
        Change::TeamState { story: None, .. }
            | Change::Block { story: None, .. }
            | Change::Unblock { .. }
            | Change::Standing { story: None, .. }
            | Change::EndStanding { .. }
            | Change::OpenRally { .. }
            | Change::CloseRally { .. }
            | Change::Shelve { .. }
            | Change::OperatorAway { .. }
            | Change::Designate { .. }
            | Change::Annotate { .. }
    );
    if !story_is_optional && !is_story_id(id) {
        return Err(LedgerError::InvalidStoryId(id.to_owned()));
    }
    if let Change::Block {
        story,
        on_story,
        on_team,
        ..
    } = change
    {
        if let Some(on) = on_story {
            if !is_story_id(on) {
                return Err(LedgerError::InvalidStoryId(on.clone()));
            }
            if story.as_deref() == Some(on.as_str()) {
                return Err(LedgerError::BlockedOnItself(on.clone()));
            }
        }
        if let Some(on) = on_team
            && on.trim().is_empty()
        {
            return Err(LedgerError::EmptyField("on_team"));
        }
    }
    if let Change::Standing {
        standing,
        releases,
        scope,
        given,
        ..
    } = change
    {
        check_standing_fields(
            *standing,
            releases.as_deref(),
            scope.as_deref(),
            given.as_ref(),
        )?;
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
        Change::NewStory { title, .. } => &[("title", title)],
        Change::Ready { .. }
        | Change::Merge { .. }
        | Change::Backfill { .. }
        | Change::OperatorAway { .. } => &[],
        Change::Designate { .. } => {
            check_designation_fields(change)?;
            &[]
        }
        Change::Claim { team, .. }
        | Change::Release { team, .. }
        | Change::Shelve { team, .. }
        | Change::TeamState { team, .. } => &[("team", team)],
        Change::Block { team, note, .. }
        | Change::Annotate { team, note, .. }
        | Change::Standing { team, note, .. } => &[("team", team), ("note", note)],
        Change::Unblock { note, .. }
        | Change::EndStanding { note, .. }
        | Change::CloseRally { note } => &[("note", note)],
        Change::OpenRally {
            name,
            projects,
            teams,
        } => {
            let prefix = |p: &String| {
                !p.is_empty() && p.len() <= 16 && p.chars().all(|c| c.is_ascii_uppercase())
            };
            let distinct = projects.iter().collect::<BTreeSet<_>>().len() == projects.len();
            if projects.is_empty() || !projects.iter().all(prefix) || !distinct {
                return Err(LedgerError::InvalidRallyProjects);
            }
            // Each team once, however it's written, and none blank.
            let folded: BTreeSet<String> = teams.iter().map(|t| t.trim().to_lowercase()).collect();
            if teams.iter().any(|t| t.trim().is_empty()) {
                return Err(LedgerError::EmptyField("team"));
            }
            if folded.len() != teams.len() {
                return Err(LedgerError::InvalidRallyTeams);
            }
            &[("name", name)]
        }
        Change::Submit { team, evidence, .. } => &[("team", team), ("evidence", evidence)],
        Change::EvidenceCheck { .. } => &[],
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
    }
    Ok(())
}

/// What each kind of standing record needs, and what it mustn't carry (RAL-64): a hold says what
/// releases it, and a grant its scope and where it came from.
fn check_standing_fields(
    standing: StandingKind,
    releases: Option<&str>,
    scope: Option<&str>,
    given: Option<&Answer>,
) -> Result<(), LedgerError> {
    let blank = |value: Option<&str>| value.is_some_and(|value| value.trim().is_empty());
    if blank(releases) {
        return Err(LedgerError::EmptyField("releases"));
    }
    if blank(scope) {
        return Err(LedgerError::EmptyField("scope"));
    }
    if let Some(Answer::Relayed { by }) = given
        && by.trim().is_empty()
    {
        return Err(LedgerError::EmptyField("relayed by"));
    }
    let wrong = |what: &str| {
        Err(LedgerError::InvalidChange(format!(
            "only a {what}; this is a {}",
            match standing {
                StandingKind::Hold => "hold",
                StandingKind::Condition => "condition",
                StandingKind::Grant => "grant",
            }
        )))
    };
    match standing {
        StandingKind::Hold if releases.is_none() => Err(LedgerError::EmptyField("releases")),
        StandingKind::Grant if scope.is_none() => Err(LedgerError::EmptyField("scope")),
        StandingKind::Grant if given.is_none() => Err(LedgerError::InvalidChange(
            "a grant says whether the operator gave it directly or it was relayed, and by whom"
                .into(),
        )),
        StandingKind::Hold | StandingKind::Condition if scope.is_some() || given.is_some() => {
            wrong("grant has a scope and says where it came from")
        }
        StandingKind::Grant | StandingKind::Condition if releases.is_some() => {
            wrong("hold says what releases it")
        }
        _ => Ok(()),
    }
}

/// The team a change acts as, as against one it names as waited on.
fn own_team(change: &Change) -> Option<&str> {
    match change {
        Change::Claim { team, .. }
        | Change::Release { team, .. }
        | Change::Shelve { team, .. }
        | Change::TeamState { team, .. }
        | Change::Block { team, .. }
        | Change::Standing { team, .. }
        | Change::Annotate { team, .. }
        | Change::Submit { team, .. } => Some(team),
        Change::Correct { team, .. }
        | Change::Verify { team, .. }
        | Change::SendBack { team, .. } => team.as_deref(),
        _ => None,
    }
}

/// The text a change carries that has a limit, required and optional, by the limit's name.
fn limited_text(change: &Change) -> Vec<(&'static str, &str)> {
    let mut text: Vec<(&'static str, &str)> = Vec::new();
    match change {
        Change::NewStory { title, .. } => text.push(("title", title)),
        Change::Ready { .. }
        | Change::Merge { .. }
        | Change::Backfill { .. }
        | Change::OperatorAway { .. }
        | Change::Designate { .. } => {}
        Change::Claim { team, .. } | Change::Release { team, .. } | Change::Shelve { team, .. } => {
            text.push(("team", team))
        }
        Change::TeamState { team, note, .. } => {
            text.push(("team", team));
            text.extend(note.as_deref().map(|note| ("note", note)));
        }
        Change::Block {
            team,
            note,
            on_team,
            ..
        } => {
            text.extend([("team", team.as_str()), ("note", note)]);
            text.extend(on_team.as_deref().map(|on| ("team", on)));
        }
        Change::Annotate { team, note, .. } => {
            text.extend([("team", team.as_str()), ("note", note)])
        }
        Change::Submit {
            team,
            evidence,
            untested,
            ..
        } => {
            text.extend([("team", team.as_str()), ("evidence", evidence)]);
            text.extend(untested.as_deref().map(|untested| ("note", untested)));
        }
        Change::EvidenceCheck { .. } => {}
        Change::Correct { note, team, .. } => {
            text.push(("note", note));
            text.extend(team.as_deref().map(|team| ("team", team)));
        }
        Change::Verify { note, team, .. } | Change::SendBack { note, team, .. } => {
            text.push(("note", note));
            text.extend(team.as_deref().map(|team| ("team", team)));
        }
        Change::Standing {
            team,
            note,
            releases,
            scope,
            given,
            ..
        } => {
            text.extend([("team", team.as_str()), ("note", note)]);
            text.extend(releases.as_deref().map(|releases| ("note", releases)));
            text.extend(scope.as_deref().map(|scope| ("note", scope)));
            if let Some(Answer::Relayed { by }) = given {
                text.push(("team", by));
            }
        }
        Change::Unblock { note, .. }
        | Change::EndStanding { note, .. }
        | Change::CloseRally { note }
        | Change::RequestRead { note, .. }
        | Change::WithdrawRead { note, .. } => text.push(("note", note)),
        Change::OpenRally { name, teams, .. } => {
            text.push(("note", name));
            text.extend(teams.iter().map(|team| ("team", team.as_str())));
        }
    }
    text
}

/// Limits on a new record's text and names (RAL-16, RAL-41). Only new records meet them: replay
/// doesn't, so tightening one never makes a ledger that already holds a longer note, or a name
/// with a character now refused, unreadable.
fn check_text(record: &Record, parsed: &Change, change: &Change) -> Result<(), LedgerError> {
    for (name, value) in limited_text(change) {
        check_limit(name, value)?;
        if name == "team" && value.chars().any(crate::record::is_invisible) {
            return Err(LedgerError::InvisibleCharacter("team"));
        }
    }
    let mut names = vec![("source.credential", record.source().credential.as_str())];
    names.extend(
        record
            .source()
            .declared_name
            .as_deref()
            .map(|name| ("declared name", name)),
    );
    if let Change::Backfill { entered_by, .. } = parsed {
        if !crate::record::is_good_name(entered_by) {
            return Err(LedgerError::ControlCharacter("entered_by"));
        }
        names.push(("entered_by", entered_by));
    }
    match names
        .into_iter()
        .find(|(_, name)| !crate::record::is_clear_name(name))
    {
        Some((field, _)) => Err(LedgerError::InvisibleCharacter(field)),
        None => Ok(()),
    }
}

/// Refuses a record with secret-looking text in any string its body carries, or in its declared
/// name. Not the credential, which is how the service knows the writer, not text it wrote.
/// The most characters in a watchman's name, which goes into a signed statement.
const MAX_WATCHMAN_CHARS: usize = 64;
/// The most characters in an armoured SSH signature.
const MAX_SIGNATURE_CHARS: usize = 4096;

/// A designation's form: a name that can't change the statement's lines, a chain hash, a key
/// fingerprint and an armoured signature.
/// Whether `scope` is one a v3 designation can give (RAL-63): `all`, `team:<team>` or
/// `machine:<credential>`, with a name held to the watchman's plain characters.
pub fn is_scope(scope: &str) -> bool {
    let plain = |name: &str| {
        !name.is_empty()
            && name.chars().count() <= MAX_WATCHMAN_CHARS
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.@".contains(c))
    };
    match scope.split_once(':') {
        None => scope == "all",
        Some(("team" | "machine", name)) => plain(name),
        Some(_) => false,
    }
}

fn check_designation_fields(change: &Change) -> Result<(), LedgerError> {
    let Change::Designate {
        watchman,
        credential,
        action,
        scope,
        after_hash,
        key_fingerprint,
        signature,
        ..
    } = change
    else {
        return Ok(());
    };
    // v3 (RAL-63): an add names a watchman, its credential and its scope; an end names only the
    // watchman it ends. Without an action, as under v1 and v2, there's no scope.
    match action {
        Some(DesignateAction::Add) => {
            if watchman.is_none() || credential.is_none() || scope.is_none() {
                return Err(LedgerError::InvalidDesignation(
                    "a v3 add names a watchman, its credential and its scope",
                ));
            }
        }
        Some(DesignateAction::End) => {
            if watchman.is_none() || credential.is_some() || scope.is_some() {
                return Err(LedgerError::InvalidDesignation(
                    "a v3 end names only the watchman it ends",
                ));
            }
        }
        None if scope.is_some() => {
            return Err(LedgerError::InvalidDesignation("only a v3 add has a scope"));
        }
        None => {}
    }
    if let Some(scope) = scope
        && !is_scope(scope)
    {
        return Err(LedgerError::InvalidDesignation(
            "scope: all, team:<team> or machine:<credential>, each name 1 to 64 letters, digits, \
             '-', '_', '.' or '@'",
        ));
    }
    // A credential goes in the signed statement, so it's held to the same plain characters as
    // the watchman's name, and only a designation of a watchman is bound to one.
    if let Some(credential) = credential {
        let fits = !credential.is_empty()
            && credential.chars().count() <= MAX_WATCHMAN_CHARS
            && credential
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.@".contains(c));
        if !fits {
            return Err(LedgerError::InvalidDesignation(
                "credential: 1 to 64 letters, digits, '-', '_', '.' or '@'",
            ));
        }
        if watchman.is_none() {
            return Err(LedgerError::InvalidDesignation(
                "an --end names no watchman, so it binds no credential",
            ));
        }
    }
    if let Some(name) = watchman {
        let fits = !name.is_empty()
            && name.chars().count() <= MAX_WATCHMAN_CHARS
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.@".contains(c));
        if !fits || name == "none" {
            return Err(LedgerError::InvalidDesignation(
                "watchman: 1 to 64 letters, digits, '-', '_', '.' or '@', and not 'none'",
            ));
        }
    }
    if after_hash.len() != 64
        || !after_hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(LedgerError::InvalidDesignation(
            "after_hash: 64 lowercase hex digits",
        ));
    }
    let fingerprint_ok = key_fingerprint.starts_with("SHA256:")
        && key_fingerprint.len() <= 100
        && key_fingerprint[7..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b));
    if !fingerprint_ok {
        return Err(LedgerError::InvalidDesignation(
            "key_fingerprint: SHA256:<base64>",
        ));
    }
    if !signature.starts_with("-----BEGIN SSH SIGNATURE-----")
        || !signature
            .trim_end()
            .ends_with("-----END SSH SIGNATURE-----")
        || signature.chars().count() > MAX_SIGNATURE_CHARS
    {
        return Err(LedgerError::InvalidDesignation(
            "signature: an armoured SSH signature, at most 4096 characters",
        ));
    }
    Ok(())
}

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
        "title" => MAX_TITLE_CHARS,
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

    fn part_submit(k: u32, evidence: &str) -> Record {
        team(
            json!({"change": "submit", "story": STORY, "team": "rally", "evidence": evidence,
                    "part": k, "of": 3}),
        )
    }

    fn part_read(change: &str, k: u32, commit: &str) -> Record {
        verifier(
            json!({"change": change, "story": STORY, "note": "read it", "commit": commit,
                        "part": k}),
        )
    }

    fn part_error(result: Result<(), LedgerError>) -> String {
        match result {
            Err(LedgerError::Part { why, .. }) => why,
            other => panic!("expected a part error, got {other:?}"),
        }
    }

    /// RAL-79: DSM-37's case. Each part is submitted and read as it lands while the story stays
    /// claimed; the final close waits for every part, then marks the story Verified.
    #[test]
    fn each_part_is_read_as_it_lands_and_the_close_waits_for_them_all() {
        let ev = |c: &str| format!("Castor-o7/rally@{c}");
        let mut ledger = ledger_with(&[ready(), claim()]);
        push(&mut ledger, &part_submit(1, &ev(COMMIT)));
        assert_eq!(state(&ledger), State::Claimed);
        let story = ledger.story(STORY).unwrap();
        assert_eq!(story.parts[&1].state, State::InReview);
        assert_eq!((story.parts_verified(), story.parts_total()), (0, 3));
        // The submitter can't read their own part.
        let own = record(
            Kind::Verified,
            "alice",
            json!({"change": "verify", "story": STORY,
                         "note": "mine", "commit": COMMIT, "part": 1}),
        );
        assert!(matches!(
            ledger.check(ledger.last_seq() + 1, &own),
            Err(LedgerError::SelfVerification { .. })
        ));
        push(&mut ledger, &part_read("verify", 1, COMMIT));
        push(&mut ledger, &part_submit(2, &ev(OTHER_COMMIT)));
        push(&mut ledger, &part_read("send_back", 2, OTHER_COMMIT));
        let story = ledger.story(STORY).unwrap();
        assert_eq!(story.parts[&1].verified_commit.as_deref(), Some(COMMIT));
        assert_eq!(story.parts[&2].state, State::SentBack);
        // A sent-back part is resubmitted under the same claim; a verified one can't be.
        push(&mut ledger, &part_submit(2, &ev(OTHER_COMMIT)));
        assert!(
            part_error(ledger.check(ledger.last_seq() + 1, &part_submit(1, &ev(COMMIT))))
                .contains("verified")
        );
        assert!(
            part_error(ledger.check(ledger.last_seq() + 1, &part_submit(2, &ev(COMMIT))))
                .contains("amend")
        );
        // The final submit puts the story in review; its verify waits for part 2.
        push(&mut ledger, &submit());
        assert_eq!(
            ledger.check(ledger.last_seq() + 1, &verify()),
            Err(LedgerError::PartsOpen {
                story: STORY.into(),
                parts: vec![2],
                missing: vec![3],
            })
        );
        // A part can't be submitted once the story is in review.
        assert!(
            part_error(ledger.check(ledger.last_seq() + 1, &part_submit(3, &ev(COMMIT))))
                .contains("InReview")
        );
        push(&mut ledger, &part_read("verify", 2, OTHER_COMMIT));
        // Part 3 was declared and never submitted, so the close still waits.
        assert_eq!(
            ledger.check(ledger.last_seq() + 1, &verify()),
            Err(LedgerError::PartsOpen {
                story: STORY.into(),
                parts: vec![],
                missing: vec![3],
            })
        );
        // The team says the story ended at two parts, by amending the close; it can't say one.
        let close_of = |n: u32| {
            team(json!({"change": "submit", "story": STORY, "team": "rally",
                        "evidence": "abc123", "amend": true, "of": n}))
        };
        assert!(matches!(
            ledger.check(ledger.last_seq() + 1, &close_of(1)),
            Err(LedgerError::InvalidChange(why)) if why.contains("highest submitted")
        ));
        push(&mut ledger, &close_of(2));
        push(&mut ledger, &verify());
        assert_eq!(state(&ledger), State::Verified);
        assert_eq!(ledger.story(STORY).unwrap().parts_verified(), 2);
        assert_eq!(ledger.story(STORY).unwrap().parts_total(), 2);
        // Replay gives the same parts.
        let again = ledger_with(&[
            ready(),
            claim(),
            part_submit(1, &ev(COMMIT)),
            part_read("verify", 1, COMMIT),
            part_submit(2, &ev(OTHER_COMMIT)),
            part_read("send_back", 2, OTHER_COMMIT),
            part_submit(2, &ev(OTHER_COMMIT)),
            submit(),
            part_read("verify", 2, OTHER_COMMIT),
            team(json!({"change": "submit", "story": STORY, "team": "rally",
                        "evidence": "abc123", "amend": true, "of": 2})),
            verify(),
        ]);
        assert_eq!(
            again.story(STORY).unwrap().parts,
            ledger.story(STORY).unwrap().parts
        );
        assert_eq!(state(&again), State::Verified);
    }

    /// RAL-79 with RAL-71: a part in review is amended by its submitter, and a read of it must then
    /// name the amended commit.
    #[test]
    fn a_part_is_amended_alone_and_its_read_must_name_the_amended_commit() {
        let ev = |c: &str| format!("Castor-o7/rally@{c}");
        let mut ledger = ledger_with(&[ready(), claim()]);
        push(&mut ledger, &part_submit(1, &ev(COMMIT)));
        let amend = |by: &str, evidence: &str| {
            record(
                Kind::Declared,
                by,
                json!({"change": "submit", "story": STORY, "team": "rally",
                   "evidence": evidence, "amend": true, "part": 1}),
            )
        };
        assert!(
            part_error(ledger.check(ledger.last_seq() + 1, &amend("alice", &ev(COMMIT))))
                .contains("nothing")
        );
        assert!(
            part_error(ledger.check(ledger.last_seq() + 1, &amend("bob", &ev(OTHER_COMMIT))))
                .contains("alice")
        );
        // Part 2 was never submitted, so it can't be amended.
        let two = record(
            Kind::Declared,
            "alice",
            json!({"change": "submit", "story": STORY,
                         "team": "rally", "evidence": ev(COMMIT), "amend": true, "part": 2}),
        );
        assert!(part_error(ledger.check(ledger.last_seq() + 1, &two)).contains("in review"));
        push(&mut ledger, &amend("alice", &ev(OTHER_COMMIT)));
        // After a restart, the amended part is the submission Rally checks.
        let jobs = crate::evidence::unchecked(&ledger);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].evidence, ev(OTHER_COMMIT));
        assert!(
            part_error(ledger.check(ledger.last_seq() + 1, &part_read("verify", 1, COMMIT)))
                .contains("amended")
        );
        push(&mut ledger, &part_read("verify", 1, OTHER_COMMIT));
        assert_eq!(
            ledger.story(STORY).unwrap().parts[&1].state,
            State::Verified
        );
        // The story's own submission is untouched: a story-level amend is still refused.
        assert!(matches!(
            ledger.check(ledger.last_seq() + 1, &amend_story()),
            Err(LedgerError::NotInReview { .. })
        ));
    }

    fn amend_story() -> Record {
        team(
            json!({"change": "submit", "story": STORY, "team": "rally", "evidence": "x",
                    "amend": true}),
        )
    }

    #[test]
    fn parts_and_their_total_are_capped_and_the_total_never_drops_a_part() {
        let mut ledger = ledger_with(&[ready(), claim()]);
        let with = |k: u32, of: Option<u32>| {
            let mut body = json!({"change": "submit", "story": STORY, "team": "rally",
                                  "evidence": "e", "part": k});
            if let Some(n) = of {
                body["of"] = json!(n);
            }
            team(body)
        };
        let next = ledger.last_seq() + 1;
        assert!(part_error(ledger.check(next, &with(MAX_PARTS + 1, None))).contains("1 to 64"));
        assert!(part_error(ledger.check(next, &with(1, Some(MAX_PARTS + 1)))).contains("1 to 64"));
        push(&mut ledger, &with(3, None));
        // A later part can't declare a total below a part already submitted.
        assert!(
            part_error(ledger.check(ledger.last_seq() + 1, &with(1, Some(2))))
                .contains("below part 3")
        );
        // `of` on a story with no parts is refused: there's nothing for it to count.
        let mut plain = ledger_with(&[ready(), claim()]);
        let of = team(json!({"change": "submit", "story": STORY, "team": "rally",
                             "evidence": "e", "of": 2}));
        assert!(matches!(
            plain.check(plain.last_seq() + 1, &of),
            Err(LedgerError::InvalidChange(why)) if why.contains("give --part")
        ));
        push(&mut plain, &submit());
        assert_eq!(state(&plain), State::InReview);
    }

    #[test]
    fn parts_are_numbered_from_one_and_need_the_claim() {
        let mut ledger = ledger_with(&[ready()]);
        assert!(
            part_error(ledger.check(ledger.last_seq() + 1, &part_submit(1, "e"))).contains("Ready")
        );
        push(&mut ledger, &claim());
        assert!(
            part_error(ledger.check(ledger.last_seq() + 1, &part_submit(0, "e")))
                .contains("from 1")
        );
        assert!(
            part_error(ledger.check(ledger.last_seq() + 1, &part_submit(4, "e"))).contains("--of")
        );
        assert!(
            part_error(ledger.check(ledger.last_seq() + 1, &part_read("verify", 1, COMMIT)))
                .contains("hasn't")
        );
    }

    /// RAL-72: a submit with no claim in force says to claim first, with the command; a submit
    /// from review says to amend instead (RAL-71).
    #[test]
    fn a_submit_without_a_claim_says_to_claim_first() {
        let mut ledger = ledger_with(&[ready(), claim(), submit()]);
        assert_eq!(
            apply_next(&mut ledger, &submit()),
            Err(LedgerError::AlreadyInReview(STORY.into()))
        );
        apply_next(&mut ledger, &send_back()).unwrap();
        let lapsed = apply_next(&mut ledger, &submit()).unwrap_err();
        assert_eq!(
            lapsed,
            LedgerError::ClaimLapsed {
                story: STORY.into(),
                from: State::SentBack
            }
        );
        let text = lapsed.to_string();
        assert!(text.contains("re-claim first"), "{text}");
        assert!(
            text.contains(&format!("rally claim {STORY} --team")),
            "{text}"
        );
        // Re-claiming lets it be submitted again.
        apply_next(&mut ledger, &claim()).unwrap();
        apply_next(&mut ledger, &submit()).unwrap();

        let mut released = ledger_with(&[ready(), claim(), release()]);
        let text = apply_next(&mut released, &submit())
            .unwrap_err()
            .to_string();
        assert!(text.contains("claim it first"), "{text}");
    }

    /// RAL-72: a refused story ID says what the form is and how to get a new number.
    #[test]
    fn a_refused_story_id_explains_the_form() {
        let ledger = Ledger::default();
        for id in ["DSM-45b", "DSM-045", "dsm-45", "DSM"] {
            // A leading zero is refused for new records only, by check.
            let refused = ledger
                .check(1, &team(json!({"change": "ready", "story": id})))
                .unwrap_err();
            assert_eq!(refused, LedgerError::InvalidStoryId(id.into()));
            let text = refused.to_string();
            assert!(
                text.contains("capital letters, a hyphen, then a number"),
                "{text}"
            );
            assert!(text.contains("rally new-story"), "{text}");
        }
    }

    #[test]
    fn standing_records_are_declared_checked_by_kind_and_end_once() {
        let rec = |kind: Kind, body: serde_json::Value| {
            let source = crate::record::Source {
                credential: "a1".into(),
                // An observed record names no one.
                declared_name: (kind != Kind::Observed).then(|| "r".into()),
            };
            Record::new(kind, source, 1_000, body).unwrap()
        };
        let grant = json!({"change": "standing", "standing": "grant", "team": "rally",
                           "scope": "merge", "note": "yes", "given": {"from": "direct"}});
        let mut ledger = Ledger::default();
        // Only declared: a grant can't be written as verified or observed.
        for kind in [Kind::Verified, Kind::Observed] {
            assert!(matches!(
                ledger.clone().apply(1, &rec(kind, grant.clone())),
                Err(LedgerError::WrongKind { .. })
            ));
        }
        ledger.apply(1, &rec(Kind::Declared, grant)).unwrap();
        let standing = &ledger.standings()[&1];
        assert_eq!(standing.standing, StandingKind::Grant);
        assert_eq!(standing.given, Some(Answer::Direct));

        // Each kind carries what it needs, and nothing that belongs to another.
        let refused = [
            json!({"change": "standing", "standing": "hold", "team": "rally", "note": "n"}),
            json!({"change": "standing", "standing": "grant", "team": "rally", "note": "n",
                   "given": {"from": "direct"}}),
            json!({"change": "standing", "standing": "grant", "team": "rally", "note": "n",
                   "scope": "s"}),
            json!({"change": "standing", "standing": "grant", "team": "rally", "note": "n",
                   "scope": "s", "given": {"from": "relayed", "by": " "}}),
            json!({"change": "standing", "standing": "condition", "team": "rally", "note": "n",
                   "releases": "r"}),
            json!({"change": "standing", "standing": "hold", "team": "rally", "note": "n",
                   "releases": "r", "scope": "s"}),
            json!({"change": "standing", "standing": "hold", "team": "rally", "note": " ",
                   "releases": "r"}),
        ];
        for body in refused {
            assert!(
                ledger.check(2, &rec(Kind::Declared, body.clone())).is_err(),
                "{body}"
            );
        }

        let end = json!({"change": "end_standing", "standing": 1, "note": "withdrawn"});
        ledger.check(2, &rec(Kind::Declared, end.clone())).unwrap();
        ledger.apply(2, &rec(Kind::Declared, end.clone())).unwrap();
        assert!(ledger.standings()[&1].ended.is_some());
        assert_eq!(
            ledger.check(3, &rec(Kind::Declared, end)),
            Err(LedgerError::InvalidStandingTarget(1))
        );
    }

    #[test]
    fn a_rally_opens_once_closes_once_and_keeps_what_came_before() {
        let rec = |kind: Kind, body: serde_json::Value| {
            let source = crate::record::Source {
                credential: "a1".into(),
                declared_name: (kind != Kind::Observed).then(|| "r".into()),
            };
            Record::new(kind, source, 1_000, body).unwrap()
        };
        let mut ledger = Ledger::default();
        ledger.apply(1, &ready()).unwrap();
        let close = json!({"change": "close_rally", "note": "done"});
        assert_eq!(
            ledger.check(2, &rec(Kind::Declared, close.clone())),
            Err(LedgerError::NoRallyOpen)
        );
        // A rally names projects by story prefix, and has a name.
        for body in [
            json!({"change": "open_rally", "name": "x", "projects": []}),
            json!({"change": "open_rally", "name": "x", "projects": ["ral"]}),
            json!({"change": "open_rally", "name": "x", "projects": ["RAL-1"]}),
            json!({"change": "open_rally", "name": " ", "projects": ["RAL"]}),
            json!({"change": "open_rally", "name": "x", "projects": ["RAL", "RAL"]}),
            json!({"change": "open_rally", "name": "x", "projects": ["RAL"], "teams": [""]}),
            json!({"change": "open_rally", "name": "x", "projects": ["RAL"], "teams": ["a", " A"]}),
        ] {
            assert!(
                ledger.check(2, &rec(Kind::Declared, body.clone())).is_err(),
                "{body}"
            );
        }
        let open = json!({"change": "open_rally", "name": "Friday", "projects": ["MUS", "RAL"],
                          "teams": ["rally", "muster"]});
        assert!(
            ledger.check(2, &rec(Kind::Observed, open.clone())).is_err(),
            "declared only"
        );
        ledger.check(2, &rec(Kind::Declared, open.clone())).unwrap();
        ledger.apply(2, &rec(Kind::Declared, open.clone())).unwrap();
        let rally = ledger.open_rally().unwrap();
        assert_eq!((rally.seq, rally.name.as_str()), (2, "Friday"));
        assert_eq!(rally.projects, ["MUS", "RAL"]);
        assert_eq!(rally.teams, ["rally", "muster"]);
        assert_eq!(
            ledger.check(3, &rec(Kind::Declared, open.clone())),
            Err(LedgerError::RallyAlreadyOpen(2))
        );
        ledger
            .check(3, &rec(Kind::Declared, close.clone()))
            .unwrap();
        ledger
            .apply(3, &rec(Kind::Declared, close.clone()))
            .unwrap();
        assert!(ledger.open_rally().is_none());
        assert_eq!(ledger.rallies()[0].closed.as_ref().unwrap().seq, 3);
        assert!(
            ledger.story(STORY).is_some(),
            "nothing before a rally is removed"
        );
        assert_eq!(
            ledger.check(4, &rec(Kind::Declared, close)),
            Err(LedgerError::NoRallyOpen)
        );
        ledger.check(4, &rec(Kind::Declared, open)).unwrap();
    }

    #[test]
    fn follows_the_normal_path() {
        let ledger = ledger_with(&[ready(), claim(), submit(), verify()]);
        let story = ledger.story(STORY).unwrap();
        assert_eq!(story.state, State::Verified);
        assert_eq!(story.team.as_deref(), Some("rally"));
        assert_eq!(story.history.len(), 4);
    }

    fn also_read(credential: &str, name: &str, corrects: u64, commit: &str) -> Record {
        let source = Source {
            credential: credential.into(),
            declared_name: Some(name.into()),
        };
        let body = json!({"change": "annotate", "corrects": corrects, "team": "watchman",
                          "note": "also read #123's head", "also_read": [commit]});
        Record::new(Kind::Declared, source, 1_000, body).unwrap()
    }

    /// RAL-67: the verifier adds a commit read to their own verification in force, and only they
    /// can; a new verdict drops what was added.
    #[test]
    fn a_verifier_adds_commits_read_to_their_own_verification() {
        let base = [ready(), claim(), submit(), verify()];
        // The verifier, same credential and name: added, and shown with the story.
        let mut records = base.to_vec();
        records.push(also_read("a1", "Watch", 4, OTHER_COMMIT));
        let ledger = ledger_with(&records);
        let story = ledger.story(STORY).unwrap();
        assert_eq!(story.also_read, [OTHER_COMMIT]);
        assert_eq!(story.verified_commit.as_deref(), Some(COMMIT));

        let refused = |record: Record| {
            let ledger = ledger_with(&base);
            ledger.check(5, &record).unwrap_err().to_string()
        };
        // Another credential, though it declares the verifier's name.
        assert!(
            refused(also_read("murph", "watch", 4, OTHER_COMMIT)).contains("only the verifier")
        );
        // The same credential under another name.
        assert!(refused(also_read("a1", "alice", 4, OTHER_COMMIT)).contains("only the verifier"));
        // A record that isn't the verification.
        assert!(refused(also_read("a1", "watch", 3, OTHER_COMMIT)).contains("in force"));
        // A short hash.
        assert!(refused(also_read("a1", "watch", 4, "abc1234")).contains("full hash"));

        // Sent back and verified again: the first verification is no longer in force, and what
        // was added to it is dropped.
        let mut again = records.clone();
        let correct = json!({"change": "correct", "story": STORY, "corrects": 4,
                             "note": "re-read", "state": "sent_back"});
        again.push(verifier(correct));
        let ledger = ledger_with(&again);
        assert!(ledger.story(STORY).unwrap().also_read.is_empty());
        assert!(
            ledger
                .check(7, &also_read("a1", "watch", 4, COMMIT))
                .is_err()
        );
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

    fn backfilled(
        kind: Kind,
        backfiller: &str,
        entered_by: &str,
        entered_at_ms: i64,
        entry: serde_json::Value,
    ) -> Record {
        record(
            kind,
            backfiller,
            json!({
                "change": "backfill", "entered_at_ms": entered_at_ms, "entered_by": entered_by,
                "entry": entry
            }),
        )
    }

    #[test]
    fn a_backfilled_entry_keeps_the_records_own_time_beside_when_it_was_entered() {
        // Records are made at 1_000; the fallback board entry was at 400, by bob, and the
        // watchman backfills it.
        let claim = json!({"change": "claim", "story": STORY, "team": "rally"});
        let entry = backfilled(Kind::Declared, "watch", "bob", 400, claim.clone());
        let ledger = ledger_with(&[ready(), entry]);
        let story = ledger.story(STORY).unwrap();
        assert_eq!(story.state, State::Claimed);
        let live = &story.history[0];
        let back = &story.history[1];
        assert_eq!(live.backfilled, None);
        assert_eq!(back.at_ms, 1_000, "the record's own time, not the entry's");
        assert_eq!(back.source.declared_name.as_deref(), Some("watch"));
        assert_eq!(
            back.backfilled,
            Some(Backfill {
                entered_at_ms: 400,
                entered_by: "bob".into()
            })
        );
        assert_eq!(
            back.change,
            serde_json::from_value::<Change>(claim).unwrap()
        );
        // A team's state the same way.
        let state = json!({"change": "team_state", "team": "rally", "status": "working"});
        let ledger = ledger_with(&[backfilled(Kind::Declared, "watch", "bob", 400, state)]);
        let team = &ledger.teams()["rally"].current;
        assert_eq!(team.at_ms, 1_000);
        assert_eq!(team.backfilled.as_ref().unwrap().entered_by, "bob");
    }

    #[test]
    fn a_backfilled_entry_cannot_pass_for_a_live_one() {
        let claim = json!({"change": "claim", "story": STORY, "team": "rally"});
        for (entered_at_ms, reason) in [
            (1_000, "not before"),
            (1_001, "after"),
            (0, "epoch"),
            (-5, "before epoch"),
        ] {
            let mut ledger = ledger_with(&[ready()]);
            let entry = backfilled(Kind::Declared, "watch", "bob", entered_at_ms, claim.clone());
            assert_eq!(
                apply_next(&mut ledger, &entry),
                Err(LedgerError::BackfillNotEarlier {
                    entered_at_ms,
                    at_ms: 1_000
                }),
                "{reason}"
            );
            assert_eq!(state(&ledger), State::Ready);
        }
        let mut ledger = ledger_with(&[ready()]);
        let nobody = backfilled(Kind::Declared, "watch", " ", 400, claim.clone());
        assert_eq!(
            apply_next(&mut ledger, &nobody),
            Err(LedgerError::EmptyField("entered_by"))
        );
        let control = backfilled(Kind::Declared, "watch", "bob\u{7}", 400, claim.clone());
        assert_eq!(
            ledger.check(ledger.last_seq() + 1, &control),
            Err(LedgerError::ControlCharacter("entered_by"))
        );
        let hidden = backfilled(Kind::Declared, "watch", "b\u{200b}ob", 400, claim.clone());
        assert_eq!(
            ledger.check(ledger.last_seq() + 1, &hidden),
            Err(LedgerError::InvisibleCharacter("entered_by"))
        );
        let inner = json!({
            "change": "backfill", "entered_at_ms": 300, "entered_by": "bob", "entry": claim
        });
        let nested = backfilled(Kind::Declared, "watch", "bob", 400, inner);
        assert!(matches!(
            apply_next(&mut ledger, &nested),
            Err(LedgerError::InvalidChange(_))
        ));
        // The entry itself is still checked: a bad change inside is refused the same way.
        let short = json!({"change": "merge", "story": STORY, "commit": "abc"});
        let bad = backfilled(Kind::Declared, "watch", "bob", 400, short);
        assert_eq!(
            apply_next(&mut ledger, &bad),
            Err(LedgerError::InvalidCommit)
        );
        assert_eq!(ledger.story(STORY).unwrap().history.len(), 1);
    }

    #[test]
    fn a_backfilled_entry_is_checked_as_whoever_entered_it() {
        let verify = json!({"change": "verify", "story": STORY, "note": "fine", "commit": COMMIT});
        // The watchman backfills a verification the submitter wrote on the board: refused, as it
        // would have been live.
        let mut ledger = ledger_with(&[ready(), claim(), submit()]);
        let own = backfilled(Kind::Verified, "watch", "alice", 400, verify.clone());
        assert!(matches!(
            apply_next(&mut ledger, &own),
            Err(LedgerError::SelfVerification { .. })
        ));
        // The submitter backfills the watchman's verification: allowed, and the verifier is the
        // watchman. The record's kind is still what the entry needs.
        let theirs = backfilled(Kind::Verified, "alice", "watch", 400, verify.clone());
        apply_next(&mut ledger, &theirs).unwrap();
        assert_eq!(state(&ledger), State::Verified);
        let declared = backfilled(Kind::Declared, "alice", "watch", 400, verify.clone());
        let mut ledger = ledger_with(&[ready(), claim(), submit()]);
        assert!(matches!(
            apply_next(&mut ledger, &declared),
            Err(LedgerError::WrongKind { .. })
        ));
        // A backfilled submission makes whoever entered it an author, and the new rules apply to
        // the entry: a backfilled verification names its commit.
        let submit = json!({"change": "submit", "story": STORY, "team": "rally", "evidence": "x"});
        let mut ledger = ledger_with(&[ready(), claim()]);
        apply_next(
            &mut ledger,
            &backfilled(Kind::Declared, "watch", "bob", 400, submit),
        )
        .unwrap();
        assert_eq!(
            ledger.story(STORY).unwrap().submitted_by.as_deref(),
            Some("bob")
        );
        assert!(matches!(
            apply_next(&mut ledger, &verify_as("bob")),
            Err(LedgerError::SelfVerification { .. })
        ));
        let no_commit = json!({"change": "verify", "story": STORY, "note": "fine"});
        let old = backfilled(Kind::Verified, "watch", "watch", 400, no_commit);
        assert_eq!(
            ledger.check(ledger.last_seq() + 1, &old),
            Err(LedgerError::MissingCommit)
        );
    }

    fn block(body: serde_json::Value) -> Record {
        let mut full = json!({"change": "block", "team": "rally", "note": "waiting"});
        full.as_object_mut()
            .unwrap()
            .extend(body.as_object().unwrap().clone());
        team(full)
    }

    /// A declared record from `alice` at a given time, for the operator's window (RAL-56).
    fn timed(at_ms: i64, body: serde_json::Value) -> Record {
        let source = Source {
            credential: "a1".into(),
            declared_name: Some("alice".into()),
        };
        Record::new(Kind::Declared, source, at_ms, body).unwrap()
    }

    const HOUR_MS: u64 = 3_600_000;

    /// A question to the operator about STORY, asked at 1,000 ms, as record 2.
    fn asked_operator() -> Vec<Record> {
        vec![
            ready(),
            timed(
                1_000,
                json!({
                    "change": "block", "team": "rally", "story": STORY, "on_team": "operator",
                    "note": "may Rally read the token file?"
                }),
            ),
        ]
    }

    fn shelve(at_ms: i64, blocker: u64, team: &str) -> Record {
        timed(
            at_ms,
            json!({"change": "shelve", "blocker": blocker, "team": team, "window_ms": HOUR_MS}),
        )
    }

    #[test]
    fn a_story_is_shelved_only_after_the_operators_window() {
        let ledger = ledger_with(&asked_operator());
        let early = shelve(1_000 + HOUR_MS as i64 - 1, 2, "rally");
        assert_eq!(
            ledger.check(3, &early),
            Err(LedgerError::InvalidShelf(
                2,
                "the operator's window hasn't passed since the question was asked"
            ))
        );
        let due = shelve(1_000 + HOUR_MS as i64, 2, "rally");
        let mut shelved = ledger.clone();
        shelved.apply(3, &due).unwrap();
        let shelf = shelved.blockers()[&2].shelves.last().unwrap();
        assert_eq!((shelf.seq, shelf.window_ms), (3, HOUR_MS));
        // Once is enough.
        assert_eq!(
            shelved.check(4, &shelve(9 * HOUR_MS as i64, 2, "rally")),
            Err(LedgerError::InvalidShelf(2, "it's already shelved"))
        );
        // The answer closes the question, and the shelf stays as history.
        let answer = timed(
            2 * HOUR_MS as i64,
            json!({"change": "unblock", "blocker": 2, "note": "yes", "answer": {"from": "direct"}}),
        );
        shelved.check(4, &answer).unwrap();
        shelved.apply(4, &answer).unwrap();
        let blocker = &shelved.blockers()[&2];
        assert!(blocker.closed.is_some() && !blocker.shelves.is_empty());
        assert_eq!(
            blocker.closed.as_ref().unwrap().answer,
            Some(Answer::Direct)
        );
    }

    #[test]
    fn only_the_waiting_team_shelves_an_open_question_to_the_operator() {
        let mut records = asked_operator();
        records.push(timed(
            1_000,
            json!({"change": "block", "team": "rally", "story": STORY, "on_team": "muster",
                   "note": "waiting on MUS-44"}),
        ));
        records.push(timed(
            1_000,
            json!({"change": "block", "team": "rally", "on_team": "operator", "note": "a login"}),
        ));
        let ledger = ledger_with(&records);
        let late = 1_000 + HOUR_MS as i64;
        for (blocker, team, why) in [
            (2, "muster", "another team is waiting on it"),
            (3, "rally", "it doesn't wait on the operator"),
            (4, "rally", "it blocks a team, not a story"),
        ] {
            assert_eq!(
                ledger.check(5, &shelve(late, blocker, team)),
                Err(LedgerError::InvalidShelf(blocker, why))
            );
        }
        assert_eq!(
            ledger.check(5, &shelve(late, 1, "rally")),
            Err(LedgerError::InvalidBlockerTarget(1))
        );
        let zero = timed(
            late,
            json!({"change": "shelve", "blocker": 2, "team": "rally", "window_ms": 0}),
        );
        assert_eq!(
            ledger.check(5, &zero),
            Err(LedgerError::InvalidShelf(
                2,
                "the window must be longer than nothing"
            ))
        );
    }

    #[test]
    fn closing_a_question_to_the_operator_says_where_the_answer_came_from() {
        let mut records = asked_operator();
        records.push(block(json!({"on_team": "muster"})));
        let ledger = ledger_with(&records);
        let close = |blocker: u64, answer: serde_json::Value| {
            let mut body = json!({"change": "unblock", "blocker": blocker, "note": "answered"});
            if !answer.is_null() {
                body["answer"] = answer;
            }
            timed(5_000, body)
        };
        assert!(matches!(
            ledger.check(4, &close(2, serde_json::Value::Null)),
            Err(LedgerError::AnswerSource(_))
        ));
        assert!(
            ledger
                .check(4, &close(2, json!({"from": "direct"})))
                .is_ok()
        );
        assert!(
            ledger
                .check(
                    4,
                    &close(2, json!({"from": "relayed", "by": "grand-photon"}))
                )
                .is_ok()
        );
        assert_eq!(
            ledger.check(4, &close(2, json!({"from": "relayed", "by": " "}))),
            Err(LedgerError::EmptyField("relayed by"))
        );
        assert!(
            ledger
                .check(4, &close(2, json!({"from": "rumour"})))
                .is_err()
        );
        // Another blocker's closing doesn't claim to be the operator's answer.
        assert!(matches!(
            ledger.check(4, &close(3, json!({"from": "direct"}))),
            Err(LedgerError::AnswerSource(_))
        ));
        assert!(ledger.check(4, &close(3, serde_json::Value::Null)).is_ok());
        // Written before RAL-56, a question to the operator closed without one still replays.
        records.push(close(2, serde_json::Value::Null));
        assert!(Ledger::replay(&records).is_ok());
    }

    /// Questions to the operator about RAL-2, RAL-3 and RAL-4, asked at 0, as records 1 to 3.
    fn three_questions() -> Vec<Record> {
        ["RAL-2", "RAL-3", "RAL-4"]
            .into_iter()
            .map(|story| {
                timed(
                    1,
                    json!({"change": "block", "team": "rally", "story": story,
                           "on_team": "operator", "note": "may we?"}),
                )
            })
            .collect()
    }

    fn away(at_ms: i64, ends_at_ms: i64, shelves: &[u64]) -> Record {
        let source = Source {
            credential: "rally".into(),
            declared_name: None,
        };
        let body = json!({"change": "operator_away", "entered_at_ms": at_ms,
                          "ends_at_ms": ends_at_ms, "shelves": shelves});
        Record::new(Kind::Observed, source, at_ms, body).unwrap()
    }

    fn answer(at_ms: i64, blocker: u64) -> Record {
        timed(
            at_ms,
            json!({"change": "unblock", "blocker": blocker, "note": "yes",
                   "answer": {"from": "direct"}}),
        )
    }

    fn push(ledger: &mut Ledger, record: &Record) {
        let seq = ledger.last_seq() + 1;
        ledger.check(seq, record).unwrap();
        ledger.apply(seq, record).unwrap();
    }

    const W: i64 = HOUR_MS as i64;

    #[test]
    fn the_operator_is_away_after_a_run_of_unanswered_shelves() {
        let mut ledger = ledger_with(&three_questions());
        push(&mut ledger, &shelve(1 + W, 1, "rally"));
        push(&mut ledger, &shelve(1 + W, 2, "rally"));
        assert_eq!(ledger.unanswered_shelves(1 + W), [4, 5]);
        // Not on the second: the run must be all the unanswered shelves, and the third isn't yet.
        assert_eq!(
            ledger.check(6, &away(2 * W, 3 * W, &[4, 5, 6])),
            Err(LedgerError::InvalidAway(
                "its shelves must be the run of unanswered shelves"
            ))
        );
        push(&mut ledger, &shelve(1 + W, 3, "rally"));
        assert_eq!(ledger.unanswered_shelves(1 + W), [4, 5, 6]);
        // Only Rally records it, as observed.
        let declared = timed(
            2 * W,
            json!({"change": "operator_away", "entered_at_ms": 2 * W, "ends_at_ms": 3 * W,
                   "shelves": [4, 5, 6]}),
        );
        assert!(matches!(
            ledger.check(7, &declared),
            Err(LedgerError::WrongKind { .. })
        ));
        push(&mut ledger, &away(2 * W, 3 * W, &[4, 5, 6]));
        let entered = ledger.operator_away().unwrap();
        assert_eq!((entered.seq, entered.shelves.clone()), (7, vec![4, 5, 6]));
        assert!(entered.active_at(3 * W - 1) && !entered.active_at(3 * W));
        // The run starts again, and a second away can't start inside the first.
        assert!(ledger.unanswered_shelves(2 * W).is_empty());
        assert_eq!(
            ledger.check(8, &away(2 * W + 5, 4 * W, &[])),
            Err(LedgerError::InvalidAway(
                "its shelves must be the run of unanswered shelves"
            ))
        );
    }

    #[test]
    fn an_answer_ends_the_away_hour_early_by_the_services_time_only() {
        let mut ledger = ledger_with(&three_questions());
        for blocker in 1..=3 {
            push(&mut ledger, &shelve(1 + W, blocker, "rally"));
        }
        push(&mut ledger, &away(2 * W, 3 * W, &[4, 5, 6]));
        // Recorded after the hour, though it declares it was entered during it: doesn't count.
        let late = timed(
            3 * W + 1,
            json!({"change": "backfill", "entered_at_ms": 2 * W + 10, "entered_by": "alice",
                   "entry": {"change": "unblock", "blocker": 1, "note": "yes",
                             "answer": {"from": "direct"}}}),
        );
        let mut backdated = ledger.clone();
        push(&mut backdated, &late);
        assert_eq!(backdated.operator_away().unwrap().ended_early_by, None);
        // Recorded during the hour: ends it.
        push(&mut ledger, &answer(2 * W + 10, 2));
        let ended = ledger.operator_away().unwrap();
        assert_eq!(ended.ended_early_by, Some(8));
        assert_eq!(ended.ended_early_at_ms, Some(2 * W + 10));
        assert!(!ended.active_at(2 * W + 11));
        // The operator is back, so the other shelves are asked again from the answer's time,
        // as when the hour runs out: shelvable again a window after it, not before.
        assert_eq!(
            ledger.check(9, &shelve(3 * W + 9, 3, "rally")),
            Err(LedgerError::InvalidShelf(
                3,
                "the operator's window hasn't passed since the question was asked"
            ))
        );
        push(&mut ledger, &shelve(3 * W + 10, 3, "rally"));
        assert_eq!(ledger.blockers()[&3].shelves.len(), 2);
    }

    #[test]
    fn after_the_hour_questions_are_asked_again_and_the_run_starts_from_nothing() {
        let mut ledger = ledger_with(&three_questions());
        for blocker in 1..=3 {
            push(&mut ledger, &shelve(1 + W, blocker, "rally"));
        }
        push(&mut ledger, &away(2 * W, 3 * W, &[4, 5, 6]));
        // During the hour the shelves stand.
        assert_eq!(
            ledger.check(8, &shelve(3 * W - 1, 1, "rally")),
            Err(LedgerError::InvalidShelf(1, "it's already shelved"))
        );
        // After it, each is asked again, with the window counted from the hour's end.
        assert_eq!(
            ledger.check(8, &shelve(4 * W - 1, 1, "rally")),
            Err(LedgerError::InvalidShelf(
                1,
                "the operator's window hasn't passed since the question was asked"
            ))
        );
        assert!(ledger.unanswered_shelves(3 * W).is_empty());
        push(&mut ledger, &shelve(4 * W, 1, "rally"));
        push(&mut ledger, &shelve(4 * W, 2, "rally"));
        assert_eq!(ledger.blockers()[&1].shelves.len(), 2);
        // Two after the hour aren't a run of three; a third is.
        assert_eq!(ledger.unanswered_shelves(4 * W), [8, 9]);
        push(&mut ledger, &shelve(4 * W, 3, "rally"));
        assert_eq!(ledger.unanswered_shelves(4 * W), [8, 9, 10]);
        push(&mut ledger, &away(4 * W + 1, 5 * W, &[8, 9, 10]));
    }

    #[test]
    fn an_answer_at_any_time_starts_the_run_again() {
        let mut ledger = ledger_with(&three_questions());
        push(&mut ledger, &shelve(1 + W, 1, "rally"));
        push(&mut ledger, &shelve(1 + W, 2, "rally"));
        push(&mut ledger, &answer(1 + W, 1));
        assert!(ledger.unanswered_shelves(1 + W).is_empty());
        push(&mut ledger, &shelve(1 + W, 3, "rally"));
        assert_eq!(ledger.unanswered_shelves(1 + W), [7]);
    }

    #[test]
    fn a_read_names_the_readers_team_but_never_the_storys_own() {
        let ledger = ledger_with(&[ready(), claim(), submit()]);
        let read = |team: &str| {
            verifier(
                json!({"change": "verify", "story": STORY, "note": "read it",
                            "commit": COMMIT, "team": team}),
            )
        };
        assert_eq!(
            ledger.check(4, &read("Rally")),
            Err(LedgerError::ReaderIsAuthor("Rally".into()))
        );
        assert_eq!(
            ledger.check(4, &read("operator")),
            Err(LedgerError::ReservedTeam)
        );
        assert_eq!(
            ledger.check(4, &read(" ")),
            Err(LedgerError::EmptyField("team"))
        );
        assert!(ledger.check(4, &read("muster")).is_ok());
        let back = verifier(
            json!({"change": "send_back", "story": STORY, "note": "no test",
                                    "team": "rally"}),
        );
        assert_eq!(
            ledger.check(4, &back),
            Err(LedgerError::ReaderIsAuthor("rally".into()))
        );
        // Without one, as before RAL-57.
        assert!(ledger.check(4, &verify()).is_ok());
    }

    #[test]
    fn a_new_story_takes_a_number_above_every_one_its_prefix_has() {
        let new =
            |story: &str| team(json!({"change": "new_story", "story": story, "title": "A title"}));
        // STORY is RAL-3.
        let mut ledger = ledger_with(&[ready()]);
        assert_eq!(ledger.highest_number("RAL"), 3);
        // Taken, or below the highest: refused, so two teams can't share one.
        for story in ["RAL-3", "RAL-1"] {
            assert_eq!(
                ledger.check(2, &new(story)),
                Err(LedgerError::NumberTaken {
                    story: story.into(),
                    highest: 3
                })
            );
        }
        push(&mut ledger, &new("RAL-4"));
        assert_eq!(ledger.story("RAL-4").unwrap().state, State::Ready);
        assert_eq!(
            ledger.check(3, &new("RAL-4")),
            Err(LedgerError::NumberTaken {
                story: "RAL-4".into(),
                highest: 4
            })
        );
        // Another prefix has its own numbers, a skip ahead is allowed, and IDs by hand still work.
        push(&mut ledger, &new("MUS-1"));
        push(&mut ledger, &new("RAL-9"));
        push(
            &mut ledger,
            &team(json!({"change": "ready", "story": "RAL-5"})),
        );
        assert_eq!(ledger.highest_number("RAL"), 9);
        assert_eq!(ledger.highest_number("MUS"), 1);
        let blank = team(json!({"change": "new_story", "story": "RAL-10", "title": " "}));
        assert_eq!(
            ledger.check(ledger.last_seq() + 1, &blank),
            Err(LedgerError::EmptyField("title"))
        );
    }

    #[test]
    fn the_submitter_amends_evidence_while_in_review_and_history_keeps_both() {
        let amend = |name: &str, team: &str| {
            record(
                Kind::Declared,
                name,
                json!({"change": "submit", "story": STORY, "team": team,
                       "evidence": format!("o/r@{COMMIT}"), "amend": true}),
            )
        };
        // Not before a submission: there's nothing to amend.
        let mut ledger = ledger_with(&[ready(), claim()]);
        assert_eq!(
            ledger.check(3, &amend("alice", "rally")),
            Err(LedgerError::NotInReview {
                story: STORY.into(),
                from: State::Claimed
            })
        );
        push(&mut ledger, &submit());
        // A second plain submit is refused, and the error says to amend.
        assert_eq!(
            ledger.check(4, &submit()),
            Err(LedgerError::AlreadyInReview(STORY.into()))
        );
        assert!(
            LedgerError::AlreadyInReview(STORY.into())
                .to_string()
                .contains("rally amend RAL-3")
        );
        // Only the holding team, and only the submitter.
        assert!(matches!(
            ledger.check(4, &amend("alice", "other")),
            Err(LedgerError::WrongTeam { .. })
        ));
        assert_eq!(
            ledger.check(4, &amend("bob", "rally")),
            Err(LedgerError::NotTheSubmitter {
                story: STORY.into(),
                submitter: "alice".into()
            })
        );
        assert!(matches!(
            ledger.check(
                4,
                &verifier(json!({"change": "submit", "story": STORY,
                "team": "rally", "evidence": "x", "amend": true}))
            ),
            Err(LedgerError::WrongKind { .. })
        ));
        push(&mut ledger, &amend("Alice", "rally"));
        let story = ledger.story(STORY).unwrap();
        assert_eq!(story.state, State::InReview);
        assert_eq!(story.submitted_by.as_deref(), Some("alice"));
        let evidence: Vec<(&str, bool)> = story
            .history
            .iter()
            .filter_map(|entry| match &entry.change {
                Change::Submit {
                    evidence, amend, ..
                } => Some((evidence.as_str(), *amend)),
                _ => None,
            })
            .collect();
        let fixed = format!("o/r@{COMMIT}");
        assert_eq!(evidence, [("abc123", false), (fixed.as_str(), true)]);
        // The amendment is the submission Rally checks next.
        let jobs = crate::evidence::unchecked(&ledger);
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            (jobs[0].submit, jobs[0].evidence.as_str()),
            (4, fixed.as_str())
        );
        // The same evidence again changes nothing, so it's refused.
        assert_eq!(
            ledger.check(5, &amend("alice", "rally")),
            Err(LedgerError::NothingAmended(STORY.into()))
        );
        // A read of another commit than the amended evidence's is refused, verify or send-back,
        // saying which record amended it; a send-back naming no commit still stands.
        let stale = |change: &str| {
            verifier(json!({"change": change, "story": STORY, "note": "read it",
                            "commit": OTHER_COMMIT}))
        };
        for change in ["verify", "send_back"] {
            assert_eq!(
                ledger.check(5, &stale(change)),
                Err(LedgerError::ReadBeforeAmendment {
                    story: STORY.into(),
                    amended_at: 4,
                    commit: COMMIT.into()
                })
            );
        }
        assert!(ledger.check(5, &send_back()).is_ok());
        // The amended commit, in any case, is read.
        let upper = verifier(
            json!({"change": "verify", "story": STORY, "note": "read it",
                                    "commit": COMMIT.to_ascii_uppercase()}),
        );
        assert!(ledger.check(5, &upper).is_ok());
        push(&mut ledger, &verify());
        assert_eq!(state(&ledger), State::Verified);
        assert!(matches!(
            ledger.check(6, &amend("alice", "rally")),
            Err(LedgerError::NotInReview { .. })
        ));
        // A plain submit leaves `amend` out of the record, so older readers see no change.
        let plain = Change::Submit {
            story: STORY.into(),
            team: "rally".into(),
            evidence: "e".into(),
            untested: None,
            amend: false,
            part: None,
            of: None,
        };
        let plain = serde_json::to_value(&plain).unwrap();
        assert!(plain.get("amend").is_none());
        // Nor `part` or `of` (RAL-79): a story without parts writes what it always wrote.
        assert!(plain.get("part").is_none() && plain.get("of").is_none());
    }

    #[test]
    fn rally_records_one_evidence_check_per_submission_as_observed() {
        let observed = |submit: u64| {
            let source = Source {
                credential: "rally".into(),
                declared_name: None,
            };
            let body = json!({"change": "evidence_check", "story": STORY, "submit": submit,
                              "outcome": {"checkable": true, "exists": false}});
            Record::new(Kind::Observed, source, 1_000, body).unwrap()
        };
        let mut ledger = ledger_with(&[ready(), claim(), submit()]);
        // Only of a submission of this story, and only by Rally.
        assert_eq!(
            ledger.check(4, &observed(2)),
            Err(LedgerError::InvalidEvidenceCheck(
                2,
                "it isn't a submission of this story"
            ))
        );
        let declared = team(
            json!({"change": "evidence_check", "story": STORY, "submit": 3,
                                    "outcome": {"checkable": true}}),
        );
        assert!(matches!(
            ledger.check(4, &declared),
            Err(LedgerError::WrongKind { .. })
        ));
        push(&mut ledger, &observed(3));
        // It changes no state, and there's one per submission.
        assert_eq!(state(&ledger), State::InReview);
        assert_eq!(
            ledger.check(5, &observed(3)),
            Err(LedgerError::InvalidEvidenceCheck(3, "it's already checked"))
        );
        // What wasn't tested is its own field, limited like a note.
        let mut claimed = ledger_with(&[ready(), claim()]);
        let with_untested = team(json!({"change": "submit", "story": STORY, "team": "rally",
                                         "evidence": "r@1", "untested": "the Windows build"}));
        push(&mut claimed, &with_untested);
        let too_long = team(json!({"change": "submit", "story": STORY, "team": "rally",
                                    "evidence": "r@1", "untested": "x".repeat(4_001)}));
        assert_eq!(
            ledger_with(&[ready(), claim()]).check(3, &too_long),
            Err(LedgerError::TooLong("note", 4_000))
        );
    }

    #[test]
    fn no_team_can_be_named_operator() {
        let ledger = ledger_with(&[ready()]);
        for name in ["operator", "Operator", " OPERATOR "] {
            for body in [
                json!({"change": "claim", "story": STORY, "team": name}),
                json!({"change": "team_state", "team": name, "status": "working"}),
                json!({"change": "block", "team": name, "note": "waiting"}),
            ] {
                assert_eq!(
                    ledger.check(2, &team(body.clone())),
                    Err(LedgerError::ReservedTeam),
                    "{body}"
                );
            }
        }
        // Waiting on the operator is what the name is for.
        let body = json!({"change": "block", "team": "rally", "on_team": "operator", "note": "n"});
        assert!(ledger.check(2, &team(body)).is_ok());
    }

    fn unblock(blocker: u64) -> Record {
        record(
            Kind::Declared,
            "bob",
            json!({"change": "unblock", "blocker": blocker, "note": "MUS-44 merged"}),
        )
    }

    #[test]
    fn a_blocker_stays_open_until_someone_closes_it_with_who_and_why() {
        let mut ledger = ledger_with(&[
            ready(),
            block(json!({"story": STORY, "on_story": "MUS-44", "on_team": "muster"})),
        ]);
        let open = &ledger.blockers()[&2];
        assert_eq!(open.team, "rally");
        assert_eq!(open.story.as_deref(), Some(STORY));
        assert_eq!(
            open.on_story.as_deref(),
            Some("MUS-44"),
            "another team's story"
        );
        assert_eq!(open.source.declared_name.as_deref(), Some("alice"));
        assert_eq!(open.closed, None);
        // A blocker is apart from the story: its state doesn't move.
        assert_eq!(state(&ledger), State::Ready);
        apply_next(&mut ledger, &unblock(2)).unwrap();
        let closed = ledger.blockers()[&2].closed.as_ref().unwrap();
        assert_eq!(closed.seq, 3);
        assert_eq!(closed.source.declared_name.as_deref(), Some("bob"));
        assert_eq!(closed.note, "MUS-44 merged");
        // Closed once: it's no longer open to close, and nothing else is either.
        assert_eq!(
            apply_next(&mut ledger, &unblock(2)),
            Err(LedgerError::InvalidBlockerTarget(2))
        );
        assert_eq!(
            apply_next(&mut ledger, &unblock(1)),
            Err(LedgerError::InvalidBlockerTarget(1))
        );
    }

    #[test]
    fn a_blocked_story_and_a_blocked_team_are_recorded_separately() {
        let ledger = ledger_with(&[block(json!({})), block(json!({"story": "RAL-9"}))]);
        assert_eq!(ledger.blockers()[&1].story, None, "the whole team");
        assert_eq!(ledger.blockers()[&2].story.as_deref(), Some("RAL-9"));
        // Neither touches the team's declared state.
        assert!(ledger.teams().is_empty());
    }

    #[test]
    fn a_blocker_is_checked_like_other_text() {
        let mut ledger = Ledger::default();
        for (body, expected) in [
            (json!({"note": " "}), LedgerError::EmptyField("note")),
            (json!({"team": ""}), LedgerError::EmptyField("team")),
            (
                json!({"story": "ral-1"}),
                LedgerError::InvalidStoryId("ral-1".into()),
            ),
            (
                json!({"on_story": "MUS"}),
                LedgerError::InvalidStoryId("MUS".into()),
            ),
            (json!({"on_team": " "}), LedgerError::EmptyField("on_team")),
            (
                json!({"story": "RAL-2", "on_story": "RAL-2"}),
                LedgerError::BlockedOnItself("RAL-2".into()),
            ),
        ] {
            assert_eq!(
                apply_next(&mut ledger, &block(body.clone())),
                Err(expected),
                "{body}"
            );
        }
        let zero = block(json!({"on_story": "MUS-044"}));
        assert_eq!(
            ledger.check(1, &zero),
            Err(LedgerError::InvalidStoryId("MUS-044".into()))
        );
        let verified = record(
            Kind::Verified,
            "watch",
            json!({"change": "block", "team": "rally", "note": "x"}),
        );
        assert!(matches!(
            apply_next(&mut ledger, &verified),
            Err(LedgerError::WrongKind { .. })
        ));
        assert!(ledger.blockers().is_empty());
    }

    #[test]
    fn checking_an_unblock_needs_only_its_blocker() {
        let ledger = ledger_with(&[block(json!({}))]);
        assert!(ledger.check(2, &unblock(1)).is_ok());
        assert_eq!(
            ledger.check(2, &unblock(7)),
            Err(LedgerError::InvalidBlockerTarget(7))
        );
        // A verifier can close one too, and a backfilled close keeps both times.
        let mut ledger = ledger_with(&[block(json!({}))]);
        let backfilled = record(
            Kind::Declared,
            "watch",
            json!({
                "change": "backfill", "entered_at_ms": 400, "entered_by": "bob",
                "entry": {"change": "unblock", "blocker": 1, "note": "done"}
            }),
        );
        assert!(ledger.check(2, &backfilled).is_ok());
        apply_next(&mut ledger, &backfilled).unwrap();
        let closed = ledger.blockers()[&1].closed.as_ref().unwrap();
        assert_eq!(closed.backfilled.as_ref().unwrap().entered_by, "bob");
        assert_eq!(closed.at_ms, 1_000);
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
                    // A resubmit in review, and one without a claim, have their own errors
                    // (RAL-71, RAL-72).
                    matches!(
                        result,
                        Err(LedgerError::InvalidTransition { .. }
                            | LedgerError::AlreadyInReview(_)
                            | LedgerError::ClaimLapsed { .. })
                    ),
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
        let unknown = json!({"change": "team_state", "team": "rally", "status": "asleep"});
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
        let ledger = ledger_with(&[ready(), claim()]);
        let next = ledger.last_seq() + 1;
        let long = "x".repeat(4_001);
        let body = json!({"change": "correct", "story": STORY, "corrects": 2, "note": long});
        assert_eq!(
            ledger.check(next, &team(body)),
            Err(LedgerError::TooLong("note", 4_000))
        );
        let long = "x".repeat(1_001);
        let body = json!({"change": "submit", "story": STORY, "team": "rally", "evidence": long});
        assert_eq!(
            ledger.check(next, &team(body)),
            Err(LedgerError::TooLong("evidence", 1_000))
        );
        let long = "x".repeat(65);
        let body = json!({"change": "team_state", "team": long, "status": "working"});
        assert_eq!(
            ledger.check(next, &team(body)),
            Err(LedgerError::TooLong("team", 64))
        );
        let long = "x".repeat(65);
        let body = json!({"change": "block", "team": "rally", "on_team": long, "note": "n"});
        assert_eq!(
            ledger.check(next, &team(body)),
            Err(LedgerError::TooLong("team", 64))
        );
    }

    #[test]
    fn refuses_control_characters_in_team_names() {
        let ledger = Ledger::default();
        let body = json!({"change": "team_state", "team": "ral\nly", "status": "working"});
        assert_eq!(
            ledger.check(1, &team(body)),
            Err(LedgerError::ControlCharacter("team"))
        );
    }

    #[test]
    fn refuses_invisible_and_direction_characters_in_new_names() {
        let ledger = ledger_with(&[ready()]);
        // A zero-width space, a right-to-left override, a word joiner, a tag character.
        for name in [
            "al\u{200b}ice",
            "\u{202e}ecila",
            "ali\u{2060}ce",
            "alice\u{e0041}",
        ] {
            let next = ledger.last_seq() + 1;
            let body = json!({"change": "claim", "story": STORY, "team": name});
            assert_eq!(
                ledger.check(next, &team(body)),
                Err(LedgerError::InvisibleCharacter("team")),
                "{name:?}"
            );
            let body = json!({"change": "team_state", "team": "rally", "status": "working"});
            let declared = record(Kind::Declared, name, body.clone());
            assert_eq!(
                ledger.check(next, &declared),
                Err(LedgerError::InvisibleCharacter("declared name")),
                "{name:?}"
            );
            let source = Source {
                credential: name.into(),
                declared_name: Some("alice".into()),
            };
            let machine = Record::new(Kind::Declared, source, 1_000, body).unwrap();
            assert_eq!(
                ledger.check(next, &machine),
                Err(LedgerError::InvisibleCharacter("source.credential")),
                "{name:?}"
            );
            let body = json!({
                "change": "block", "team": "rally", "on_team": name, "note": "waiting"
            });
            assert_eq!(
                ledger.check(next, &team(body)),
                Err(LedgerError::InvisibleCharacter("team")),
                "{name:?}"
            );
        }
        // Letters from other scripts are names like any other.
        let body = json!({"change": "claim", "story": STORY, "team": "équipe-東京"});
        assert!(ledger.check(ledger.last_seq() + 1, &team(body)).is_ok());
    }

    #[test]
    fn an_old_ledger_replays_after_its_limits_tighten() {
        // Records written before a limit, or a refused character, still replay: those rules are
        // for new records only (RAL-41).
        let long = "x".repeat(4_001);
        let hidden = "ral\u{200b}ly";
        let records = [
            ready(),
            team(json!({"change": "claim", "story": STORY, "team": hidden})),
            record(
                Kind::Declared,
                "ali\u{202e}ce",
                json!({"change": "submit", "story": STORY, "team": hidden, "evidence": long}),
            ),
        ];
        let ledger = ledger_with(&records);
        assert_eq!(ledger.story(STORY).unwrap().state, State::InReview);
        for (seq, record) in records.iter().enumerate().skip(1) {
            let before = ledger_with(&records[..seq]);
            assert!(
                before.check(seq as u64 + 1, record).is_err(),
                "record {seq}"
            );
        }
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

    /// RAL-63: a v3 add names a watchman, credential and scope; an end only a watchman; and
    /// only a v3 add has a scope, which must be one of the three forms.
    #[test]
    fn a_v3_designation_has_the_fields_its_action_needs() {
        let base = || {
            json!({"change": "designate", "watchman": "w", "after_seq": 0,
                   "after_hash": "a".repeat(64), "key_fingerprint": "SHA256:abc",
                   "signature": "-----BEGIN SSH SIGNATURE-----\nx\n-----END SSH SIGNATURE-----"})
        };
        let with = |fields: serde_json::Value| {
            let mut body = base();
            for (k, v) in fields.as_object().unwrap() {
                if v.is_null() {
                    body.as_object_mut().unwrap().remove(k);
                } else {
                    body[k] = v.clone();
                }
            }
            let change: Change = serde_json::from_value(body).unwrap();
            check_fields(&change)
        };
        assert!(with(json!({"action": "add", "credential": "a1-w", "scope": "all"})).is_ok());
        for scope in ["team:rally", "machine:a1", "machine:a1-watch.2@x"] {
            assert!(
                with(json!({"action": "add", "credential": "c", "scope": scope})).is_ok(),
                "{scope}"
            );
        }
        assert!(with(json!({"action": "end"})).is_ok());
        for (fields, why) in [
            (json!({"action": "add", "credential": "c"}), "no scope"),
            (json!({"action": "add", "scope": "all"}), "no credential"),
            (
                json!({"action": "add", "credential": "c", "scope": "all", "watchman": null}),
                "no watchman",
            ),
            (
                json!({"action": "end", "credential": "c"}),
                "an end binds nothing",
            ),
            (
                json!({"action": "end", "scope": "all"}),
                "an end has no scope",
            ),
            (
                json!({"action": "end", "watchman": null}),
                "an end names its watchman",
            ),
            (json!({"scope": "all"}), "a scope without v3"),
            (
                json!({"action": "add", "credential": "c", "scope": "host:a1"}),
                "unknown kind",
            ),
            (
                json!({"action": "add", "credential": "c", "scope": "team:"}),
                "empty name",
            ),
            (
                json!({"action": "add", "credential": "c", "scope": "team:a b"}),
                "a space",
            ),
            (
                json!({"action": "add", "credential": "c", "scope": "machine:a1\u{200b}"}),
                "invisible",
            ),
        ] {
            assert!(
                matches!(with(fields), Err(LedgerError::InvalidDesignation(_))),
                "{why}"
            );
        }
    }
}
