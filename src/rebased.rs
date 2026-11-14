//! `rally rebased <STORY> --head <sha>` (RAL-170): whether a head is the verified change, re-based.
//!
//! When main moves under a verified pull request, its team merges main in, and the head that will
//! merge is no longer the commit that was read. On 2026-11-13 the watchman and a team checked by
//! hand, about eight times, that the new head's own change was the one verified. This makes it one
//! read, observed and recording nothing: the verified commit's change against its merge base with
//! main, and the head's change against its merge base with main, compared patch by patch. Line
//! numbers may move; the changed lines may not. It says "same change" only when every file's
//! changed lines are identical, and otherwise names the files and the hunks that differ.
//!
//! Separately, a merge of main into the head that had to resolve a conflict is a change nobody read,
//! so each such merge is named, from git's remerge diff (the merge as recorded against the merge git
//! would have made). When it can't tell, because a commit is missing or GitHub cut the compare
//! short, it says so and never says "same". Local git first, GitHub as the fallback.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

/// One file's change, reduced to what a rebase can't move: the facts its header states (a mode
/// changed, a rename, a file added or deleted, a binary file's blobs) and its hunks' added and
/// removed lines, in order, without line numbers or context.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileChange {
    pub header: Vec<String>,
    pub hunks: Vec<Vec<String>>,
}

impl FileChange {
    fn is_empty(&self) -> bool {
        self.header.is_empty() && self.hunks.iter().all(Vec::is_empty)
    }
}

/// The file a `diff --git a/<old> b/<new>` line names, by its new path. With the same path on both
/// sides (every change but a rename) the line splits evenly, even when the path holds " b/".
fn git_path(rest: &str) -> String {
    let rest = rest.strip_prefix("a/").unwrap_or(rest);
    if rest.len() > 3 {
        let half = (rest.len() - 3) / 2;
        if rest.is_char_boundary(half)
            && rest.is_char_boundary(half + 3)
            && &rest[half..half + 3] == " b/"
            && rest[..half] == rest[half + 3..]
        {
            return rest[..half].to_owned();
        }
    }
    match rest.find(" b/") {
        Some(at) => rest[at + 3..].to_owned(),
        None => rest.to_owned(),
    }
}

/// A unified diff (`git diff --full-index`) as each file's change. Every file is keyed from its
/// `diff --git` line, so a file with no hunks, such as a binary file, a mode change or a pure
/// rename, is still there. A file's header runs to its first `@@`: inside a hunk, a line that
/// starts `--- ` or `+++ ` is a removed `-- ` or an added `++ ` line, never a header.
pub fn changes_by_file(diff: &str) -> BTreeMap<String, FileChange> {
    let mut out: BTreeMap<String, FileChange> = BTreeMap::new();
    let mut file: Option<String> = None;
    let mut in_header = false;
    let mut index_line: Option<String> = None;
    let mut last_changed = false;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let name = git_path(rest);
            out.entry(name.clone()).or_default();
            file = Some(name);
            in_header = true;
            index_line = None;
            continue;
        }
        let Some(name) = &file else { continue };
        let change = out.get_mut(name).expect("made above");
        if line.starts_with("@@") {
            in_header = false;
            change.hunks.push(Vec::new());
            continue;
        }
        if in_header {
            if line.starts_with("index ") {
                // Blob ids move with the base; only a binary file's are its content.
                index_line = Some(line.to_owned());
            } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
                change.header.push(
                    index_line
                        .clone()
                        .unwrap_or_else(|| "binary, without its blob ids".to_owned()),
                );
            } else if !(line.starts_with("--- ")
                || line.starts_with("+++ ")
                || line.starts_with("similarity index ")
                || line.starts_with("dissimilarity index "))
            {
                // new file mode, deleted file mode, old mode, new mode, rename from/to, copy
                // from/to: facts of the change itself.
                change.header.push(line.to_owned());
            }
        } else if let Some(hunk) = change.hunks.last_mut() {
            if line.starts_with('+') || line.starts_with('-') {
                hunk.push(line.to_owned());
                last_changed = true;
                continue;
            }
            // `\ No newline at end of file` marks the line before it. After a changed line it's
            // part of the change: a head that differs only by a final newline isn't the same.
            if line.starts_with('\\') && last_changed {
                hunk.push(line.to_owned());
            }
        }
        last_changed = false;
    }
    // A hunk of context alone changes nothing.
    for change in out.values_mut() {
        change.hunks.retain(|h| !h.is_empty());
    }
    out
}

/// The files whose change differs between the verified change and the head's: by their header
/// facts, or by the first hunk that differs, shown as it is in each.
pub fn differences(verified: &str, head: &str) -> Vec<Value> {
    let (a, b) = (changes_by_file(verified), changes_by_file(head));
    let mut names: Vec<&String> = a.keys().chain(b.keys()).collect();
    names.sort();
    names.dedup();
    let mut out = Vec::new();
    for name in names {
        let (x, y) = (a.get(name), b.get(name));
        if x == y {
            continue;
        }
        let only_in = match (x, y) {
            (None, _) => Some("head"),
            (_, None) => Some("verified"),
            _ => None,
        };
        let (x, y) = (
            x.cloned().unwrap_or_default(),
            y.cloned().unwrap_or_default(),
        );
        if x.header != y.header {
            out.push(json!({
                "file": name, "only_in": only_in,
                "verified_header": x.header, "head_header": y.header,
            }));
            continue;
        }
        let at = (0..x.hunks.len().max(y.hunks.len()))
            .find(|i| x.hunks.get(*i) != y.hunks.get(*i))
            .unwrap_or(0);
        // Shown from the hunk's first changed line that differs, so a long hunk (a new file is
        // one) shows where the two part, not its start.
        let (xh, yh) = (x.hunks.get(at), y.hunks.get(at));
        let from = match (xh, yh) {
            (Some(a), Some(b)) => (0..a.len().max(b.len()))
                .find(|i| a.get(*i) != b.get(*i))
                .unwrap_or(0),
            _ => 0,
        };
        let shown = |h: Option<&Vec<String>>| {
            h.map(|lines| {
                lines
                    .iter()
                    .skip(from)
                    .take(12)
                    .cloned()
                    .collect::<Vec<_>>()
            })
        };
        out.push(json!({
            "file": name,
            "only_in": only_in,
            "hunk": at + 1,
            "from_changed_line": from + 1,
            "verified_hunk": shown(xh),
            "head_hunk": shown(yh),
        }));
    }
    out
}

/// Which attributes a remerge used (RAL-176), said with every local answer.
pub const REMERGE_ATTRIBUTES: &str = "each merge is remerged with the .gitattributes of its first \
     parent, the side it was made on, not this checkout's; this machine's own attributes \
     ($GIT_DIR/info/attributes, core.attributesFile) still apply";

/// Said with every comparison made: what "same change" can't see.
pub const CAVEAT: &str = "compared hunk by hunk, in order, without line numbers: a hunk that moved \
                          to another place in its file, unchanged, reads as the same change";

/// The verdict on two changes: "same change" only when both change something and every file's
/// changed lines are identical. A verified change that's empty, as when the verified commit is
/// already in main, leaves nothing to compare, so that can't be told.
fn judge(verified: &str, head: &str) -> (&'static str, Vec<Value>, Option<&'static str>) {
    if changes_by_file(verified).values().all(FileChange::is_empty) {
        return (
            "can't tell",
            Vec::new(),
            Some(
                "the verified commit changes nothing against its merge base with main (it may \
                 already be in main), so there's no change to compare the head's with",
            ),
        );
    }
    let differing = differences(verified, head);
    let verdict = if differing.is_empty() {
        "same change"
    } else {
        "changed"
    };
    (verdict, differing, None)
}

/// Runs git in `dir`, giving its output, or why not.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("can't run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

fn has(dir: &Path, sha: &str) -> bool {
    git(dir, &["cat-file", "-e", &format!("{sha}^{{commit}}")]).is_ok()
}

/// The comparison from a local checkout, or `Err` saying why it can't be made here, such as a
/// commit this checkout doesn't have.
pub fn local(dir: &Path, verified: &str, head: &str, main: &str) -> Result<Value, String> {
    for (what, sha) in [
        ("verified commit", verified),
        ("head", head),
        ("main", main),
    ] {
        if !has(dir, sha) {
            return Err(format!("this checkout doesn't have the {what} {sha}"));
        }
    }
    let base = |sha: &str| git(dir, &["merge-base", sha, main]).map(|s| s.trim().to_owned());
    let (vbase, hbase) = (base(verified)?, base(head)?);
    let diff = |from: &str, to: &str| {
        git(
            dir,
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--full-index",
                "--find-renames",
                from,
                to,
            ],
        )
    };
    let (vdiff, hdiff) = (diff(&vbase, verified)?, diff(&hbase, head)?);
    let (verdict, differing, why) = judge(&vdiff, &hdiff);
    // Each merge brought into the head since the verified commit, and what its remerge diff shows:
    // a conflict resolved, or a change made in the merge itself. Only the head's own merges, on its
    // first-parent line: main's history brings other branches' merges with it, and a conflict
    // another team resolved in one was read when that team's work was (RAL-172).
    let merges = git(
        dir,
        &[
            "rev-list",
            "--merges",
            "--first-parent",
            &format!("{verified}..{head}"),
        ],
    )?;
    let mut resolved = Vec::new();
    for merge in merges.lines().filter(|l| !l.is_empty()) {
        // With the attributes the merge was made with, its first parent's (the head's side, where
        // the team ran `git merge`), not whatever this checkout has: a union-merged file is no
        // conflict, wherever it's checked (RAL-176).
        let attributes = format!("--attr-source={merge}^1");
        let remerge = git(
            dir,
            &[
                &attributes,
                "show",
                "--remerge-diff",
                "--format=",
                "--no-color",
                merge,
            ],
        )?;
        let files: Vec<String> = remerge
            .lines()
            .filter_map(|l| l.strip_prefix("diff --git "))
            .map(git_path)
            .collect();
        if !files.is_empty() {
            resolved.push(json!({"merge": merge, "files": files}));
        }
    }
    let contains_main = hbase == main;
    Ok(json!({
        "how": "local git",
        "verified_base": vbase, "head_base": hbase, "head_contains_main": contains_main,
        "verdict": verdict,
        "why": why,
        "caveat": CAVEAT,
        "differences": differing,
        "conflict_resolutions": resolved,
        "remerge_attributes": REMERGE_ATTRIBUTES,
    }))
}

/// The comparison from GitHub's compares of main...verified and main...head, each `{files:
/// [{filename, patch}], truncated}`. A file with no patch (too large for GitHub to show), or a
/// compare GitHub cut short, means it can't tell. GitHub has no remerge diff, so whether a merge of
/// main resolved a conflict is unknown, and that's said.
pub fn from_github(verified: &Value, head: &Value) -> Value {
    let as_diff = |compare: &Value| -> Option<String> {
        if compare["truncated"] == true {
            return None;
        }
        let mut text = String::new();
        for file in compare["files"].as_array()? {
            let name = file["filename"].as_str()?;
            let patch = file["patch"].as_str()?;
            text.push_str(&format!("diff --git a/{name} b/{name}\n"));
            // GitHub's own word for what happened to the file, as the header's facts.
            match file["status"].as_str() {
                Some("renamed") => text.push_str(&format!(
                    "rename from {}\nrename to {name}\n",
                    file["previous_filename"].as_str().unwrap_or_default()
                )),
                Some("added") => text.push_str("new file\n"),
                Some("removed") => text.push_str("deleted file\n"),
                _ => {}
            }
            text.push_str(patch);
            text.push('\n');
        }
        Some(text)
    };
    match (as_diff(verified), as_diff(head)) {
        (Some(v), Some(h)) => {
            let (verdict, differing, why) = judge(&v, &h);
            json!({
                "how": "GitHub",
                "verdict": verdict,
                "why": why,
                "caveat": CAVEAT,
                "differences": differing,
                "conflict_resolutions": null,
                "conflict_note": "GitHub has no remerge diff, so whether a merge of main into the \
                                  head resolved a conflict can't be told from here: run it in a \
                                  checkout that has both commits",
                "modes_note": "GitHub's compare doesn't show file modes, so a mode change, made \
                               or undone, can't be seen from here: run it in a checkout that has \
                               both commits",
            })
        }
        _ => json!({
            "how": "GitHub",
            "verdict": "can't tell",
            "why": "GitHub's compare was cut short, or a file's patch was too large to show",
        }),
    }
}

/// The verified commit of a story, from `rally show <STORY>`'s detail, and the repository it's
/// in, from the submission whose evidence names that commit (or the latest one, if none does).
/// With `part`, a story in parts (docs/parts.md): that part's verified commit and the repository
/// its evidence names, since until its close a story in parts has no verified commit of its own
/// (RAL-173).
pub fn verified_of(detail: &Value, part: Option<u32>) -> Result<(String, String), String> {
    if let Some(k) = part {
        let entry = &detail["part_list"][k.to_string()];
        if entry.is_null() {
            return Err(format!("the story has no part {k}"));
        }
        let Some(verified) = entry["verified_commit"].as_str() else {
            return Err(format!(
                "part {k} has no verified commit (it's {}): there's nothing to compare a head with",
                entry["state"].as_str().unwrap_or("unknown")
            ));
        };
        let repo = entry["evidence"]
            .as_str()
            .and_then(|e| crate::evidence::parse(e).ok())
            .map(|(repo, _)| repo)
            .ok_or(format!("part {k}'s evidence doesn't name a repository"))?;
        return Ok((repo, verified.to_owned()));
    }
    let Some(verified) = detail["verified_commit"].as_str() else {
        let verified_parts: Vec<&String> = detail["part_list"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, p)| p["verified_commit"].is_string())
            .map(|(k, _)| k)
            .collect();
        let hint = if verified_parts.is_empty() {
            String::new()
        } else {
            format!(
                "; it's in parts, and part {} {} verified: name one with --part",
                verified_parts
                    .iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                if verified_parts.len() == 1 {
                    "is"
                } else {
                    "are"
                }
            )
        };
        return Err(format!(
            "the story has no verified commit (it's {}): there's nothing to compare a head \
             with{hint}",
            detail["state"].as_str().unwrap_or("unknown")
        ));
    };
    let submits: Vec<(String, String)> = detail["history"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["change"]["change"] == "submit")
        .filter_map(|entry| crate::evidence::parse(entry["change"]["evidence"].as_str()?).ok())
        .collect();
    let repo = submits
        .iter()
        .rev()
        .find(|(_, commit)| commit == verified)
        .or(submits.last())
        .map(|(repo, _)| repo.clone())
        .ok_or("no submission of the story names a repository")?;
    Ok((repo, verified.to_owned()))
}

/// The commit to compare a head with, and the repository: the ledger's verified commit (of the
/// story, or with `part` of that part), or with `given` (`--verified <sha>`, RAL-177) that commit,
/// for replaying after the fact, with the repository from the story's submissions. The third value
/// says what the ledger has, when `given` was used, so the output can say it was overridden.
pub fn pick_verified(
    detail: &Value,
    part: Option<u32>,
    given: Option<&str>,
) -> Result<(String, String, Option<Value>), String> {
    let Some(given) = given else {
        let (repo, verified) = verified_of(detail, part)?;
        return Ok((repo, verified, None));
    };
    if given.len() != 40 || !given.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("--verified takes a full commit".to_owned());
    }
    let ledger = verified_of(detail, part);
    // The repository: the ledger's, or else the latest evidence that names one.
    let repo = match &ledger {
        Ok((repo, _)) => repo.clone(),
        Err(_) => detail["history"]
            .as_array()
            .into_iter()
            .flatten()
            .rev()
            .filter_map(|e| e["change"]["evidence"].as_str())
            .find_map(|e| crate::evidence::parse(e).ok())
            .map(|(repo, _)| repo)
            .ok_or("no submission of the story names a repository")?,
    };
    let note = json!({
        "given": "with --verified, for a replay: not the ledger's verified commit, and not recorded",
        "ledger_verified": ledger.ok().map(|(_, v)| v),
    });
    Ok((repo, given.to_ascii_lowercase(), Some(note)))
}

/// A head that is the commit it's compared with (RAL-177): after a check at a re-based head is
/// recorded, that head is the verified commit, and comparing it with itself means nothing.
pub fn compared_with_itself(verified: &str, head: &str) -> Option<Value> {
    verified.eq_ignore_ascii_case(head).then(|| {
        cant_tell(
            "--head is the commit it would be compared with: comparing a commit with itself \
             means nothing. Once a check at a re-based head is recorded, that head is the \
             verified commit; to replay it, give the commit verified before with --verified",
        )
    })
}

/// What's said when the comparison can't be made at all.
pub fn cant_tell(why: &str) -> Value {
    json!({"verdict": "can't tell", "why": why})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway repository with `main` holding one file, and helpers to commit on branches.
    struct Repo(tempfile::TempDir);

    impl Repo {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let repo = Self(dir);
            repo.git(&["init", "-q", "-b", "main"]);
            repo.git(&["config", "user.email", "t@t"]);
            repo.git(&["config", "user.name", "t"]);
            repo.write("a.txt", "one\ntwo\nthree\nfour\nfive\n");
            repo.write("b.txt", "alpha\nbeta\n");
            repo.commit("start");
            repo
        }
        fn path(&self) -> &Path {
            self.0.path()
        }
        fn git(&self, args: &[&str]) -> String {
            git(self.path(), args).unwrap()
        }
        fn write(&self, name: &str, text: &str) {
            std::fs::write(self.path().join(name), text).unwrap();
        }
        fn commit(&self, message: &str) -> String {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", message]);
            self.git(&["rev-parse", "HEAD"]).trim().to_owned()
        }
    }

    /// The verified change on a branch, then main moving on under it.
    fn setup() -> (Repo, String) {
        let r = Repo::new();
        r.git(&["checkout", "-q", "-b", "story"]);
        r.write("a.txt", "one\ntwo\nTHREE\nfour\nfive\n");
        let verified = r.commit("RAL-1: the change");
        r.git(&["checkout", "-q", "main"]);
        r.write("b.txt", "alpha\nbeta\ngamma\n");
        r.commit("main moves");
        (r, verified)
    }

    fn main(r: &Repo) -> String {
        r.git(&["rev-parse", "main"]).trim().to_owned()
    }

    #[test]
    fn a_clean_merge_of_main_is_the_same_change() {
        let (r, verified) = setup();
        r.git(&["checkout", "-q", "story"]);
        r.git(&["merge", "-q", "--no-edit", "main"]);
        let head = r.git(&["rev-parse", "HEAD"]).trim().to_owned();
        let out = local(r.path(), &verified, &head, &main(&r)).unwrap();
        assert_eq!(out["verdict"], "same change", "{out}");
        assert_eq!(out["head_contains_main"], true);
        assert_eq!(out["conflict_resolutions"], json!([]));
    }

    #[test]
    fn a_head_that_changed_more_names_the_file_and_hunk() {
        let (r, verified) = setup();
        r.git(&["checkout", "-q", "story"]);
        r.git(&["merge", "-q", "--no-edit", "main"]);
        r.write("a.txt", "one\ntwo\nTHREE\nfour\nFIVE\n");
        let head = r.commit("and another thing");
        let out = local(r.path(), &verified, &head, &main(&r)).unwrap();
        assert_eq!(out["verdict"], "changed", "{out}");
        assert_eq!(out["differences"][0]["file"], "a.txt");
        assert!(
            out["differences"][0]["head_hunk"]
                .to_string()
                .contains("+FIVE")
        );
    }

    #[test]
    fn a_merge_that_resolved_a_conflict_is_named() {
        let (r, verified) = setup();
        // Main also changes the verified line, so merging it in conflicts.
        r.git(&["checkout", "-q", "main"]);
        r.write("a.txt", "one\ntwo\nthree!\nfour\nfive\n");
        r.commit("main touches the same line");
        r.git(&["checkout", "-q", "story"]);
        assert!(
            git(r.path(), &["merge", "-q", "--no-edit", "main"]).is_err(),
            "it conflicts"
        );
        r.write("a.txt", "one\ntwo\nTHREE\nfour\nfive\n");
        r.git(&["add", "-A"]);
        r.git(&["commit", "-q", "--no-edit"]);
        let head = r.git(&["rev-parse", "HEAD"]).trim().to_owned();
        let out = local(r.path(), &verified, &head, &main(&r)).unwrap();
        let resolved = out["conflict_resolutions"].as_array().unwrap();
        assert_eq!(resolved.len(), 1, "{out}");
        assert_eq!(resolved[0]["files"], json!(["a.txt"]));
    }

    #[test]
    fn a_missing_commit_or_a_cut_short_compare_cant_tell() {
        let (r, verified) = setup();
        let missing = "f".repeat(40);
        let error = local(r.path(), &verified, &missing, &main(&r)).unwrap_err();
        assert!(error.contains("doesn't have the head"), "{error}");
        let cut = json!({"truncated": true, "files": []});
        let whole = json!({"files": [{"filename": "a.txt", "patch": "@@ -1 +1 @@\n-a\n+b"}]});
        assert_eq!(from_github(&cut, &whole)["verdict"], "can't tell");
        let no_patch = json!({"files": [{"filename": "big.bin"}]});
        assert_eq!(from_github(&whole, &no_patch)["verdict"], "can't tell");
        // Same on GitHub, but whether merges resolved conflicts is said to be unknown.
        let same = from_github(&whole, &whole);
        assert_eq!(same["verdict"], "same change");
        assert!(same["conflict_resolutions"].is_null());
        assert!(
            same["conflict_note"]
                .as_str()
                .unwrap()
                .contains("can't be told")
        );
        assert!(same["modes_note"].as_str().unwrap().contains("file modes"));
        // A verified commit already in main changes nothing against it: never "same".
        let r = Repo::new();
        let tip = main(&r);
        let out = local(r.path(), &tip, &tip, &tip).unwrap();
        assert_eq!(out["verdict"], "can't tell", "{out}");
        let empty = json!({"files": []});
        assert_eq!(from_github(&empty, &empty)["verdict"], "can't tell");
        // A story never verified has nothing to compare with.
        let unverified = json!({"state": "submitted", "history": []});
        assert!(
            verified_of(&unverified, None)
                .unwrap_err()
                .contains("no verified commit")
        );
    }

    #[test]
    fn the_verified_commit_and_its_repo_come_from_the_story() {
        let v = "a".repeat(40);
        let detail = json!({"verified_commit": v, "state": "verified", "history": [
            {"change": {"change": "submit", "evidence": format!("o/old@{v}")}},
            {"change": {"change": "submit", "evidence": format!("o/later@{}", "b".repeat(40))}},
        ]});
        assert_eq!(verified_of(&detail, None).unwrap(), ("o/old".to_owned(), v));
    }

    #[test]
    fn line_numbers_moving_dont_make_a_difference() {
        let a = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n";
        let b = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -10,3 +12,3 @@\n a\n-b\n+B\n c\n";
        assert!(differences(a, b).is_empty());
        let c = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n a\n-b\n+C\n c\n";
        assert_eq!(differences(a, c)[0]["file"], "x");
    }

    /// A head with main merged in, and then `extra` done on the story branch, against the verified
    /// commit, where the verified change is `change` on top of `setup()`'s start.
    fn compare_after(change: impl Fn(&Repo), extra: impl Fn(&Repo)) -> Value {
        let r = Repo::new();
        r.git(&["config", "core.fileMode", "true"]);
        r.git(&["checkout", "-q", "-b", "story"]);
        change(&r);
        let verified = r.commit("RAL-1: the change");
        r.git(&["checkout", "-q", "main"]);
        r.write("b.txt", "alpha\nbeta\ngamma\n");
        r.commit("main moves");
        r.git(&["checkout", "-q", "story"]);
        r.git(&["merge", "-q", "--no-edit", "main"]);
        extra(&r);
        r.git(&["add", "-A"]);
        let _ = git(r.path(), &["commit", "-q", "-m", "more"]);
        let head = r.git(&["rev-parse", "HEAD"]).trim().to_owned();
        local(r.path(), &verified, &head, &main(&r)).unwrap()
    }

    fn write_bytes(r: &Repo, name: &str, bytes: &[u8]) {
        std::fs::write(r.path().join(name), bytes).unwrap();
    }

    #[test]
    fn a_binary_file_is_compared_by_its_blobs() {
        let add = |r: &Repo| write_bytes(r, "img.bin", &[0, 1, 2, 3, 0, 255]);
        let same = compare_after(add, |_| {});
        assert_eq!(same["verdict"], "same change", "{same}");
        let changed = compare_after(add, |r| write_bytes(r, "img.bin", &[0, 9, 9, 9, 0, 255]));
        assert_eq!(changed["verdict"], "changed", "{changed}");
        assert_eq!(changed["differences"][0]["file"], "img.bin");
        assert!(
            changed["differences"][0]["head_header"]
                .to_string()
                .contains("index ")
        );
    }

    #[test]
    fn a_mode_change_is_a_change() {
        use std::os::unix::fs::PermissionsExt;
        let chmod = |r: &Repo, mode| {
            std::fs::set_permissions(
                r.path().join("a.txt"),
                std::fs::Permissions::from_mode(mode),
            )
            .unwrap()
        };
        let edit = |r: &Repo| r.write("a.txt", "one\ntwo\nTHREE\nfour\nfive\n");
        let changed = compare_after(edit, |r| chmod(r, 0o755));
        assert_eq!(changed["verdict"], "changed", "{changed}");
        assert!(
            changed["differences"][0]["head_header"]
                .to_string()
                .contains("new mode 100755")
        );
        // A mode change alone, with no hunk at all, is still seen.
        let only_mode = compare_after(|r| chmod(r, 0o755), |_| {});
        assert_eq!(only_mode["verdict"], "same change", "{only_mode}");
        let undone = compare_after(|r| chmod(r, 0o755), |r| chmod(r, 0o644));
        assert_eq!(
            undone["verdict"], "changed",
            "the head undoes the mode change: {undone}"
        );
        assert_eq!(undone["differences"][0]["only_in"], "verified");
    }

    #[test]
    fn a_pure_rename_is_a_change() {
        let rename = |r: &Repo| {
            r.git(&["mv", "a.txt", "renamed.txt"]);
        };
        let same = compare_after(rename, |_| {});
        assert_eq!(same["verdict"], "same change", "{same}");
        let other = compare_after(rename, |r| {
            r.git(&["mv", "renamed.txt", "other.txt"]);
        });
        assert_eq!(other["verdict"], "changed", "{other}");
    }

    #[test]
    fn lines_like_headers_inside_a_hunk_are_lines() {
        let diff = |removed: &str, added: &str| {
            format!(
                "diff --git a/x.sql b/x.sql\nindex 1..2 100644\n--- a/x.sql\n+++ b/x.sql\n\
                 @@ -1,2 +1,2 @@\n a\n--- {removed}\n+++ {added}\n"
            )
        };
        // A removed SQL comment `-- one` is the diff line `--- one`; an added `++ two` is `+++ two`.
        let parsed = changes_by_file(&diff("one", "two"));
        assert_eq!(
            parsed.keys().collect::<Vec<_>>(),
            ["x.sql"],
            "no bogus file"
        );
        assert_eq!(parsed["x.sql"].hunks, [["--- one", "+++ two"]]);
        let differing = differences(&diff("one", "two"), &diff("ONE", "two"));
        assert_eq!(
            differing.len(),
            1,
            "a differing removed `-- ` line is a difference"
        );
        assert_eq!(differing[0]["verified_hunk"][0], "--- one");
        assert_eq!(differing[0]["from_changed_line"], 1);
        // A long hunk is shown from where the two part.
        let long = |last: &str| {
            let mut d = "diff --git a/n b/n\nnew file mode 100644\n--- /dev/null\n+++ b/n\n@@ -0,0 +1,30 @@\n"
                .to_owned();
            for i in 0..29 {
                d.push_str(&format!("+line {i}\n"));
            }
            d.push_str(&format!("+{last}\n"));
            d
        };
        let parted = differences(&long("end"), &long("END"));
        assert_eq!(parted[0]["from_changed_line"], 30);
        assert_eq!(parted[0]["verified_hunk"], json!(["+end"]));
        assert_eq!(parted[0]["head_hunk"], json!(["+END"]));
        assert!(differences(&diff("one", "two"), &diff("one", "two")).is_empty());
    }

    #[test]
    fn a_path_is_read_from_the_diff_git_line() {
        assert_eq!(git_path("a/x b/x"), "x");
        assert_eq!(git_path("a/has b/in it b/has b/in it"), "has b/in it");
        assert_eq!(git_path("a/old b/new"), "new");
    }

    #[test]
    fn the_output_says_what_same_cant_see() {
        let out = compare_after(
            |r| r.write("a.txt", "one\ntwo\nTHREE\nfour\nfive\n"),
            |_| {},
        );
        assert!(out["caveat"].as_str().unwrap().contains("moved"), "{out}");
    }

    #[test]
    fn a_final_newline_is_part_of_the_change() {
        let edit = |r: &Repo| r.write("a.txt", "one\ntwo\nthree\nfour\nFIVE\n");
        let same = compare_after(edit, |_| {});
        assert_eq!(same["verdict"], "same change", "{same}");
        let no_newline = compare_after(edit, |r| r.write("a.txt", "one\ntwo\nthree\nfour\nFIVE"));
        assert_eq!(no_newline["verdict"], "changed", "{no_newline}");
        assert!(
            no_newline["differences"][0]["head_hunk"]
                .to_string()
                .contains("No newline at end of file"),
            "{no_newline}"
        );
        // The marker after an unchanged line of context isn't a change.
        let diff = |end: &str| {
            format!(
                "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-a\n+A\n {end}\n\
                 \\ No newline at end of file\n"
            )
        };
        assert_eq!(changes_by_file(&diff("b"))["x"].hunks, [["-a", "+A"]]);
    }

    /// RAL-172, from replaying RAL-163's re-based head: a conflict another branch resolved inside
    /// main's history isn't the head's. Only the head's own merges are named.
    #[test]
    fn a_conflict_resolved_inside_mains_history_isnt_the_heads() {
        let (r, verified) = setup();
        // Another team's branch conflicts with main and resolves it, then lands in main.
        r.git(&["checkout", "-q", "-b", "other", "main"]);
        r.write("b.txt", "alpha\nBETA\ngamma\n");
        r.commit("other team");
        r.git(&["checkout", "-q", "main"]);
        r.write("b.txt", "alpha\nbeta!\ngamma\n");
        r.commit("main moves again");
        r.git(&["checkout", "-q", "other"]);
        assert!(
            git(r.path(), &["merge", "-q", "--no-edit", "main"]).is_err(),
            "it conflicts"
        );
        r.write("b.txt", "alpha\nBETA!\ngamma\n");
        r.git(&["add", "-A"]);
        r.git(&["commit", "-q", "--no-edit"]);
        r.git(&["checkout", "-q", "main"]);
        r.git(&["merge", "-q", "--no-ff", "--no-edit", "other"]);
        // The story merges main in cleanly.
        r.git(&["checkout", "-q", "story"]);
        r.git(&["merge", "-q", "--no-edit", "main"]);
        let head = r.git(&["rev-parse", "HEAD"]).trim().to_owned();
        let out = local(r.path(), &verified, &head, &main(&r)).unwrap();
        assert_eq!(out["verdict"], "same change", "{out}");
        assert_eq!(out["conflict_resolutions"], json!([]), "{out}");
    }

    /// RAL-173: a story in parts, before its close, is compared by a part's verified commit, as
    /// RAL-163's part 2 (f8c61e2) needed on 2026-11-14. Without --part it says which parts are.
    #[test]
    fn a_part_is_compared_by_its_own_verified_commit() {
        let (p1, p2) = ("4".repeat(40), "f".repeat(40));
        let detail = json!({"state": "claimed", "parts": "2/3 verified", "history": [], "part_list": {
            "1": {"evidence": format!("Castor-o7/rally@{p1}"), "state": "verified", "verified_commit": p1},
            "2": {"evidence": format!("Castor-o7/rally@{p2}"), "state": "verified", "verified_commit": p2},
            "3": {"evidence": format!("Castor-o7/rally@{}", "a".repeat(40)), "state": "submitted"},
        }});
        assert_eq!(
            verified_of(&detail, Some(2)).unwrap(),
            ("Castor-o7/rally".to_owned(), p2)
        );
        let whole = verified_of(&detail, None).unwrap_err();
        assert!(
            whole.contains("part 1, 2 are verified: name one with --part"),
            "{whole}"
        );
        assert!(
            verified_of(&detail, Some(3))
                .unwrap_err()
                .contains("part 3 has no verified commit")
        );
        assert!(
            verified_of(&detail, Some(9))
                .unwrap_err()
                .contains("no part 9")
        );
    }

    /// RAL-176, from the watchman's run on 2026-11-14: a checkout on an old branch without the
    /// head's `.gitattributes` remerged a union-merged roadmap with the default driver, and named
    /// it as a conflict resolved. The remerge now uses the merge's own side's attributes.
    #[test]
    fn a_remerge_uses_the_heads_attributes_not_the_checkouts() {
        let r = Repo::new();
        let old = main(&r);
        r.write("roadmap.md", "# Roadmap\n- one\n");
        r.write(".gitattributes", "roadmap.md merge=union\n");
        r.commit("the union driver");
        r.git(&["checkout", "-q", "-b", "story"]);
        r.write("roadmap.md", "# Roadmap\n- one\n- story\n");
        r.write("a.txt", "one\ntwo\nTHREE\nfour\nfive\n");
        let verified = r.commit("RAL-1: the change");
        r.git(&["checkout", "-q", "main"]);
        r.write("roadmap.md", "# Roadmap\n- one\n- main\n");
        r.commit("main appends too");
        r.git(&["checkout", "-q", "story"]);
        r.git(&["merge", "-q", "--no-edit", "main"]);
        let head = r.git(&["rev-parse", "HEAD"]).trim().to_owned();
        // The checkout sits on a commit with no .gitattributes, as the watchman's did.
        r.git(&["checkout", "-q", &old]);
        assert!(!r.path().join(".gitattributes").exists());
        let out = local(r.path(), &verified, &head, &main(&r)).unwrap();
        assert_eq!(out["conflict_resolutions"], json!([]), "{out}");
        assert!(
            out["remerge_attributes"]
                .as_str()
                .unwrap()
                .contains("first parent")
        );
        // The same remerge with the checkout's attributes (none) shows the conflict: what it was.
        let plain = git(r.path(), &["show", "--remerge-diff", "--format=", &head]).unwrap();
        assert!(plain.contains("remerge CONFLICT"), "{plain}");
    }

    /// RAL-177: `--verified` replays after the fact, saying it was given and what the ledger has;
    /// a head that is the commit compared with says it means nothing, never "same".
    #[test]
    fn a_replay_names_its_verified_commit_and_a_head_never_compares_with_itself() {
        let (old, now) = ("8".repeat(40), "0".repeat(40));
        let detail = json!({"verified_commit": now, "state": "verified", "history": [
            {"change": {"change": "submit", "evidence": format!("o/r@{old}")}},
            {"change": {"change": "submit", "evidence": format!("o/r@{now}")}},
        ]});
        let (repo, verified, note) = pick_verified(&detail, None, Some(&old)).unwrap();
        assert_eq!((repo.as_str(), verified.as_str()), ("o/r", old.as_str()));
        let note = note.unwrap();
        assert_eq!(note["ledger_verified"], now);
        assert!(note["given"].as_str().unwrap().contains("not recorded"));
        // Without it, the ledger's, and no note.
        let (_, ledger, none) = pick_verified(&detail, None, None).unwrap();
        assert_eq!((ledger.as_str(), none), (now.as_str(), None));
        // A story not yet verified can still be replayed against a given commit.
        let unverified = json!({"state": "submitted", "history": [
            {"change": {"change": "submit", "evidence": format!("o/r@{old}")}}]});
        assert!(
            pick_verified(&unverified, None, Some(&old))
                .unwrap()
                .2
                .unwrap()["ledger_verified"]
                .is_null()
        );
        assert!(
            pick_verified(&detail, None, Some("abc"))
                .unwrap_err()
                .contains("full commit")
        );
        // The head that is the verified commit.
        let itself = compared_with_itself(&now, &now.to_uppercase()).unwrap();
        assert_eq!(itself["verdict"], "can't tell");
        assert!(
            itself["why"]
                .as_str()
                .unwrap()
                .contains("with itself means nothing")
        );
        assert!(compared_with_itself(&old, &now).is_none());
    }
}
