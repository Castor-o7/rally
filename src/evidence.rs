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
        }
    }

    /// Whether checking again could change it: only CI that's still running.
    pub fn is_final(&self) -> bool {
        self.ci != Some(Ci::Pending)
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
    };
    let Some(forge) = forge else {
        outcome.reason = Some("not checked: the service has no GitHub token".into());
        return outcome;
    };
    let message = match forge.commit_message(&repo, &commit) {
        Ok(message) => message,
        Err(error) => {
            outcome.reason = Some(format!("GitHub couldn't be read: {error}"));
            return outcome;
        }
    };
    let Some(message) = message else {
        outcome.exists = Some(false);
        return outcome;
    };
    outcome.exists = Some(true);
    let named = names(&message, story)
        || match forge.pull_titles(&repo, &commit) {
            Ok(titles) => titles.iter().any(|title| names(title, story)),
            Err(error) => {
                outcome.reason = Some(format!("its pull requests couldn't be read: {error}"));
                false
            }
        };
    outcome.names_story = Some(named);
    match forge.ci(&repo, &commit) {
        Ok(ci) => outcome.ci = Some(ci),
        Err(error) => outcome.reason = Some(format!("its CI couldn't be read: {error}")),
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

/// Every submission still in review whose latest submit has no evidence check yet.
pub fn unchecked(ledger: &crate::ledger::Ledger) -> Vec<Job> {
    use crate::ledger::{Change, State};
    ledger
        .stories()
        .iter()
        .filter(|(_, story)| story.state == State::InReview)
        .filter_map(|(id, story)| {
            let submit = story
                .history
                .iter()
                .rev()
                .find(|entry| matches!(entry.change, Change::Submit { .. }))?;
            let Change::Submit { evidence, .. } = &submit.change else {
                return None;
            };
            let checked = story.history.iter().any(|entry| {
                matches!(&entry.change, Change::EvidenceCheck { submit: s, .. } if *s == submit.seq)
            });
            (!checked).then(|| Job {
                story: id.clone(),
                submit: submit.seq,
                evidence: evidence.clone(),
                submitted_at_ms: submit.at_ms,
            })
        })
        .collect()
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
        outcome.reason = Some(format!(
            "CI was still running when the wait of {} minutes ran out",
            ceiling_ms / 60_000
        ));
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
}

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

        // CI couldn't be read: said so.
        let down = Fake {
            message: Some("RAL-5".into()),
            ..Fake::default()
        };
        let outcome = check(Some(&down), "RAL-5", &evidence());
        assert!(outcome.reason.unwrap().contains("CI couldn't be read"));
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
