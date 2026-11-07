//! `rally gate <range>` (RAL-70): before `main` moves, every story the range's commits name must
//! be verified. From the DiscoSim rally of 2026-11-06, where `main` was moved past a story a peer had
//! already sent back (issues log §2.5).
//!
//! A commit names a story the way evidence does (RAL-5): its ID as a whole word, in the commit's
//! subject, in a merge commit's pull request title (the body's first line), or in a trailer such as
//! `Story: RAL-70`. Bodies are otherwise left out, since they cite other stories in passing ("after
//! MUS-95 was written by hand"). Only the messages are read, never the files, and nothing in the
//! checkout is run.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use serde_json::{Map, Value, json};

/// The most commits a range may have. More is refused rather than read in part.
pub const MAX_COMMITS: usize = 5_000;
const MAX_ID_BYTES: usize = 32;

/// A commit's full hash and message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    /// Its parents' full hashes, first parent first.
    pub parents: Vec<String>,
    pub message: String,
}

/// The commits in `range`, newest first, read from the checkout at `repo`.
pub fn commits(repo: &Path, range: &str) -> Result<Vec<Commit>, String> {
    if range.is_empty() || range.starts_with('-') || range.chars().any(char::is_whitespace) {
        return Err(format!(
            "{range:?} isn't a commit range: give one such as 19cc9d1..origin/main"
        ));
    }
    let limit = format!("--max-count={}", MAX_COMMITS + 1);
    // A checkout's own git config can name programs for git to run; the ones log could reach are
    // turned off, and nothing that runs a program (textconv, signatures, a pager) is asked for.
    let out = Command::new("git")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "log.showSignature=false",
            "--no-pager",
            "log",
            "--no-color",
            "--format=%H%x00%P%x00%B%x00",
            &limit,
            "--end-of-options",
            range,
            "--",
        ])
        .current_dir(repo)
        .output()
        .map_err(|e| format!("git couldn't be run: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git couldn't read {range:?}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut fields = text.split('\0');
    let mut found = Vec::new();
    while let (Some(sha), Some(parents), Some(message)) =
        (fields.next(), fields.next(), fields.next())
    {
        let sha = sha.trim();
        if sha.is_empty() {
            break;
        }
        found.push(Commit {
            sha: sha.to_owned(),
            parents: parents.split_whitespace().map(str::to_owned).collect(),
            message: message.to_owned(),
        });
    }
    if found.len() > MAX_COMMITS {
        return Err(format!(
            "{range:?} has more than {MAX_COMMITS} commits: give a shorter range"
        ));
    }
    Ok(found)
}

/// Whether commit `a` is `b` or an ancestor of it in the checkout at `repo`, by
/// `git merge-base --is-ancestor`, with the same guards as [`commits`]. An error, such as a commit
/// the checkout doesn't have, counts as no (RAL-79).
pub fn is_ancestor(repo: &Path, a: &str, b: &str) -> bool {
    let full = |c: &str| matches!(c.len(), 40 | 64) && c.bytes().all(|x| x.is_ascii_hexdigit());
    if !full(a) || !full(b) {
        return false;
    }
    if a.eq_ignore_ascii_case(b) {
        return true;
    }
    Command::new("git")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "--no-pager",
            "merge-base",
            "--is-ancestor",
            "--end-of-options",
            a,
            b,
        ])
        .current_dir(repo)
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Every story ID `text` names as a whole word, as [`crate::evidence::names`] would match it:
/// `RAL-5` in "fix (RAL-5)", but not in `XRAL-5`, `RAL-50` or `ral-5`.
pub fn ids(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut found = BTreeSet::new();
    let mut i = 0;
    while i < bytes.len() {
        let starts = bytes[i].is_ascii_uppercase()
            && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'-'));
        if !starts {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_uppercase() {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'-' {
            let digits = j + 1;
            let mut k = digits;
            while k < bytes.len() && bytes[k].is_ascii_digit() {
                k += 1;
            }
            let bounded = k == bytes.len() || !bytes[k].is_ascii_alphanumeric();
            if k > digits && bounded && k - i <= MAX_ID_BYTES {
                found.insert(text[i..k].to_owned());
            }
            i = k.max(i + 1);
        } else {
            i = j;
        }
    }
    found
}

/// The parts of a commit message that say which story it's for: the subject; for a merge, the
/// first line of the body, where GitHub writes the pull request's title; and the closing
/// paragraph's lines, if every one of them is a `Key: value` trailer.
pub fn naming(message: &str) -> String {
    let mut paragraphs = message
        .trim()
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty());
    let mut parts: Vec<&str> = Vec::new();
    let Some(first) = paragraphs.next() else {
        return String::new();
    };
    let subject = first.lines().next().unwrap_or_default();
    parts.push(subject);
    let rest: Vec<&str> = paragraphs.collect();
    if subject.starts_with("Merge ")
        && let Some(title) = rest.first().and_then(|p| p.lines().next())
    {
        parts.push(title);
    }
    if let Some(last) = rest.last() {
        let trailer = |line: &str| {
            line.split_once(": ").is_some_and(|(key, _)| {
                !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        };
        if last.lines().all(trailer) {
            parts.extend(last.lines());
        }
    }
    parts.join("\n")
}

fn prefix(id: &str) -> &str {
    id.split_once('-').map_or(id, |(prefix, _)| prefix)
}

/// The gate's finding for `range`, given its commits and the ledger's stories (a map of ID to a
/// row with a `state`, as `rally show` lists them). `ok` is false if any story named isn't
/// verified, or if an ID that isn't in the ledger has a prefix that is, unless it's in `allow`:
/// the watchman's word that the range only mentions it, as a commit filing it on the roadmap does.
pub fn judge(
    range: &str,
    commits: &[Commit],
    stories: &Map<String, Value>,
    allow: &BTreeSet<String>,
) -> Value {
    judge_with(range, commits, stories, allow, &|commit, read| {
        commit == read
    })
}

/// Whether `commit` is covered by a read of `read`, given whether one commit is the other or an
/// ancestor of it: a verified part's read covers the commits before it on its branch, and a merge
/// whose merged-in parents are covered (RAL-79).
fn covered(commit: &Commit, read: &str, ancestor: &dyn Fn(&str, &str) -> bool) -> bool {
    ancestor(&commit.sha, read)
        || (commit.parents.len() > 1 && commit.parents[1..].iter().all(|p| ancestor(p, read)))
}

/// [`judge`], where `ancestor(a, b)` says whether commit `a` is `b` or an ancestor of it, as
/// `git merge-base --is-ancestor` does. A story that isn't verified still passes for a commit a
/// verified part's read covers (RAL-79): its `part_commits`, as `rally show` lists them.
pub fn judge_with(
    range: &str,
    commits: &[Commit],
    stories: &Map<String, Value>,
    allow: &BTreeSet<String>,
    ancestor: &dyn Fn(&str, &str) -> bool,
) -> Value {
    let prefixes: BTreeSet<&str> = stories.keys().map(|id| prefix(id)).collect();
    let mut named: BTreeMap<String, Vec<&Commit>> = BTreeMap::new();
    for commit in commits {
        for id in ids(&naming(&commit.message)) {
            named.entry(id).or_default().push(commit);
        }
    }
    let mut shown = Map::new();
    let mut refused = Vec::new();
    let mut other = Map::new();
    let mut allowed = Vec::new();
    for (id, named_by) in named {
        let shas: Vec<&str> = named_by.iter().map(|c| c.sha.as_str()).collect();
        if allow.contains(&id) {
            let state = stories.get(&id).map(|row| row["state"].clone());
            shown.insert(
                id.clone(),
                json!({"state": state, "commits": shas, "allowed": true}),
            );
            allowed.push(id);
            continue;
        }
        match stories.get(&id) {
            Some(row) => {
                let state = row["state"].as_str().unwrap_or("unknown");
                // Commits a verified part's read covers pass on their own (RAL-79).
                let reads: Vec<&str> = row["part_commits"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                let by_parts: Vec<&str> = named_by
                    .iter()
                    .filter(|c| reads.iter().any(|read| covered(c, read, ancestor)))
                    .map(|c| c.sha.as_str())
                    .collect();
                let all_by_parts = by_parts.len() == named_by.len();
                if state != "verified" && !all_by_parts {
                    let why = match state {
                        "sent_back" => "sent back",
                        "in_review" => "submitted but not verified",
                        "claimed" => "claimed, not submitted",
                        "ready" => "ready, not claimed",
                        _ => "not verified",
                    };
                    refused.push(json!({"story": id, "state": state, "why": why}));
                }
                let mut entry = json!({"state": state, "commits": shas});
                if !by_parts.is_empty() {
                    entry["covered_by_parts"] = json!(by_parts);
                }
                if let Some(parts) = row.get("parts") {
                    entry["parts"] = parts.clone();
                }
                shown.insert(id, entry);
            }
            None if prefixes.contains(prefix(&id)) => {
                refused.push(json!({"story": id, "state": null, "why": "not in the ledger"}));
                shown.insert(id, json!({"state": null, "commits": shas}));
            }
            None => {
                other.insert(id, json!(shas));
            }
        }
    }
    let ok = refused.is_empty();
    let mut value = json!({
        "ok": ok,
        "range": range,
        "commits": commits.len(),
        "stories": shown,
    });
    if !ok {
        value["refused"] = json!(refused);
    }
    if !allowed.is_empty() {
        value["allowed"] = json!(allowed);
    }
    if !other.is_empty() {
        // Words shaped like IDs whose prefix the ledger has never used, such as SHA-256.
        value["not_story_ids"] = Value::Object(other);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(sha: &str, message: &str) -> Commit {
        Commit {
            sha: sha.into(),
            parents: Vec::new(),
            message: message.into(),
        }
    }

    /// RAL-79: a claimed story's commits pass when a verified part's read covers them, as itself,
    /// an ancestor, or a merge of a covered branch; one it doesn't cover still refuses.
    #[test]
    fn a_verified_parts_read_covers_its_commits_and_its_ancestors() {
        let stories: Map<String, Value> = [(
            "DSM-37".to_owned(),
            json!({"state": "claimed", "parts": "1/4 verified", "part_commits": ["p1"]}),
        )]
        .into_iter()
        .collect();
        // a0 is before p1 on its branch; m merges p1's branch; late came after the read.
        let ancestor = |a: &str, b: &str| a == b || (a == "a0" && b == "p1");
        let merge = Commit {
            sha: "m".into(),
            parents: vec!["main".into(), "p1".into()],
            message: "Merge pull request #9\n\nDSM-37 part 1".into(),
        };
        let covered = [
            commit("p1", "DSM-37 part 1: bar"),
            commit("a0", "DSM-37: start"),
            merge,
        ];
        let found = judge_with("x..y", &covered, &stories, &BTreeSet::new(), &ancestor);
        assert_eq!(found["ok"], true, "{found}");
        assert_eq!(
            found["stories"]["DSM-37"]["covered_by_parts"],
            json!(["p1", "a0", "m"])
        );
        assert_eq!(found["stories"]["DSM-37"]["parts"], "1/4 verified");

        let late = [
            commit("p1", "DSM-37 part 1"),
            commit("late", "DSM-37 part 2"),
        ];
        let found = judge_with("x..y", &late, &stories, &BTreeSet::new(), &ancestor);
        assert_eq!(found["ok"], false);
        assert_eq!(found["refused"][0]["story"], "DSM-37");
    }

    fn none() -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn ledger(rows: &[(&str, &str)]) -> Map<String, Value> {
        rows.iter()
            .map(|(id, state)| ((*id).to_owned(), json!({"state": state})))
            .collect()
    }

    #[test]
    fn ids_are_whole_words_as_evidence_matches_them() {
        let text = "feat: gate (RAL-70), after RAL-5 and DSM-42.\nXRAL-1 RAL-50x ral-3 RAL- -9 A-1";
        let found = ids(text);
        let found: Vec<&str> = found.iter().map(String::as_str).collect();
        // XRAL-1 is its own ID, not RAL-1; RAL-50x and ral-3 aren't IDs at all.
        assert_eq!(found, ["A-1", "DSM-42", "RAL-5", "RAL-70", "XRAL-1"]);
        for id in &found {
            assert!(crate::evidence::names(text, id), "{id}");
        }
        assert!(!crate::evidence::names(text, "RAL-1"));
        assert!(ids("x-RAL-5").is_empty());
        assert_eq!(ids("RAL-5"), BTreeSet::from(["RAL-5".to_owned()]));
    }

    #[test]
    fn a_range_of_verified_stories_passes() {
        let commits = [
            commit("a", "feat: one (RAL-1)"),
            commit("b", "Merge pull request #9\n\nfix: two (RAL-2)"),
        ];
        let found = judge(
            "x..y",
            &commits,
            &ledger(&[("RAL-1", "verified"), ("RAL-2", "verified")]),
            &none(),
        );
        assert_eq!(found["ok"], true);
        assert_eq!(found["commits"], 2);
        assert_eq!(found["stories"]["RAL-2"]["commits"], json!(["b"]));
        assert!(found.get("refused").is_none());
    }

    #[test]
    fn a_sent_back_or_unverified_story_is_refused() {
        let commits = [
            commit("a", "fix: act happiness (DSM-42)"),
            commit("b", "feat: DSM-43"),
            commit("c", "feat: DSM-44"),
            commit("d", "wip DSM-45"),
            commit("e", "ok DSM-46"),
        ];
        let stories = ledger(&[
            ("DSM-42", "sent_back"),
            ("DSM-43", "in_review"),
            ("DSM-44", "claimed"),
            ("DSM-45", "ready"),
            ("DSM-46", "verified"),
        ]);
        let found = judge("x..y", &commits, &stories, &none());
        assert_eq!(found["ok"], false);
        let refused: Vec<&str> = found["refused"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["story"].as_str().unwrap())
            .collect();
        assert_eq!(refused, ["DSM-42", "DSM-43", "DSM-44", "DSM-45"]);
        assert_eq!(found["refused"][0]["why"], "sent back");
    }

    #[test]
    fn an_unknown_id_fails_only_when_its_prefix_is_the_ledgers() {
        let commits = [commit("a", "feat: DSM-99 on SHA-256 and UTF-8 (RAL-1)")];
        let found = judge(
            "x..y",
            &commits,
            &ledger(&[("RAL-1", "verified"), ("DSM-1", "verified")]),
            &none(),
        );
        assert_eq!(found["ok"], false);
        assert_eq!(found["refused"][0]["story"], "DSM-99");
        assert_eq!(found["refused"][0]["why"], "not in the ledger");
        assert_eq!(
            found["not_story_ids"],
            json!({"SHA-256": ["a"], "UTF-8": ["a"]})
        );

        let found = judge(
            "x..y",
            &[commit("a", "SHA-256 (RAL-1)")],
            &ledger(&[("RAL-1", "verified")]),
            &none(),
        );
        assert_eq!(found["ok"], true);
    }

    #[test]
    fn only_the_subject_a_merges_title_and_trailers_name_a_story() {
        let message = "fix: digest flags merges (RAL-8)\n\nAfter MUS-95 was handed out twice; see RAL-31.\n\n\
                       Story: RAL-67\nCo-Authored-By: someone <a@b>\n";
        assert_eq!(
            ids(&naming(message)),
            BTreeSet::from(["RAL-67".into(), "RAL-8".into()])
        );
        let merge = "Merge pull request #140 from Castor-o7/ral67/also-read\n\n\
                     feat: a verifier adds commits also read (RAL-67)\n";
        assert_eq!(ids(&naming(merge)), BTreeSet::from(["RAL-67".into()]));
        // A closing paragraph that isn't all trailers is prose, and left out.
        let prose = "feat: x (RAL-1)\n\nNote: this replaces RAL-2.\nIt also does more.";
        assert_eq!(ids(&naming(prose)), BTreeSet::from(["RAL-1".into()]));
    }

    #[test]
    fn an_allowed_story_is_listed_and_not_refused() {
        let commits = [commit("a", "docs: file RAL-67 and RAL-68 on the roadmap")];
        let stories = ledger(&[("RAL-67", "ready"), ("RAL-68", "ready")]);
        let allow = BTreeSet::from(["RAL-67".to_owned()]);
        let found = judge("x..y", &commits, &stories, &allow);
        assert_eq!(found["ok"], false);
        assert_eq!(found["refused"][0]["story"], "RAL-68");
        assert_eq!(found["allowed"], json!(["RAL-67"]));
        assert_eq!(found["stories"]["RAL-67"]["allowed"], true);
        let allow = BTreeSet::from(["RAL-67".to_owned(), "RAL-68".to_owned()]);
        assert_eq!(judge("x..y", &commits, &stories, &allow)["ok"], true);
    }

    #[test]
    fn an_empty_range_passes_with_nothing_named() {
        let found = judge("x..x", &[], &ledger(&[("RAL-1", "ready")]), &none());
        assert_eq!(found["ok"], true);
        assert_eq!(found["commits"], 0);
    }
}
