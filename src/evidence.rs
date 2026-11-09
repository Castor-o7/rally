//! Checking submitted evidence against GitHub (RAL-5), read-only (RAL-18).
//!
//! Evidence written as `owner/repo@<full commit>` is checked for three things: whether the commit
//! exists, whether it names the story (in its message or a pull request that contains it), and
//! what its CI says. The answer is recorded by Rally itself as an observed `evidence_check`. It's
//! evidence, not a verdict: `ci: passed` is GitHub's word on that one commit, and RAL-31 still
//! compares the commit that's merged with the one that was read.
//!
//! The service only ever sends GETs to [`API_HOST`], never follows a redirect, and never checks
//! out or runs submitted code.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The only host a token is ever sent to.
pub const API_HOST: &str = "api.github.com";

/// What a commit's CI says, from its check runs and combined status together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ci {
    Passed,
    Failed,
    Pending,
    /// No check runs and no statuses.
    None,
}

/// The outcome of one check of a submission's evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    /// Whether the evidence named a repository and full commit Rally can check.
    pub checkable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub names_story: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci: Option<Ci>,
    /// Why something couldn't be checked, or why the check stopped, such as no token, or CI
    /// still running when the wait ran out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// A read of GitHub failed, which may pass: a network error, a 5xx or a rate limit. Not
    /// recorded; it only means the check is tried again until the ceiling.
    #[serde(skip)]
    pub unread: bool,
}

impl Outcome {
    fn not_checked(reason: impl Into<String>) -> Self {
        Self {
            checkable: false,
            repo: None,
            commit: None,
            exists: None,
            names_story: None,
            ci: None,
            reason: Some(reason.into()),
            unread: false,
        }
    }

    /// Whether checking again could change it: CI that's still running, or a read that failed.
    pub fn is_final(&self) -> bool {
        self.ci != Some(Ci::Pending) && !self.unread
    }
}

/// `owner/repo@<40 or 64 hex digits>`, as `rally submit --evidence` asks for.
pub fn parse(evidence: &str) -> Result<(String, String), String> {
    let (repo, commit) = evidence
        .trim()
        .rsplit_once('@')
        .ok_or("the evidence isn't owner/repo@<full commit>")?;
    let name_ok = |part: &str| {
        !part.is_empty()
            && part.len() <= 100
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            && part != "."
            && part != ".."
    };
    let mut parts = repo.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err("the evidence's repository isn't owner/repo".into());
    };
    if !name_ok(owner) || !name_ok(name) {
        return Err("the evidence's repository isn't owner/repo".into());
    }
    if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("the evidence's commit isn't a full hash".into());
    }
    Ok((repo.to_owned(), commit.to_ascii_lowercase()))
}

/// Whether `text` names `story` as a whole word: `RAL-5`, but not `RAL-50` or `XRAL-5`.
pub fn names(text: &str, story: &str) -> bool {
    text.match_indices(story).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + story.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
            && !after.is_some_and(|c| c.is_ascii_alphanumeric())
    })
}

/// Read-only access to the forge, so the check can be tested without GitHub.
pub trait Forge {
    /// The commit's message, or `None` if the repository has no such commit.
    fn commit_message(&self, repo: &str, commit: &str) -> Result<Option<String>, String>;
    /// Whether the token can see the repository at all (RAL-128). GitHub answers 404 for a
    /// commit in a private repository the token can't read, just as for a missing one, so a
    /// missing commit is only "doesn't exist" once the repository itself is seen.
    fn repo_visible(&self, _repo: &str) -> Result<bool, String> {
        Ok(true)
    }
    /// The titles of pull requests that contain the commit.
    fn pull_titles(&self, repo: &str, commit: &str) -> Result<Vec<String>, String>;
    /// What the commit's check runs and statuses say.
    fn ci(&self, repo: &str, commit: &str) -> Result<Ci, String>;
}

/// Checks `evidence` for `story` once. CI may still be pending; the caller decides whether to
/// check again. `forge` is `None` without a token, and then nothing is asked of GitHub.
pub fn check(forge: Option<&dyn Forge>, story: &str, evidence: &str) -> Outcome {
    let (repo, commit) = match parse(evidence) {
        Ok(parsed) => parsed,
        Err(reason) => return Outcome::not_checked(reason),
    };
    let mut outcome = Outcome {
        checkable: true,
        repo: Some(repo.clone()),
        commit: Some(commit.clone()),
        exists: None,
        names_story: None,
        ci: None,
        reason: None,
        unread: false,
    };
    let Some(forge) = forge else {
        outcome.reason = Some("not checked: the service has no GitHub token".into());
        return outcome;
    };
    let message = match forge.commit_message(&repo, &commit) {
        Ok(message) => message,
        Err(error) => {
            outcome.reason = Some(format!("GitHub couldn't be read: {error}"));
            outcome.unread = true;
            return outcome;
        }
    };
    let Some(message) = message else {
        match forge.repo_visible(&repo) {
            Ok(true) => outcome.exists = Some(false),
            // Unknown, not false: the commit may well be there (RAL-128).
            Ok(false) => {
                outcome.reason = Some(format!(
                    "not checked: GitHub shows the service's token no repository {repo}. It's \
                     private and outside the token's access, or it doesn't exist, so whether the \
                     commit exists is unknown"
                ));
            }
            Err(error) => {
                outcome.reason = Some(format!("GitHub couldn't be read: {error}"));
                outcome.unread = true;
            }
        }
        return outcome;
    };
    outcome.exists = Some(true);
    // Not named in the message and its pull requests unread is unknown, not "not named".
    outcome.names_story = if names(&message, story) {
        Some(true)
    } else {
        match forge.pull_titles(&repo, &commit) {
            Ok(titles) => Some(titles.iter().any(|title| names(title, story))),
            Err(error) => {
                outcome.reason = Some(format!("its pull requests couldn't be read: {error}"));
                outcome.unread = true;
                None
            }
        }
    };
    match forge.ci(&repo, &commit) {
        Ok(ci) => outcome.ci = Some(ci),
        Err(error) => {
            let why = format!("its CI couldn't be read: {error}");
            outcome.reason = Some(match outcome.reason.take() {
                Some(earlier) => format!("{earlier}; {why}"),
                None => why,
            });
            outcome.unread = true;
        }
    }
    outcome
}

/// One state from GitHub's check runs and combined status: failed if any failed, pending if any
/// is still running, passed if all passed, none if there's neither.
pub fn combine(check_runs: &Value, status: &Value) -> Ci {
    let mut seen = false;
    let mut pending = false;
    for run in check_runs["check_runs"].as_array().into_iter().flatten() {
        seen = true;
        match (run["status"].as_str(), run["conclusion"].as_str()) {
            (Some("completed"), Some("success" | "neutral" | "skipped")) => {}
            (Some("completed"), _) => return Ci::Failed,
            _ => pending = true,
        }
    }
    // A combined status with no statuses is "pending" with nothing behind it, so count them.
    if status["statuses"].as_array().is_some_and(|s| !s.is_empty()) {
        seen = true;
        match status["state"].as_str() {
            Some("success") => {}
            Some("pending") => pending = true,
            _ => return Ci::Failed,
        }
    }
    match (seen, pending) {
        (false, _) => Ci::None,
        (true, true) => Ci::Pending,
        (true, false) => Ci::Passed,
    }
}

/// GitHub's REST API with a read-only token, at [`API_HOST`] only.
pub struct GitHub {
    agent: ureq::Agent,
    token: String,
    base: String,
}

impl GitHub {
    pub fn new(token: String) -> Self {
        Self::at(token, format!("https://{API_HOST}"))
    }

    /// The same client against another base, for tests only.
    fn at(token: String, base: String) -> Self {
        let agent = ureq::Agent::config_builder()
            // A redirect could send the token elsewhere, so none is followed.
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(std::time::Duration::from_secs(20)))
            .build()
            .new_agent();
        Self { agent, token, base }
    }

    /// GETs `path` under the base, returning the status and the body read as JSON.
    fn get(&self, path: &str) -> Result<(u16, Value), String> {
        let mut response = self
            .agent
            .get(&format!("{}{path}", self.base))
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "rally-evidence-check")
            .call()
            .map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            return Err(format!(
                "GitHub answered with a redirect ({status}), not followed"
            ));
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(4 * 1024 * 1024)
            .read_to_string()
            .map_err(|e| e.to_string())?;
        let value = serde_json::from_str(&body).unwrap_or(Value::Null);
        Ok((status, value))
    }

    fn ok(&self, path: &str) -> Result<Value, String> {
        match self.get(path)? {
            (200, value) => Ok(value),
            (status, value) => Err(format!(
                "GitHub answered {status}: {}",
                value["message"].as_str().unwrap_or("no reason given")
            )),
        }
    }
}

/// What a merge attempt with the service's token showed (RAL-18).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum MergeProbe {
    /// GitHub refused the merge for want of access: the token is read-only for that repository.
    Refused { status: u16, message: String },
    /// GitHub got past the access check, so the token can write. Nothing merged: the head given
    /// can never match.
    CanWrite { status: u16, message: String },
    /// Neither: the answer doesn't show which.
    Unclear { status: u16, message: String },
}

/// The head a merge attempt names: no commit has it, so GitHub can never merge on it.
pub const NO_HEAD: &str = "0000000000000000000000000000000000000000";

impl GitHub {
    /// Try to merge pull request `pull` of `repo` with this token. The pull request must be read
    /// first, so a refusal isn't mistaken for a missing one, and must be closed or merged, so the
    /// attempt can't merge even if the token could write; the head named can't match either. Only the operator
    /// runs this (`rally check-github-token`); the service never does.
    pub fn probe_merge(&self, repo: &str, pull: u64) -> Result<MergeProbe, String> {
        let read = self.ok(&format!("/repos/{repo}/pulls/{pull}"))?;
        // Only a closed or merged pull request: GitHub can't merge one of those whatever it does
        // with the head named, so the attempt can never merge even if the token can write.
        if read["state"].as_str() != Some("closed") {
            return Err(format!(
                "pull request {pull} isn't closed, so no merge is tried: point it at a merged one"
            ));
        }
        let mut response = self
            .agent
            .put(&format!("{}/repos/{repo}/pulls/{pull}/merge", self.base))
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "rally-evidence-check")
            .header("Content-Type", "application/json")
            .send(serde_json::json!({ "sha": NO_HEAD }).to_string())
            .map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(64 * 1024)
            .read_to_string()
            .unwrap_or_default();
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v["message"].as_str().map(str::to_owned))
            .unwrap_or_default();
        Ok(match status {
            // 404 too: GitHub answers a write it won't allow with 404 on some tokens, and the
            // pull request was just read.
            403 | 404 => MergeProbe::Refused { status, message },
            405 | 409 | 422 => MergeProbe::CanWrite { status, message },
            _ => MergeProbe::Unclear { status, message },
        })
    }
}

impl Forge for GitHub {
    fn commit_message(&self, repo: &str, commit: &str) -> Result<Option<String>, String> {
        match self.get(&format!("/repos/{repo}/commits/{commit}"))? {
            (200, value) => Ok(Some(
                value["commit"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )),
            (404 | 422, _) => Ok(None),
            (status, value) => Err(format!(
                "GitHub answered {status}: {}",
                value["message"].as_str().unwrap_or("no reason given")
            )),
        }
    }

    fn repo_visible(&self, repo: &str) -> Result<bool, String> {
        match self.get(&format!("/repos/{repo}"))? {
            (200, _) => Ok(true),
            (404, _) => Ok(false),
            (status, value) => Err(format!(
                "GitHub answered {status}: {}",
                value["message"].as_str().unwrap_or("no reason given")
            )),
        }
    }

    fn pull_titles(&self, repo: &str, commit: &str) -> Result<Vec<String>, String> {
        let value = self.ok(&format!("/repos/{repo}/commits/{commit}/pulls"))?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|pull| pull["title"].as_str().map(str::to_owned))
            .collect())
    }

    fn ci(&self, repo: &str, commit: &str) -> Result<Ci, String> {
        let runs = self.ok(&format!(
            "/repos/{repo}/commits/{commit}/check-runs?per_page=100"
        ))?;
        let status = self.ok(&format!("/repos/{repo}/commits/{commit}/status"))?;
        Ok(combine(&runs, &status))
    }
}

/// One open pull request: its number, the repository its head is in, and its head commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenPull {
    pub number: u64,
    pub repo: String,
    pub sha: String,
}

/// Reading roadmaps in open pull requests (RAL-68), read-only.
pub trait Roadmaps {
    /// The repository's open pull requests, at most [`MAX_OPEN_PULLS`] and one more, so that
    /// more than that can be told.
    fn open_pulls(&self, repo: &str) -> Result<Vec<OpenPull>, String>;
    /// The file at `path` in `repo` at `sha`, or `None` when it isn't there.
    fn file_at(&self, repo: &str, path: &str, sha: &str) -> Result<Option<String>, String>;
}

/// The most open pull requests whose roadmaps are read.
pub const MAX_OPEN_PULLS: usize = 100;

/// Whether `repo` is `owner/repo` in the characters GitHub allows.
pub fn valid_repo(repo: &str) -> bool {
    let name_ok = |part: &str| {
        !part.is_empty()
            && part.len() <= 100
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            && part != "."
            && part != ".."
    };
    let mut parts = repo.split('/');
    matches!((parts.next(), parts.next(), parts.next()), (Some(o), Some(n), None) if name_ok(o) && name_ok(n))
}

/// Whether `path` is a plain relative path inside a repository: no `..`, no leading `/`.
pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 200
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
}

/// The highest `prefix` number written in `path` in any open pull request of `repo` (RAL-68), so
/// `rally new-story` hands out none of them. When anything can't be read, it says so and gives
/// no number, rather than a guess from what it could read.
///
/// Only repositories of `owners` are read, compared without case, so a client can't spend the
/// token on another owner's repositories, such as private ones it can see; anything else is
/// refused before GitHub is asked.
pub fn open_pull_ids(
    roadmaps: &dyn Roadmaps,
    owners: &[String],
    repo: &str,
    path: &str,
    prefix: &str,
) -> Value {
    let refuse = |reason: String| serde_json::json!({"checked": false, "reason": reason});
    if !valid_repo(repo) {
        return refuse("the repository isn't owner/repo".into());
    }
    let owner = repo.split('/').next().unwrap_or("");
    if !owners.iter().any(|o| o.eq_ignore_ascii_case(owner)) {
        return refuse(format!(
            "the service reads open pull requests only for {}",
            owners.join(", ")
        ));
    }
    if !valid_path(path) {
        return refuse("the roadmap isn't a plain path inside the repository".into());
    }
    if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_uppercase()) {
        return refuse("the prefix isn't capital letters".into());
    }
    let pulls = match roadmaps.open_pulls(repo) {
        Ok(pulls) => pulls,
        Err(error) => return refuse(format!("the open pull requests couldn't be read: {error}")),
    };
    if pulls.len() > MAX_OPEN_PULLS {
        return refuse(format!("more than {MAX_OPEN_PULLS} open pull requests"));
    }
    let mut highest = 0u64;
    let mut uses = Vec::new();
    for pull in &pulls {
        let text = match roadmaps.file_at(&pull.repo, path, &pull.sha) {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(error) => {
                return refuse(format!(
                    "pull request #{}'s roadmap couldn't be read: {error}",
                    pull.number
                ));
            }
        };
        let top = crate::next::items(&text, prefix)
            .iter()
            .filter_map(|item| item.id.split_once('-')?.1.parse::<u64>().ok())
            .max();
        if let Some(top) = top {
            highest = highest.max(top);
            uses.push(
                serde_json::json!({"pull": pull.number, "highest": format!("{prefix}-{top}")}),
            );
        }
    }
    serde_json::json!({"checked": true, "kind": "observed", "repo": repo, "path": path,
                       "pulls": pulls.len(), "highest": highest, "uses": uses})
}

/// Standard base64, as GitHub's contents API gives a file, ignoring line breaks.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

impl Roadmaps for GitHub {
    fn open_pulls(&self, repo: &str) -> Result<Vec<OpenPull>, String> {
        let value = self.ok(&format!(
            "/repos/{repo}/pulls?state=open&per_page={}",
            MAX_OPEN_PULLS + 1
        ))?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|pull| {
                let head_repo = pull["head"]["repo"]["full_name"].as_str()?;
                let sha = pull["head"]["sha"].as_str()?;
                (valid_repo(head_repo) && sha.bytes().all(|b| b.is_ascii_hexdigit())).then(|| {
                    OpenPull {
                        number: pull["number"].as_u64().unwrap_or(0),
                        repo: head_repo.to_owned(),
                        sha: sha.to_owned(),
                    }
                })
            })
            .collect())
    }

    fn file_at(&self, repo: &str, path: &str, sha: &str) -> Result<Option<String>, String> {
        match self.get(&format!("/repos/{repo}/contents/{path}?ref={sha}"))? {
            (200, value) => {
                if value["encoding"] != "base64" {
                    return Err("GitHub didn't give the file's contents".into());
                }
                let bytes = base64_decode(value["content"].as_str().unwrap_or(""))
                    .ok_or("GitHub's copy of the file couldn't be decoded")?;
                String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|_| "the file isn't text".into())
            }
            (404, _) => Ok(None),
            (status, value) => Err(format!(
                "GitHub answered {status}: {}",
                value["message"].as_str().unwrap_or("no reason given")
            )),
        }
    }
}

/// One submission to check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub story: String,
    /// The submit record's number.
    pub submit: u64,
    pub evidence: String,
    /// When it was submitted, by the service's clock: the wait for CI counts from here.
    pub submitted_at_ms: i64,
}

/// Every submission still in review whose latest submit has no evidence check yet: a story's,
/// and each part's in review (RAL-79).
pub fn unchecked(ledger: &crate::ledger::Ledger) -> Vec<Job> {
    use crate::ledger::{Change, State};
    let mut jobs = Vec::new();
    for (id, story) in ledger.stories() {
        let mut seqs: Vec<u64> = story
            .parts
            .values()
            .filter(|part| part.state == State::InReview)
            .map(|part| part.submitted_at)
            .collect();
        if story.state == State::InReview
            && let Some(submit) = story
                .history
                .iter()
                .rev()
                .find(|entry| matches!(entry.change, Change::Submit { part: None, .. }))
        {
            seqs.push(submit.seq);
        }
        for seq in seqs {
            let Some(submit) = story.history.iter().find(|entry| entry.seq == seq) else {
                continue;
            };
            let Change::Submit { evidence, .. } = &submit.change else {
                continue;
            };
            let checked = story.history.iter().any(|entry| {
                matches!(&entry.change, Change::EvidenceCheck { submit: s, .. } if *s == seq)
            });
            if !checked {
                jobs.push(Job {
                    story: id.clone(),
                    submit: seq,
                    evidence: evidence.clone(),
                    submitted_at_ms: submit.at_ms,
                });
            }
        }
    }
    jobs.sort_by_key(|job| job.submit);
    jobs
}

/// After one check: record it, or check again later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Record(Outcome),
    Again,
}

/// Records a final outcome at once. CI still running is checked again until `ceiling_ms` after
/// the submission, then recorded as still pending, saying the wait ran out.
pub fn step(mut outcome: Outcome, job: &Job, now_ms: i64, ceiling_ms: u64) -> Step {
    if outcome.is_final() {
        return Step::Record(outcome);
    }
    if now_ms.saturating_sub(job.submitted_at_ms) >= i64::try_from(ceiling_ms).unwrap_or(i64::MAX) {
        let minutes = ceiling_ms / 60_000;
        outcome.reason = Some(match outcome.reason.take() {
            // A read that kept failing says what failed.
            Some(why) if outcome.unread => {
                format!("{why}, still when the wait of {minutes} minutes ran out")
            }
            _ => format!("CI was still running when the wait of {minutes} minutes ran out"),
        });
        return Step::Record(outcome);
    }
    Step::Again
}

/// The most submissions waiting to be checked. Past it, new ones aren't queued, and a restart
/// picks them up from the ledger.
pub const MAX_QUEUED: usize = 1_000;

/// What `serve` is given for evidence checks: a read-only token, if the operator has placed one,
/// and the times.
#[derive(Clone, Debug)]
pub struct Settings {
    pub token: Option<String>,
    pub timing: Timing,
    /// The GitHub owners whose repositories the token may be used to read open pull requests'
    /// roadmaps in (RAL-68). Set on the service, never by a request.
    pub owners: Vec<String>,
}

/// The owners `/v1/open-ids` may read, unless the service is told otherwise.
pub const DEFAULT_OWNERS: &str = "Castor-o7";

/// How many submissions are checked at once.
pub const WORKERS: usize = 2;

/// Times for the background checks.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// How long a submission's CI is waited for, at most.
    pub ceiling_ms: u64,
    /// How long between two checks of one submission whose CI is still running.
    pub poll_ms: u64,
}

type Recorder = dyn Fn(&Job, &Outcome) + Send + Sync;

struct Queue {
    /// Jobs and when each is next due, by the service's clock.
    jobs: std::sync::Mutex<Vec<(i64, Job)>>,
    ready: std::sync::Condvar,
}

/// A fixed pool of workers checking queued submissions, so slowness at GitHub never grows the
/// number of threads: each job waits its turn, and a pending one goes back in the queue.
#[derive(Clone)]
pub struct Checker {
    queue: std::sync::Arc<Queue>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

impl Checker {
    /// Starts `workers` threads. `record` appends the outcome as Rally's observed record.
    pub fn start(
        forge: Option<std::sync::Arc<dyn Forge + Send + Sync>>,
        workers: usize,
        timing: Timing,
        record: std::sync::Arc<Recorder>,
    ) -> Self {
        let queue = std::sync::Arc::new(Queue {
            jobs: std::sync::Mutex::new(Vec::new()),
            ready: std::sync::Condvar::new(),
        });
        for _ in 0..workers.max(1) {
            let queue = queue.clone();
            let forge = forge.clone();
            let record = record.clone();
            std::thread::spawn(move || {
                loop {
                    let job = {
                        let mut jobs = queue.jobs.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            let now = now_ms();
                            let due = jobs
                                .iter()
                                .enumerate()
                                .min_by_key(|(_, (at, _))| *at)
                                .map(|(index, (at, _))| (index, *at));
                            match due {
                                Some((index, at)) if at <= now => break jobs.swap_remove(index).1,
                                Some((_, at)) => {
                                    let wait = std::time::Duration::from_millis((at - now) as u64);
                                    jobs = queue
                                        .ready
                                        .wait_timeout(jobs, wait)
                                        .unwrap_or_else(|e| e.into_inner())
                                        .0;
                                }
                                None => {
                                    jobs =
                                        queue.ready.wait(jobs).unwrap_or_else(|e| e.into_inner());
                                }
                            }
                        }
                    };
                    let forge = forge.as_deref().map(|forge| forge as &dyn Forge);
                    let outcome = check(forge, &job.story, &job.evidence);
                    match step(outcome, &job, now_ms(), timing.ceiling_ms) {
                        Step::Record(outcome) => record(&job, &outcome),
                        Step::Again => {
                            let mut jobs = queue.jobs.lock().unwrap_or_else(|e| e.into_inner());
                            jobs.push((now_ms() + timing.poll_ms as i64, job));
                            queue.ready.notify_one();
                        }
                    }
                }
            });
        }
        Self { queue }
    }

    /// Queues a submission to check now. Returns false if the queue is full.
    pub fn submit(&self, job: Job) -> bool {
        let mut jobs = self.queue.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if jobs.len() >= MAX_QUEUED {
            return false;
        }
        jobs.push((now_ms(), job));
        self.queue.ready.notify_one();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    #[derive(Default)]
    struct Fake {
        message: Option<String>,
        titles: Vec<String>,
        ci: Option<Ci>,
        calls: RefCell<usize>,
    }

    impl Forge for Fake {
        fn commit_message(&self, _: &str, _: &str) -> Result<Option<String>, String> {
            *self.calls.borrow_mut() += 1;
            Ok(self.message.clone())
        }
        fn pull_titles(&self, _: &str, _: &str) -> Result<Vec<String>, String> {
            Ok(self.titles.clone())
        }
        fn ci(&self, _: &str, _: &str) -> Result<Ci, String> {
            self.ci.ok_or_else(|| "down".to_owned())
        }
    }

    fn evidence() -> String {
        format!("Castor-o7/rally@{COMMIT}")
    }

    #[test]
    fn evidence_must_name_a_repository_and_a_full_commit() {
        assert_eq!(
            parse(&evidence()).unwrap(),
            ("Castor-o7/rally".into(), COMMIT.into())
        );
        for bad in [
            "rally@abc",
            "Castor-o7/rally@0123456",
            "Castor-o7/rally",
            "a/b/c@0123456789abcdef0123456789abcdef01234567",
            "../x@0123456789abcdef0123456789abcdef01234567",
            "muster#34",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        let outcome = check(None, "RAL-5", "muster#34");
        assert!(!outcome.checkable && outcome.reason.is_some());
    }

    #[test]
    fn a_story_is_named_only_as_a_whole_word() {
        assert!(names("feat: check evidence (RAL-5)", "RAL-5"));
        assert!(names("RAL-5: x", "RAL-5"));
        assert!(!names("feat: RAL-50", "RAL-5"));
        assert!(!names("XRAL-5", "RAL-5"));
        assert!(!names("PRE-RAL-5", "RAL-5"));
    }

    #[test]
    fn without_a_token_nothing_is_asked_of_github() {
        let outcome = check(None, "RAL-5", &evidence());
        assert!(outcome.checkable);
        assert_eq!(outcome.exists, None);
        assert!(outcome.reason.unwrap().contains("no GitHub token"));
    }

    /// RAL-128: a private repository the token can't see answers 404 for every commit, so its
    /// commits are unknown, not missing, and the check isn't tried again.
    #[test]
    fn a_commit_in_a_repository_the_token_cant_see_is_unknown_not_missing() {
        struct Hidden;
        impl Forge for Hidden {
            fn commit_message(&self, _: &str, _: &str) -> Result<Option<String>, String> {
                Ok(None)
            }
            fn repo_visible(&self, _: &str) -> Result<bool, String> {
                Ok(false)
            }
            fn pull_titles(&self, _: &str, _: &str) -> Result<Vec<String>, String> {
                unreachable!()
            }
            fn ci(&self, _: &str, _: &str) -> Result<Ci, String> {
                unreachable!()
            }
        }
        let outcome = check(Some(&Hidden), "RAL-5", &evidence());
        assert!(outcome.checkable);
        assert_eq!(
            (outcome.exists, outcome.names_story, outcome.ci),
            (None, None, None)
        );
        assert!(
            outcome.reason.as_deref().unwrap().contains("unknown"),
            "{outcome:?}"
        );
        assert!(outcome.is_final());
    }

    /// The client asks for the repository after a 404 on the commit, and says what it saw.
    #[test]
    fn the_client_reads_the_repository_after_a_missing_commit() {
        let (base, server) = recording_github(2, |_, _| (404, r#"{"message":"Not Found"}"#));
        let github = GitHub::at("read-only-token".into(), base);
        let outcome = check(Some(&github), "RAL-5", &evidence());
        let seen = server.join().unwrap();
        assert_eq!(outcome.exists, None, "{outcome:?}");
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1].1, "/repos/Castor-o7/rally");
        assert!(seen.iter().all(|(method, _, _)| method == "GET"));

        let (base, server) = recording_github(2, |_, path| {
            if path == "/repos/Castor-o7/rally" {
                (200, r#"{"full_name":"Castor-o7/rally"}"#)
            } else {
                (422, r#"{"message":"No commit found"}"#)
            }
        });
        let github = GitHub::at("read-only-token".into(), base);
        let outcome = check(Some(&github), "RAL-5", &evidence());
        server.join().unwrap();
        assert_eq!(outcome.exists, Some(false), "{outcome:?}");
        assert!(outcome.is_final());
    }

    #[test]
    fn each_failure_is_recorded_as_found() {
        // The commit isn't there.
        let missing = Fake::default();
        let outcome = check(Some(&missing), "RAL-5", &evidence());
        assert_eq!((outcome.exists, outcome.names_story), (Some(false), None));

        // There, but naming another story, with no pull request naming this one.
        let other = Fake {
            message: Some("feat: RAL-50".into()),
            titles: vec!["something else".into()],
            ci: Some(Ci::Passed),
            ..Fake::default()
        };
        let outcome = check(Some(&other), "RAL-5", &evidence());
        assert_eq!(outcome.names_story, Some(false));
        assert_eq!(outcome.ci, Some(Ci::Passed));

        // Named by its pull request's title, and CI failed.
        let failed = Fake {
            message: Some("wip".into()),
            titles: vec!["feat: check evidence (RAL-5)".into()],
            ci: Some(Ci::Failed),
            ..Fake::default()
        };
        let outcome = check(Some(&failed), "RAL-5", &evidence());
        assert_eq!(
            (outcome.exists, outcome.names_story, outcome.ci),
            (Some(true), Some(true), Some(Ci::Failed))
        );
        assert!(outcome.is_final());

        // CI still running: not final, so it's checked again.
        let pending = Fake {
            message: Some("RAL-5".into()),
            ci: Some(Ci::Pending),
            ..Fake::default()
        };
        assert!(!check(Some(&pending), "RAL-5", &evidence()).is_final());

        // CI couldn't be read: said so, and not final, so it's tried again.
        let down = Fake {
            message: Some("RAL-5".into()),
            ..Fake::default()
        };
        let outcome = check(Some(&down), "RAL-5", &evidence());
        assert!(!outcome.is_final());
        assert!(outcome.reason.unwrap().contains("CI couldn't be read"));
    }

    /// A forge whose every read fails, as GitHub does in an outage or past its rate limit.
    struct Down;
    impl Forge for Down {
        fn commit_message(&self, _: &str, _: &str) -> Result<Option<String>, String> {
            Err("GitHub answered 503".into())
        }
        fn pull_titles(&self, _: &str, _: &str) -> Result<Vec<String>, String> {
            Err("GitHub answered 403: API rate limit exceeded".into())
        }
        fn ci(&self, _: &str, _: &str) -> Result<Ci, String> {
            Err("GitHub answered 503".into())
        }
    }

    #[test]
    fn a_failed_read_is_tried_again_and_recorded_with_its_reason_at_the_ceiling() {
        let job = Job {
            story: "RAL-5".into(),
            submit: 4,
            evidence: evidence(),
            submitted_at_ms: 0,
        };
        let outcome = check(Some(&Down), "RAL-5", &evidence());
        assert!(!outcome.is_final());
        assert_eq!(step(outcome.clone(), &job, 1, 60_000), Step::Again);
        let Step::Record(recorded) = step(outcome, &job, 60_000, 60_000) else {
            panic!("not recorded at the ceiling");
        };
        let reason = recorded.reason.unwrap();
        assert!(
            reason.contains("GitHub couldn't be read") && reason.contains("ran out"),
            "{reason}"
        );
    }

    #[test]
    fn unread_pull_requests_leave_whether_the_story_is_named_unknown() {
        struct NoPulls;
        impl Forge for NoPulls {
            fn commit_message(&self, _: &str, _: &str) -> Result<Option<String>, String> {
                Ok(Some("wip".into()))
            }
            fn pull_titles(&self, _: &str, _: &str) -> Result<Vec<String>, String> {
                Err("GitHub answered 502".into())
            }
            fn ci(&self, _: &str, _: &str) -> Result<Ci, String> {
                Ok(Ci::Passed)
            }
        }
        let outcome = check(Some(&NoPulls), "RAL-5", &evidence());
        assert_eq!(outcome.names_story, None);
        assert!(
            outcome
                .reason
                .as_deref()
                .unwrap()
                .contains("pull requests couldn't be read")
        );
        assert!(!outcome.is_final());
        // Named in the message, the pull requests aren't needed.
        let named = Fake {
            message: Some("feat: RAL-5".into()),
            ci: Some(Ci::Passed),
            ..Fake::default()
        };
        assert_eq!(
            check(Some(&named), "RAL-5", &evidence()).names_story,
            Some(true)
        );
    }

    #[test]
    fn the_checker_tries_again_after_a_failed_read() {
        use std::sync::{Arc, Mutex};
        struct Flaky(Mutex<u32>);
        impl Forge for Flaky {
            fn commit_message(&self, _: &str, _: &str) -> Result<Option<String>, String> {
                let mut calls = self.0.lock().unwrap();
                *calls += 1;
                if *calls == 1 {
                    Err("GitHub answered 503".into())
                } else {
                    Ok(Some("RAL-5".into()))
                }
            }
            fn pull_titles(&self, _: &str, _: &str) -> Result<Vec<String>, String> {
                Ok(Vec::new())
            }
            fn ci(&self, _: &str, _: &str) -> Result<Ci, String> {
                Ok(Ci::Passed)
            }
        }
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let sink = recorded.clone();
        let checker = Checker::start(
            Some(Arc::new(Flaky(Mutex::new(0)))),
            1,
            Timing {
                ceiling_ms: 60_000,
                poll_ms: 20,
            },
            Arc::new(move |_: &Job, outcome: &Outcome| {
                sink.lock().unwrap().push(outcome.clone());
            }),
        );
        assert!(checker.submit(Job {
            story: "RAL-5".into(),
            submit: 4,
            evidence: evidence(),
            submitted_at_ms: now_ms(),
        }));
        for _ in 0..100 {
            if !recorded.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let recorded = recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            (recorded[0].exists, recorded[0].ci),
            (Some(true), Some(Ci::Passed))
        );
        assert_eq!(recorded[0].reason, None);
    }

    #[test]
    fn check_runs_and_statuses_combine_into_one_state() {
        let run =
            |status: &str, conclusion: Value| json!({"status": status, "conclusion": conclusion});
        let none = json!({"check_runs": []});
        let no_status = json!({"state": "pending", "statuses": []});
        assert_eq!(combine(&none, &no_status), Ci::None);
        let passed = json!({"check_runs": [run("completed", json!("success")), run("completed", json!("skipped"))]});
        assert_eq!(combine(&passed, &no_status), Ci::Passed);
        let running = json!({"check_runs": [run("completed", json!("success")), run("in_progress", Value::Null)]});
        assert_eq!(combine(&running, &no_status), Ci::Pending);
        let failed = json!({"check_runs": [run("in_progress", Value::Null), run("completed", json!("failure"))]});
        assert_eq!(combine(&failed, &no_status), Ci::Failed);
        let status_failed = json!({"state": "failure", "statuses": [{"state": "failure"}]});
        assert_eq!(combine(&passed, &status_failed), Ci::Failed);
    }

    /// One request a stand-in GitHub saw: its method, path and body.
    type Seen = (String, String, String);

    /// A stand-in GitHub on a local port that answers each request with `reply(method, path)`
    /// and records every request line and body, until `count` requests have come.
    fn recording_github(
        count: usize,
        reply: fn(&str, &str) -> (u16, &'static str),
    ) -> (String, std::thread::JoinHandle<Vec<Seen>>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for stream in listener.incoming().take(count) {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_owned();
                let path = parts.next().unwrap_or_default().to_owned();
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).unwrap();
                let (status, json) = reply(&method, &path);
                let answer = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                );
                stream.write_all(answer.as_bytes()).unwrap();
                seen.push((method, path, String::from_utf8_lossy(&body).into_owned()));
            }
            seen
        });
        (base, handle)
    }

    /// RAL-18: a whole check only reads. Every request is a GET of one of the four read endpoints
    /// for the submitted commit; nothing clones, fetches a tarball or writes.
    #[test]
    fn a_check_only_reads_the_commit_its_pulls_and_its_ci() {
        let (base, server) = recording_github(4, |_, path| {
            if path.ends_with("/pulls") {
                (200, r#"[{"title":"feat: RAL-18"}]"#)
            } else if path.contains("/check-runs") {
                (
                    200,
                    r#"{"check_runs":[{"status":"completed","conclusion":"success"}]}"#,
                )
            } else if path.ends_with("/status") {
                (200, r#"{"state":"success","statuses":[]}"#)
            } else {
                // Not naming the story, so its pull requests are read too.
                (200, r#"{"commit":{"message":"feat: the work"}}"#)
            }
        });
        let github = GitHub::at("read-only-token".into(), base);
        let outcome = check(Some(&github), "RAL-18", &evidence());
        let seen = server.join().unwrap();
        assert_eq!(outcome.exists, Some(true));
        assert_eq!(outcome.ci, Some(Ci::Passed));
        let prefix = format!("/repos/Castor-o7/rally/commits/{COMMIT}");
        let allowed = [
            prefix.clone(),
            format!("{prefix}/pulls"),
            format!("{prefix}/check-runs?per_page=100"),
            format!("{prefix}/status"),
        ];
        assert_eq!(seen.len(), 4, "{seen:?}");
        for (method, path, body) in &seen {
            assert_eq!(method, "GET", "{path}");
            assert!(allowed.contains(path), "an unexpected request: {path}");
            assert!(body.is_empty(), "a GET with a body: {path}");
        }
    }

    /// RAL-18: nothing in the evidence check can start a program, so it can't check out, build
    /// or run what was submitted. Its only other way out is the HTTP client above.
    #[test]
    fn the_evidence_check_starts_no_program() {
        let source = include_str!("evidence.rs");
        for needle in [
            ["std::", "process"].concat(),
            ["Command", "::new"].concat(),
            ["libc::", "exec"].concat(),
            ["rustix::", "process"].concat(),
        ] {
            assert!(!source.contains(&needle), "evidence.rs uses {needle}");
        }
    }

    #[test]
    fn a_merge_attempt_refused_for_access_is_reported_as_refused() {
        let (base, server) = recording_github(2, |method, _| match method {
            "GET" => (200, r#"{"number":7,"state":"closed","merged":true}"#),
            _ => (
                403,
                r#"{"message":"Resource not accessible by personal access token"}"#,
            ),
        });
        let github = GitHub::at("read-only-token".into(), base);
        let probe = github.probe_merge("Castor-o7/rally", 7).unwrap();
        let seen = server.join().unwrap();
        assert!(
            matches!(probe, MergeProbe::Refused { status: 403, .. }),
            "{probe:?}"
        );
        assert_eq!(seen[1].0, "PUT");
        assert_eq!(seen[1].1, "/repos/Castor-o7/rally/pulls/7/merge");
        // The head named can never match, so even a token that can write merges nothing.
        let body: Value = serde_json::from_str(&seen[1].2).unwrap();
        assert_eq!(body["sha"], NO_HEAD);
    }

    #[test]
    fn a_merge_attempt_that_gets_past_the_access_check_is_reported_as_able_to_write() {
        let (base, server) = recording_github(2, |method, _| match method {
            "GET" => (200, r#"{"number":7,"state":"closed","merged":true}"#),
            _ => (405, r#"{"message":"Pull Request is not mergeable"}"#),
        });
        let github = GitHub::at("too-much-token".into(), base);
        let probe = github.probe_merge("Castor-o7/rally", 7).unwrap();
        server.join().unwrap();
        assert!(
            matches!(probe, MergeProbe::CanWrite { status: 405, .. }),
            "{probe:?}"
        );
    }

    #[test]
    fn no_merge_is_attempted_on_an_open_pull_request() {
        let (base, server) = recording_github(1, |_, _| (200, r#"{"number":7,"state":"open"}"#));
        let github = GitHub::at("maybe-writable-token".into(), base);
        let err = github.probe_merge("Castor-o7/rally", 7).unwrap_err();
        let seen = server.join().unwrap();
        assert!(err.contains("isn't closed"), "{err}");
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].0, "GET");
    }

    #[test]
    fn no_merge_is_attempted_on_a_pull_request_that_cant_be_read() {
        let (base, server) = recording_github(1, |_, _| (404, r#"{"message":"Not Found"}"#));
        let github = GitHub::at("read-only-token".into(), base);
        let err = github.probe_merge("Castor-o7/rally", 7).unwrap_err();
        let seen = server.join().unwrap();
        assert!(err.contains("404"), "{err}");
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "GET");
    }

    struct FakeRoadmaps {
        pulls: Result<Vec<OpenPull>, String>,
        files: Vec<(u64, Result<Option<String>, String>)>,
    }

    impl Roadmaps for FakeRoadmaps {
        fn open_pulls(&self, _: &str) -> Result<Vec<OpenPull>, String> {
            self.pulls.clone()
        }
        fn file_at(&self, _: &str, _: &str, sha: &str) -> Result<Option<String>, String> {
            let number: u64 = sha.parse().unwrap();
            self.files
                .iter()
                .find(|(n, _)| *n == number)
                .map(|(_, file)| file.clone())
                .unwrap_or(Ok(None))
        }
    }

    fn owners() -> Vec<String> {
        vec![DEFAULT_OWNERS.to_owned()]
    }

    /// A GitHub that mustn't be asked anything.
    struct Untouchable;

    impl Roadmaps for Untouchable {
        fn open_pulls(&self, _: &str) -> Result<Vec<OpenPull>, String> {
            panic!("GitHub was asked about a repository outside the allowlist")
        }
        fn file_at(&self, _: &str, _: &str, _: &str) -> Result<Option<String>, String> {
            panic!("GitHub was asked about a repository outside the allowlist")
        }
    }

    /// RAL-68: a repository outside the service's owners is refused, and the token is never used.
    #[test]
    fn a_repository_outside_the_owners_is_refused_without_asking_github() {
        for repo in ["someone-else/private-repo", "Castor-o7-evil/muster"] {
            let out = open_pull_ids(&Untouchable, &owners(), repo, "docs/roadmap.md", "MUS");
            assert_eq!(out["checked"], false, "{out}");
            assert!(
                out["reason"]
                    .as_str()
                    .unwrap()
                    .contains("only for Castor-o7"),
                "{out}"
            );
        }
        // The owner's case doesn't matter, as on GitHub.
        let fake = FakeRoadmaps {
            pulls: Ok(vec![]),
            files: vec![],
        };
        let out = open_pull_ids(
            &fake,
            &owners(),
            "castor-o7/muster",
            "docs/roadmap.md",
            "MUS",
        );
        assert_eq!(out["checked"], true, "{out}");
    }

    fn pull(number: u64) -> OpenPull {
        OpenPull {
            number,
            repo: "Castor-o7/muster".into(),
            sha: number.to_string(),
        }
    }

    fn roadmap(ids: &[&str]) -> String {
        ids.iter()
            .map(|id| format!("- [ ] **{id}: Something.** Done when: it is.\n"))
            .collect()
    }

    /// RAL-68: an ID written by hand in an open pull request (MUS-95) is seen, so it isn't
    /// handed out again.
    #[test]
    fn an_id_in_an_open_pull_requests_roadmap_is_seen() {
        let fake = FakeRoadmaps {
            pulls: Ok(vec![pull(140), pull(141), pull(142)]),
            files: vec![
                (140, Ok(Some(roadmap(&["MUS-90", "MUS-95"])))),
                (141, Ok(None)),
                (142, Ok(Some(roadmap(&["MUS-91", "RAL-99"])))),
            ],
        };
        let out = open_pull_ids(
            &fake,
            &owners(),
            "Castor-o7/muster",
            "docs/roadmap.md",
            "MUS",
        );
        assert_eq!(out["checked"], true, "{out}");
        assert_eq!(out["kind"], "observed");
        assert_eq!(out["highest"], 95);
        assert_eq!(out["pulls"], 3);
        assert_eq!(out["uses"][0]["pull"], 140);
        assert_eq!(out["uses"][0]["highest"], "MUS-95");
        // Another prefix's IDs don't count.
        assert_eq!(out["uses"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn when_github_cant_be_read_it_says_so_and_gives_no_number() {
        let unreadable_list = FakeRoadmaps {
            pulls: Err("GitHub answered 503".into()),
            files: vec![],
        };
        let out = open_pull_ids(
            &unreadable_list,
            &owners(),
            "Castor-o7/muster",
            "docs/roadmap.md",
            "MUS",
        );
        assert_eq!(out["checked"], false);
        assert!(out["reason"].as_str().unwrap().contains("503"), "{out}");
        assert!(out.get("highest").is_none());
        // One pull request's file failing fails the whole check, rather than a partial guess.
        let one_unreadable = FakeRoadmaps {
            pulls: Ok(vec![pull(140), pull(141)]),
            files: vec![
                (140, Ok(Some(roadmap(&["MUS-95"])))),
                (141, Err("timed out".into())),
            ],
        };
        let out = open_pull_ids(
            &one_unreadable,
            &owners(),
            "Castor-o7/muster",
            "docs/roadmap.md",
            "MUS",
        );
        assert_eq!(out["checked"], false);
        assert!(out["reason"].as_str().unwrap().contains("#141"), "{out}");
        // Too many to read.
        let many = FakeRoadmaps {
            pulls: Ok((0..=MAX_OPEN_PULLS as u64).map(pull).collect()),
            files: vec![],
        };
        let out = open_pull_ids(
            &many,
            &owners(),
            "Castor-o7/muster",
            "docs/roadmap.md",
            "MUS",
        );
        assert_eq!(out["checked"], false);
    }

    #[test]
    fn only_a_plain_repository_path_and_prefix_are_asked_for() {
        let fake = FakeRoadmaps {
            pulls: Ok(vec![]),
            files: vec![],
        };
        for (repo, path, prefix) in [
            ("Castor-o7/muster/x", "docs/roadmap.md", "MUS"),
            ("Castor-o7/muster", "../etc/passwd", "MUS"),
            ("Castor-o7/muster", "/docs/roadmap.md", "MUS"),
            ("Castor-o7/muster", "docs/road map.md", "MUS"),
            ("Castor-o7/muster", "docs/roadmap.md", "mus"),
        ] {
            assert_eq!(
                open_pull_ids(&fake, &owners(), repo, path, prefix)["checked"],
                false,
                "{repo} {path} {prefix}"
            );
        }
        assert_eq!(
            open_pull_ids(
                &fake,
                &owners(),
                "Castor-o7/muster",
                "docs/roadmap.md",
                "MUS"
            )["checked"],
            true
        );
    }

    #[test]
    fn github_file_contents_are_decoded() {
        assert_eq!(base64_decode("aGVs\nbG8=").unwrap(), b"hello");
        assert!(base64_decode("not*base64").is_none());
    }

    /// RAL-68 through the real client: open pull requests and a file at each head, read with
    /// GETs only.
    #[test]
    fn the_client_reads_open_pull_requests_and_their_roadmaps_with_gets() {
        let (base, server) = recording_github(3, |_, path| {
            if path.starts_with("/repos/Castor-o7/muster/pulls") {
                (
                    200,
                    r#"[{"number": 140, "head": {"sha": "aa11", "repo": {"full_name": "Castor-o7/muster"}}},
                          {"number": 141, "head": {"sha": "bb22", "repo": null}},
                          {"number": 142, "head": {"sha": "cc33", "repo": {"full_name": "a-fork/muster"}}}]"#,
                )
            } else if path.contains("ref=aa11") {
                // "- [ ] **MUS-95: x.**\n", base64.
                (
                    200,
                    r#"{"encoding": "base64", "content": "LSBbIF0gKipNVVMtOTU6IHguKioK"}"#,
                )
            } else {
                (404, r#"{"message": "Not Found"}"#)
            }
        });
        let github = GitHub::at("read-only-token".into(), base);
        let out = open_pull_ids(
            &github,
            &owners(),
            "Castor-o7/muster",
            "docs/roadmap.md",
            "MUS",
        );
        let seen = server.join().unwrap();
        assert_eq!(out["checked"], true, "{out}");
        assert_eq!(out["highest"], 95);
        // A deleted fork's pull request has no head repository and is passed over.
        assert_eq!(out["pulls"], 2);
        assert!(
            seen.iter().all(|(method, _, _)| method == "GET"),
            "{seen:?}"
        );
        assert_eq!(
            seen[0].1,
            "/repos/Castor-o7/muster/pulls?state=open&per_page=101"
        );
        assert_eq!(
            seen[1].1,
            "/repos/Castor-o7/muster/contents/docs/roadmap.md?ref=aa11"
        );
        assert_eq!(
            seen[2].1,
            "/repos/a-fork/muster/contents/docs/roadmap.md?ref=cc33"
        );
    }

    /// A redirect is never followed, so the token can't be carried to another host.
    #[test]
    fn a_redirect_does_not_carry_the_token_elsewhere() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let elsewhere = TcpListener::bind("127.0.0.1:0").unwrap();
        let elsewhere_addr = elsewhere.local_addr().unwrap();
        elsewhere.set_nonblocking(true).unwrap();
        let redirector = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", redirector.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = redirector.accept().unwrap();
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            let reply = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{elsewhere_addr}/steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(reply.as_bytes()).unwrap();
        });
        let github = GitHub::at("secret-token".into(), base);
        let result = github.commit_message("Castor-o7/rally", COMMIT);
        server.join().unwrap();
        assert!(result.unwrap_err().contains("redirect"));
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(elsewhere.accept().is_err(), "the redirect was followed");
    }

    #[test]
    fn pending_ci_is_checked_again_until_the_ceiling_then_recorded() {
        let job = Job {
            story: "RAL-5".into(),
            submit: 4,
            evidence: evidence(),
            submitted_at_ms: 1_000,
        };
        let pending = Outcome {
            checkable: true,
            repo: Some("Castor-o7/rally".into()),
            commit: Some(COMMIT.into()),
            exists: Some(true),
            names_story: Some(true),
            ci: Some(Ci::Pending),
            reason: None,
            unread: false,
        };
        let ceiling = 30 * 60_000;
        assert_eq!(
            step(pending.clone(), &job, 1_000 + ceiling as i64 - 1, ceiling),
            Step::Again
        );
        let Step::Record(recorded) = step(pending.clone(), &job, 1_000 + ceiling as i64, ceiling)
        else {
            panic!("not recorded at the ceiling");
        };
        assert_eq!(recorded.ci, Some(Ci::Pending));
        assert!(
            recorded
                .reason
                .unwrap()
                .contains("wait of 30 minutes ran out")
        );
        let passed = Outcome {
            ci: Some(Ci::Passed),
            ..pending
        };
        assert_eq!(
            step(passed.clone(), &job, 1_000, ceiling),
            Step::Record(passed)
        );
    }

    #[test]
    fn the_checker_records_each_submission_once_its_ci_is_final() {
        use std::sync::{Arc, Mutex};
        struct Turns(Mutex<Vec<Ci>>);
        impl Forge for Turns {
            fn commit_message(&self, _: &str, _: &str) -> Result<Option<String>, String> {
                Ok(Some("RAL-5".into()))
            }
            fn pull_titles(&self, _: &str, _: &str) -> Result<Vec<String>, String> {
                Ok(Vec::new())
            }
            fn ci(&self, _: &str, _: &str) -> Result<Ci, String> {
                let mut turns = self.0.lock().unwrap();
                Ok(if turns.len() > 1 {
                    turns.remove(0)
                } else {
                    turns[0]
                })
            }
        }
        let forge: Arc<dyn Forge + Send + Sync> = Arc::new(Turns(Mutex::new(vec![
            Ci::Pending,
            Ci::Pending,
            Ci::Passed,
        ])));
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let sink = recorded.clone();
        let checker = Checker::start(
            Some(forge),
            2,
            Timing {
                ceiling_ms: 60_000,
                poll_ms: 20,
            },
            Arc::new(move |job: &Job, outcome: &Outcome| {
                sink.lock().unwrap().push((job.submit, outcome.ci));
            }),
        );
        assert!(checker.submit(Job {
            story: "RAL-5".into(),
            submit: 4,
            evidence: evidence(),
            submitted_at_ms: now_ms(),
        }));
        for _ in 0..100 {
            if !recorded.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(*recorded.lock().unwrap(), [(4, Some(Ci::Passed))]);
    }

    #[test]
    fn the_real_client_only_talks_to_the_api_host() {
        let github = GitHub::new("t".into());
        assert_eq!(github.base, "https://api.github.com");
    }
}
