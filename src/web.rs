//! The read-only viewer (RAL-25, RAL-27, RAL-29): a page on this machine's loopback address.
//!
//! It holds no write path: it reads the ledger with a read-only credential or from a file, roadmap
//! files through `gh`, and the roster through `muster-shim roster`. Those are read by a background
//! thread, so requests never wait on them and a page can always show how old its data is.
//!
//! A web page in the same browser is the threat here. Requests must name this address in `Host`,
//! which stops DNS rebinding, and any `Origin` must be this page's own. A request the browser marks
//! as from another site is refused. Nothing sends CORS headers, and the content security policy
//! allows no inline script. Only a signed-in browser is served (RAL-29). The long-lived key stays
//! in a private file and never goes in a URL: `rally web --login` sends it in a header to ask the
//! running viewer for a one-time code. Opening `/?code=<code>` shows a "Sign in" button, and only
//! the button's POST spends the code, which works once, within a minute (RAL-73). It becomes a
//! random session cookie, HttpOnly and SameSite=Strict. A session holds only while the key it was
//! made under is the one in the file, so `rally web --new-key` signs every browser out. Sessions are
//! kept, as hashes, in a private file beside the key, so a restart signs no one out (RAL-80). See
//! `docs/web.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::net::{SocketAddr, TcpListener};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::views::UNTRUSTED_TEXT;

const INDEX: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const APP_CSS: &str = include_str!("../web/app.css");
const SIGN_IN: &str = include_str!("../web/sign-in.html");

/// The projects shown by default.
pub const DEFAULT_PROJECTS: &str = include_str!("../web/projects.json");

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; \
                   base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
/// The sign-in page's policy: the same, except that its one form may post back here.
const SIGN_IN_CSP: &str =
    "default-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";
const LIVE_EVERY: Duration = Duration::from_secs(30);
const ROADMAPS_EVERY: Duration = Duration::from_secs(300);
const COMMAND_LIMIT: Duration = Duration::from_secs(20);

/// A project's roadmap, and what its ticks mean there.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub name: String,
    pub prefix: String,
    pub repo: String,
    pub path: String,
    pub tick_meaning: String,
    /// The Rally team that works this roadmap, for `rally wake` (RAL-54).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    /// That team's sessions on Muster's roster: Muster project names or session nicknames, listed
    /// explicitly, since a session's project doesn't always name its team.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<String>,
}

/// Counts of checkboxes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Boxes {
    pub ticked: u32,
    pub total: u32,
    pub percent: u32,
}

/// The checkboxes under one heading.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Section {
    pub name: String,
    pub ticked: u32,
    pub total: u32,
}

/// A fence line's character, its length and what follows it: three or more backticks or tildes,
/// indented by at most three spaces. A backtick fence's info string can't hold a backtick.
fn fence_marker(line: &str) -> Option<(char, usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let kind = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let length = rest.len() - rest.trim_start_matches(kind).len();
    if length < 3 {
        return None;
    }
    let after = &rest[length..];
    if kind == '`' && after.contains('`') {
        return None;
    }
    Some((kind, length, after))
}

/// Every checkbox line outside code fences: the `##` heading it's under, whether it's ticked, the
/// text after the box, and its line number from 1. The counts and the story list both read lines
/// through this.
fn roadmap_items(text: &str) -> Vec<(String, bool, String, usize)> {
    let mut items = Vec::new();
    let mut current = "Before the first heading".to_owned();
    // The open fence's character and length, as CommonMark has them.
    let mut fence: Option<(char, usize)> = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        let marker = fence_marker(line);
        match (fence, marker) {
            (None, Some((kind, length, _))) => {
                fence = Some((kind, length));
                continue;
            }
            (Some((open, open_length)), Some((kind, length, rest)))
                if kind == open && length >= open_length && rest.trim().is_empty() =>
            {
                fence = None;
                continue;
            }
            (Some(_), _) => continue,
            (None, None) => {}
        }
        if let Some(heading) = trimmed.strip_prefix("## ") {
            current = heading.trim().to_owned();
            continue;
        }
        let item = trimmed
            .strip_prefix("- [")
            .or_else(|| trimmed.strip_prefix("* ["));
        let ticked = match item.and_then(|rest| rest.get(..2)) {
            Some("x]" | "X]") => true,
            Some(" ]") => false,
            _ => continue,
        };
        let after = item.and_then(|rest| rest.get(2..)).unwrap_or("").trim();
        items.push((current.clone(), ticked, after.to_owned(), index + 1));
    }
    items
}

/// Counts a roadmap's checkboxes overall and by `##` heading, ignoring examples in code fences.
pub fn parse_roadmap(text: &str) -> (Boxes, Vec<Section>) {
    let mut sections: Vec<Section> = Vec::new();
    for (name, ticked, _, _) in roadmap_items(text) {
        if sections.last().is_none_or(|section| section.name != name) {
            sections.push(Section {
                name,
                ticked: 0,
                total: 0,
            });
        }
        let section = sections.last_mut().expect("just added");
        section.total += 1;
        section.ticked += u32::from(ticked);
    }
    let total: u32 = sections.iter().map(|section| section.total).sum();
    let ticked: u32 = sections.iter().map(|section| section.ticked).sum();
    // Round down, so an unfinished roadmap never shows as 100%.
    let percent = (ticked * 100).checked_div(total).unwrap_or(0);
    (
        Boxes {
            ticked,
            total,
            percent,
        },
        sections,
    )
}

/// One story on a roadmap, for a project's page (RAL-43). The title is the roadmap's own words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Story {
    pub id: Option<String>,
    pub title: String,
    pub ticked: bool,
    pub section: String,
    /// The roadmap line, from 1, for matching to blame. Not shown.
    #[serde(skip)]
    pub line: usize,
    /// For a ticked story, the date (`YYYY-MM-DD`) of the commit that last changed its line: the
    /// tick's date, unless the line was edited after it (RAL-44).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_changed: Option<String>,
}

const MAX_TITLE_CHARS: usize = 200;

/// The stories on a roadmap, the same boxes `parse_roadmap` counts. A line written as
/// `**PFX-1: Title.** …` gives the id and title; anything else keeps its opening words as the title.
pub fn parse_stories(text: &str, prefix: &str) -> Vec<Story> {
    roadmap_items(text)
        .into_iter()
        .map(|(section, ticked, after, line)| {
            let lead = after
                .strip_prefix("**")
                .and_then(|rest| rest.split_once("**"))
                .map_or(after.as_str(), |(bold, _)| bold)
                .trim();
            let (id, title) = match lead.split_once(':') {
                Some((id, title))
                    if id
                        .strip_prefix(prefix)
                        .and_then(|n| n.strip_prefix('-'))
                        .is_some_and(|n| {
                            !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())
                        }) =>
                {
                    (Some(id.to_owned()), title.trim().trim_end_matches('.'))
                }
                _ => (None, lead),
            };
            Story {
                id,
                title: title.chars().take(MAX_TITLE_CHARS).collect(),
                ticked,
                section,
                line,
                last_changed: None,
            }
        })
        .collect()
}

/// GitHub's blame of a roadmap, as `(first line, last line, date)` for each range, from the
/// `ranges` of a GraphQL `blame`. Keeps only the lines and the date of the commit, as `YYYY-MM-DD`;
/// a range whose numbers or date don't parse is dropped, and so is everything else, including the
/// commit message, which an agent wrote.
pub fn shape_blame(value: &Value) -> Vec<(u64, u64, String)> {
    let Some(ranges) = value.as_array() else {
        return Vec::new();
    };
    ranges
        .iter()
        .filter_map(|range| {
            let first = range["startingLine"].as_u64()?;
            let last = range["endingLine"].as_u64()?;
            let date = range["commit"]["committedDate"].as_str()?.get(..10)?;
            let bytes = date.as_bytes();
            let is_date = bytes.len() == 10
                && bytes[4] == b'-'
                && bytes[7] == b'-'
                && [0, 1, 2, 3, 5, 6, 8, 9]
                    .iter()
                    .all(|&i| bytes[i].is_ascii_digit());
            (first >= 1 && first <= last && is_date).then(|| (first, last, date.to_owned()))
        })
        .collect()
}

/// Gives each ticked story the date of the blame range holding its line.
pub fn date_stories(stories: &mut [Story], ranges: &[(u64, u64, String)]) {
    for story in stories.iter_mut().filter(|story| story.ticked) {
        let line = story.line as u64;
        story.last_changed = ranges
            .iter()
            .find(|(first, last, _)| (*first..=*last).contains(&line))
            .map(|(_, _, date)| date.clone());
    }
}

/// Whether a `docs/muster` file name is a day summary: exactly `YYYY-MM-DD.md`.
fn is_day_file(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 13
        && name.ends_with(".md")
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && [0, 1, 2, 3, 5, 6, 8, 9]
            .iter()
            .all(|&i| bytes[i].is_ascii_digit())
}

/// The newest day summary in a `docs/muster` listing, ignoring every other name.
pub fn latest_summary(names: &[String]) -> Option<String> {
    names.iter().filter(|name| is_day_file(name)).max().cloned()
}

const MAX_PR_TITLE_CHARS: usize = 300;

/// A project's recently merged PRs from `gh pr list --json number,title,mergedAt`, newest first,
/// keeping only those three fields. Titles are agent-written, so the page shows them as untrusted text.
pub fn shape_merged(value: &Value) -> Value {
    let mut rows: Vec<Value> = value
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|pr| {
                    Some(json!({
                        "number": pr["number"].as_u64()?,
                        "title": pr["title"].as_str()?.chars().take(MAX_PR_TITLE_CHARS).collect::<String>(),
                        "merged_at": pr["mergedAt"].as_str()?,
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    rows.sort_by(|a, b| b["merged_at"].as_str().cmp(&a["merged_at"].as_str()));
    json!(rows)
}

/// The most blockers [`progress`] lists as holding up a roadmap's stories.
pub const TOP_BLOCKERS: usize = 10;

/// One roadmap's progress for the watchman's planning (RAL-23), from its roadmap reading and the
/// ledger reading. It never compares roadmaps or teams. When the roadmap or the ledger hasn't been
/// read, that part says so and why, rather than showing zeros.
pub fn progress(prefix: &str, roadmap: &Value, ledger: &Value) -> Value {
    let wanted = format!("{prefix}-");
    let mut value = json!({});
    let roadmap_stories = roadmap["stories"].as_array();
    value["roadmap"] = match roadmap_stories {
        None => json!({
            "available": false,
            "reason": roadmap["error"].as_str().unwrap_or("still reading it."),
        }),
        Some(_) => {
            let mut part = json!({"available": true, "read_at_ms": roadmap["read_at_ms"]});
            if let Some(error) = roadmap["error"].as_str() {
                // An older reading is kept after a failed one, so its age shows.
                part["last_read_failed"] = json!(error);
            }
            part
        }
    };
    if ledger["available"] != json!(true) || ledger["stories"].as_object().is_none() {
        value["ledger"] = json!({
            "available": false,
            "reason": ledger["reason"].as_str().unwrap_or("the ledger isn't readable, or failed its check."),
        });
        return value;
    }
    value["ledger"] = json!({"available": true, "as_of": ledger["as_of"]});
    let tracked: Vec<(&String, &Value)> = ledger["stories"]
        .as_object()
        .map(|stories| {
            stories
                .iter()
                .filter(|(id, _)| id.starts_with(&wanted))
                .collect()
        })
        .unwrap_or_default();

    let mut states = serde_json::Map::new();
    for state in ["ready", "claimed", "in_review", "verified", "sent_back"] {
        states.insert(state.into(), json!(0));
    }
    for (_, story) in &tracked {
        if let Some(state) = story["state"].as_str() {
            let count = states.get(state).and_then(Value::as_u64).unwrap_or(0);
            states.insert(state.into(), json!(count + 1));
        }
    }
    // A roadmap read with no checkboxes, such as DiscoSim's table, has no ticks to count, so its
    // progress comes from the ledger's states instead (RAL-81): Verified of the stories the ledger
    // tracks under this prefix. Ticks and Verified mean different things, so it's labelled as the
    // ledger's, and only stands in where there are no boxes at all.
    let no_boxes = roadmap_stories.is_some() && roadmap["boxes"]["total"].as_u64() == Some(0);
    if no_boxes && !tracked.is_empty() {
        let verified = states.get("verified").and_then(Value::as_u64).unwrap_or(0);
        let total = tracked.len() as u64;
        value["from_ledger"] = json!({
            "kind": "verified", "as_of": ledger["as_of"],
            "verified": verified, "tracked": total, "percent": verified * 100 / total,
        });
    }
    value["states"] = Value::Object(states);
    value["tracked"] = json!(tracked.len());

    // Open blockers, with the chains the ledger's view worked out.
    let blockers: Vec<&Value> = ledger["blockers"]
        .as_array()
        .map(|b| b.iter().collect())
        .unwrap_or_default();
    let blocked: std::collections::BTreeSet<&str> = blockers
        .iter()
        .filter_map(|blocker| blocker["story"].as_str())
        .collect();
    let ready_unblocked: Vec<&String> = tracked
        .iter()
        .filter(|(id, story)| story["state"] == "ready" && !blocked.contains(id.as_str()))
        .map(|(id, _)| *id)
        .collect();
    value["ready_unblocked"] = json!(ready_unblocked);

    // A blocker holds up a story when the story is the one it blocks, or the one blocked by any
    // blocker whose chain reaches it.
    let mut holds: std::collections::BTreeMap<u64, std::collections::BTreeSet<String>> =
        std::collections::BTreeMap::new();
    for blocker in &blockers {
        let Some(story) = blocker["story"].as_str().filter(|s| s.starts_with(&wanted)) else {
            continue;
        };
        let chain = blocker["chain"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64);
        for seq in blocker["seq"].as_u64().into_iter().chain(chain) {
            holds.entry(seq).or_default().insert(story.to_owned());
        }
    }
    let mut held: Vec<(u64, std::collections::BTreeSet<String>)> = holds.into_iter().collect();
    // Most stories first; ties in record order.
    held.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    let top: Vec<Value> = held
        .into_iter()
        .take(TOP_BLOCKERS)
        .map(|(seq, stories)| {
            let blocker = blockers.iter().find(|b| b["seq"].as_u64() == Some(seq));
            json!({
                "blocker": seq, "holds_up": stories,
                "team": blocker.map(|b| b["team"].clone()), "note": blocker.map(|b| b["note"].clone()),
            })
        })
        .collect();
    value["top_blockers"] = json!(top);
    if ledger["blockers_truncated"] == json!(true) {
        value["blockers_truncated"] = json!(true);
    }

    // Ticks the ledger hasn't verified: absent from the ledger, or there but not Verified.
    if let Some(stories) = roadmap_stories {
        let unverified: Vec<Value> = stories
            .iter()
            .filter(|story| story["ticked"] == json!(true))
            .filter_map(|story| {
                let id = story["id"].as_str()?;
                let state = ledger["stories"][id]["state"].as_str();
                (state != Some("verified")).then(|| {
                    json!({
                        "id": id, "title": story["title"], "last_changed": story["last_changed"],
                        "ledger_state": state,
                    })
                })
            })
            .collect();
        value["ticks_without_verified"] = json!(unverified);
    }
    value
}

/// The verified figure for a project's stories. It comes only from the ledger, never from ticks.
pub fn verified_for(prefix: &str, ledger: &Value) -> Value {
    let wanted = format!("{prefix}-");
    let stories = ledger["stories"].as_object();
    let matching: Vec<&Value> = stories
        .map(|stories| {
            stories
                .iter()
                .filter(|(id, _)| id.starts_with(&wanted))
                .map(|(_, story)| story)
                .collect()
        })
        .unwrap_or_default();
    if matching.is_empty() {
        return json!({"available": false});
    }
    let verified = matching
        .iter()
        .filter(|story| story["state"] == "verified")
        .count();
    json!({"available": true, "verified": verified, "tracked": matching.len()})
}

/// The story states in which a team still holds a story: it claimed it and hasn't been verified.
const HELD_STATES: [&str; 3] = ["claimed", "in_review", "sent_back"];

/// Which projects are in the current rally (RAL-69): those with a story held by a team that's on
/// Muster's roster now. A team is on the roster when a session's nickname is the team's name, or a
/// session matches the team's `sessions` in the project list, as `rally wake` matches them; a team's
/// name is never guessed from a session's project. Nothing is hidden from the ledger itself: this
/// only decides what the overview shows. Without a roster or a ledger it can't tell, and says so,
/// and then the viewer shows everything.
pub fn current_rally(projects: &[Project], ledger: &Value, roster: &Value) -> Value {
    let mut value = match ledger["rallies"]["open"].as_object() {
        Some(_) => opened_rally(&ledger["rallies"]["open"], roster),
        None => derived_rally(projects, ledger, roster),
    };
    // Past rallies, newest first, each kept whole and shown as the ledger stood when it closed.
    value["past"] = json!(
        ledger["rallies"]["rallies"]
            .as_array()
            .map(|all| all
                .iter()
                .filter(|rally| rally["closed"].is_object())
                .map(|rally| json!({
                    "name": rally["name"], "projects": rally["projects"],
                    "opened_seq": rally["seq"], "opened_at_ms": rally["at_ms"],
                    "closed_seq": rally["closed"]["seq"], "closed_at_ms": rally["closed"]["at_ms"],
                    "note": rally["closed"]["note"],
                }))
                .collect::<Vec<_>>())
            .unwrap_or_default()
    );
    // An older service is said plainly; any other failure is a warning, since an open rally may
    // be missing from the board (RAL-77).
    if ledger["rallies"]["available"] == json!(false) {
        let reason = ledger["rallies"]["reason"].clone();
        if ledger["rallies"]["older_service"] == json!(true) {
            value["rallies_unavailable"] = reason;
        } else {
            value["rallies_error"] = reason;
        }
    }
    value
}

/// The board of a rally someone opened (RAL-76): its projects and teams as declared, and only what
/// was recorded since it opened. A listed team also takes in the ledger team of each listed
/// session's Muster project, so `rally` comes with the session working on it.
fn opened_rally(open: &Value, roster: &Value) -> Value {
    let fold = |name: &str| name.trim().to_lowercase();
    let listed: Vec<String> = open["teams"]
        .as_array()
        .map(|teams| teams.iter().filter_map(Value::as_str).map(fold).collect())
        .unwrap_or_default();
    let mut teams: BTreeSet<String> = listed.iter().cloned().collect();
    if let Some(sessions) = roster["sessions"].as_array() {
        for session in sessions {
            let nickname = session["nickname"].as_str().map(fold);
            if nickname.is_some_and(|n| listed.contains(&n))
                && let Some(project) = session["project"].as_str()
            {
                teams.insert(fold(project));
            }
        }
    }
    json!({
        "known": true,
        "kind": "declared",
        "opened": true,
        "name": open["name"],
        "since_seq": open["seq"],
        "opened_at_ms": open["at_ms"],
        "opened_by": open["source"]["declared_name"],
        "prefixes": open["projects"],
        "teams": open["teams"],
        "teams_listed": !listed.is_empty(),
        "active_teams": teams,
        "rule": "This rally's board: its projects and teams as declared when it opened, and only \
                 what was recorded since. Earlier work is kept in the ledger and under past rallies.",
    })
}

/// What an opened rally's board shows (RAL-76): in its projects, each story recorded since it
/// opened, and each story still held (claimed, in review or sent back) whenever that was, so work
/// in flight when the rally opens stays on it; and each declared team the rally takes in, whatever
/// the age of its last record.
pub fn board(rally: &Value, ledger: &Value) -> Value {
    let since = rally["since_seq"].as_u64().unwrap_or(0);
    let prefixes: Vec<&str> = rally["prefixes"]
        .as_array()
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let stories: Vec<&String> = ledger["stories"]
        .as_object()
        .map(|stories| {
            stories
                .iter()
                .filter(|(id, story)| {
                    let prefix = id.split_once('-').map(|(p, _)| p).unwrap_or("");
                    let held = HELD_STATES.contains(&story["state"].as_str().unwrap_or(""));
                    prefixes.contains(&prefix)
                        && (held || story["last"].as_u64().is_some_and(|last| last > since))
                })
                .map(|(id, _)| id)
                .collect()
        })
        .unwrap_or_default();
    let active = rally["active_teams"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let teams: Vec<&String> = ledger["teams"]
        .as_object()
        .map(|teams| {
            teams
                .keys()
                .filter(|name| {
                    active
                        .iter()
                        .any(|a| a.as_str() == Some(&name.trim().to_lowercase()))
                })
                .collect()
        })
        .unwrap_or_default();
    json!({"stories": stories, "teams": teams})
}

/// With no rally open, RAL-69's reading of what's in play from Muster's roster.
fn derived_rally(projects: &[Project], ledger: &Value, roster: &Value) -> Value {
    if roster["available"] != json!(true) {
        return json!({"known": false, "reason": "Muster's roster couldn't be read, so every project is shown."});
    }
    let Some(stories) = ledger["stories"]
        .as_object()
        .filter(|_| ledger["available"] == json!(true))
    else {
        return json!({"known": false, "reason": "the ledger couldn't be read, so every project is shown."});
    };
    let fold = |name: &str| name.trim().to_lowercase();
    let sessions = roster["sessions"].as_array().cloned().unwrap_or_default();
    let on_roster = |team: &str| {
        let team = fold(team);
        sessions.iter().any(|session| {
            let nickname = session["nickname"].as_str().map(fold);
            let project = session["project"].as_str().map(fold);
            nickname.as_deref() == Some(team.as_str())
                || projects
                    .iter()
                    .filter(|p| p.team.as_deref().map(fold).as_deref() == Some(team.as_str()))
                    .flat_map(|p| p.sessions.iter().map(|name| fold(name)))
                    .any(|name| Some(&name) == nickname.as_ref() || Some(&name) == project.as_ref())
        })
    };
    let mut active: BTreeSet<String> = BTreeSet::new();
    let mut prefixes: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let teams = ledger["teams"]
        .as_object()
        .into_iter()
        .flat_map(|t| t.keys());
    let holders = stories.values().filter_map(|story| story["team"].as_str());
    for team in teams.map(String::as_str).chain(holders) {
        if on_roster(team) {
            active.insert(fold(team));
        }
    }
    for (id, story) in stories {
        let held = HELD_STATES.contains(&story["state"].as_str().unwrap_or(""));
        let Some(team) = story["team"].as_str().filter(|_| held) else {
            continue;
        };
        let Some((prefix, _)) = id.split_once('-') else {
            continue;
        };
        if active.contains(&fold(team)) {
            prefixes
                .entry(prefix.to_owned())
                .or_default()
                .push(json!({"story": id, "team": team, "state": story["state"]}));
        }
    }
    json!({
        "known": true,
        "kind": "observed",
        "prefixes": prefixes.keys().collect::<Vec<_>>(),
        "held": prefixes,
        "active_teams": active,
        "rule": "A project is in this rally when a team on Muster's roster now holds one of its stories \
                 (claimed, in review or sent back). Other projects keep their records and pages.",
    })
}

/// Whether the project with `prefix` is in the current rally, by [`current_rally`]'s reading. When
/// that couldn't be told, every project is.
pub fn in_rally(rally: &Value, prefix: &str) -> bool {
    rally["known"] != json!(true)
        || rally["prefixes"]
            .as_array()
            .is_some_and(|list| list.iter().any(|p| p == prefix))
}

/// Where the viewer reads the ledger.
pub type LedgerReader = Box<dyn Fn() -> Result<Value, String> + Send>;

/// Where the viewer reads the ledger as it stood at a past record, with the corrections made to
/// it since (RAL-28).
pub type PastReader = Box<dyn Fn(u64) -> Result<Value, String> + Send>;

#[derive(Default)]
struct Snapshot {
    /// Each project's latest roadmap reading, kept after a failed read so that its age shows.
    roadmaps: Vec<Value>,
    /// The watchman's day writeup for the whole muster, from Rally's own repository (RAL-43).
    muster_summary: Option<Value>,
    ledger: Option<Value>,
    roster: Option<Value>,
    backup: Option<Value>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Runs a command, giving up after `limit`, so that a hung program can't freeze the page's data.
/// Returns its standard output if it succeeded.
/// The most output a command may give: a roadmap or the roster, with plenty of room.
const MAX_COMMAND_OUTPUT: u64 = 8 * 1024 * 1024;

fn run_with_timeout(mut command: Command, limit: Duration) -> Result<Vec<u8>, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("can't run it: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no output")?;
    // Read on another thread, so that a full pipe can't block the child while we wait for it.
    let (sender, output) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        // One byte past the limit says it was exceeded; the rest is never read, and the child
        // is killed below if that leaves it blocked.
        let read = (&mut stdout)
            .take(MAX_COMMAND_OUTPUT + 1)
            .read_to_end(&mut bytes);
        let _ = sender.send(read.map(|_| bytes));
    });
    let started = Instant::now();
    // Output the reader finished with before the child was seen to exit.
    let mut early: Option<std::io::Result<Vec<u8>>> = None;
    let collect = |early: &mut Option<std::io::Result<Vec<u8>>>, wait: Duration| {
        let read = match early.take() {
            Some(read) => read,
            None => output
                .recv_timeout(wait)
                .map_err(|_| "its output didn't finish".to_owned())?,
        };
        read.map_err(|e| e.to_string()).and_then(too_much)
    };
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) if status.success() => {
                // Something it left running could hold its output open, so this needs a limit too.
                let left = limit.saturating_sub(started.elapsed());
                return collect(&mut early, left.max(Duration::from_millis(100)));
            }
            Some(_) => {
                // Stopping at the limit ends a command still writing with a broken pipe, which it
                // reports as a failure; say what really happened.
                collect(&mut early, Duration::from_millis(100))?;
                return Err("it reported a failure".to_owned());
            }
            None if started.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("it didn't finish within {}s", limit.as_secs()));
            }
            None => {
                if early.is_none()
                    && let Ok(read) = output.try_recv()
                {
                    early = Some(read);
                }
                // Past the limit the reader stops, and a child still writing would block on a
                // full pipe until its time ran out, so end it now.
                if early.as_ref().is_some_and(|read| {
                    read.as_ref()
                        .is_ok_and(|bytes| bytes.len() as u64 > MAX_COMMAND_OUTPUT)
                }) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return collect(&mut early, Duration::ZERO);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Refuses output over [`MAX_COMMAND_OUTPUT`].
fn too_much(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if bytes.len() as u64 > MAX_COMMAND_OUTPUT {
        Err(format!(
            "it printed too much, over {} MiB",
            MAX_COMMAND_OUTPUT / 1024 / 1024
        ))
    } else {
        Ok(bytes)
    }
}

fn run_gh(args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("gh");
    command.args(args);
    let output = run_with_timeout(command, COMMAND_LIMIT).map_err(|e| format!("gh: {e}"))?;
    String::from_utf8(output).map_err(|_| "gh returned something unreadable".to_owned())
}

/// The merged pull request that contains `commit` in `repo`, as its head at merge time and its
/// merge or squash commit (RAL-8, RAL-67, for RAL-31's comparison), read-only through `gh`. `None`
/// when no merged pull request contains it. `repo` must already be checked as `owner/repo`
/// (`evidence::parse`); `commit` must be hex.
pub fn pr_head(repo: &str, commit: &str) -> Result<Option<(String, String)>, String> {
    if commit.is_empty() || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("the merged commit isn't a hash".into());
    }
    let out = run_gh(&[
        "api",
        &format!("repos/{repo}/commits/{commit}/pulls"),
        "--jq",
        "[.[] | select(.merged_at != null)][0] | select(. != null) | .head.sha + \" \" + .merge_commit_sha",
    ])?;
    let line = out.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let hex = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit());
    match line.split_once(' ') {
        Some((head, merge)) if hex(head) && hex(merge) => {
            Ok(Some((head.to_owned(), merge.to_owned())))
        }
        _ => Err("gh returned something that isn't two commits".into()),
    }
}

/// A project's roadmap from GitHub, as `(commit, text)`.
pub fn read_roadmap(project: &Project) -> Result<(String, String), String> {
    let commit = run_gh(&[
        "api",
        &format!("repos/{}/commits/HEAD", project.repo),
        "--jq",
        ".sha",
    ])?;
    let commit = commit.trim().to_owned();
    let contents = format!(
        "repos/{}/contents/{}?ref={commit}",
        project.repo, project.path
    );
    let text = run_gh(&["api", &contents, "-H", "Accept: application/vnd.github.raw"])?;
    Ok((commit, text))
}

/// Rally's own repository, which also holds the watchman's writeup of the whole muster, in
/// [`WATCHMAN_FOLDER`], apart from Rally's own day summaries.
pub const RALLY_REPO: &str = "Castor-o7/rally";

/// Where each team keeps its day summaries, as `YYYY-MM-DD.md`.
const SUMMARY_FOLDER: &str = "docs/muster";

/// Where the watchman keeps its writeups of the whole muster, in Rally's repository.
const WATCHMAN_FOLDER: &str = "docs/muster/watchman";

/// Lists a folder at `commit` through GraphQL, which answers a missing folder with nothing rather than
/// an error, so "no summaries yet" isn't mistaken for a failed read.
const LIST_FOLDER: &str = "query($owner:String!,$name:String!,$expr:String!){repository(owner:$owner,name:$name)\
{object(expression:$expr){... on Tree{entries{name}}}}}";

/// The newest `<folder>/YYYY-MM-DD.md` in a repository at `commit`, as text with its file and
/// commit, or `{"missing": true}` when there's none. Read with the same caps as a roadmap.
fn read_summary(repo: &str, commit: &str, folder: &str) -> Result<Value, String> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or("the repository isn't owner/name")?;
    let names = run_gh(&[
        "api",
        "graphql",
        "-F",
        &format!("owner={owner}"),
        "-F",
        &format!("name={name}"),
        "-F",
        &format!("expr={commit}:{folder}"),
        "-f",
        &format!("query={LIST_FOLDER}"),
        "--jq",
        ".data.repository.object.entries[]?.name",
    ])?;
    let names: Vec<String> = names.lines().map(str::to_owned).collect();
    let Some(name) = latest_summary(&names) else {
        return Ok(json!({"missing": true, "read_at_ms": now_ms()}));
    };
    let file = format!("{folder}/{name}");
    let contents = format!("repos/{repo}/contents/{file}?ref={commit}");
    let text = run_gh(&["api", &contents, "-H", "Accept: application/vnd.github.raw"])?;
    Ok(json!({"file": file, "commit": commit, "text": text, "read_at_ms": now_ms()}))
}

/// Blame of a file at `commit`, the commit the roadmap was read at, so its lines match.
const BLAME: &str = "query($owner:String!,$name:String!,$expr:String!,$path:String!){repository(owner:$owner,name:$name)\
{object(expression:$expr){... on Commit{blame(path:$path){ranges{startingLine endingLine commit{committedDate}}}}}}}";

/// A roadmap's blame at `commit`, shaped by `shape_blame`: one call per project (RAL-44).
fn read_blame(repo: &str, commit: &str, path: &str) -> Result<Value, String> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or("the repository isn't owner/name")?;
    let output = run_gh(&[
        "api",
        "graphql",
        "-F",
        &format!("owner={owner}"),
        "-F",
        &format!("name={name}"),
        "-F",
        &format!("expr={commit}"),
        "-F",
        &format!("path={path}"),
        "-f",
        &format!("query={BLAME}"),
        "--jq",
        ".data.repository.object.blame.ranges",
    ])?;
    let value: Value =
        serde_json::from_str(&output).map_err(|_| "gh returned something unreadable".to_owned())?;
    let ranges: Vec<Value> = shape_blame(&value)
        .into_iter()
        .map(|(first, last, date)| json!([first, last, date]))
        .collect();
    Ok(json!({"commit": commit, "ranges": ranges, "read_at_ms": now_ms()}))
}

/// A blame value's ranges, back in `shape_blame`'s form.
fn blame_ranges(blame: &Value) -> Vec<(u64, u64, String)> {
    blame["ranges"]
        .as_array()
        .map(|ranges| {
            ranges
                .iter()
                .filter_map(|range| {
                    Some((
                        range[0].as_u64()?,
                        range[1].as_u64()?,
                        range[2].as_str()?.to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A project's last 20 merged PRs, shaped by `shape_merged`.
fn read_merged(repo: &str) -> Result<Value, String> {
    let output = run_gh(&[
        "pr",
        "list",
        "-R",
        repo,
        "--state",
        "merged",
        "--limit",
        "20",
        "--json",
        "number,title,mergedAt",
    ])?;
    let value: Value =
        serde_json::from_str(&output).map_err(|_| "gh returned something unreadable".to_owned())?;
    Ok(json!({"prs": shape_merged(&value), "read_at_ms": now_ms()}))
}

/// The watchman's writeup, read with Rally's own entry in the project list, so the viewer makes no
/// GitHub call that isn't for a listed project. It's never Rally's own day summary.
fn muster_summary(projects: &[Project], roadmaps: &[Value]) -> Value {
    let Some(index) = projects
        .iter()
        .position(|project| project.repo == RALLY_REPO)
    else {
        return json!({"reason": format!("{RALLY_REPO} isn't in the project list.")});
    };
    roadmaps
        .get(index)
        .and_then(|row| row.get("watchman_summary"))
        .cloned()
        .unwrap_or_else(|| json!({"reason": "The watchman's writeup hasn't been read yet."}))
}

/// Reads every project's roadmap, day summary and merged PRs one after another, each `gh` call with its
/// own 20-second limit. With a handful of projects that fits the five-minute cadence easily; if gh
/// is slow for many projects at once, one refresh can run longer than that, and the page shows the age.
fn refresh_roadmaps(projects: &[Project], previous: &[Value]) -> Vec<Value> {
    projects
        .iter()
        .enumerate()
        .map(|(index, project)| match read_roadmap(project) {
            Ok((commit, text)) => {
                let (boxes, sections) = parse_roadmap(&text);
                let kept = |field: &str, error: String| {
                    let mut value = previous
                        .get(index)
                        .and_then(|row| row.get(field))
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    value["error"] = json!(error);
                    value
                };
                let summary = read_summary(&project.repo, &commit, SUMMARY_FOLDER)
                    .unwrap_or_else(|error| kept("summary", error));
                let watchman_summary = (project.repo == RALLY_REPO).then(|| {
                    read_summary(&project.repo, &commit, WATCHMAN_FOLDER)
                        .unwrap_or_else(|error| kept("watchman_summary", error))
                });
                let merged =
                    read_merged(&project.repo).unwrap_or_else(|error| kept("merged", error));
                let blame = read_blame(&project.repo, &commit, &project.path)
                    .unwrap_or_else(|error| kept("blame", error));
                let mut stories = parse_stories(&text, &project.prefix);
                // A kept blame from another commit would date the wrong lines.
                if blame["commit"].as_str() == Some(commit.as_str()) {
                    date_stories(&mut stories, &blame_ranges(&blame));
                }
                let mut row = json!({
                    "boxes": boxes,
                    "sections": sections,
                    "stories": stories,
                    "blame": blame,
                    "summary": summary,
                    "merged": merged,
                    "commit": commit,
                    "read_at_ms": now_ms(),
                });
                if let Some(watchman_summary) = watchman_summary {
                    row["watchman_summary"] = watchman_summary;
                }
                row
            }
            Err(error) => {
                let mut kept = previous.get(index).cloned().unwrap_or_else(|| json!({}));
                kept["error"] = json!(error);
                kept
            }
        })
        .collect()
}

/// Reads the roster, keeping only fields the page shows and Muster's own labels for them.
fn read_roster() -> Value {
    let unavailable = |reason: &str| json!({"available": false, "reason": reason});
    let mut command = Command::new("muster-shim");
    command.arg("roster");
    let output = match run_with_timeout(command, COMMAND_LIMIT) {
        Ok(output) => output,
        Err(error) => {
            return unavailable(&format!(
                "muster-shim couldn't read the roster ({error}). This machine may need the \
                 rollout of Muster's MUS-44."
            ));
        }
    };
    match serde_json::from_slice::<Value>(&output) {
        Ok(value) => shape_roster(&value, now_ms()),
        Err(_) => unavailable("muster-shim's roster couldn't be read."),
    }
}

/// Keeps only the fields the page shows, in Muster's categories. A later format that drops a field
/// leaves it blank rather than guessed.
pub fn shape_roster(value: &Value, read_at_ms: u64) -> Value {
    let list = value["sessions"].as_array().or_else(|| value.as_array());
    let Some(list) = list else {
        return json!({"available": false, "reason": "muster-shim's roster has an unknown format."});
    };
    let sessions: Vec<Value> = list
        .iter()
        .map(|session| {
            let declared = &session["declared"];
            let attested = &session["attested"];
            json!({
                "project": declared["project"],
                "nickname": session["nickname"],
                "machine": attested["machine"],
                "wake": session["wake"],
                // Only `waiting` comes from the daemon. The other wake values are declared.
                "wake_observed": session["wake"] == "waiting",
                "current_task": declared["current_task"],
                "activity": declared["activity"],
                "last_activity_at": declared["last_activity_at"],
            })
        })
        .collect();
    json!({"available": true, "sessions": sessions, "read_at_ms": read_at_ms})
}

struct App {
    projects: Vec<Project>,
    /// Local folders of day summaries to read instead of GitHub's (RAL-45).
    local: LocalSummaries,
    snapshot: Mutex<Snapshot>,
    allowed_hosts: Vec<String>,
    /// The file holding the long-lived viewer key, read on each use, so a new key takes effect at
    /// once (RAL-29).
    key_file: std::path::PathBuf,
    /// One-time codes and when each stops working.
    codes: Mutex<Vec<(String, Instant)>>,
    /// Signed-in sessions, oldest first, kept in `sessions_file` too (RAL-80).
    sessions: Mutex<Vec<Session>>,
    sessions_file: std::path::PathBuf,
    /// How long a one-time code works.
    code_ttl: Duration,
    /// The backup directory on this machine to report on, if any (RAL-15).
    backup: Option<BackupWatch>,
    /// Reads a past state. Held by one read at a time, since each is a replay (RAL-28).
    past: Mutex<PastReader>,
}

/// Folders on this machine holding day summaries, which the operator's rule keeps out of git, so
/// GitHub never has them (RAL-45). Each is read for its newest `YYYY-MM-DD.md`, read-only.
#[derive(Clone, Debug, Default)]
pub struct LocalSummaries {
    /// A team's summaries, by its project's prefix.
    pub by_prefix: std::collections::BTreeMap<String, std::path::PathBuf>,
    /// The watchman's writeups for the whole muster.
    pub watchman: Option<std::path::PathBuf>,
}

/// The newest day summary in a local folder, as a summary panel shows it, saying it was read from a
/// local file and naming it. A missing or unreadable folder, or a file over the same cap as a
/// GitHub read, gives an `error` instead.
pub fn read_local_summary(folder: &std::path::Path) -> Value {
    let mut value =
        json!({"local": true, "folder": folder.display().to_string(), "read_at_ms": now_ms()});
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            value["error"] = json!(format!("the folder {} doesn't exist", folder.display()));
            return value;
        }
        Err(error) => {
            value["error"] = json!(format!(
                "the folder {} can't be read: {error}",
                folder.display()
            ));
            return value;
        }
    };
    let names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    let Some(name) = latest_summary(&names) else {
        value["missing"] = json!(true);
        return value;
    };
    let path = folder.join(&name);
    value["file"] = json!(path.display().to_string());
    let read = std::fs::File::open(&path).and_then(|file| {
        use std::io::Read;
        let mut bytes = Vec::new();
        file.take(MAX_COMMAND_OUTPUT + 1).read_to_end(&mut bytes)?;
        Ok(bytes)
    });
    match read.map_err(|error| error.to_string()).and_then(|bytes| {
        let bytes = too_much(bytes)
            .map_err(|_| format!("it's over {} MiB", MAX_COMMAND_OUTPUT / 1024 / 1024))?;
        String::from_utf8(bytes).map_err(|_| "it isn't UTF-8 text".to_owned())
    }) {
        Ok(text) => value["text"] = json!(text),
        Err(reason) => value["error"] = json!(format!("{name} can't be read: {reason}")),
    }
    value
}

/// Replaces the summaries read from GitHub with local ones where a folder is set (RAL-45), so a
/// failed GitHub read doesn't hide a local summary either.
fn apply_local_summaries(projects: &[Project], roadmaps: &mut [Value], local: &LocalSummaries) {
    for (project, row) in projects.iter().zip(roadmaps.iter_mut()) {
        if !row.is_object() {
            *row = json!({});
        }
        if let Some(folder) = local.by_prefix.get(&project.prefix) {
            row["summary"] = read_local_summary(folder);
        }
        if project.repo == RALLY_REPO
            && let Some(folder) = &local.watchman
        {
            row["watchman_summary"] = read_local_summary(folder);
        }
    }
}

/// A backup directory the viewer reports on: the head it reached, how old it is, and whether that's
/// overdue (RAL-15).
#[derive(Clone, Debug)]
pub struct BackupWatch {
    pub dir: std::path::PathBuf,
    pub max_age_hours: f64,
}

/// What the viewer says about this machine's backup. `watched` is false when there's none to report
/// on; otherwise `overdue` is true when the last backup is older than allowed, or there is none, or
/// its head can't be read.
pub fn backup_status(
    saved: Result<Option<crate::backup::Saved>, String>,
    now_ms: i64,
    max_age_hours: f64,
) -> Value {
    match saved {
        Err(reason) => json!({
            "watched": true, "available": false, "overdue": true,
            "reason": format!("the backup's head can't be read: {reason}"),
        }),
        Ok(None) => json!({
            "watched": true, "available": false, "overdue": true,
            "reason": "no backup has been made yet",
        }),
        Ok(Some(saved)) => {
            let age_hours = (now_ms - saved.at_ms).max(0) as f64 / 3_600_000.0;
            json!({
                "watched": true, "available": true, "seq": saved.seq, "saved_at_ms": saved.at_ms,
                "age_hours": age_hours, "max_age_hours": max_age_hours,
                "overdue": age_hours > max_age_hours,
            })
        }
    }
}

/// Runs the viewer until the process ends. Refuses anything but a loopback address.
/// How browsers sign in to the viewer (RAL-29).
#[derive(Clone, Debug)]
pub struct SignIn {
    /// The private file holding the long-lived viewer key.
    pub key_file: std::path::PathBuf,
    /// How long a one-time code works.
    pub code_ttl: Duration,
}

pub async fn serve(
    listener: TcpListener,
    sign_in: SignIn,
    projects: Vec<Project>,
    ledger: LedgerReader,
    past: PastReader,
    backup: Option<BackupWatch>,
    local: LocalSummaries,
) -> Result<(), Box<dyn std::error::Error>> {
    let address = listener.local_addr()?;
    let sessions_file = sessions_file(&sign_in.key_file);
    let sessions = load_sessions(&sessions_file, &sign_in.key_file);
    let app = Arc::new(App {
        projects,
        local,
        snapshot: Mutex::new(Snapshot::default()),
        allowed_hosts: allowed_hosts(address)?,
        key_file: sign_in.key_file,
        codes: Mutex::new(Vec::new()),
        sessions: Mutex::new(sessions),
        sessions_file,
        code_ttl: sign_in.code_ttl,
        backup,
        past: Mutex::new(past),
    });
    let refresher = app.clone();
    std::thread::spawn(move || refresh_forever(&refresher, &ledger));

    let router = Router::new()
        .route(
            "/",
            get(|| async { page("text/html; charset=utf-8", INDEX) }),
        )
        .route("/app.js", get(|| async { page("text/javascript", APP_JS) }))
        .route("/app.css", get(|| async { page("text/css", APP_CSS) }))
        .route("/api/status", get(status))
        .route("/api/past", get(past_view))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app);
    listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(listener)?;
    axum::serve(listener, router).await?;
    Ok(())
}

fn allowed_hosts(address: SocketAddr) -> Result<Vec<String>, String> {
    if !address.ip().is_loopback() {
        return Err(format!(
            "the viewer only runs on a loopback address, not {address}: remote viewing needs its \
             own review (RAL-34)"
        ));
    }
    // The address it listens on, and `localhost` only for the addresses that name usually means.
    let mut hosts = vec![address.to_string()];
    let usual = address.ip() == std::net::Ipv4Addr::LOCALHOST
        || address.ip() == std::net::Ipv6Addr::LOCALHOST;
    if usual {
        hosts.push(format!("localhost:{}", address.port()));
    }
    Ok(hosts)
}

fn refresh_forever(app: &App, ledger: &LedgerReader) {
    let mut roadmaps_due = Instant::now();
    loop {
        if Instant::now() >= roadmaps_due {
            let previous = app.snapshot.lock().expect("no panics").roadmaps.clone();
            let mut roadmaps = refresh_roadmaps(&app.projects, &previous);
            apply_local_summaries(&app.projects, &mut roadmaps, &app.local);
            let summary = muster_summary(&app.projects, &roadmaps);
            let mut snapshot = app.snapshot.lock().expect("no panics");
            snapshot.roadmaps = roadmaps;
            snapshot.muster_summary = Some(summary);
            drop(snapshot);
            roadmaps_due = Instant::now() + ROADMAPS_EVERY;
        }
        let ledger_value = match ledger() {
            Ok(mut value) => {
                value["available"] = json!(true);
                value["read_at_ms"] = json!(now_ms());
                value
            }
            Err(reason) => json!({"available": false, "reason": reason}),
        };
        let roster = read_roster();
        let backup = app.backup.as_ref().map(|watch| {
            let saved = crate::backup::saved(&watch.dir).map_err(|e| e.to_string());
            backup_status(saved, now_ms() as i64, watch.max_age_hours)
        });
        {
            let mut snapshot = app.snapshot.lock().expect("no panics");
            snapshot.ledger = Some(ledger_value);
            snapshot.roster = Some(roster);
            snapshot.backup = backup;
        }
        std::thread::sleep(LIVE_EVERY);
    }
}

fn page(content_type: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

/// Which past moment to show: a record number (RAL-28), or a closed rally by the record that
/// opened it, shown as its own board as it stood when it closed (RAL-77).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PastQuery {
    as_of: Option<u64>,
    rally: Option<u64>,
}

/// The closed rally opened at record `opened`, from the rallies the viewer last read.
pub fn closed_rally(ledger: &Value, opened: u64) -> Result<Value, String> {
    let rally = ledger["rallies"]["rallies"]
        .as_array()
        .and_then(|all| all.iter().find(|r| r["seq"].as_u64() == Some(opened)))
        .ok_or_else(|| format!("no rally was opened at record {opened}"))?;
    if !rally["closed"].is_object() {
        return Err(format!(
            "the rally opened at record {opened} is still open; it's the board now"
        ));
    }
    Ok(rally.clone())
}

/// A closed rally's own board (RAL-77): what `board` shows for it, on the ledger as it stood at
/// its close. The roster then isn't kept, so its teams are those it named and those holding its
/// stories then.
pub fn past_board(record: &Value, past: &Value) -> Value {
    let mut rally = opened_rally(record, &json!({}));
    let held: BTreeSet<String> = board(&rally, past)["stories"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|id| past["stories"][id.as_str()?]["team"].as_str())
        .map(|team| team.trim().to_lowercase())
        .collect();
    let mut teams: BTreeSet<String> = rally["active_teams"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|team| team.as_str().map(str::to_owned))
        .collect();
    teams.extend(held);
    rally["active_teams"] = json!(teams);
    rally["board"] = board(&rally, past);
    rally["opened"] = json!(false);
    rally["closed"] = json!(true);
    rally["closed_seq"] = record["closed"]["seq"].clone();
    rally["closed_at_ms"] = record["closed"]["at_ms"].clone();
    rally["note"] = record["closed"]["note"].clone();
    rally["closed_by"] = record["closed"]["source"]["declared_name"].clone();
    rally["rule"] = json!(
        "This rally's board as it stood when it closed: its projects, the stories recorded for \
         them while it was open or still held then, and the teams it named or that held them. \
         The roster of the time isn't kept."
    );
    rally
}

/// Whether a past moment can be shown, given the newest record the viewer has read: a record
/// that exists, from the first one on. `Err` says why not.
pub fn past_moment(as_of: u64, newest: Option<u64>) -> Result<u64, String> {
    if as_of == 0 {
        return Err("as_of is a record number, from 1".into());
    }
    match newest {
        Some(newest) if as_of > newest => Err(format!(
            "the ledger has {newest} records, so there's no record {as_of} yet"
        )),
        _ => Ok(as_of),
    }
}

/// The ledger as it stood at a past record, and the corrections made since to what it held then,
/// labelled as later (RAL-28).
async fn past_view(
    State(app): State<Arc<App>>,
    query: Result<axum::extract::Query<PastQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(axum::extract::Query(query)) = query else {
        return (
            StatusCode::BAD_REQUEST,
            "as_of and rally are record numbers: as_of a moment, rally the record that opened one",
        )
            .into_response();
    };
    if query.rally.is_some() == query.as_of.is_some() {
        return (
            StatusCode::BAD_REQUEST,
            "ask for one of as_of, a record number, or rally, the record that opened one",
        )
            .into_response();
    }
    let (newest, rally) = {
        let snapshot = app.snapshot.lock().expect("no panics");
        let ledger = snapshot.ledger.as_ref();
        let newest = ledger.and_then(|ledger| ledger["as_of"].as_u64());
        let rally = match (query.rally, ledger) {
            (None, _) => None,
            (Some(_), None) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "the rallies haven't been read yet",
                )
                    .into_response();
            }
            (Some(opened), Some(ledger)) => match closed_rally(ledger, opened) {
                Ok(rally) => Some(rally),
                Err(reason) => return (StatusCode::NOT_FOUND, reason).into_response(),
            },
        };
        (newest, rally)
    };
    let as_of = match &rally {
        Some(rally) => rally["closed"]["seq"].as_u64().unwrap_or(0),
        None => query.as_of.unwrap_or(0),
    };
    let seq = match past_moment(as_of, newest) {
        Ok(seq) => seq,
        Err(reason) => return (StatusCode::BAD_REQUEST, reason).into_response(),
    };
    let read = tokio::task::spawn_blocking(move || {
        let Ok(reader) = app.past.try_lock() else {
            return Err(None);
        };
        reader(seq).map_err(Some)
    })
    .await;
    match read {
        Ok(Ok(mut value)) => {
            if let Some(rally) = rally {
                value["rally"] = past_board(&rally, &value);
            }
            Json(value).into_response()
        }
        Ok(Err(None)) => (
            StatusCode::TOO_MANY_REQUESTS,
            "another past moment is being read; try again in a moment",
        )
            .into_response(),
        Ok(Err(Some(reason))) => (StatusCode::BAD_GATEWAY, reason).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "the read failed").into_response(),
    }
}

async fn status(State(app): State<Arc<App>>) -> Response {
    let snapshot = app.snapshot.lock().expect("no panics");
    let ledger = snapshot
        .ledger
        .clone()
        .unwrap_or_else(|| json!({"available": false, "reason": "still reading it."}));
    let roster = snapshot
        .roster
        .clone()
        .unwrap_or_else(|| json!({"available": false, "reason": "still reading it."}));
    let mut rally = current_rally(&app.projects, &ledger, &roster);
    if rally["opened"] == json!(true) {
        rally["board"] = board(&rally, &ledger);
    }
    let projects: Vec<Value> = app
        .projects
        .iter()
        .enumerate()
        .map(|(index, project)| {
            let mut row = snapshot
                .roadmaps
                .get(index)
                .cloned()
                .unwrap_or_else(|| json!({}));
            row["name"] = json!(project.name);
            row["prefix"] = json!(project.prefix);
            row["repo"] = json!(project.repo);
            row["path"] = json!(project.path);
            row["tick_meaning"] = json!(project.tick_meaning);
            row["verified"] = verified_for(&project.prefix, &ledger);
            row["progress"] = progress(&project.prefix, &row, &ledger);
            row["in_rally"] = json!(in_rally(&rally, &project.prefix));
            row
        })
        .collect();
    let muster_summary = snapshot
        .muster_summary
        .clone()
        .unwrap_or_else(|| json!({"reason": "still reading it."}));
    let backup = snapshot
        .backup
        .clone()
        .unwrap_or_else(|| json!({"watched": false}));
    Json(json!({
        "untrusted_text": UNTRUSTED_TEXT,
        "ledger": ledger,
        "roster": roster,
        "backup": backup,
        "projects": projects,
        "rally": rally,
        "muster_summary": muster_summary,
    }))
    .into_response()
}

async fn guard(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    let refused = |status: StatusCode, message: &'static str| (status, message).into_response();
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let mut response = if !app.allowed_hosts.iter().any(|allowed| allowed == host) {
        refused(
            StatusCode::FORBIDDEN,
            "this page is only served to its own address",
        )
    } else if !origin_allowed(request.headers().get(header::ORIGIN), host)
        || !fetched_from_here(request.headers().get("sec-fetch-site"))
    {
        refused(StatusCode::FORBIDDEN, "other sites can't read this page")
    } else if *request.method() == Method::POST && request.uri().path() == CODE_PATH {
        mint_code(&app, request.headers())
    } else if *request.method() == Method::POST && request.uri().path() == "/" {
        sign_in_posted(&app, request).await
    } else if !matches!(*request.method(), Method::GET | Method::HEAD) {
        refused(StatusCode::METHOD_NOT_ALLOWED, "this page is read-only")
    } else if request.uri().path() == "/"
        && let Some(offered) = query_code(request.uri().query())
    {
        sign_in_page(offered)
    } else if !signed_in(&app, request.headers()) {
        refused(
            StatusCode::UNAUTHORIZED,
            "sign in: run `rally web --login` and open the address it prints",
        )
    } else {
        next.run(request).await
    };
    // Only the sign-in page has a form; every other answer gets the policy that allows none.
    let policy = if response.extensions().get::<SignInPage>().is_some() {
        SIGN_IN_CSP
    } else {
        CSP
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(policy),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

/// The cookie that carries a session.
const COOKIE: &str = "rally_view";

/// Where `rally web --login` asks for a one-time code.
pub const CODE_PATH: &str = "/api/code";

/// The most codes held at once; the oldest go first.
const MAX_HELD: usize = 64;

/// The most sessions kept at once; the oldest go first (RAL-80).
const MAX_SESSIONS: usize = 32;

/// How long a session lasts from its sign-in, across restarts: 14 days (RAL-80).
const SESSION_MAX_AGE_MS: u64 = 14 * 24 * 60 * 60 * 1000;

/// A signed-in session: the SHA-256 of the session and of the key it was made under, and when it
/// was made, by the wall clock, so its age holds across restarts (RAL-80).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Session {
    sha256: String,
    key_sha256: String,
    made_at_ms: u64,
}

impl Session {
    /// Whether it's still good at `now_ms` under the key whose hash is `key_sha256`. A clock set
    /// back before it was made doesn't make it last longer.
    fn live(&self, key_sha256: &str, now_ms: u64) -> bool {
        self.key_sha256 == key_sha256
            && now_ms >= self.made_at_ms
            && now_ms - self.made_at_ms < SESSION_MAX_AGE_MS
    }
}

/// The key now in the key file, or `None` if it can't be read.
fn current_key(app: &App) -> Option<String> {
    std::fs::read_to_string(&app.key_file)
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|key| is_key(key))
}

/// A one-time code, for a client that isn't a browser and shows the key in a header. A browser
/// always says where a request came from, so a page can't ask for one.
fn mint_code(app: &App, headers: &axum::http::HeaderMap) -> Response {
    let refused = |status: StatusCode, message: &'static str| (status, message).into_response();
    if headers.contains_key(header::ORIGIN) || headers.contains_key("sec-fetch-site") {
        return refused(
            StatusCode::FORBIDDEN,
            "codes are only for rally web --login",
        );
    }
    let offered = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let Some(key) = current_key(app) else {
        return refused(
            StatusCode::SERVICE_UNAVAILABLE,
            "the viewer key can't be read",
        );
    };
    if !offered.is_some_and(|offered| same_key(offered, &key)) {
        return refused(StatusCode::UNAUTHORIZED, "that isn't this viewer's key");
    }
    let Ok(code) = new_key() else {
        return refused(StatusCode::INTERNAL_SERVER_ERROR, "no code could be made");
    };
    let now = Instant::now();
    let mut codes = app.codes.lock().unwrap_or_else(|e| e.into_inner());
    codes.retain(|(_, until)| *until > now);
    if codes.len() >= MAX_HELD {
        codes.remove(0);
    }
    codes.push((code.clone(), now + app.code_ttl));
    Json(json!({"code": code, "expires_in_seconds": app.code_ttl.as_secs()})).into_response()
}

/// Marks the sign-in page's response, so `guard` gives it its own policy.
#[derive(Clone, Copy)]
struct SignInPage;

/// The most a sign-in form's body can be: `code=` and a code, with room to spare.
const SIGN_IN_BODY_LIMIT: usize = 1024;

/// Opening a code's address shows a "Sign in" button and spends nothing (RAL-73). Browsers fetch
/// addresses ahead of a visit (Chrome preloads what's typed or pasted into the address bar), and
/// such a fetch used to spend the code before the operator's own visit arrived. Only the button's
/// POST spends it. The code is shown again only if it has a code's form, so nothing else offered
/// in the address can reach the page's markup.
fn sign_in_page(offered: &str) -> Response {
    if !is_key(offered) {
        return (
            StatusCode::UNAUTHORIZED,
            "that code isn't good: it was used, it expired, or it's wrong; run rally web --login",
        )
            .into_response();
    }
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
        axum::Extension(SignInPage),
        SIGN_IN.replace("{{CODE}}", offered),
    )
        .into_response()
}

/// The sign-in button's POST: a form body of `code=<code>`.
async fn sign_in_posted(app: &App, request: Request) -> Response {
    let Ok(body) = axum::body::to_bytes(request.into_body(), SIGN_IN_BODY_LIMIT).await else {
        return (StatusCode::PAYLOAD_TOO_LARGE, "that isn't a sign-in form").into_response();
    };
    let body = String::from_utf8_lossy(&body);
    // A POST with no code in it isn't a sign-in, and nothing else here takes a write.
    let Some(offered) = body
        .split('&')
        .find_map(|pair| pair.trim().strip_prefix("code="))
    else {
        return (StatusCode::METHOD_NOT_ALLOWED, "this page is read-only").into_response();
    };
    sign_in(app, offered)
}

/// Exchanges a one-time code for a session cookie, once, before it expires.
fn sign_in(app: &App, offered: &str) -> Response {
    let refused = |status: StatusCode, message: &'static str| (status, message).into_response();
    let now = Instant::now();
    let found = {
        let mut codes = app.codes.lock().unwrap_or_else(|e| e.into_inner());
        let index = codes.iter().position(|(code, _)| same_key(offered, code));
        // Used or not, it's spent: a code works once.
        index
            .map(|index| codes.remove(index))
            .filter(|(_, until)| *until > now)
    };
    let (Some(_), Some(key)) = (found, current_key(app)) else {
        return refused(
            StatusCode::UNAUTHORIZED,
            "that code isn't good: it was used, it expired, or it's wrong; run rally web --login",
        );
    };
    let Ok(session) = new_key() else {
        return refused(
            StatusCode::INTERNAL_SERVER_ERROR,
            "no session could be made",
        );
    };
    {
        let key = sha256_hex(&key);
        let now = now_ms();
        let mut sessions = app.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.retain(|kept| kept.live(&key, now));
        sessions.push(Session {
            sha256: sha256_hex(&session),
            key_sha256: key,
            made_at_ms: now,
        });
        let extra = sessions.len().saturating_sub(MAX_SESSIONS);
        sessions.drain(..extra);
        // Saved under the lock, so two sign-ins can't save out of order. A failed save leaves
        // this session good until the viewer restarts.
        if let Err(error) = save_sessions(&app.sessions_file, &sessions) {
            eprintln!(
                "rally web: couldn't keep the sessions in {}: {error}",
                app.sessions_file.display()
            );
        }
    }
    let cookie = format!("{COOKIE}={session}; HttpOnly; SameSite=Strict; Path=/");
    match HeaderValue::from_str(&cookie) {
        Ok(cookie) => (
            StatusCode::SEE_OTHER,
            [
                (header::SET_COOKIE, cookie),
                (header::LOCATION, HeaderValue::from_static("/")),
            ],
        )
            .into_response(),
        Err(_) => refused(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the session can't be set",
        ),
    }
}

/// Whether the request's cookie is a session made under the key now in the file.
fn signed_in(app: &App, headers: &axum::http::HeaderMap) -> bool {
    let (Some(held), Some(key)) = (cookie_key(headers), current_key(app)) else {
        return false;
    };
    let (held, key, now) = (sha256_hex(held), sha256_hex(&key), now_ms());
    app.sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|session| session.live(&key, now) && same_key(&held, &session.sha256))
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Where the sessions are kept: beside the key file, named after it (RAL-80).
pub fn sessions_file(key_file: &std::path::Path) -> std::path::PathBuf {
    let mut name = key_file.file_name().unwrap_or_default().to_os_string();
    name.push(".sessions");
    key_file.with_file_name(name)
}

/// The kept sessions made under the key now in `key_file` and younger than the most a session
/// lasts, the newest [`MAX_SESSIONS`]. Anything unreadable, older, or made under another key, as
/// after `--new-key`, is left out, so it signs no one in.
fn load_sessions(file: &std::path::Path, key_file: &std::path::Path) -> Vec<Session> {
    let Some(key) = std::fs::read_to_string(key_file)
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|key| is_key(key))
    else {
        return Vec::new();
    };
    let key = sha256_hex(&key);
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    let Ok(kept) = serde_json::from_str::<Value>(&text) else {
        eprintln!(
            "rally web: {} isn't a sessions file, so no one is signed in",
            file.display()
        );
        return Vec::new();
    };
    let now = now_ms();
    let mut sessions: Vec<Session> = kept["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let session = Session {
                sha256: row["session_sha256"]
                    .as_str()
                    .filter(|s| is_key(s))?
                    .to_owned(),
                key_sha256: row["key_sha256"].as_str()?.to_owned(),
                made_at_ms: row["made_at_ms"].as_u64()?,
            };
            session.live(&key, now).then_some(session)
        })
        .collect();
    sessions.sort_by_key(|session| session.made_at_ms);
    let extra = sessions.len().saturating_sub(MAX_SESSIONS);
    sessions.drain(..extra);
    sessions
}

/// Writes the sessions to a private file, replacing the old one whole, so a crash mid-write leaves
/// the old file or the new one, never part of one.
fn save_sessions(file: &std::path::Path, sessions: &[Session]) -> Result<(), String> {
    let rows: Vec<Value> = sessions
        .iter()
        .map(|session| {
            json!({"session_sha256": session.sha256, "key_sha256": session.key_sha256,
                   "made_at_ms": session.made_at_ms})
        })
        .collect();
    let text = serde_json::to_vec(&json!({"sessions": rows})).map_err(|e| e.to_string())?;
    let mut temporary = file.as_os_str().to_os_string();
    temporary.push(".new");
    let temporary = std::path::PathBuf::from(temporary);
    crate::access::write_private(&temporary, &text).map_err(|e| e.to_string())?;
    std::fs::rename(&temporary, file).map_err(|e| e.to_string())
}

/// A browser says where a request came from in `Sec-Fetch-Site`: only this page itself, or the
/// operator typing the address, is served. A client that doesn't send it is judged by the rest.
fn fetched_from_here(site: Option<&HeaderValue>) -> bool {
    site.is_none_or(|site| matches!(site.as_bytes(), b"same-origin" | b"none"))
}

fn query_code(query: Option<&str>) -> Option<&str> {
    query?
        .split('&')
        .find_map(|pair| pair.strip_prefix("code="))
}

fn cookie_key(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(&format!("{COOKIE}=")))
}

/// Compares keys in time that doesn't depend on where they differ.
fn same_key(offered: &str, key: &str) -> bool {
    offered.len() == key.len()
        && offered
            .bytes()
            .zip(key.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

/// A new viewer key: 32 random bytes, as hex.
pub fn new_key() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(hex::encode(bytes))
}

/// Whether `text` is a key `new_key` could have made.
pub fn is_key(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

fn origin_allowed(origin: Option<&HeaderValue>, host: &str) -> bool {
    origin.is_none_or(|origin| origin.as_bytes() == format!("http://{host}").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RAL-80: a session lasts 14 days from its sign-in, only under its own key, and a clock set
    /// back doesn't stretch it.
    #[test]
    fn a_session_lasts_its_age_under_its_own_key() {
        let made = Session {
            sha256: "a".repeat(64),
            key_sha256: "k".into(),
            made_at_ms: 1_000,
        };
        assert!(made.live("k", 1_000));
        assert!(made.live("k", 1_000 + SESSION_MAX_AGE_MS - 1));
        assert!(!made.live("k", 1_000 + SESSION_MAX_AGE_MS));
        assert!(!made.live("other", 1_000));
        assert!(!made.live("k", 999));
        assert_eq!(SESSION_MAX_AGE_MS, 14 * 86_400_000);
    }

    #[test]
    fn a_past_moment_is_a_record_that_exists() {
        assert_eq!(past_moment(1, Some(10)), Ok(1));
        assert_eq!(past_moment(10, Some(10)), Ok(10));
        assert!(past_moment(0, Some(10)).is_err());
        assert!(
            past_moment(11, Some(10))
                .unwrap_err()
                .contains("no record 11")
        );
        // Before the ledger has been read, the service is the one to say.
        assert_eq!(past_moment(5, None), Ok(5));
    }

    #[test]
    fn counts_ticks_overall_and_by_section() {
        let text = "# Title\n\n## One\n- [x] a\n- [ ] b\n  - [X] nested\n\n## Two\n* [ ] c\n";
        let (boxes, sections) = parse_roadmap(text);
        assert_eq!(
            boxes,
            Boxes {
                ticked: 2,
                total: 4,
                percent: 50
            }
        );
        assert_eq!(sections.len(), 2);
        assert_eq!((sections[0].ticked, sections[0].total), (2, 3));
        assert_eq!(sections[1].name, "Two");
    }

    #[test]
    fn ignores_examples_in_code_fences_and_things_that_only_look_like_boxes() {
        let text = "```markdown\n- [x] example\n```\n- [link](url)\n- [] no\nnot - [x] a box\n- [ ] real\n";
        let (boxes, _) = parse_roadmap(text);
        assert_eq!((boxes.ticked, boxes.total), (0, 1));
    }

    #[test]
    fn an_unfinished_roadmap_never_rounds_up_to_100() {
        let text = format!("{}- [ ] last\n", "- [x] done\n".repeat(999));
        assert_eq!(parse_roadmap(&text).0.percent, 99);
    }

    #[test]
    fn an_empty_roadmap_has_no_percentage_to_divide() {
        assert_eq!(parse_roadmap("# nothing").0, Boxes::default());
    }

    #[test]
    fn stories_come_from_the_same_boxes_the_counts_use() {
        let text = "## Open\n- [ ] **RAL-43: Give each project an info page.** Requested…\n\
                    ```\n- [x] **RAL-9: An example.**\n```\n\
                    ## Done\n- [x] **RAL-1: Keep a ledger.** Done when…\n- [x] plain words, no id\n\
                    - [ ] **RALLY-2: Another prefix.**\n";
        let stories = parse_stories(text, "RAL");
        assert_eq!(stories.len() as u32, parse_roadmap(text).0.total);
        assert_eq!(stories[0].id.as_deref(), Some("RAL-43"));
        assert_eq!(stories[0].title, "Give each project an info page");
        assert_eq!(
            (stories[0].ticked, stories[0].section.as_str()),
            (false, "Open")
        );
        assert_eq!(
            (stories[1].id.as_deref(), stories[1].ticked),
            (Some("RAL-1"), true)
        );
        assert_eq!(
            (stories[2].id.as_deref(), stories[2].title.as_str()),
            (None, "plain words, no id")
        );
        assert_eq!(
            stories[3].id, None,
            "another project's prefix isn't this one's id"
        );
        let long = format!("- [ ] {}\n", "x".repeat(5000));
        assert_eq!(
            parse_stories(&long, "RAL")[0].title.chars().count(),
            MAX_TITLE_CHARS
        );
    }

    #[test]
    fn only_a_dated_day_file_is_a_summary_and_the_latest_wins() {
        let names: Vec<String> = [
            "2026-11-02.md",
            "2026-11-03.md",
            "notes.md",
            "2026-11-04.md.bak",
            "2026-1-05.md",
            "9999-99-99.txt",
            "../2026-12-01.md",
            "2026-11-03-extra.md",
        ]
        .map(str::to_owned)
        .to_vec();
        assert_eq!(latest_summary(&names).as_deref(), Some("2026-11-03.md"));
        assert_eq!(latest_summary(&["README.md".to_owned()]), None);
    }

    #[test]
    fn blame_keeps_each_range_s_lines_and_date_only() {
        let value = json!([
            {"startingLine": 1, "endingLine": 3, "commit": {"committedDate": "2026-10-29T08:00:00Z", "message": "secret"}},
            {"startingLine": 4, "endingLine": 4, "commit": {"committedDate": "<script>"}},
            {"startingLine": "5", "endingLine": 5, "commit": {"committedDate": "2026-10-30T08:00:00Z"}},
            {"startingLine": 7, "endingLine": 6, "commit": {"committedDate": "2026-10-30T08:00:00Z"}},
        ]);
        assert_eq!(shape_blame(&value), vec![(1, 3, "2026-10-29".to_owned())]);
        assert!(shape_blame(&json!({"not": "a list"})).is_empty());
    }

    #[test]
    fn ticked_lines_take_the_date_of_the_range_holding_them() {
        let text = "## Open\n- [ ] **RAL-1: Open.**\n- [x] **RAL-2: Ticked.**\n\n```\n- [x] **RAL-9: An example.**\n```\n- [x] **RAL-3: Also ticked.**\n";
        let mut stories = parse_stories(text, "RAL");
        assert_eq!(stories.len(), 3, "the fenced example isn't a story");
        let ranges = vec![
            (1, 3, "2026-10-29".to_owned()),
            (4, 7, "2026-10-30".to_owned()),
            (8, 8, "2026-11-01".to_owned()),
        ];
        date_stories(&mut stories, &ranges);
        let dates: Vec<_> = stories.iter().map(|s| s.last_changed.as_deref()).collect();
        assert_eq!(dates, [None, Some("2026-10-29"), Some("2026-11-01")]);
        // A line no range covers stays undated.
        let mut stories = parse_stories(text, "RAL");
        date_stories(&mut stories, &[(1, 2, "2026-10-29".to_owned())]);
        assert!(stories.iter().all(|s| s.last_changed.is_none()));
    }

    #[test]
    fn merged_prs_keep_three_fields_newest_first() {
        let value = json!([
            {"number": 1, "title": "older", "mergedAt": "2026-11-02T10:00:00Z", "author": {"login": "x"}},
            {"number": 2, "title": "newer", "mergedAt": "2026-11-03T10:00:00Z", "body": "secret"},
            {"number": "bad", "title": "skipped", "mergedAt": "2026-11-03T11:00:00Z"},
        ]);
        let shaped = shape_merged(&value);
        assert_eq!(
            shaped,
            json!([
                {"number": 2, "title": "newer", "merged_at": "2026-11-03T10:00:00Z"},
                {"number": 1, "title": "older", "merged_at": "2026-11-02T10:00:00Z"},
            ])
        );
        assert_eq!(shape_merged(&json!("nope")), json!([]));
    }

    #[test]
    fn verified_comes_only_from_the_ledger() {
        let none = json!({"available": false});
        assert_eq!(verified_for("RAL", &none)["available"], false);

        let ledger = json!({"stories": {
            "RAL-1": {"state": "verified"},
            "RAL-2": {"state": "in_review"},
            "RALLY-3": {"state": "verified"},
            "MUS-1": {"state": "verified"},
        }});
        let figure = verified_for("RAL", &ledger);
        assert_eq!(figure["verified"], 1);
        assert_eq!(figure["tracked"], 2);
        assert_eq!(verified_for("CAT", &ledger)["available"], false);
    }

    #[test]
    fn the_backup_status_says_how_old_the_backup_is_and_when_that_is_overdue() {
        use crate::backup::Saved;
        let saved = |at_ms| {
            Ok(Some(Saved {
                seq: 7,
                hash: "x".into(),
                at_ms,
            }))
        };
        let hour = 3_600_000;
        let fresh = backup_status(saved(10 * hour), 10 * hour + hour / 2, 1.0);
        assert_eq!(fresh["watched"], true);
        assert_eq!(fresh["available"], true);
        assert_eq!(fresh["seq"], 7);
        assert_eq!(fresh["overdue"], false);
        assert_eq!(fresh["age_hours"], 0.5);
        let overdue = backup_status(saved(10 * hour), 12 * hour, 1.0);
        assert_eq!(overdue["overdue"], true);
        assert_eq!(overdue["age_hours"], 2.0);
        // A head from the future counts as now, not as negative age.
        let ahead = backup_status(saved(12 * hour), 10 * hour, 1.0);
        assert_eq!(ahead["age_hours"], 0.0);
        let none = backup_status(Ok(None), 0, 1.0);
        assert_eq!(none["available"], false);
        assert_eq!(none["overdue"], true);
        let damaged = backup_status(Err("torn".into()), 0, 1.0);
        assert_eq!(damaged["overdue"], true);
        assert!(damaged["reason"].as_str().unwrap().contains("torn"));
    }

    /// RAL-81: a roadmap with no checkboxes takes its progress from the ledger's states; one with
    /// boxes, or not yet read, or with nothing in the ledger, doesn't.
    #[test]
    fn a_roadmap_without_boxes_takes_its_progress_from_the_ledger() {
        let ledger = json!({
            "available": true, "as_of": 40,
            "stories": {
                "DSM-1": {"state": "verified"}, "DSM-2": {"state": "verified"},
                "DSM-3": {"state": "in_review"}, "DSM-4": {"state": "claimed"},
                "RAL-1": {"state": "verified"},
            },
            "blockers": [],
        });
        let table = "| ID | Story |\n|---|---|\n| DSM-1 | one |\n";
        let (boxes, _) = parse_roadmap(table);
        assert_eq!(boxes.total, 0);
        let roadmap =
            json!({"boxes": boxes, "stories": parse_stories(table, "DSM"), "read_at_ms": 1});
        let value = progress("DSM", &roadmap, &ledger);
        assert_eq!(
            value["from_ledger"],
            json!({"kind": "verified", "as_of": 40, "verified": 2, "tracked": 4, "percent": 50})
        );
        assert_eq!(value["states"]["in_review"], 1);

        // Boxes, even none ticked, are the roadmap's own progress.
        let boxed = "- [ ] **DSM-1: one.**\n";
        let roadmap =
            json!({"boxes": parse_roadmap(boxed).0, "stories": parse_stories(boxed, "DSM")});
        assert!(progress("DSM", &roadmap, &ledger)["from_ledger"].is_null());
        // A roadmap not read yet has no boxes to judge, and a prefix the ledger doesn't track has
        // nothing to count.
        assert!(progress("DSM", &json!({}), &ledger)["from_ledger"].is_null());
        let unread = json!({"boxes": boxes, "stories": [], "read_at_ms": 1});
        assert!(progress("CAT", &unread, &ledger)["from_ledger"].is_null());
    }

    #[test]
    fn progress_counts_states_finds_unblocked_ready_stories_and_unverified_ticks() {
        let roadmap = json!({
            "read_at_ms": 7,
            "stories": [
                {"id": "RAL-1", "title": "one", "ticked": true, "last_changed": "2026-11-03"},
                {"id": "RAL-2", "title": "two", "ticked": true},
                {"id": "RAL-3", "title": "three", "ticked": true},
                {"id": "RAL-4", "title": "four", "ticked": false},
                {"id": null, "title": "a note", "ticked": true},
            ],
        });
        let ledger = json!({
            "available": true, "as_of": 30,
            "stories": {
                "RAL-1": {"state": "verified"}, "RAL-2": {"state": "in_review"},
                "RAL-4": {"state": "ready"}, "RAL-5": {"state": "ready"}, "RAL-6": {"state": "ready"},
                "MUS-1": {"state": "ready"},
            },
            "blockers": [
                {"seq": 10, "team": "rally", "story": "RAL-5", "note": "a", "chain": [12]},
                {"seq": 11, "team": "rally", "story": "RAL-6", "note": "b", "chain": [12]},
                {"seq": 12, "team": "muster", "story": "MUS-44", "note": "c", "chain": []},
                {"seq": 13, "team": "rally", "note": "team-wide", "chain": []},
            ],
        });
        let value = progress("RAL", &roadmap, &ledger);
        assert_eq!(value["roadmap"]["available"], true);
        assert_eq!(value["tracked"], 5);
        assert_eq!(value["states"]["ready"], 3);
        assert_eq!(value["states"]["verified"], 1);
        assert_eq!(value["states"]["claimed"], 0);
        assert_eq!(value["ready_unblocked"], json!(["RAL-4"]));
        // 12 holds up both RAL-5 and RAL-6 through their chains, so it comes first.
        assert_eq!(value["top_blockers"][0]["blocker"], 12);
        assert_eq!(
            value["top_blockers"][0]["holds_up"],
            json!(["RAL-5", "RAL-6"])
        );
        assert_eq!(value["top_blockers"].as_array().unwrap().len(), 3);
        let ticks = value["ticks_without_verified"].as_array().unwrap();
        let ids: Vec<&str> = ticks.iter().map(|t| t["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["RAL-2", "RAL-3"]);
        assert_eq!(ticks[0]["ledger_state"], "in_review");
        assert!(
            ticks[1]["ledger_state"].is_null(),
            "not in the ledger at all"
        );
        // Nothing compares this roadmap with another.
        for key in value.as_object().unwrap().keys() {
            assert!(
                !["rank", "score", "compare", "share"]
                    .iter()
                    .any(|w| key.contains(w)),
                "{key}"
            );
        }
    }

    #[test]
    fn progress_says_what_couldnt_be_read_rather_than_showing_zeros() {
        let ledger = json!({"available": true, "as_of": 1, "stories": {}});
        let unread = progress("RAL", &json!({"error": "gh failed"}), &ledger);
        assert_eq!(unread["roadmap"]["available"], false);
        assert_eq!(unread["roadmap"]["reason"], "gh failed");
        assert!(unread.get("ticks_without_verified").is_none());
        let stale = progress(
            "RAL",
            &json!({"stories": [], "error": "timed out", "read_at_ms": 3}),
            &ledger,
        );
        assert_eq!(stale["roadmap"]["last_read_failed"], "timed out");
        let no_ledger = progress(
            "RAL",
            &json!({"stories": []}),
            &json!({"available": false, "reason": "down"}),
        );
        assert_eq!(no_ledger["ledger"]["available"], false);
        assert_eq!(no_ledger["ledger"]["reason"], "down");
        assert!(no_ledger.get("states").is_none());
        assert!(no_ledger.get("ticks_without_verified").is_none());
        // A ledger that failed its check carries no stories, so it's not read as empty.
        let failed = progress(
            "RAL",
            &json!({"stories": []}),
            &json!({"available": true, "check": {"ok": false}}),
        );
        assert_eq!(failed["ledger"]["available"], false);
    }

    #[test]
    fn a_local_summary_is_the_newest_day_file_and_says_where_it_came_from() {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in [
            ("2026-11-03.md", "older"),
            ("2026-11-04.md", "newest"),
            ("notes.md", "not a day"),
            ("2026-11-05.txt", "wrong kind"),
        ] {
            std::fs::write(dir.path().join(name), text).unwrap();
        }
        // A folder named like a day file isn't read as one.
        std::fs::create_dir(dir.path().join("2026-12-31.md")).unwrap();
        let value = read_local_summary(dir.path());
        assert_eq!(value["local"], true);
        assert_eq!(value["text"], "newest");
        assert!(value["file"].as_str().unwrap().ends_with("2026-11-04.md"));
        assert!(value.get("error").is_none());

        let empty = tempfile::tempdir().unwrap();
        let none = read_local_summary(empty.path());
        assert_eq!(none["missing"], true);
        assert!(none.get("text").is_none());

        let gone = read_local_summary(&empty.path().join("not-there"));
        assert!(gone["error"].as_str().unwrap().contains("doesn't exist"));
        assert!(gone.get("text").is_none() && gone.get("missing").is_none());

        std::fs::write(empty.path().join("2026-11-04.md"), [0xff, 0xfe]).unwrap();
        let binary = read_local_summary(empty.path());
        assert!(binary["error"].as_str().unwrap().contains("UTF-8"));
    }

    #[test]
    fn a_local_summary_over_the_cap_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(dir.path().join("2026-11-04.md")).unwrap();
        file.set_len(MAX_COMMAND_OUTPUT + 1).unwrap();
        let value = read_local_summary(dir.path());
        assert!(value["error"].as_str().unwrap().contains("MiB"), "{value}");
        assert!(value.get("text").is_none());
    }

    #[test]
    fn local_folders_replace_only_the_summaries_they_name() {
        let projects: Vec<Project> = serde_json::from_str(DEFAULT_PROJECTS).unwrap();
        let team = tempfile::tempdir().unwrap();
        std::fs::write(team.path().join("2026-11-04.md"), "rally's day").unwrap();
        let watch = tempfile::tempdir().unwrap();
        std::fs::write(watch.path().join("2026-11-04.md"), "the muster's day").unwrap();
        let github = json!({"summary": {"missing": true}});
        let mut roadmaps = vec![github.clone(); projects.len()];
        // Without folders nothing changes.
        apply_local_summaries(&projects, &mut roadmaps, &LocalSummaries::default());
        assert!(roadmaps.iter().all(|row| *row == github));
        let local = LocalSummaries {
            by_prefix: [("RAL".to_owned(), team.path().to_owned())].into(),
            watchman: Some(watch.path().to_owned()),
        };
        apply_local_summaries(&projects, &mut roadmaps, &local);
        for (project, row) in projects.iter().zip(&roadmaps) {
            if project.prefix == "RAL" {
                assert_eq!(row["summary"]["text"], "rally's day");
                assert_eq!(row["watchman_summary"]["text"], "the muster's day");
            } else {
                assert_eq!(*row, github, "{} is untouched", project.prefix);
            }
        }
        assert_eq!(
            muster_summary(&projects, &roadmaps)["text"],
            "the muster's day"
        );
    }

    fn rally_projects() -> Vec<Project> {
        serde_json::from_value(json!([
            {"name": "Rally", "prefix": "RAL", "repo": "o/rally", "path": "r.md", "tick_meaning": "m",
             "team": "rally", "sessions": ["rally"]},
            {"name": "Paradox", "prefix": "PDX", "repo": "o/pdx", "path": "r.md", "tick_meaning": "m"},
            {"name": "catapp", "prefix": "CAT", "repo": "o/cat", "path": "r.md", "tick_meaning": "m"},
        ]))
        .unwrap()
    }

    fn rally_roster(sessions: Value) -> Value {
        json!({"available": true, "sessions": sessions, "read_at_ms": 1})
    }

    #[test]
    fn only_projects_held_by_a_team_on_the_roster_are_in_the_rally() {
        let ledger = json!({"available": true, "teams": {"catapp": {"status": "resting"}}, "stories": {
            "RAL-1": {"state": "claimed", "team": "rally"},
            "DSM-2": {"state": "in_review", "team": "crafty-wolverine"},
            "DSM-3": {"state": "verified", "team": "gone-otter"},
            "CAT-4": {"state": "claimed", "team": "catapp"},
            "PDX-5": {"state": "verified", "team": "pdx"},
            "PDX-6": {"state": "ready"},
        }});
        let roster = rally_roster(json!([
            // The rally team by its listed session project; DiscoSim's team by its nickname.
            {"nickname": "plush-tortoise", "project": "rally"},
            {"nickname": "Crafty-Wolverine ", "project": "Discoteque"},
            // A session whose project happens to be "catapp" isn't the catapp team: nothing lists it.
            {"nickname": "someone", "project": "catapp"},
        ]));
        let rally = current_rally(&rally_projects(), &ledger, &roster);
        assert_eq!(rally["known"], true);
        assert_eq!(rally["prefixes"], json!(["DSM", "RAL"]));
        assert_eq!(rally["active_teams"], json!(["crafty-wolverine", "rally"]));
        assert_eq!(rally["held"]["DSM"][0]["story"], "DSM-2");
        assert!(in_rally(&rally, "RAL"));
        assert!(
            !in_rally(&rally, "PDX"),
            "verified and ready stories hold nothing"
        );
        assert!(
            !in_rally(&rally, "CAT"),
            "a claim by a team not on the roster is left over"
        );
    }

    #[test]
    fn a_verified_story_or_an_empty_roster_puts_nothing_in_the_rally() {
        let ledger = json!({"available": true, "stories": {"RAL-1": {"state": "verified", "team": "rally"}}});
        let on = rally_roster(json!([{"nickname": "x", "project": "rally"}]));
        let rally = current_rally(&rally_projects(), &ledger, &on);
        assert_eq!(rally["prefixes"], json!([]));
        assert!(!in_rally(&rally, "RAL"));
        let held = json!({"available": true, "stories": {"RAL-1": {"state": "sent_back", "team": "rally"}}});
        let empty = current_rally(&rally_projects(), &held, &rally_roster(json!([])));
        assert!(!in_rally(&empty, "RAL"), "no one on the roster holds it");
        assert!(in_rally(
            &current_rally(&rally_projects(), &held, &on),
            "RAL"
        ));
    }

    #[test]
    fn an_opened_rally_decides_the_board_and_closed_ones_are_kept_as_past() {
        let ledger = json!({"available": true, "stories": {
            "DSM-2": {"state": "claimed", "team": "crafty-wolverine", "last": 3},
        }, "rallies": {
            "open": {"seq": 40, "at_ms": 9, "name": "Friday", "projects": ["MUS", "RAL", "MAR"],
                     "teams": ["plush-tortoise", "Wise-Leopard"], "source": {"declared_name": "w"}},
            "rallies": [
                {"seq": 40, "name": "Friday", "projects": ["MUS", "RAL", "MAR"]},
                {"seq": 10, "name": "DiscoSim", "projects": ["DSM"],
                 "closed": {"seq": 39, "at_ms": 8, "note": "stood down"}},
            ],
        }});
        let roster = rally_roster(json!([
            {"nickname": "plush-tortoise", "project": "rally"},
            {"nickname": "crafty-wolverine", "project": "Discoteque"},
        ]));
        let rally = current_rally(&rally_projects(), &ledger, &roster);
        assert_eq!(rally["opened"], true);
        assert_eq!(rally["since_seq"], 40);
        assert_eq!(rally["prefixes"], json!(["MUS", "RAL", "MAR"]));
        // A listed session brings its project's team; a held story elsewhere brings nothing.
        assert_eq!(
            rally["active_teams"],
            json!(["plush-tortoise", "rally", "wise-leopard"])
        );
        assert!(in_rally(&rally, "RAL") && in_rally(&rally, "MAR"));
        assert!(!in_rally(&rally, "DSM") && !in_rally(&rally, "PDX"));
        assert_eq!(rally["past"][0]["name"], "DiscoSim");
        assert_eq!(rally["past"][0]["closed_seq"], 39);
        // Without the service's rallies (older than RAL-76), it falls back and says why.
        let older = json!({"available": true, "stories": {},
                           "rallies": {"available": false, "older_service": true, "reason": "404"}});
        let rally = current_rally(&rally_projects(), &older, &roster);
        assert!(rally.get("opened").is_none());
        assert_eq!(rally["rallies_unavailable"], "404");
        assert!(rally.get("rallies_error").is_none());
        // Any other failure isn't taken for an older service (RAL-77).
        for reason in [
            "refused (401): that isn't a token",
            "the service couldn't be reached",
        ] {
            let failed = json!({"available": true, "stories": {},
                                "rallies": {"available": false, "older_service": false, "reason": reason}});
            let rally = current_rally(&rally_projects(), &failed, &roster);
            assert!(rally.get("rallies_unavailable").is_none(), "{rally}");
            assert_eq!(rally["rallies_error"], reason);
        }
    }

    /// RAL-77: a past rally opens as its own board, as the ledger stood when it closed.
    #[test]
    fn a_past_rally_is_its_own_board_as_it_stood_when_it_closed() {
        let ledger = json!({"rallies": {"rallies": [
            {"seq": 40, "name": "Friday", "projects": ["RAL"]},
            {"seq": 10, "at_ms": 1, "name": "DiscoSim", "projects": ["DSM"], "teams": ["crafty-wolverine"],
             "source": {"declared_name": "frank-crocodile"},
             "closed": {"seq": 39, "at_ms": 8, "note": "stood down",
                        "source": {"declared_name": "frank-crocodile"}}},
        ]}});
        assert!(
            closed_rally(&ledger, 40)
                .unwrap_err()
                .contains("still open")
        );
        assert!(closed_rally(&ledger, 11).unwrap_err().contains("no rally"));
        let record = closed_rally(&ledger, 10).unwrap();
        // The ledger at record 39, the close.
        let then = json!({"stories": {
            "DSM-22": {"state": "claimed", "team": "crafty-wolverine", "last": 30},
            "DSM-48": {"state": "verified", "team": "silken-birch", "last": 20},
            "DSM-1": {"state": "verified", "team": "old-team", "last": 5},
            "RAL-9": {"state": "claimed", "team": "rally", "last": 25},
        }, "teams": {
            "crafty-wolverine": {"status": "working"},
            "silken-birch": {"status": "resting"},
            "old-team": {"status": "resting"},
            "rally": {"status": "working"},
        }});
        let rally = past_board(&record, &then);
        assert_eq!(rally["name"], "DiscoSim");
        assert_eq!(rally["closed"], true);
        assert_eq!(rally["opened"], false);
        assert_eq!(rally["closed_seq"], 39);
        assert_eq!(rally["note"], "stood down");
        assert_eq!(rally["closed_by"], "frank-crocodile");
        // Its project's stories recorded while it was open or held then; not before, not others.
        let mut stories: Vec<&str> = rally["board"]["stories"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        stories.sort_unstable();
        assert_eq!(stories, ["DSM-22", "DSM-48"]);
        // The team it named, and the team holding one of its stories then.
        let mut teams: Vec<&str> = rally["board"]["teams"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        teams.sort_unstable();
        assert_eq!(teams, ["crafty-wolverine", "silken-birch"]);
    }

    #[test]
    fn work_in_flight_when_a_rally_opens_stays_on_its_board() {
        let ledger = json!({"stories": {
            "RAL-80": {"state": "claimed", "team": "rally", "last": 100},
            "MUS-103": {"state": "in_review", "team": "muster", "last": 90},
            "RAL-5": {"state": "verified", "team": "rally", "last": 50},
            "RAL-81": {"state": "ready", "last": 106},
            "DSM-22": {"state": "claimed", "team": "crafty-wolverine", "last": 100},
        }, "teams": {"rally": {"seq": 3}, "muster": {"seq": 4}, "catapp": {"seq": 200}}});
        let rally = json!({"since_seq": 105, "prefixes": ["RAL", "MUS", "MAR"],
                           "active_teams": ["rally", "muster", "plush-tortoise"]});
        let board = board(&rally, &ledger);
        assert_eq!(board["stories"], json!(["MUS-103", "RAL-80", "RAL-81"]));
        assert_eq!(
            board["teams"],
            json!(["muster", "rally"]),
            "listed teams stay, whatever their age"
        );
    }

    #[test]
    fn without_a_roster_or_a_ledger_every_project_is_shown() {
        let ledger = json!({"available": true, "stories": {}});
        let unread = json!({"available": false, "reason": "still reading it."});
        for rally in [
            current_rally(&rally_projects(), &ledger, &unread),
            current_rally(&rally_projects(), &unread, &rally_roster(json!([]))),
        ] {
            assert_eq!(rally["known"], false);
            assert!(
                rally["reason"]
                    .as_str()
                    .unwrap()
                    .contains("every project is shown")
            );
            assert!(in_rally(&rally, "PDX") && in_rally(&rally, "RAL"));
        }
    }

    #[test]
    fn the_roster_keeps_only_what_the_page_shows() {
        let roster = json!({"sessions": [{
            "wake": "waiting",
            "nickname": "x",
            "observed": {"waiting": true},
            "attested": {"machine": "a1", "operator": "someone@example.com"},
            "declared": {"project": "rally", "current_task": "RAL-1",
                         "activity": "busy", "last_activity_at": 5, "cwd": "/secret"},
        }]});
        let shaped = shape_roster(&roster, 9);
        let text = shaped.to_string();
        assert!(!text.contains("/secret"), "{text}");
        assert!(!text.contains("example.com"), "{text}");
        assert_eq!(shaped["sessions"][0]["machine"], "a1");
        assert_eq!(shaped["sessions"][0]["nickname"], "x");
        assert_eq!(shaped["sessions"][0]["wake_observed"], true);
        let declared_wake = json!({"sessions": [{"wake": "channel", "declared": {}}]});
        assert_eq!(
            shape_roster(&declared_wake, 9)["sessions"][0]["wake_observed"],
            false
        );
        assert_eq!(shaped["sessions"][0]["current_task"], "RAL-1");
        assert_eq!(shape_roster(&json!("nope"), 9)["available"], false);
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_command_is_given_up_on() {
        let mut command = Command::new("sleep");
        command.arg("30");
        let started = Instant::now();
        let error = run_with_timeout(command, Duration::from_millis(100)).unwrap_err();
        assert!(error.contains("didn't finish"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn a_command_with_a_lot_of_output_finishes() {
        let mut command = Command::new("sh");
        command.args(["-c", "head -c 1000000 /dev/zero"]);
        let output = run_with_timeout(command, Duration::from_secs(10)).unwrap();
        assert_eq!(output.len(), 1_000_000);
    }

    #[cfg(unix)]
    #[test]
    fn something_left_holding_the_output_open_is_given_up_on() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & exit 0"]);
        let started = Instant::now();
        let error = run_with_timeout(command, Duration::from_millis(300)).unwrap_err();
        assert!(error.contains("output"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_command_is_an_error() {
        let mut command = Command::new("sh");
        command.args(["-c", "exit 3"]);
        assert!(run_with_timeout(command, Duration::from_secs(5)).is_err());
    }

    #[test]
    fn a_command_that_prints_too_much_is_refused() {
        let mut command = Command::new("sh");
        command.args(["-c", "head -c 9000000 /dev/zero"]);
        let error = run_with_timeout(command, Duration::from_secs(10)).unwrap_err();
        assert!(error.contains("too much"), "{error}");
    }

    #[test]
    fn fences_open_and_close_as_markdown_says() {
        let count = |text: &str| parse_roadmap(text).0;
        // A longer fence isn't closed by a shorter one.
        let longer = "````\n```\n- [x] inside\n````\n- [ ] outside\n";
        assert_eq!((count(longer).ticked, count(longer).total), (0, 1));
        // Nor by the other kind.
        let other = "```\n~~~\n- [x] inside\n```\n- [ ] outside\n";
        assert_eq!((count(other).ticked, count(other).total), (0, 1));
        // A closing fence can't have text after it.
        let text_after = "```\n``` not a close\n- [x] inside\n```\n- [ ] outside\n";
        assert_eq!((count(text_after).ticked, count(text_after).total), (0, 1));
        // Four spaces of indent is code, not a fence, so what follows still counts.
        let indented = "    ```\n- [x] counted\n";
        assert_eq!((count(indented).ticked, count(indented).total), (1, 1));
        // An info string is fine on an opening fence.
        let info = "```rust\n- [x] inside\n```\n- [ ] outside\n";
        assert_eq!((count(info).ticked, count(info).total), (0, 1));
    }

    #[test]
    fn each_loopback_address_allows_its_own_host() {
        let hosts = allowed_hosts("127.0.0.2:7300".parse().unwrap()).unwrap();
        assert!(hosts.contains(&"127.0.0.2:7300".to_owned()), "{hosts:?}");
        assert!(!hosts.contains(&"localhost:7300".to_owned()), "{hosts:?}");
        assert!(!hosts.contains(&"127.0.0.1:7300".to_owned()), "{hosts:?}");
        let hosts = allowed_hosts("127.0.0.1:7300".parse().unwrap()).unwrap();
        assert_eq!(hosts, ["127.0.0.1:7300", "localhost:7300"]);
        let hosts = allowed_hosts("[::1]:7300".parse().unwrap()).unwrap();
        assert_eq!(hosts, ["[::1]:7300", "localhost:7300"]);
    }

    #[test]
    fn the_overview_shows_the_watchmans_writeup_never_rallys_own_summary() {
        let projects: Vec<Project> = serde_json::from_value(json!([
            {"name": "Muster", "repo": "Castor-o7/muster", "path": "docs/roadmap.md",
             "prefix": "MUS", "tick_meaning": "x"},
            {"name": "Rally", "repo": RALLY_REPO, "path": "docs/roadmap.md",
             "prefix": "RAL", "tick_meaning": "x"},
        ]))
        .unwrap();
        let rows = [
            json!({"summary": {"text": "muster's day"}}),
            json!({"summary": {"text": "rally's day"}, "watchman_summary": {"text": "the muster"}}),
        ];
        assert_eq!(muster_summary(&projects, &rows)["text"], "the muster");
        // With no writeup read yet, Rally's own summary isn't put in its place.
        let rows = [rows[0].clone(), json!({"summary": {"text": "rally's day"}})];
        assert!(muster_summary(&projects, &rows).get("text").is_none());
    }

    #[test]
    fn only_loopback_addresses_are_allowed() {
        assert!(allowed_hosts("127.0.0.1:7300".parse().unwrap()).is_ok());
        assert!(allowed_hosts("[::1]:7300".parse().unwrap()).is_ok());
        assert!(allowed_hosts("192.168.1.5:7300".parse().unwrap()).is_err());
        assert!(allowed_hosts("0.0.0.0:7300".parse().unwrap()).is_err());
    }

    #[test]
    fn the_page_never_builds_markup_from_text() {
        for banned in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "eval(",
        ] {
            assert!(!APP_JS.contains(banned), "{banned}");
        }
        assert!(!INDEX.contains("<script>"), "no inline script");
        assert!(!INDEX.contains("style="), "no inline style");
        assert!(!INDEX.contains("http://") && !INDEX.contains("https://"));
        for banned in ["<script", "style=", "http://", "https://"] {
            assert!(!SIGN_IN.contains(banned), "sign-in page: {banned}");
        }
        // The code is the only thing put into it, once, as a form value.
        assert_eq!(SIGN_IN.matches("{{CODE}}").count(), 1);
        assert!(SIGN_IN.contains(r#"method="post" action="/""#));
    }

    #[test]
    fn the_default_projects_can_be_read() {
        let projects: Vec<Project> = serde_json::from_str(DEFAULT_PROJECTS).unwrap();
        assert!(projects.iter().any(|project| project.prefix == "RAL"));
    }
}
