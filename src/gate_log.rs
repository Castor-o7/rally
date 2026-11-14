//! A gate's log, as `rally batch end --gate-log` records it (RAL-162).
//!
//! What it's worth, and what it isn't: the log's sha256, taken by the CLI rather than typed, binds
//! the log the watchman keeps to the one it named when the batch ended, so a log changed later is
//! caught. It doesn't show the gate ran: the watchman could name any file. So everything here is
//! declared, labelled as the log kept by the watchman with no retention promised, and the result
//! lines are read from it by one fixed parser, never typed. What it does catch is the slip: a log
//! that doesn't name the batch's tip as the commit it ran on is refused, which is the wrong log for
//! the wrong batch, or a gate run on a stale tip.
//!
//! The format is DiscoSim's `dsm-gate` (2026-11-13 on): a first line
//! `# dsm-gate: commit <sha> tree <sha> scenes: <scenes> started <time> script sha256 <hex>`, then
//! the QUICK phase, a suite at a time (`== test_sim ==`, or `== test_layout (what it covers) ==`),
//! then `QUICK OK` (or `QUICK OK (...)`), and for each scene a `== res://tests/<scene>.tscn ==`
//! header. Each suite or scene header is followed by `=== N passed, M failed ===`, and all of them
//! are recorded, the QUICK phase's suites by name and the scenes by theirs (RAL-181: before, only
//! the scenes were, three of about 113). A log without the first line is refused plainly rather
//! than guessed at.

use serde::{Deserialize, Serialize};

/// The longest log read, so a wrong path to a huge file isn't read whole.
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// The most suites and scenes recorded (RAL-181: DiscoSim's gate runs about 113).
pub const MAX_SCENES: usize = 256;

/// One scene's result line, or that its header had none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub scene: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<u64>,
}

/// What's recorded of a gate's log: where it was named, its hash and size, and what its own lines
/// say. Declared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateLog {
    /// The path as the watchman gave it.
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    /// The commit the log says the gate ran on.
    pub commit: String,
    pub tree: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub started: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub script: String,
    /// The `QUICK OK` line, if there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenes: Vec<Scene>,
}

impl GateLog {
    /// How many scene tests failed, by the log's own lines, and how many scenes gave no result.
    pub fn failures(&self) -> (u64, usize) {
        (
            self.scenes.iter().filter_map(|s| s.failed).sum(),
            self.scenes.iter().filter(|s| s.failed.is_none()).count(),
        )
    }
}

fn is_sha(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The word after `key` in `line`, if `key` is there.
fn after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = &line[line.find(key)? + key.len()..];
    rest.split_whitespace().next()
}

/// A suite or scene header's name: `== res://tests/<scene>.tscn ==` gives the scene, and
/// `== <suite> ==` or `== <suite> (what it covers) ==` the suite (RAL-181). A `=== ... ===` result
/// line isn't one.
fn suite_header(line: &str) -> Option<&str> {
    if line.starts_with("===") {
        return None;
    }
    let inner = line.strip_prefix("== ")?.strip_suffix(" ==")?.trim();
    if let Some(scene) = inner
        .strip_prefix("res://tests/")
        .and_then(|rest| rest.strip_suffix(".tscn"))
    {
        return Some(scene);
    }
    let name = inner.split(" (").next().unwrap_or(inner).trim();
    (!name.is_empty() && !name.contains(char::is_whitespace)).then_some(name)
}

/// Reads a `dsm-gate` log's lines. `path`, `sha256` and `bytes` are the caller's, which read the
/// file; this only parses, so it can be tested without one.
pub fn parse(text: &str, path: &str, sha256: &str, bytes: u64) -> Result<GateLog, String> {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    let Some(header) = first.strip_prefix("# dsm-gate:") else {
        return Err(
            "the log has no `# dsm-gate: commit …` first line, so it doesn't say which commit the \
             gate ran on: logs from before that line was added can't be recorded"
                .to_owned(),
        );
    };
    let commit = after(header, "commit ").unwrap_or_default();
    let tree = after(header, "tree ").unwrap_or_default();
    if !is_sha(commit) || !is_sha(tree) {
        return Err(format!(
            "the log's first line doesn't name a full commit and tree: {first:?}"
        ));
    }
    let started = after(header, "started ").unwrap_or_default().to_owned();
    let script = after(header, "script sha256 ")
        .unwrap_or_default()
        .to_owned();
    let mut quick = None;
    let mut scenes: Vec<Scene> = Vec::new();
    for line in lines {
        let line = line.trim();
        if quick.is_none() && line.starts_with("QUICK OK") {
            quick = Some(line.chars().take(200).collect());
            // A QUICK suite with no count line, such as the bench, which is judged by its own
            // verdict suite, is covered by the gate's QUICK OK and left out (RAL-181).
            scenes.retain(|s| s.passed.is_some());
        } else if let Some(scene) = suite_header(line) {
            if scenes.len() >= MAX_SCENES {
                return Err(format!("the log has more than {MAX_SCENES} scenes"));
            }
            scenes.push(Scene {
                scene: scene.chars().take(120).collect(),
                passed: None,
                failed: None,
            });
        } else if let Some(counts) = line
            .strip_prefix("=== ")
            .and_then(|rest| rest.strip_suffix(" ==="))
            && let Some(last) = scenes.last_mut().filter(|s| s.passed.is_none())
        {
            // "N passed, M failed", or with ", K skipped" after (RAL-181).
            let words: Vec<&str> = counts.split([' ', ',']).filter(|w| !w.is_empty()).collect();
            if let [p, "passed", f, "failed", rest @ ..] = words.as_slice()
                && (rest.is_empty() || matches!(rest, [k, "skipped"] if k.parse::<u64>().is_ok()))
                && let (Ok(p), Ok(f)) = (p.parse(), f.parse())
            {
                last.passed = Some(p);
                last.failed = Some(f);
            }
        }
    }
    Ok(GateLog {
        path: path.to_owned(),
        sha256: sha256.to_owned(),
        bytes,
        commit: commit.to_owned(),
        tree: tree.to_owned(),
        started,
        script,
        quick,
        scenes,
    })
}

/// Reads, hashes and parses the log at `path`.
pub fn read(path: &std::path::Path) -> Result<GateLog, String> {
    use sha2::{Digest, Sha256};
    let meta =
        std::fs::metadata(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    if meta.len() > MAX_BYTES {
        return Err(format!(
            "{} is {} bytes, more than a gate log's {MAX_BYTES}",
            path.display(),
            meta.len()
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let sha = hex::encode(Sha256::digest(&bytes));
    let text = String::from_utf8_lossy(&bytes);
    parse(&text, &path.display().to_string(), &sha, bytes.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(commit: &str) -> String {
        format!(
            "# dsm-gate: commit {commit} tree {} scenes: ui_flow ui_fit says_why started \
             2026-11-14T03:00:00Z script sha256 0123456789abcdef\n\
             building...\n\
             QUICK OK (paired bench decided)\n\
             == res://tests/ui_flow.tscn ==\n\
             ok 1\n\
             === 12 passed, 0 failed ===\n\
             == res://tests/ui_fit.tscn ==\n\
             === 8 passed, 1 failed ===\n\
             == res://tests/says_why.tscn ==\n\
             crashed\n",
            "b".repeat(40)
        )
    }

    #[test]
    fn a_dsm_gate_log_is_read_by_its_own_lines() {
        let commit = "a".repeat(40);
        let out = parse(&log(&commit), "/x/a.log", &"f".repeat(64), 99).unwrap();
        assert_eq!(out.commit, commit);
        assert_eq!(out.tree, "b".repeat(40));
        assert_eq!(out.started, "2026-11-14T03:00:00Z");
        assert_eq!(out.script, "0123456789abcdef");
        assert_eq!(
            out.quick.as_deref(),
            Some("QUICK OK (paired bench decided)")
        );
        let scenes: Vec<(&str, Option<u64>, Option<u64>)> = out
            .scenes
            .iter()
            .map(|s| (s.scene.as_str(), s.passed, s.failed))
            .collect();
        assert_eq!(
            scenes,
            [
                ("ui_flow", Some(12), Some(0)),
                ("ui_fit", Some(8), Some(1)),
                ("says_why", None, None)
            ]
        );
        assert_eq!(out.failures(), (1, 1));
        assert_eq!((out.path.as_str(), out.bytes), ("/x/a.log", 99));
    }

    #[test]
    fn a_log_without_its_commit_line_is_refused_plainly() {
        let old = "QUICK OK\n== res://tests/ui_flow.tscn ==\n=== 1 passed, 0 failed ===\n";
        let error = parse(old, "p", "s", 1).unwrap_err();
        assert!(error.contains("doesn't say which commit"), "{error}");
        let short = "# dsm-gate: commit abc123 tree def456 scenes: x started t script sha256 0\n";
        assert!(
            parse(short, "p", "s", 1)
                .unwrap_err()
                .contains("full commit")
        );
    }

    #[test]
    fn the_file_is_hashed_by_rally_not_typed() {
        use sha2::{Digest, Sha256};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gate.log");
        let text = log(&"a".repeat(40));
        std::fs::write(&path, &text).unwrap();
        let out = read(&path).unwrap();
        assert_eq!(out.sha256, hex::encode(Sha256::digest(text.as_bytes())));
        assert_eq!(out.bytes, text.len() as u64);
    }

    /// RAL-181: the QUICK phase's suites are recorded too, by name, beside the scenes, as today's
    /// logs have them; a result line or a heading with words isn't a suite.
    #[test]
    fn every_suite_is_recorded_the_quick_phases_too() {
        let text = format!(
            "# dsm-gate: commit {} tree {} scenes: x started t script sha256 0\n\
             == test_sim ==\nGodot Engine v4\n=== DISCOSIM test_sim ===\n  PASS 1\n\
             === 65 passed, 0 failed ===\n\
             == test_layout (build catalogue, rotation, layout saves) ==\n\
             === DISCOSIM test_layout ===\n=== 232 passed, 1 failed ===\n\
             == not a suite name ==\n\
             == test_club_nav (the sim's walk graph) ==\n=== 25 passed, 0 failed, 0 skipped ===\n\
             == bench (ms per sim tick by crowd size) ==\nBENCH guests>=0 mean=0.5\n\
             QUICK OK\n\
             == res://tests/test_ui_flow.tscn ==\n=== DISCOSIM test_ui_flow ===\n\
             === 267 passed, 0 failed ===\n",
            "a".repeat(40),
            "b".repeat(40)
        );
        let out = parse(&text, "p", &"f".repeat(64), 1).unwrap();
        let got: Vec<(&str, Option<u64>, Option<u64>)> = out
            .scenes
            .iter()
            .map(|s| (s.scene.as_str(), s.passed, s.failed))
            .collect();
        assert_eq!(
            got,
            [
                ("test_sim", Some(65), Some(0)),
                ("test_layout", Some(232), Some(1)),
                ("test_club_nav", Some(25), Some(0)),
                ("test_ui_flow", Some(267), Some(0)),
            ]
        );
        // The bench gives no count and is covered by QUICK OK. Without a QUICK OK, a suite with
        // no result is kept, and counts as giving none.
        let no_quick = text.replace("QUICK OK\n", "");
        let bare = parse(&no_quick, "p", &"f".repeat(64), 1).unwrap();
        assert!(
            bare.scenes
                .iter()
                .any(|s| s.scene == "bench" && s.passed.is_none())
        );
        assert!(!out.scenes.iter().any(|s| s.scene == "bench"));
        assert_eq!(bare.failures(), (1, 1));
        assert_eq!(out.failures(), (1, 0));
        assert_eq!(out.quick.as_deref(), Some("QUICK OK"));
    }
}
