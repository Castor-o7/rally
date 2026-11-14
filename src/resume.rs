//! What a restarted session needs to resume (RAL-152, issue 1.73). When every session on a machine
//! ends at once and comes back under the same nicknames, each new session starts with no memory,
//! and the watchman had to work out by hand which earlier session had held each nickname and where
//! its transcript was. `rally session-start` now tells it, read-only:
//!
//! - **Who it is.** This session's id ties it to a Muster nickname through the tag Muster's own
//!   SessionStart hook leaves at the head of its transcript, or through Muster's roster. Without
//!   such a tie it says nothing of anyone else's work: a new, unrelated session in the same folder
//!   is never told it holds another session's claims.
//! - **Which session held that nickname before.** Muster records each takeover but offers no read
//!   of it, so Rally reads the head of each recent transcript in the project's folder for the same
//!   tag. That's observed from the transcript files on this machine, never Muster's word. Only the
//!   tag is taken from a transcript; nothing of its content is shown or recorded.
//! - **What that nickname was doing,** from the ledger: what it holds, what it last recorded,
//!   reads waiting on it and its last note, each declared, as the session that wrote it said.
//!
//! It tells and never acts: nothing here writes a claim, a release or a note.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::session::clean;

/// How much of a transcript's head is read for its tag: the SessionStart hook's context is in its
/// first lines.
pub const HEAD_BYTES: usize = 64 * 1024;

/// How far back an earlier transcript is looked for.
pub const LOOK_BACK_MS: i64 = 14 * 24 * 60 * 60 * 1000;

/// The most transcripts read for their tag, newest first.
pub const MAX_TRANSCRIPTS: usize = 200;

/// Claude Code's folder name for a working directory: every character but a letter or digit
/// becomes `-`, so `/home/joshd/src` is `-home-joshd-src`.
pub fn project_slug(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Every Muster session tag in `head`, `<muster_session nickname="X" id="Y">`, as written in a
/// transcript, where its quotes are escaped once or more: `(nickname, id)`, in order.
pub fn tags_of(head: &str) -> Vec<(String, String)> {
    const OPEN: &str = "muster_session nickname=";
    let unquote = |text: &str| -> Option<String> {
        let text = text.trim_start_matches('\\').strip_prefix('"')?;
        let end = text.find(['"', '\\'])?;
        Some(text[..end].to_owned())
    };
    head.match_indices(OPEN)
        .filter_map(|(at, _)| {
            let rest = &head[at + OPEN.len()..];
            let nickname = unquote(rest)?;
            let rest = &rest[rest.find(" id=")? + " id=".len()..];
            let id = unquote(rest)?;
            let name_ok = !nickname.is_empty()
                && nickname.len() <= 64
                && nickname
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            let id_ok = id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
            (name_ok && id_ok).then_some((nickname, id))
        })
        .collect()
}

/// The first tag in `head`.
pub fn tag_of(head: &str) -> Option<(String, String)> {
    tags_of(head).into_iter().next()
}

/// The tag at the head of the transcript at `path` that names the transcript's own session (its
/// file name), so a transcript that merely quotes another session's tag isn't taken for it.
pub fn transcript_tag(path: &Path) -> Option<(String, String)> {
    use std::io::Read;
    let mut head = vec![0u8; HEAD_BYTES];
    let mut file = std::fs::File::open(path).ok()?;
    let mut read = 0;
    while read < HEAD_BYTES {
        match file.read(&mut head[read..]) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(_) => return None,
        }
    }
    head.truncate(read);
    let own = path.file_stem()?.to_str()?;
    tags_of(&String::from_utf8_lossy(&head))
        .into_iter()
        .find(|(_, id)| id == own)
}

/// How this session is tied to its nickname, which is what lets it be told it's a restart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tie {
    /// The tag at the head of this session's own transcript.
    Transcript,
    /// Muster's roster, matched by this session's id.
    Roster,
}

impl Tie {
    fn words(&self) -> &'static str {
        match self {
            Self::Transcript => "its transcript's Muster tag",
            Self::Roster => "Muster's roster",
        }
    }
}

/// This session's nickname, and how it's known: from its own transcript's tag, else the roster.
pub fn identify(
    transcript: &Path,
    session_id: &str,
    roster: Option<&Value>,
) -> Option<(String, Tie)> {
    if let Some((nickname, id)) = transcript_tag(transcript)
        && id == session_id
    {
        return Some((nickname, Tie::Transcript));
    }
    roster?["sessions"]
        .as_array()?
        .iter()
        .find(|session| session["session_id"] == session_id)
        .and_then(|session| session["nickname"].as_str())
        .map(|nickname| (nickname.to_owned(), Tie::Roster))
}

/// The session that held a nickname before this one, as found from its transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Earlier {
    pub id: String,
    /// When its transcript was last written, in ms since 1970.
    pub last_written_ms: i64,
    pub transcript: PathBuf,
    pub scratchpad: PathBuf,
    pub scratchpad_exists: bool,
}

fn modified_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(since.as_millis()).ok()
}

/// The newest other session in `dir` (a project's transcripts folder) whose transcript's tag names
/// `nickname`, last written within [`LOOK_BACK_MS`] of `now_ms`. `scratch_root` is where scratchpads
/// live for this project: `/tmp/claude-<uid>/<slug>`.
pub fn earlier_session(
    dir: &Path,
    nickname: &str,
    session_id: &str,
    scratch_root: &Path,
    now_ms: i64,
) -> Option<Earlier> {
    let mut transcripts: Vec<(i64, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
        .filter(|path| path.file_stem().and_then(|s| s.to_str()) != Some(session_id))
        .filter_map(|path| Some((modified_ms(&path)?, path)))
        .filter(|(at, _)| now_ms.saturating_sub(*at) <= LOOK_BACK_MS)
        .collect();
    transcripts.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    transcripts
        .into_iter()
        .take(MAX_TRANSCRIPTS)
        .find_map(|(at, path)| {
            let (name, id) = transcript_tag(&path)?;
            (name == nickname).then(|| {
                let scratchpad = scratch_root.join(&id).join("scratchpad");
                Earlier {
                    scratchpad_exists: scratchpad.is_dir(),
                    id,
                    last_written_ms: at,
                    transcript: path,
                    scratchpad,
                }
            })
        })
}

fn age(at_ms: i64, now_ms: i64) -> String {
    crate::digest::age(at_ms, now_ms)
}

/// The record a line of the block describes, in a few words.
fn what(record: &Value) -> String {
    let change = &record["change"];
    let kind = change["change"].as_str().unwrap_or("?");
    let mut words = format!(
        "{} {}",
        clean(record["story"].as_str().unwrap_or("?")),
        kind.replace('_', " ")
    );
    if let Some(part) = change["part"].as_u64() {
        words.push_str(&format!(" part {part}"));
    }
    if let Some(evidence) = change["evidence"].as_str() {
        words.push_str(&format!(" at {}", clean(evidence)));
    }
    if let Some(commit) = change["commit"].as_str() {
        words.push_str(&format!(
            " at {}",
            clean(&commit.chars().take(12).collect::<String>())
        ));
    }
    words
}

/// The lines telling a restarted session what it needs to resume. `nickname` is tied to this
/// session by `tie`; `earlier` is the session found to hold it before, if any; `records` is the
/// team's record from the ledger, oldest first; `stories` the ledger's stories. Empty when the
/// nickname has nothing behind it: no earlier session and no records, so a new session is told
/// nothing.
pub fn lines(
    nickname: &str,
    tie: &Tie,
    earlier: Option<&Earlier>,
    records: &[Value],
    stories: &Map<String, Value>,
    now_ms: i64,
) -> Vec<String> {
    let mine: Vec<&Value> = records
        .iter()
        .filter(|r| r["by"].as_str() == Some(nickname))
        .collect();
    if earlier.is_none() && mine.is_empty() {
        return Vec::new();
    }
    let name = clean(nickname);
    let mut out = vec![match earlier {
        Some(_) => format!(
            "You are a restart of {name}: this session's id is tied to {name} by {} [observed], \
             and an earlier session held it. Nothing here was done for you: claim, release and \
             note as you decide.",
            tie.words()
        ),
        None => format!(
            "This session is {name}: its id is tied to {name} by {} [observed]. Nothing here was \
             done for you: claim, release and note as you decide.",
            tie.words()
        ),
    }];
    match earlier {
        Some(earlier) => {
            out.push(format!(
                "- Earlier session of {name}: {} [observed from its transcript on this machine, \
                 last written {}]",
                clean(&earlier.id),
                age(earlier.last_written_ms, now_ms)
            ));
            out.push(format!(
                "  transcript: {}",
                clean(&earlier.transcript.display().to_string())
            ));
            out.push(format!(
                "  scratchpad: {}{}",
                clean(&earlier.scratchpad.display().to_string()),
                if earlier.scratchpad_exists {
                    ""
                } else {
                    " (gone)"
                }
            ));
        }
        None => out.push(format!(
            "- Rally can't tell whether an earlier session held {name}: no earlier transcript of \
             it on this machine, or this session is continuing. Muster records each takeover but \
             offers no read of it yet."
        )),
    }
    if mine.is_empty() {
        out.push(format!(
            "- A session named {name} recorded nothing in the ledger."
        ));
        return out;
    }
    out.push(format!("A session named {name} recorded these [declared]:"));
    // What it holds now, by the ledger's state of each story it claimed.
    let held: Vec<String> = stories
        .iter()
        .filter(|(_, row)| row["team"].as_str() == Some(nickname))
        .filter(|(_, row)| {
            matches!(
                row["state"].as_str(),
                Some("claimed" | "in_review" | "sent_back")
            )
        })
        .map(|(id, row)| {
            format!(
                "{} ({})",
                clean(id),
                clean(row["state"].as_str().unwrap_or("?"))
            )
        })
        .collect();
    out.push(format!(
        "- Holds: {}",
        if held.is_empty() {
            "nothing".to_owned()
        } else {
            held.join(", ")
        }
    ));
    let last = |kinds: &[&str]| {
        mine.iter()
            .rev()
            .find(|r| kinds.contains(&r["change"]["change"].as_str().unwrap_or("")))
            .copied()
    };
    if let Some(story) = mine.iter().rev().find(|r| r["record"] == "story") {
        out.push(format!(
            "- Last recorded on a story: {}, {}",
            what(story),
            age(story["at_ms"].as_i64().unwrap_or(now_ms), now_ms)
        ));
    }
    if let Some(sent) = last(&["submit", "verify", "send_back", "merge"]) {
        let id = sent["story"].as_str().unwrap_or("");
        let now = stories
            .get(id)
            .and_then(|row| row["state"].as_str())
            .unwrap_or("unknown");
        out.push(format!(
            "- Last submit, verify or merge: {}, {}; that story is now {}",
            what(sent),
            age(sent["at_ms"].as_i64().unwrap_or(now_ms), now_ms),
            clean(now)
        ));
    }
    let reads: Vec<String> = stories
        .iter()
        .filter(|(_, row)| row["team"].as_str() == Some(nickname))
        .filter_map(|(id, row)| {
            let open = row["requested_reads"].as_u64().filter(|n| *n > 0)?;
            Some(format!("{} ({open})", clean(id)))
        })
        .collect();
    if !reads.is_empty() {
        out.push(format!(
            "- Reads waiting on its stories: {}",
            reads.join(", ")
        ));
    }
    if let Some(note) = mine
        .iter()
        .rev()
        .find(|r| r["record"] == "team_state" && r["note"].is_string())
    {
        out.push(format!(
            "- Its last note: \"{}\" ({}, {})",
            clean(note["note"].as_str().unwrap_or("")),
            clean(note["status"].as_str().unwrap_or("?")),
            age(note["at_ms"].as_i64().unwrap_or(now_ms), now_ms)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ME: &str = "441312bb-598d-41c4-85db-70d947b01da4";
    const BEFORE: &str = "00baf303-0000-4000-8000-000000000000";
    const OTHER: &str = "a1800a85-0000-4000-8000-000000000000";

    fn transcript(dir: &Path, id: &str, nickname: &str) -> PathBuf {
        let path = dir.join(format!("{id}.jsonl"));
        std::fs::write(
            &path,
            format!(
                "{{\"type\":\"attachment\",\"content\":\"SessionStart hook additional context: \
                 <muster_session nickname=\\\"{nickname}\\\" id=\\\"{id}\\\">Muster tools attach \
                 this identity.</muster_session>\"}}\n{{\"type\":\"user\"}}\n"
            ),
        )
        .unwrap();
        path
    }

    #[test]
    fn the_slug_is_claude_codes() {
        assert_eq!(project_slug("/home/joshd/src"), "-home-joshd-src");
        assert_eq!(
            project_slug("/home/joshd/Documents/GitHub/Discoteque"),
            "-home-joshd-Documents-GitHub-Discoteque"
        );
        assert_eq!(project_slug("/a/b.c_d"), "-a-b-c-d");
    }

    #[test]
    fn a_tag_is_read_quoted_or_escaped_and_nothing_else_passes() {
        let escaped = format!("x <muster_session nickname=\\\"silent-dhole\\\" id=\\\"{ME}\\\">");
        assert_eq!(tag_of(&escaped), Some(("silent-dhole".into(), ME.into())));
        let plain = format!("<muster_session nickname=\"silent-dhole\" id=\"{ME}\">");
        // As a transcript holds it inside JSON inside JSON, escaped twice.
        let twice = format!(
            "<muster_session nickname=\\\\\\\"silent-dhole\\\\\\\" id=\\\\\\\"{ME}\\\\\\\">"
        );
        assert_eq!(tag_of(&twice), Some(("silent-dhole".into(), ME.into())));
        assert_eq!(tag_of(&plain), Some(("silent-dhole".into(), ME.into())));
        assert_eq!(
            tag_of("<muster_session nickname=\"Bad Name\" id=\"x\">"),
            None
        );
        assert_eq!(tag_of("nothing here"), None);
    }

    /// RAL-152: a session is tied to its nickname by its own transcript or by the roster, and the
    /// earlier session is the newest other transcript of that nickname, never another's, and never
    /// one whose tag names a session other than its file.
    #[test]
    fn the_earlier_session_is_the_newest_other_transcript_of_the_nickname() {
        let dir = tempfile::tempdir().unwrap();
        let mine = transcript(dir.path(), ME, "silent-dhole");
        transcript(dir.path(), OTHER, "wise-crocodile");
        let before = transcript(dir.path(), BEFORE, "silent-dhole");
        // A transcript quoting someone else's tag isn't theirs.
        std::fs::write(
            dir.path()
                .join("11111111-1111-4111-8111-111111111111.jsonl"),
            format!("<muster_session nickname=\"silent-dhole\" id=\"{ME}\">"),
        )
        .unwrap();
        assert_eq!(
            identify(&mine, ME, None),
            Some(("silent-dhole".into(), Tie::Transcript))
        );
        let roster = json!({"sessions": [{"session_id": ME, "nickname": "silent-dhole"}]});
        let missing = dir.path().join("none.jsonl");
        assert_eq!(
            identify(&missing, ME, Some(&roster)),
            Some(("silent-dhole".into(), Tie::Roster))
        );
        assert_eq!(identify(&missing, ME, None), None);

        let scratch = dir.path().join("scratch");
        std::fs::create_dir_all(scratch.join(BEFORE).join("scratchpad")).unwrap();
        let now = modified_ms(&before).unwrap() + 1_000;
        let earlier = earlier_session(dir.path(), "silent-dhole", ME, &scratch, now).unwrap();
        assert_eq!(earlier.id, BEFORE);
        assert_eq!(earlier.transcript, before);
        assert!(earlier.scratchpad_exists);
        assert!(earlier_session(dir.path(), "steely-ridge", ME, &scratch, now).is_none());
        // Past the look-back, nothing.
        assert!(
            earlier_session(
                dir.path(),
                "silent-dhole",
                ME,
                &scratch,
                now + LOOK_BACK_MS + 5_000
            )
            .is_none()
        );
    }

    fn stories() -> Map<String, Value> {
        json!({
            "DSM-211": {"state": "in_review", "team": "silent-dhole", "requested_reads": 1},
            "DSM-200": {"state": "verified", "team": "silent-dhole"},
            "DSM-5": {"state": "claimed", "team": "wise-crocodile"},
        })
        .as_object()
        .unwrap()
        .clone()
    }

    fn records() -> Vec<Value> {
        vec![
            json!({"seq": 1, "at_ms": 0, "by": "mellow-weevil", "record": "story", "story": "DSM-211",
                   "change": {"change": "assign", "story": "DSM-211", "team": "silent-dhole"}}),
            json!({"seq": 2, "at_ms": 0, "by": "silent-dhole", "record": "team_state",
                   "status": "working", "note": "part 2 renders"}),
            json!({"seq": 3, "at_ms": 0, "by": "silent-dhole", "record": "story", "story": "DSM-211",
                   "change": {"change": "submit", "story": "DSM-211", "part": 1,
                              "evidence": "Castor-o7/Discoteque@525aa5e"}}),
        ]
    }

    /// RAL-152: a restart is told its earlier session, what it holds, its last submit and that
    /// story's state now, reads waiting and its last note, each labelled.
    #[test]
    fn a_restart_is_told_what_it_needs_to_resume() {
        let earlier = Earlier {
            id: BEFORE.into(),
            last_written_ms: 0,
            transcript: "/t/00baf303.jsonl".into(),
            scratchpad: "/s/00baf303/scratchpad".into(),
            scratchpad_exists: false,
        };
        let out = lines(
            "silent-dhole",
            &Tie::Transcript,
            Some(&earlier),
            &records(),
            &stories(),
            60_000,
        )
        .join("\n");
        for expected in [
            "You are a restart of silent-dhole: this session's id is tied to silent-dhole by its \
             transcript's Muster tag [observed], and an earlier session held it.",
            "Nothing here was done for you",
            &format!("- Earlier session of silent-dhole: {BEFORE} [observed from its transcript"),
            "scratchpad: /s/00baf303/scratchpad (gone)",
            "A session named silent-dhole recorded these [declared]:",
            "- Holds: DSM-211 (in_review)",
            "- Last submit, verify or merge: DSM-211 submit part 1 at Castor-o7/Discoteque@525aa5e",
            "that story is now in_review",
            "- Reads waiting on its stories: DSM-211 (1)",
            "- Its last note: \"part 2 renders\" (working",
        ] {
            assert!(out.contains(expected), "{expected}\n{out}");
        }
        assert!(!out.contains("DSM-5"), "another team's claim: {out}");
    }

    /// RAL-152: with no earlier session and no records, a new session is told nothing; with
    /// records but no transcript, it's told plainly that Rally can't tell.
    #[test]
    fn a_new_session_is_told_nothing_and_an_unknown_one_is_said_plainly() {
        assert!(lines("fresh-name", &Tie::Roster, None, &[], &stories(), 0).is_empty());
        let only_others = vec![records()[0].clone()];
        assert!(
            lines(
                "silent-dhole",
                &Tie::Roster,
                None,
                &only_others,
                &stories(),
                0
            )
            .is_empty()
        );
        let out = lines(
            "silent-dhole",
            &Tie::Roster,
            None,
            &records(),
            &stories(),
            0,
        )
        .join("\n");
        assert!(
            out.contains("tied to silent-dhole by Muster's roster"),
            "{out}"
        );
        assert!(out.contains("This session is silent-dhole"), "{out}");
        assert!(!out.contains("restart"), "{out}");
        assert!(
            out.contains("Rally can't tell whether an earlier session held silent-dhole"),
            "{out}"
        );
    }
}
