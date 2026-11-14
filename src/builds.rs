//! What's live, what's staged and what's merged but not live, for `rally brief --watchman`
//! (RAL-167).
//!
//! Every part is observed, from its own source: the service's commit is the service's own word
//! (`/v1/whoami`, RAL-156), this CLI's is its own build, and main's tip and the commits between
//! are GitHub's. The commits merged but not live are mapped to stories by the story ID in each
//! commit's message, as a merge commit names its branch (`ral-148-digest-scope`) or a commit its
//! story (`RAL-148: ...`). "Staged" is a hint, not a fact: the CLI on this machine being ahead of
//! the service usually means a build is installed and waiting on a restart. Nothing reads the
//! lamplighter's rollout files.

use serde_json::{Value, json};

/// Rally's own repository on GitHub, from `Cargo.toml`.
pub fn own_repo() -> Option<&'static str> {
    env!("CARGO_PKG_REPOSITORY").strip_prefix("https://github.com/")
}

/// Every Rally story ID a commit message names, in order, once each: `RAL-148`, `ral-148` or
/// `ral148` (as a branch such as `ral147/checks-403` has it).
pub fn stories_in(message: &str) -> Vec<String> {
    let lower = message.to_lowercase();
    let bytes = lower.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while let Some(at) = lower[i..].find("ral") {
        let start = i + at;
        // Not inside a word, such as "general" or "rally".
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let mut j = start + 3;
        if bytes.get(j) == Some(&b'-') {
            j += 1;
        }
        let digits_start = j;
        while bytes.get(j).is_some_and(u8::is_ascii_digit) {
            j += 1;
        }
        if before_ok && j > digits_start {
            let id = format!("RAL-{}", &lower[digits_start..j]);
            if !out.contains(&id) {
                out.push(id);
            }
        }
        i = start + 3;
    }
    out
}

/// GitHub's commit date, `2026-11-13T20:10:00Z`, as milliseconds since 1970.
fn iso_ms(text: &str) -> Option<i64> {
    let text = text.strip_suffix('Z')?;
    crate::inbox::expiry_ms(&format!("{} UTC", text.replacen('T', " ", 1)))
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

/// The `builds` section. `live` is the service's commit, if it says; `cli` this build's;
/// `main` main's tip; `behind` GitHub's compare of live…main and `staged` of live…cli, each
/// `{status, ahead_by, commits: [{sha, message, date}]}`, or why it couldn't be read.
pub fn section(
    live: Option<&str>,
    cli: Option<&str>,
    main: Result<String, String>,
    behind: Option<Result<Value, String>>,
    staged: Option<Result<Value, String>>,
    now_ms: i64,
) -> Value {
    let mut out = json!({
        "kind": "observed",
        "live": match live {
            Some(sha) => json!({"commit": sha, "from": "the service's /v1/whoami"}),
            None => json!({"commit": null,
                "note": "older than RAL-156: the service doesn't say its commit"}),
        },
        "cli": cli,
        "main": match &main {
            Ok(sha) => json!(sha),
            Err(why) => json!({"unread": why}),
        },
    });
    match behind {
        None => {}
        Some(Err(why)) => out["merged_not_live"] = json!({"unread": why}),
        Some(Ok(compare)) => {
            let commits = compare["commits"].as_array().cloned().unwrap_or_default();
            // Each story once, at its oldest commit between live and main.
            let mut stories: Vec<(String, i64, String)> = Vec::new();
            for commit in &commits {
                let first = commit["message"]
                    .as_str()
                    .unwrap_or_default()
                    .lines()
                    .next()
                    .unwrap_or_default();
                let at = commit["date"].as_str().and_then(iso_ms).unwrap_or(now_ms);
                for id in stories_in(first) {
                    match stories.iter_mut().find(|(s, _, _)| *s == id) {
                        Some(row) if at < row.1 => {
                            row.1 = at;
                            row.2 = commit["sha"].as_str().unwrap_or_default().to_owned();
                        }
                        Some(_) => {}
                        None => stories.push((
                            id,
                            at,
                            commit["sha"].as_str().unwrap_or_default().to_owned(),
                        )),
                    }
                }
            }
            stories.sort_by_key(|(_, at, _)| *at);
            let oldest_ms = stories.first().map(|(_, at, _)| now_ms.saturating_sub(*at));
            let count = stories.len();
            // GitHub's compare lists at most 250 commits: further behind, this reads short.
            let ahead = compare["ahead_by"].as_u64().unwrap_or(commits.len() as u64);
            let short_read = (ahead > commits.len() as u64).then(|| {
                format!(
                    "the service is {ahead} commits behind main, but GitHub's compare lists only \
                     {}: the stories and the oldest age here read short",
                    commits.len()
                )
            });
            out["merged_not_live"] = json!({
                "commits": ahead,
                "listed": commits.len(),
                "reads_short": short_read,
                "stories": stories.iter().map(|(id, at, sha)| json!({
                    "story": id, "commit": short(sha), "age_ms": now_ms.saturating_sub(*at),
                })).collect::<Vec<_>>(),
                "count": count,
                "oldest_age_ms": oldest_ms,
                "summary": if count == 0 {
                    "live is main's tip: nothing merged waits to go live".to_owned()
                } else {
                    format!(
                        "{count} stor{} merged, not live; the oldest for {} h",
                        if count == 1 { "y" } else { "ies" },
                        oldest_ms.unwrap_or(0) / 3_600_000
                    )
                },
            });
        }
    }
    if let Some(Ok(compare)) = staged
        && compare["ahead_by"].as_u64().is_some_and(|n| n > 0)
    {
        out["hint"] = json!(format!(
            "this CLI ({}) is ahead of the service ({}): staged, waiting on a restart [a hint, \
             not a fact]",
            short(cli.unwrap_or_default()),
            short(live.unwrap_or_default())
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_message_names_its_stories() {
        assert_eq!(
            stories_in("Merge pull request #232 from Castor-o7/ral-148-digest-scope"),
            ["RAL-148"]
        );
        assert_eq!(
            stories_in("Merge pull request #231 from Castor-o7/ral147/checks-403"),
            ["RAL-147"]
        );
        assert_eq!(
            stories_in("RAL-157 part 1: `rally brief --watchman`"),
            ["RAL-157"]
        );
        assert_eq!(
            stories_in("general rally fixes for RAL-9 and ral-9"),
            ["RAL-9"]
        );
        assert!(stories_in("Merge remote-tracking branch 'origin/main'").is_empty());
    }

    /// RAL-167: the section says what's live, counts the stories merged and not live with the
    /// oldest's age, and hints at a staged build when this CLI is ahead of the service.
    #[test]
    fn the_section_counts_whats_merged_and_not_live() {
        let hour = 3_600_000;
        let now = iso_ms("2026-11-14T04:00:00Z").unwrap();
        let behind = json!({"status": "ahead", "ahead_by": 4, "commits": [
            {"sha": "a".repeat(40), "message": "RAL-148: digest\n\nbody", "date": "2026-11-13T21:00:00Z"},
            {"sha": "b".repeat(40), "message": "Merge pull request #232 from o/ral-148-x", "date": "2026-11-13T22:00:00Z"},
            {"sha": "c".repeat(40), "message": "RAL-149: late checks", "date": "2026-11-14T01:00:00Z"},
            {"sha": "d".repeat(40), "message": "Merge remote-tracking branch 'origin/main'", "date": "2026-11-14T02:00:00Z"},
        ]});
        let out = section(
            Some(&"0".repeat(40)),
            Some(&"9".repeat(40)),
            Ok("d".repeat(40)),
            Some(Ok(behind)),
            Some(Ok(json!({"ahead_by": 2}))),
            now,
        );
        let merged = &out["merged_not_live"];
        assert_eq!(merged["count"], 2);
        assert_eq!(merged["commits"], 4);
        assert!(merged["reads_short"].is_null(), "all four were listed");
        assert_eq!(merged["stories"][0]["story"], "RAL-148");
        assert_eq!(merged["stories"][0]["age_ms"], 7 * hour);
        assert_eq!(merged["oldest_age_ms"], 7 * hour);
        assert_eq!(
            merged["summary"],
            "2 stories merged, not live; the oldest for 7 h"
        );
        assert!(
            out["hint"]
                .as_str()
                .unwrap()
                .starts_with("this CLI (9999999) is ahead")
        );

        // Further behind than GitHub lists: said, not hidden.
        let long = json!({"status": "ahead", "ahead_by": 300, "commits": [
            {"sha": "a".repeat(40), "message": "RAL-1: x", "date": "2026-11-13T21:00:00Z"},
        ]});
        let far = section(Some("0"), None, Ok("d".into()), Some(Ok(long)), None, now);
        assert_eq!(
            far["merged_not_live"]["reads_short"],
            "the service is 300 commits behind main, but GitHub's compare lists only 1: the \
             stories and the oldest age here read short"
        );

        // A service older than RAL-156 says nothing, and that's said.
        let old = section(None, Some("9"), Ok("d".into()), None, None, now);
        assert!(
            old["live"]["note"]
                .as_str()
                .unwrap()
                .contains("doesn't say")
        );
        assert!(old.get("merged_not_live").is_none() && old.get("hint").is_none());
        // Live at main's tip.
        let even = json!({"status": "identical", "ahead_by": 0, "commits": []});
        let level = section(
            Some("d"),
            Some("d"),
            Ok("d".into()),
            Some(Ok(even)),
            Some(Ok(json!({"ahead_by": 0}))),
            now,
        );
        assert_eq!(
            level["merged_not_live"]["summary"],
            "live is main's tip: nothing merged waits to go live"
        );
        assert!(level.get("hint").is_none());
    }
}
