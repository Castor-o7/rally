//! `rally next`: the story a team takes next, by a rule it can follow alone (RAL-56).
//!
//! The pick is a pure function of the roadmap's text, the ledger's stories and blockers, the
//! window agreed with the operator, and the time, so it can be tested without a service or a
//! clock. It writes nothing: a skip is recorded with `rally block`, and a shelve with
//! `rally shelve`, each a command the team runs itself.
//!
//! In order, a team's next story is:
//!
//! 1. one of its own that was sent back, which comes ahead of new work;
//! 2. one assigned to it (RAL-130), ready and not blocked, in roadmap order, then oldest
//!    assignment first;
//! 3. one it shelved whose question the operator has since answered, oldest shelf first;
//! 4. the first story in roadmap order that's unticked, not held or verified in the ledger, not
//!    blocked, not assigned to another team, and not waiting on an unfinished story it depends on.
//!
//! An assignment is declared and claims nothing: a story assigned to another team is skipped and
//! says so, and the team may still claim it.
//!
//! A story waiting on the operator isn't skipped: it stays the pick until the window passes,
//! then the team can shelve it and take the next one.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::ledger::{OPERATOR, same_name};
use crate::web::parse_stories;

/// A story on the roadmap, with every line of its item, for its "Depends on".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub ticked: bool,
    text: String,
}

/// The ID prefix most of a roadmap's stories use, such as `RAL`.
pub fn prefix_of(roadmap: &str) -> Option<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for line in roadmap.lines() {
        let line = line.trim_start();
        let Some(rest) = line
            .strip_prefix("- [")
            .and_then(|rest| rest.get(2..))
            .and_then(|rest| rest.trim_start().strip_prefix("**"))
        else {
            continue;
        };
        let Some((id, _)) = rest.split_once(':') else {
            continue;
        };
        if let Some((prefix, number)) = id.split_once('-')
            && !prefix.is_empty()
            && prefix.chars().all(|c| c.is_ascii_uppercase())
            && !number.is_empty()
            && number.chars().all(|c| c.is_ascii_digit())
        {
            *counts.entry(prefix.to_owned()).or_default() += 1;
        }
    }
    // Most stories first, then the first prefix alphabetically, so a tie always gives one answer.
    counts
        .into_iter()
        .max_by(|(a, x), (b, y)| x.cmp(y).then(b.cmp(a)))
        .map(|(prefix, _)| prefix)
}

/// The roadmap's stories with an ID under `prefix`, in order, each with its whole item's text.
pub fn items(roadmap: &str, prefix: &str) -> Vec<Item> {
    let lines: Vec<&str> = roadmap.lines().collect();
    parse_stories(roadmap, prefix)
        .into_iter()
        .filter_map(|story| {
            let id = story.id?;
            // The item's own line, then each indented line after it.
            let start = story.line - 1;
            let mut text = lines[start].to_owned();
            for line in &lines[start + 1..] {
                if line.trim().is_empty() || !line.starts_with(' ') {
                    break;
                }
                text.push(' ');
                text.push_str(line.trim());
            }
            Some(Item {
                id,
                title: story.title,
                ticked: story.ticked,
                text,
            })
        })
        .collect()
}

/// The stories an item says it depends on, as `Depends on RAL-46`, `Depends on MUS-4 and MUS-44`
/// or `Depends on RAL-54 to 57`, up to the end of that sentence.
pub fn depends_on(item: &Item) -> Vec<String> {
    let Some((_, rest)) = item.text.split_once("Depends on ") else {
        return Vec::new();
    };
    let sentence = rest.split(". ").next().unwrap_or(rest);
    let words: Vec<&str> = sentence
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|word| !word.is_empty())
        .collect();
    let mut ids = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let word = words[index].trim_end_matches(['.', ';', ')']);
        if let Some((prefix, number)) = word.split_once('-')
            && !prefix.is_empty()
            && prefix.chars().all(|c| c.is_ascii_uppercase())
            && let Ok(first) = number.parse::<u64>()
        {
            // `RAL-54 to 57`: a range, which is never long in a roadmap.
            let last = (words.get(index + 1) == Some(&"to"))
                .then(|| words.get(index + 2))
                .flatten()
                .and_then(|end| end.trim_end_matches(['.', ';', ')']).parse::<u64>().ok())
                .filter(|last| *last >= first && *last - first <= 50);
            for number in first..=last.unwrap_or(first) {
                ids.push(format!("{prefix}-{number}"));
            }
            if last.is_some() {
                index += 2;
            }
        }
        index += 1;
    }
    ids
}

/// What the pick reads from the ledger, as the service's views give it.
pub struct Ledger<'a> {
    /// `/v1/stories`: each story's `state` and `team`.
    pub stories: &'a serde_json::Map<String, Value>,
    /// `/v1/blockers/all`: every blocker, open and closed. Each open question to the operator
    /// carries the ledger's own `question`: when its window starts and whether it's shelved.
    pub blockers: &'a [Value],
    /// `/v1/blockers/all`'s `operator_away`, the operator's latest away hour, if it's in force.
    pub away: Option<&'a Value>,
}

fn is_open(blocker: &Value) -> bool {
    blocker.get("closed").is_none_or(Value::is_null)
}

fn on_operator(blocker: &Value) -> bool {
    blocker["on_team"]
        .as_str()
        .is_some_and(|team| same_name(team, OPERATOR))
}

fn names(blocker: &Value, key: &str, value: &str) -> bool {
    blocker[key]
        .as_str()
        .is_some_and(|found| same_name(found, value))
}

fn state_of<'a>(ledger: &'a Ledger, id: &str) -> (&'a str, Option<&'a str>) {
    match ledger.stories.get(id) {
        Some(story) => (
            story["state"].as_str().unwrap_or("ready"),
            story["team"].as_str(),
        ),
        None => ("ready", None),
    }
}

/// Whether a story it depends on is done: ticked on this roadmap, or verified or merged in the
/// ledger, for another team's.
fn done(id: &str, roadmap: &[Item], ledger: &Ledger) -> bool {
    roadmap.iter().any(|item| item.id == id && item.ticked)
        || ledger
            .stories
            .get(id)
            .is_some_and(|story| story["state"] == "verified" || story["merged"] == true)
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The team's next story and everything that bears on it, as JSON. `window_ms` is the window
/// agreed with the operator, or `None` while there isn't one; `now_ms` is the reader's time.
pub fn pick(
    team: &str,
    roadmap: &[Item],
    ledger: &Ledger,
    window_ms: Option<u64>,
    now_ms: i64,
) -> Value {
    let mut current = Vec::new();
    let mut sent_back = Vec::new();
    for (id, story) in ledger.stories {
        if !story["team"].as_str().is_some_and(|t| same_name(t, team)) {
            continue;
        }
        match story["state"].as_str() {
            Some(state @ ("claimed" | "in_review")) => {
                current.push(json!({"story": id, "state": state}));
            }
            Some("sent_back") => sent_back.push(id.clone()),
            _ => {}
        }
    }

    // While the operator is away, nothing waiting on the operator is offered. When each question
    // starts again, and whether it's shelved, is the ledger's to say, as of the service's clock:
    // the pick only reads it.
    let away_active = ledger.away.is_some();

    // The team's questions to the operator. A story's newest open question decides it; an
    // answered shelf brings it back only when no question on it is open.
    let mut open_questions: BTreeMap<String, &Value> = BTreeMap::new();
    let mut answered_shelves: BTreeMap<String, (u64, &Value)> = BTreeMap::new();
    for blocker in ledger.blockers {
        let Some(story) = blocker["story"].as_str() else {
            continue;
        };
        if !on_operator(blocker) || !names(blocker, "team", team) {
            continue;
        }
        let seq = blocker["seq"].as_u64().unwrap_or(0);
        if is_open(blocker) {
            let newer = open_questions
                .get(story)
                .is_none_or(|other| other["seq"].as_u64().unwrap_or(0) < seq);
            if newer {
                open_questions.insert(story.to_owned(), blocker);
            }
        } else if let Some(shelf) = blocker["shelves"].as_array().and_then(|s| s.last()) {
            let shelf_seq = shelf["seq"].as_u64().unwrap_or(0);
            if answered_shelves
                .get(story)
                .is_none_or(|(other, _)| *other < shelf_seq)
            {
                answered_shelves.insert(story.to_owned(), (shelf_seq, blocker));
            }
        }
    }
    let mut asking: BTreeMap<String, (&Value, i64)> = BTreeMap::new();
    let mut shelved = Vec::new();
    let mut answered: Vec<(u64, String)> = Vec::new();
    for (story, blocker) in &open_questions {
        let question = &blocker["question"];
        let start = question["window_from_ms"]
            .as_i64()
            .unwrap_or_else(|| blocker["at_ms"].as_i64().unwrap_or(0));
        if question["shelved"] == true {
            let shelf = blocker["shelves"].as_array().and_then(|s| s.last());
            shelved.push(json!({
                "story": story, "blocker": blocker["seq"], "question": blocker["note"],
                "asked_at_ms": blocker["at_ms"],
                "shelved_at_ms": shelf.map(|shelf| shelf["at_ms"].clone()), "answered": false,
            }));
        } else {
            asking.insert(story.clone(), (blocker, start));
        }
    }
    for (story, (shelf_seq, blocker)) in &answered_shelves {
        if open_questions.contains_key(story) {
            continue;
        }
        let (state, _) = state_of(ledger, story);
        let ticked = roadmap.iter().any(|item| &item.id == story && item.ticked);
        if state == "ready" && !ticked {
            answered.push((*shelf_seq, story.clone()));
            let shelf = blocker["shelves"].as_array().and_then(|s| s.last());
            shelved.push(json!({
                "story": story, "blocker": blocker["seq"], "question": blocker["note"],
                "asked_at_ms": blocker["at_ms"],
                "shelved_at_ms": shelf.map(|shelf| shelf["at_ms"].clone()),
                "answered": true, "answer": blocker["closed"]["note"],
            }));
        }
    }
    answered.sort();
    let shelved_open: Vec<&str> = shelved
        .iter()
        .filter(|shelf| shelf["answered"] == false)
        .filter_map(|shelf| shelf["story"].as_str())
        .collect();

    // Open blockers on a story, other than the team's own question to the operator.
    let blocked_on = |id: &str| {
        ledger.blockers.iter().find(|blocker| {
            is_open(blocker)
                && names(blocker, "story", id)
                && !(on_operator(blocker) && names(blocker, "team", team))
        })
    };
    // The first story `id` depends on that isn't done, by its roadmap item's "Depends on".
    let unfinished_dep = |id: &str| {
        roadmap.iter().find(|item| item.id == id).and_then(|item| {
            depends_on(item)
                .into_iter()
                .find(|dep| dep != id && !done(dep, roadmap, ledger))
        })
    };
    // Stories assigned to this team (RAL-130), ready, not blocked and not waiting on an unfinished
    // story they depend on: in roadmap order, then the oldest assignment first.
    let position = |id: &str| roadmap.iter().position(|item| item.id == id);
    let mut assigned: Vec<(usize, u64, String)> = ledger
        .stories
        .iter()
        .filter(|(_, story)| {
            story["assigned"]["team"]
                .as_str()
                .is_some_and(|t| same_name(t, team))
        })
        .filter(|(id, story)| {
            story["state"] == "ready"
                && blocked_on(id).is_none()
                && !roadmap.iter().any(|item| &item.id == *id && item.ticked)
                && unfinished_dep(id).is_none()
        })
        .map(|(id, story)| {
            (
                position(id).unwrap_or(usize::MAX),
                story["assigned"]["seq"].as_u64().unwrap_or(0),
                id.clone(),
            )
        })
        .collect();
    assigned.sort();

    let mut skipped = Vec::new();
    let mut candidates: Vec<&Item> = Vec::new();
    for item in roadmap.iter().filter(|item| !item.ticked) {
        let id = item.id.as_str();
        let (state, holder) = state_of(ledger, id);
        let reason = match (state, holder) {
            ("ready", _) => None,
            // The team's own: shown as current, or first as sent back.
            ("claimed" | "in_review" | "sent_back", Some(holder)) if same_name(holder, team) => {
                continue;
            }
            ("claimed", holder) => Some(format!("claimed by {}", holder.unwrap_or("a team"))),
            ("in_review", holder) => {
                Some(format!("in review, from {}", holder.unwrap_or("a team")))
            }
            ("sent_back", holder) => Some(format!("sent back to {}", holder.unwrap_or("a team"))),
            ("verified", _) => Some("verified, waiting for its tick".to_owned()),
            (other, _) => Some(format!("in the ledger as {other}")),
        };
        if let Some(reason) = reason {
            skipped.push(json!({"story": id, "reason": reason}));
            continue;
        }
        // Shown under `shelved`, and first once answered.
        if shelved_open.contains(&id) || answered.iter().any(|(_, story)| story == id) {
            continue;
        }
        // Held while the operator is away, and shown under `waiting_on_operator`.
        if away_active && asking.contains_key(id) {
            continue;
        }
        // Coordinated by whoever assigned it: shown, not offered, and still free to claim.
        if let Some(other) = ledger
            .stories
            .get(id)
            .and_then(|s| s["assigned"]["team"].as_str())
        {
            if !same_name(other, team) {
                skipped.push(json!({
                    "story": id,
                    "reason": format!(
                        "assigned to {other} (declared); you may still claim it if you agree that \
                         with them"
                    ),
                }));
                continue;
            }
            // The team's own, offered above unless something blocks it, which is said below.
            if assigned.iter().any(|(_, _, a)| a == id) {
                continue;
            }
        }
        if let Some(blocker) = blocked_on(id) {
            let on = blocker["on_team"]
                .as_str()
                .map(|team| format!("the team {team}"))
                .or_else(|| {
                    blocker["on_story"]
                        .as_str()
                        .map(|s| format!("the story {s}"))
                })
                .unwrap_or_else(|| "something".to_owned());
            skipped.push(json!({
                "story": id, "reason": format!("blocked: waits on {on}"), "blocker": blocker["seq"],
            }));
            continue;
        }
        let unfinished = depends_on(item)
            .into_iter()
            .find(|dep| dep != id && !done(dep, roadmap, ledger));
        if let Some(dep) = unfinished {
            let note = format!("depends on {dep}, which isn't done");
            skipped.push(json!({
                "story": id, "reason": note,
                "record_with": format!(
                    "rally block --team {} --story {id} --on-story {dep} --note {}",
                    shell_quote(team), shell_quote(&note),
                ),
            }));
            continue;
        }
        candidates.push(item);
    }

    let pick_of = |id: &str, why: &str| {
        let title = roadmap
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.title.clone());
        json!({"story": id, "title": title, "why": why})
    };
    let mut waiting_on_operator = Vec::new();
    let mut after_shelving = Value::Null;
    let chosen = if let Some(id) = sent_back.first() {
        Some(pick_of(
            id,
            "it was sent back to your team, which comes ahead of new work",
        ))
    } else if let Some((_, _, id)) = assigned.first() {
        let mut next = pick_of(id, "it was assigned to your team");
        if let Some(story) = ledger.stories.get(id) {
            next["assigned"] = story["assigned"].clone();
        }
        Some(next)
    } else if let Some((_, id)) = answered.first() {
        Some(pick_of(
            id,
            "the operator answered the question it was shelved on",
        ))
    } else if let Some(first) = candidates.first() {
        let mut next = pick_of(&first.id, "next in roadmap order");
        if let Some((question, start)) = asking.get(&first.id) {
            let due_at = window_ms.map(|window| start.saturating_add(window as i64));
            let due = due_at.is_some_and(|due_at| now_ms >= due_at);
            let mut wait = json!({
                "story": first.id, "blocker": question["seq"], "question": question["note"],
                "asked_at_ms": question["at_ms"], "window_from_ms": start,
                "shelve_due_at_ms": due_at, "shelve_due": due,
            });
            if question["at_ms"].as_i64() != Some(*start) {
                wait["ask_again"] = json!(true);
            }
            if due {
                wait["run"] = json!(format!(
                    "rally shelve {} --team {}",
                    question["seq"],
                    shell_quote(team)
                ));
            }
            waiting_on_operator.push(wait);
            next["why"] = json!("it waits on the operator's answer, and isn't skipped for that");
            if let Some(then) = candidates.get(1) {
                after_shelving = pick_of(&then.id, "next in roadmap order");
            }
        }
        Some(next)
    } else {
        None
    };
    // Other questions to the operator, on stories that aren't the pick.
    for (id, (question, start)) in &asking {
        if chosen.as_ref().is_some_and(|c| c["story"] == id.as_str()) {
            continue;
        }
        let mut wait = json!({
            "story": id, "blocker": question["seq"], "question": question["note"],
            "asked_at_ms": question["at_ms"], "window_from_ms": start,
        });
        if away_active {
            wait["held"] = json!("the operator is away: don't ask now, it's offered again after");
        }
        waiting_on_operator.push(wait);
    }
    let operator_away = match ledger.away {
        Some(away) => json!({
            "until_ms": away["ends_at_ms"],
            "note": "The operator is treated as away: no story waiting on the operator is offered \
                     until then, and no new questions should be asked. An answer ends it early.",
        }),
        _ => Value::Null,
    };

    let window_note = match window_ms {
        Some(_) => Value::Null,
        None => json!(
            "No window has been agreed with the operator, so nothing can be shelved yet: a \
             story waiting on the operator stays the pick."
        ),
    };
    json!({
        "team": team,
        "current": current,
        "next": chosen,
        "after_shelving": after_shelving,
        "waiting_on_operator": waiting_on_operator,
        "shelved": shelved,
        "skipped": skipped,
        "nothing_left": chosen.is_none(),
        "operator_window_ms": window_ms,
        "window_note": window_note,
        "operator_away": operator_away,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROADMAP: &str = "\
# Roadmap

## Phase 1

- [x] **RAL-1: Done already.** Done when: it is.
- [ ] **RAL-2: Waits on another story.** Depends on RAL-3. Done when: it
  works.
- [ ] **RAL-3: The first free one.** Done when: it works.
- [ ] **RAL-4: Waits on Muster.** Depends on MUS-4 and MUS-44. Done when: it works.
- [ ] **RAL-5: Free too.** Done when: it works.
- [ ] Not a story, no ID.

```
- [ ] **RAL-99: In a code fence.**
```
";

    const HOUR: u64 = 3_600_000;

    fn stories(rows: Value) -> serde_json::Map<String, Value> {
        rows.as_object().unwrap().clone()
    }

    /// An open question to the operator, as the blockers view gives it.
    fn question(seq: u64, story: &str, team: &str, at_ms: i64) -> Value {
        json!({"seq": seq, "team": team, "story": story, "on_team": "operator",
               "note": "may we?", "at_ms": at_ms,
               "question": {"window_from_ms": at_ms, "shelved": false}})
    }

    fn run(stories: &serde_json::Map<String, Value>, blockers: &[Value], now: i64) -> Value {
        let roadmap = items(ROADMAP, "RAL");
        let ledger = Ledger {
            stories,
            blockers,
            away: None,
        };
        pick("rally", &roadmap, &ledger, Some(HOUR), now)
    }

    fn skipped(result: &Value) -> Vec<(String, String)> {
        result["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["story"].as_str().unwrap().to_owned(),
                    s["reason"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn reads_the_roadmap_in_order_with_each_items_whole_text() {
        assert_eq!(prefix_of(ROADMAP).as_deref(), Some("RAL"));
        let found = items(ROADMAP, "RAL");
        let ids: Vec<&str> = found.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["RAL-1", "RAL-2", "RAL-3", "RAL-4", "RAL-5"]);
        assert_eq!(depends_on(&found[1]), ["RAL-3"]);
        assert_eq!(depends_on(&found[3]), ["MUS-4", "MUS-44"]);
        let range = Item {
            id: "RAL-58".into(),
            title: String::new(),
            ticked: false,
            text: "Depends on RAL-54 to 57. The operator decides when.".into(),
        };
        assert_eq!(depends_on(&range), ["RAL-54", "RAL-55", "RAL-56", "RAL-57"]);
    }

    #[test]
    fn the_first_free_story_in_roadmap_order_and_why_others_were_skipped() {
        let none = stories(json!({}));
        let result = run(&none, &[], 0);
        assert_eq!(result["next"]["story"], "RAL-3");
        assert_eq!(result["next"]["title"], "The first free one");
        assert_eq!(
            skipped(&result),
            [
                ("RAL-2".into(), "depends on RAL-3, which isn't done".into()),
                ("RAL-4".into(), "depends on MUS-4, which isn't done".into()),
            ]
        );
        // The skip can be recorded in one command.
        let record = result["skipped"][0]["record_with"].as_str().unwrap();
        assert!(record.starts_with("rally block --team 'rally' --story RAL-2 --on-story RAL-3"));
        assert_eq!(result["nothing_left"], false);
    }

    #[test]
    fn each_kind_of_skip() {
        let held = stories(json!({
            "RAL-3": {"state": "claimed", "team": "muster"},
            "RAL-5": {"state": "verified", "team": "muster"},
            "MUS-4": {"state": "verified", "team": "muster"},
            "MUS-44": {"state": "in_review", "team": "muster", "merged": true},
        }));
        let blockers = [
            json!({"seq": 9, "team": "rally", "story": "RAL-4", "on_team": "catapp",
                               "note": "waits on catapp", "at_ms": 1}),
        ];
        let result = run(&held, &blockers, 0);
        assert_eq!(
            skipped(&result),
            [
                ("RAL-2".into(), "depends on RAL-3, which isn't done".into()),
                ("RAL-3".into(), "claimed by muster".into()),
                ("RAL-4".into(), "blocked: waits on the team catapp".into()),
                ("RAL-5".into(), "verified, waiting for its tick".into()),
            ]
        );
        assert_eq!(result["skipped"][2]["blocker"], 9);
        assert!(result["next"].is_null());
        assert_eq!(result["nothing_left"], true);
        // Once the blocker is closed, MUS-4 and MUS-44 being done frees RAL-4.
        let closed = [
            json!({"seq": 9, "team": "rally", "story": "RAL-4", "on_team": "catapp",
                             "note": "n", "at_ms": 1, "closed": {"seq": 10}}),
        ];
        assert_eq!(run(&held, &closed, 0)["next"]["story"], "RAL-4");
    }

    /// RAL-130: an assigned story comes after sent-back work and ahead of roadmap order; one
    /// assigned to another team is skipped, saying so; an assigned story that's blocked isn't
    /// offered, and says why.
    #[test]
    fn assigned_stories_come_before_roadmap_order() {
        let roadmap = items(
            "- [ ] **RAL-1: a.**\n- [ ] **RAL-2: b.**\n- [ ] **RAL-3: c.**\n- [ ] **RAL-4: d.**\n",
            "RAL",
        );
        let stories = json!({
            "RAL-2": {"state": "ready", "assigned": {"team": "muster", "seq": 7}},
            "RAL-3": {"state": "ready", "assigned": {"team": "rally", "seq": 8, "by": "watch"}},
            "RAL-4": {"state": "ready", "assigned": {"team": "rally", "seq": 9}},
        });
        let blockers = vec![json!({"seq": 10, "team": "rally", "story": "RAL-3",
                                   "on_team": "muster", "note": "needs MUS-1"})];
        let pick_with = |blockers: &[Value]| {
            pick(
                "rally",
                &roadmap,
                &Ledger {
                    stories: stories.as_object().unwrap(),
                    blockers,
                    away: None,
                },
                None,
                0,
            )
        };
        let out = pick_with(&[]);
        assert_eq!(out["next"]["story"], "RAL-3", "{out}");
        assert_eq!(out["next"]["why"], "it was assigned to your team");
        assert_eq!(out["next"]["assigned"]["by"], "watch");
        let skipped = out["skipped"].as_array().unwrap();
        assert!(skipped.iter().any(|s| s["story"] == "RAL-2"
            && s["reason"].as_str().unwrap().contains("assigned to muster")));
        // Blocked: the next assigned one, and RAL-3 says why.
        let out = pick_with(&blockers);
        assert_eq!(out["next"]["story"], "RAL-4", "{out}");
        assert!(out["skipped"].as_array().unwrap().iter().any(|s| s["story"] == "RAL-3"
            && s["reason"].as_str().unwrap().contains("blocked")));
        // An assigned story that depends on an unfinished one isn't offered, and says why.
        let dep_roadmap = items(
            "- [ ] **RAL-1: a.**\n- [ ] **RAL-3: c.** Depends on RAL-1.\n- [ ] **RAL-4: d.**\n",
            "RAL",
        );
        let out = pick(
            "rally",
            &dep_roadmap,
            &Ledger {
                stories: stories.as_object().unwrap(),
                blockers: &[],
                away: None,
            },
            None,
            0,
        );
        assert_eq!(out["next"]["story"], "RAL-4", "{out}");
        assert!(
            out["skipped"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["story"] == "RAL-3"
                    && s["reason"].as_str().unwrap().contains("depends on RAL-1")),
            "{out}"
        );
        // Sent-back work still comes first.
        let mut with_back = stories.clone();
        with_back["RAL-1"] = json!({"state": "sent_back", "team": "rally"});
        let out = pick(
            "rally",
            &roadmap,
            &Ledger {
                stories: with_back.as_object().unwrap(),
                blockers: &[],
                away: None,
            },
            None,
            0,
        );
        assert_eq!(out["next"]["story"], "RAL-1");
    }

    #[test]
    fn sent_back_work_comes_first_and_current_work_is_shown() {
        let ours = stories(json!({
            "RAL-5": {"state": "sent_back", "team": "rally"},
            "RAL-9": {"state": "claimed", "team": "Rally"},
        }));
        let result = run(&ours, &[], 0);
        assert_eq!(result["next"]["story"], "RAL-5");
        assert_eq!(
            result["current"],
            json!([{"story": "RAL-9", "state": "claimed"}])
        );
    }

    #[test]
    fn a_story_waiting_on_the_operator_is_not_skipped_until_the_window_passes() {
        let none = stories(json!({}));
        let asked = [question(7, "RAL-3", "rally", 1_000)];
        // Before the window: still the pick, and no shelve offered.
        let before = run(&none, &asked, 1_000 + HOUR as i64 - 1);
        assert_eq!(before["next"]["story"], "RAL-3");
        let wait = &before["waiting_on_operator"][0];
        assert_eq!(wait["shelve_due"], false);
        assert_eq!(wait["shelve_due_at_ms"], 1_000 + HOUR as i64);
        assert!(wait.get("run").is_none());
        assert_eq!(before["after_shelving"]["story"], "RAL-5");
        // After it: still the pick until the team shelves it, which it's told how to do.
        let after = run(&none, &asked, 1_000 + HOUR as i64);
        assert_eq!(after["next"]["story"], "RAL-3");
        assert_eq!(
            after["waiting_on_operator"][0]["run"],
            "rally shelve 7 --team 'rally'"
        );
        // Without an agreed window, it's never due.
        let roadmap = items(ROADMAP, "RAL");
        let ledger = Ledger {
            stories: &none,
            blockers: &asked,
            away: None,
        };
        let unagreed = pick("rally", &roadmap, &ledger, None, i64::MAX);
        assert_eq!(unagreed["waiting_on_operator"][0]["shelve_due"], false);
        assert!(unagreed["window_note"].is_string());
    }

    #[test]
    fn a_shelved_story_steps_aside_and_returns_first_when_the_operator_answers() {
        let none = stories(json!({}));
        let mut shelf = question(7, "RAL-3", "rally", 1_000);
        shelf["shelves"] = json!([{"seq": 8, "at_ms": 1_000 + HOUR as i64}]);
        shelf["question"]["shelved"] = json!(true);
        let shelved = run(&none, std::slice::from_ref(&shelf), 2 * HOUR as i64);
        assert_eq!(shelved["next"]["story"], "RAL-5");
        assert_eq!(shelved["shelved"][0]["story"], "RAL-3");
        assert_eq!(shelved["shelved"][0]["question"], "may we?");
        assert_eq!(shelved["shelved"][0]["answered"], false);
        // The answer brings it back to the top, ahead of RAL-5.
        shelf["closed"] = json!({"seq": 12, "note": "yes, go ahead"});
        shelf.as_object_mut().unwrap().remove("question");
        let answered = run(&none, &[shelf.clone()], 3 * HOUR as i64);
        assert_eq!(answered["next"]["story"], "RAL-3");
        assert_eq!(
            answered["next"]["why"],
            "the operator answered the question it was shelved on"
        );
        assert_eq!(answered["shelved"][0]["answered"], true);
        // Once claimed, it's current work, not a shelf.
        let claimed = stories(json!({"RAL-3": {"state": "claimed", "team": "rally"}}));
        let taken = run(&claimed, &[shelf], 3 * HOUR as i64);
        assert_eq!(taken["shelved"], json!([]));
        assert_eq!(taken["next"]["story"], "RAL-5");
    }

    #[test]
    fn another_teams_question_to_the_operator_blocks_the_story() {
        let none = stories(json!({}));
        let theirs = [question(7, "RAL-3", "muster", 1_000)];
        let result = run(&none, &theirs, 0);
        assert_eq!(result["next"]["story"], "RAL-5");
        assert_eq!(
            skipped(&result)[1],
            ("RAL-3".into(), "blocked: waits on the team operator".into())
        );
    }

    /// The pick as `rally next` makes it, from a real ledger through the views, at `now`.
    fn through_views(records: &[crate::record::Record], now: i64) -> Value {
        let ledger = crate::ledger::Ledger::replay(records).unwrap();
        let page = crate::views::Page::default();
        let stories = crate::views::stories(&ledger, None, &page).unwrap();
        let blockers = crate::views::blockers_at(&ledger, true, &page, now).unwrap();
        let away = (blockers["operator_away_active"] == true).then_some(&blockers["operator_away"]);
        let view = Ledger {
            stories: stories["stories"].as_object().unwrap(),
            blockers: blockers["blockers"].as_array().unwrap(),
            away,
        };
        pick("rally", &items(ROADMAP, "RAL"), &view, Some(HOUR), now)
    }

    fn declared(at_ms: i64, body: Value) -> crate::record::Record {
        let source = crate::record::Source {
            credential: "a1".into(),
            declared_name: Some("r".into()),
        };
        crate::record::Record::new(crate::record::Kind::Declared, source, at_ms, body).unwrap()
    }

    fn ask(at_ms: i64, story: &str) -> crate::record::Record {
        declared(
            at_ms,
            json!({"change": "block", "team": "rally", "story": story, "on_team": "operator",
                   "note": format!("may we do {story}?")}),
        )
    }

    fn shelve(at_ms: i64, blocker: u64) -> crate::record::Record {
        declared(
            at_ms,
            json!({"change": "shelve", "blocker": blocker, "team": "rally", "window_ms": HOUR}),
        )
    }

    fn answer(at_ms: i64, blocker: u64) -> crate::record::Record {
        declared(
            at_ms,
            json!({"change": "unblock", "blocker": blocker, "note": "yes",
                   "answer": {"from": "direct"}}),
        )
    }

    fn away(at_ms: i64, ends_at_ms: i64, shelves: &[u64]) -> crate::record::Record {
        let source = crate::record::Source {
            credential: "rally".into(),
            declared_name: None,
        };
        let body = json!({"change": "operator_away", "entered_at_ms": at_ms,
                          "ends_at_ms": ends_at_ms, "shelves": shelves});
        crate::record::Record::new(crate::record::Kind::Observed, source, at_ms, body).unwrap()
    }

    #[test]
    fn nothing_waiting_on_the_operator_is_offered_while_the_operator_is_away() {
        let h = HOUR as i64;
        // RAL-3 and RAL-5 asked; RAL-5 and two others shelved; then the operator is away.
        let records = vec![
            ask(1, "RAL-3"),
            ask(1, "RAL-5"),
            ask(1, "RAL-8"),
            ask(1, "RAL-9"),
            shelve(1 + h, 2),
            shelve(1 + h, 3),
            shelve(1 + h, 4),
            away(2 * h, 3 * h, &[5, 6, 7]),
        ];
        let during = through_views(&records, 2 * h + 5);
        // RAL-3's question is held and RAL-5 stays shelved, so nothing is left to offer.
        assert!(during["next"].is_null(), "{during}");
        assert_eq!(during["operator_away"]["until_ms"], 3 * h);
        assert_eq!(during["waiting_on_operator"][0]["story"], "RAL-3");
        assert!(during["waiting_on_operator"][0]["held"].is_string());
        assert_eq!(during["shelved"][0]["story"], "RAL-5");

        // After the hour both are revisited, in roadmap order, asked again from its end.
        let after = through_views(&records, 3 * h);
        assert!(after["operator_away"].is_null());
        assert_eq!(after["next"]["story"], "RAL-3");
        let wait = &after["waiting_on_operator"][0];
        assert_eq!(wait["window_from_ms"], 3 * h);
        assert_eq!(wait["ask_again"], true);
        assert_eq!(wait["shelve_due_at_ms"], 4 * h);
        assert_eq!(after["after_shelving"]["story"], "RAL-5");
        assert_eq!(after["shelved"], json!([]));

        // Ended early by an answer: the operator is back, so everything it held is asked again
        // from the answer's time, as when the hour runs out.
        let mut early = records.clone();
        early.push(answer(2 * h + 5, 3));
        let early = through_views(&early, 2 * h + 6);
        assert!(early["operator_away"].is_null());
        // The answered one first, as always; RAL-5 and RAL-3 asked again from the answer's time.
        assert_eq!(early["next"]["story"], "RAL-8");
        assert_eq!(window_from(&early, "RAL-3"), Some(json!(2 * h + 5)));
        assert_eq!(window_from(&early, "RAL-5"), Some(json!(2 * h + 5)));
        let shelved = early["shelved"].as_array().unwrap();
        assert_eq!(shelved.len(), 1);
        assert_eq!(shelved[0]["story"], "RAL-8");
        assert_eq!(shelved[0]["answered"], true);
    }

    fn window_from(result: &Value, story: &str) -> Option<Value> {
        result["waiting_on_operator"]
            .as_array()?
            .iter()
            .find(|wait| wait["story"] == story)
            .map(|wait| wait["window_from_ms"].clone())
    }

    /// modest-roach's case: RAL-3 is shelved, an away hour runs out, a second one starts from
    /// other shelves and an answer ends it early. RAL-3 must be asked again, not left shelved.
    #[test]
    fn a_question_held_through_two_away_hours_is_asked_again() {
        let h = HOUR as i64;
        let records = vec![
            ask(1, "RAL-3"),
            ask(1, "RAL-8"),
            ask(1, "RAL-9"),
            shelve(1 + h, 1),
            shelve(1 + h, 2),
            shelve(1 + h, 3),
            away(2 * h, 3 * h, &[4, 5, 6]),
            // After the first hour: three new shelves, not RAL-3's, start a second.
            ask(3 * h, "RAL-10"),
            shelve(4 * h, 2),
            shelve(4 * h, 3),
            shelve(4 * h, 8),
            away(4 * h + 1, 5 * h, &[9, 10, 11]),
            answer(4 * h + 10, 2),
        ];
        let result = through_views(&records, 4 * h + 20);
        assert!(result["operator_away"].is_null(), "{result}");
        // RAL-8's answer comes first; RAL-3 is asked again from the answer, not left shelved.
        assert_eq!(result["next"]["story"], "RAL-8", "{result}");
        assert_eq!(window_from(&result, "RAL-3"), Some(json!(4 * h + 10)));
        assert!(
            !result["shelved"]
                .as_array()
                .unwrap()
                .iter()
                .any(|shelf| shelf["story"] == "RAL-3")
        );
    }

    #[test]
    fn a_storys_newest_open_question_decides_it() {
        let h = HOUR as i64;
        // An old question shelved, then a new one asked on the same story.
        let records = vec![ask(1, "RAL-3"), shelve(1 + h, 1), ask(2 * h, "RAL-3")];
        let result = through_views(&records, 2 * h + 1);
        assert_eq!(result["next"]["story"], "RAL-3", "{result}");
        assert_eq!(result["waiting_on_operator"][0]["blocker"], 3);
        assert_eq!(result["shelved"], json!([]));
    }

    #[test]
    fn a_roadmap_with_nothing_left() {
        let done = "- [x] **RAL-1: One.**\n- [x] **RAL-2: Two.**\n";
        let roadmap = items(done, "RAL");
        let none = stories(json!({}));
        let ledger = Ledger {
            stories: &none,
            blockers: &[],
            away: None,
        };
        let result = pick("rally", &roadmap, &ledger, Some(HOUR), 0);
        assert!(result["next"].is_null());
        assert_eq!(result["nothing_left"], true);
        assert_eq!(result["skipped"], json!([]));
    }
}
