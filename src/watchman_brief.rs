//! The brief a restarted watchman reads first (RAL-157 part 1): short, scoped to the open rally,
//! and in the order a watchman that has lost its memory goes looking. That order is what changed
//! since its own last record, what each team last landed, what's verified and waiting to land,
//! what waits on it, what waits on the operator, and the newest rulings. Then each team's
//! assignment and claim, and counts, with the commands that show more.
//!
//! It's built from the ledger alone. What the ledger doesn't hold isn't guessed at: which
//! commits a watchman is gating together, and what's deployed, are said to be outside it. Every
//! list is bounded and every note cut short, so the whole stays well under 20 KB. `rally brief`
//! without `--watchman` keeps the full picture. Like the full brief, it confers no authority.

use serde_json::{Value, json};

use crate::designation::{self, OperatorKey, Watch};
use crate::ledger::{Change, Ledger, StandingKind, State};
use crate::views::UNTRUSTED_TEXT;

/// The most rows in each list.
pub const ROWS: usize = 12;
/// The longest a note is shown, in characters.
pub const NOTE_CHARS: usize = 160;

fn age(now_ms: i64, at_ms: i64) -> i64 {
    now_ms.saturating_sub(at_ms).max(0)
}

fn cut(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if flat.chars().count() <= NOTE_CHARS {
        flat
    } else {
        let short: String = flat.chars().take(NOTE_CHARS).collect();
        format!("{short}…")
    }
}

fn prefix(id: &str) -> &str {
    id.split_once('-').map_or(id, |(prefix, _)| prefix)
}

/// The change's name, as records name it (`claim`, `verify`, `merge`…).
fn change_name(change: &Change) -> String {
    serde_json::to_value(change)
        .ok()
        .and_then(|v| v["change"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "?".into())
}

/// At most [`ROWS`] of `rows`, newest first by their first element, and how many there were.
fn newest(mut rows: Vec<(u64, Value)>) -> (Vec<Value>, usize) {
    rows.sort_by_key(|(seq, _)| std::cmp::Reverse(*seq));
    let total = rows.len();
    (
        rows.into_iter().take(ROWS).map(|(_, row)| row).collect(),
        total,
    )
}

/// The watchman's brief, as of `now_ms`.
pub fn brief(ledger: &Ledger, hashes: &[String], key: Option<&OperatorKey>, now_ms: i64) -> Value {
    let watchmen: Vec<Watch> = designation::in_force(ledger, hashes, key);
    let rally = ledger.open_rally();
    let projects: Option<Vec<String>> = rally.map(|r| r.projects.clone());
    let in_rally = |id: &str| {
        projects
            .as_ref()
            .is_none_or(|p| p.iter().any(|p| p.eq_ignore_ascii_case(prefix(id))))
    };
    let by_watchman = |credential: &str, declared: Option<&str>| {
        watchmen.iter().any(|w| w.wrote(credential, declared))
    };

    // Every story record, with its story, for "since" and "landed".
    let entries: Vec<(&String, &crate::ledger::HistoryEntry)> = ledger
        .stories()
        .iter()
        .flat_map(|(id, story)| story.history.iter().map(move |entry| (id, entry)))
        .collect();

    // 1. Since the watchman's own last record: what others wrote about the rally's stories.
    let last_own = entries
        .iter()
        .filter(|(_, e)| by_watchman(&e.source.credential, e.source.declared_name.as_deref()))
        .map(|(_, e)| (e.seq, e.at_ms))
        .chain(ledger.blockers().values().filter_map(|b| {
            by_watchman(&b.source.credential, b.source.declared_name.as_deref())
                .then_some((b.seq, b.at_ms))
        }))
        .chain(ledger.standings().values().filter_map(|s| {
            by_watchman(&s.source.credential, s.source.declared_name.as_deref())
                .then_some((s.seq, s.at_ms))
        }))
        .max_by_key(|(seq, _)| *seq);
    let after = last_own.map_or(0, |(seq, _)| seq);
    let since_all: Vec<&(&String, &crate::ledger::HistoryEntry)> = entries
        .iter()
        .filter(|(id, e)| {
            e.seq > after
                && in_rally(id)
                && !by_watchman(&e.source.credential, e.source.declared_name.as_deref())
        })
        .collect();
    let mut since_counts = std::collections::BTreeMap::<String, usize>::new();
    for (_, e) in &since_all {
        *since_counts.entry(change_name(&e.change)).or_default() += 1;
    }
    // Rally's own evidence checks are counted, but the rows are what teams wrote.
    let since: Vec<(u64, Value)> = since_all
        .into_iter()
        .filter(|(_, e)| !matches!(e.change, Change::EvidenceCheck { .. }))
        .map(|(id, e)| {
            (
                e.seq,
                json!({"seq": e.seq, "story": id, "change": change_name(&e.change),
                       "by": e.source.declared_name, "kind": e.kind,
                       "age_ms": age(now_ms, e.at_ms)}),
            )
        })
        .collect();
    let since_total: usize = since_counts.values().sum();
    let (since, _) = newest(since);

    // 2. Landed: each team's newest merge since the rally opened, with the commit and when.
    let opened = rally.map_or(0, |r| r.seq);
    let mut landed = std::collections::BTreeMap::<String, (u64, Value)>::new();
    let mut verified_waiting = Vec::new();
    for (id, story) in ledger.stories() {
        if !in_rally(id) {
            continue;
        }
        let team = story.team.clone().unwrap_or_else(|| "?".into());
        let merge = story.history.iter().rev().find_map(|e| match &e.change {
            Change::Merge { commit, .. } => Some((e, commit)),
            _ => None,
        });
        if let Some((entry, commit)) = merge.filter(|(entry, _)| entry.seq > opened) {
            let newer = landed.get(&team).is_none_or(|(seq, _)| entry.seq > *seq);
            if newer {
                landed.insert(
                    team.clone(),
                    (
                        entry.seq,
                        json!({"team": team, "story": id, "commit": commit, "seq": entry.seq,
                               "kind": entry.kind, "age_ms": age(now_ms, entry.at_ms)}),
                    ),
                );
            }
        } else if merge.is_none() && story.state == State::Verified {
            // 3. Verified and not merged: ready to land, with the commit read.
            let read = story.history.iter().rev().find_map(|e| match &e.change {
                Change::Verify {
                    part: None, commit, ..
                } => Some((e, commit.clone())),
                _ => None,
            });
            if let Some((entry, commit)) = read {
                verified_waiting.push((
                    entry.seq,
                    json!({"story": id, "team": team, "commit": commit,
                           "verified_by": entry.source.declared_name,
                           "age_ms": age(now_ms, entry.at_ms)}),
                ));
            }
        }
    }
    let landed: Vec<Value> = landed.into_values().map(|(_, row)| row).collect();
    let (verified_waiting, verified_waiting_total) = newest(verified_waiting);

    // 4. Waiting on the watchman: submissions in the rally waiting for a reader.
    let readers = if watchmen.is_empty() {
        crate::views::Readers::Suggest
    } else {
        crate::views::Readers::Watchmen(watchmen.clone())
    };
    let waiting_all = crate::views::waiting_with(ledger, &readers);
    let waiting: Vec<Value> = waiting_all["waiting"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|w| w["story"].as_str().is_some_and(in_rally))
        .map(|w| {
            let at = w["submitted_at_ms"].as_i64().unwrap_or(now_ms);
            json!({"story": w["story"], "part_of": w["part_of"], "team": w["team"],
                   "evidence": cut(w["evidence"].as_str().unwrap_or("")),
                   "check": w["evidence_check"]["outcome"]["ci"],
                   "age_ms": age(now_ms, at)})
        })
        .collect();
    let waiting_total = waiting.len();
    let waiting: Vec<Value> = waiting.into_iter().take(ROWS).collect();
    let closes_due = waiting_all["closes_due"].as_array().map_or(0, Vec::len);

    // 5. Waiting on the operator: the inbox (RAL-146), urgent first.
    let inbox = crate::inbox::inbox(ledger, false, now_ms);
    let mut items: Vec<Value> = inbox["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            json!({"seq": item["seq"], "urgent": item["urgent"], "note": cut(
                item["note"].as_str().unwrap_or("")), "holds_up": item["holds_up"],
                "age_ms": item["age_ms"]})
        })
        .collect();
    items.sort_by_key(|item| item["urgent"] != true);
    let operator_total = items.len();
    items.truncate(ROWS);

    // 6. Rulings, newest first: holds, conditions and grants in force, and the rally's events.
    let mut rulings: Vec<(u64, Value)> = ledger
        .standings()
        .values()
        .filter(|s| s.ended.is_none())
        .map(|s| {
            let what = match s.standing {
                StandingKind::Hold => "hold",
                StandingKind::Condition => "condition",
                StandingKind::Grant => "grant",
            };
            (
                s.seq,
                json!({"seq": s.seq, "what": what, "note": cut(&s.note),
                       "declared_by": s.source.declared_name, "kind": s.kind,
                       "age_ms": age(now_ms, s.at_ms)}),
            )
        })
        .collect();
    if let Some(rally) = rally {
        rulings.extend(rally.events.iter().map(|e| {
            (
                e.seq,
                json!({"seq": e.seq, "what": "event", "note": cut(&e.headline),
                       "declared_by": e.source.declared_name, "kind": e.kind,
                       "age_ms": age(now_ms, e.at_ms)}),
            )
        }));
    }
    let (rulings, rulings_total) = newest(rulings);

    // 7. Each team's assignment and claim in the rally: what it was given and holds.
    let mut teams = std::collections::BTreeMap::<String, Value>::new();
    for (id, story) in ledger.stories() {
        if !in_rally(id) {
            continue;
        }
        if let Some(assigned) = &story.assigned {
            let row = teams
                .entry(assigned.team.clone())
                .or_insert_with(|| json!({"team": assigned.team, "assigned": [], "holds": []}));
            row["assigned"]
                .as_array_mut()
                .expect("made as an array")
                .push(json!({"story": id, "state": story.state,
                             "age_ms": age(now_ms, assigned.at_ms)}));
        }
        if matches!(
            story.state,
            State::Claimed | State::InReview | State::SentBack
        ) && let Some(team) = &story.team
        {
            let row = teams
                .entry(team.clone())
                .or_insert_with(|| json!({"team": team, "assigned": [], "holds": []}));
            row["holds"]
                .as_array_mut()
                .expect("made as an array")
                .push(json!({"story": id, "state": story.state}));
        }
    }
    let teams_total = teams.len();
    let teams: Vec<Value> = teams.into_values().take(ROWS * 2).collect();

    // 8. Counts of the rest, each with the command that lists it.
    let open_blockers: Vec<&crate::ledger::Blocker> = ledger
        .blockers()
        .values()
        .filter(|b| b.closed.is_none())
        .collect();
    let rally_blockers = open_blockers
        .iter()
        .filter(|b| b.story.as_deref().is_none_or(in_rally))
        .count();
    let handover = designation::handover(ledger, hashes, key);
    let handover_awaiting = handover["stories"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| {
            s["status"]
                .as_str()
                .is_some_and(|t| t.starts_with("awaiting"))
                && s["story"].as_str().is_some_and(in_rally)
        })
        .count();
    let claims_outside = ledger
        .stories()
        .iter()
        .filter(|(id, story)| {
            !in_rally(id) && matches!(story.state, State::Claimed | State::SentBack)
        })
        .count();

    json!({
        "untrusted_text": UNTRUSTED_TEXT,
        "authority": crate::brief::NO_AUTHORITY,
        "as_of": ledger.last_seq(),
        "now_ms": now_ms,
        "watchman": watchmen.iter().map(|w| &w.name).collect::<Vec<_>>(),
        "rally": rally.map(|r| json!({"name": r.name, "projects": r.projects, "seq": r.seq,
                                       "age_ms": age(now_ms, r.at_ms)})),
        "since_your_last_record": {
            "your_last": last_own.map(|(seq, at)| json!({"seq": seq, "age_ms": age(now_ms, at)})),
            "by_change": since_counts, "total": since_total, "newest": since,
        },
        "landed": {
            "per_team": landed,
            "note": "Each team's newest `rally merge` since the rally opened. A story landed \
                     without one recorded isn't here.",
        },
        "in_the_gate": {
            "verified_not_merged": verified_waiting, "total": verified_waiting_total,
            "note": "Which of these the watchman is gating together isn't in the ledger yet \
                     (RAL-157 part 2 records it).",
        },
        "waiting_on_you": {"submissions": waiting, "total": waiting_total,
                           "closes_due": closes_due},
        "waiting_on_the_operator": {"items": items, "total": operator_total,
                                    "open": inbox["open"]},
        "rulings": {"newest": rulings, "total": rulings_total},
        "teams": {"rows": teams, "total": teams_total},
        "more": {
            "open_blockers_in_rally": rally_blockers,
            "open_blockers_elsewhere": open_blockers.len() - rally_blockers,
            "claims_outside_rally": claims_outside,
            "handover_awaiting_in_rally": handover_awaiting,
            "commands": {
                "full brief": "rally brief", "blockers": "rally blockers",
                "waiting": "rally waiting", "inbox": "rally inbox",
                "handover": "rally handover", "a story": "rally show <STORY>",
            },
        },
        "outside_the_ledger": "What's deployed, and which commits the watchman is gating \
                               together, aren't recorded here; nor is what was said only in \
                               chat.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::designation::testing::{Chain, keypair};

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    fn submitted(chain: &mut Chain, story: &str) {
        chain.push_as("r", json!({"change": "ready", "story": story}));
        chain.push_as(
            "r",
            json!({"change": "claim", "story": story, "team": "rally"}),
        );
        chain.push_as(
            "r",
            json!({"change": "submit", "story": story, "team": "rally",
                   "evidence": format!("o/r@{COMMIT}")}),
        );
    }

    fn verified(chain: &mut Chain, by: &str, story: &str) {
        chain.push_as(
            by,
            json!({"change": "verify", "story": story, "note": "read", "commit": COMMIT}),
        );
    }

    fn stories(rows: &Value) -> Vec<&str> {
        rows.as_array()
            .unwrap()
            .iter()
            .map(|row| row["story"].as_str().unwrap())
            .collect()
    }

    /// RAL-157: the watchman's brief keeps to the open rally, and leads with what others wrote
    /// since its own last record, what each team landed, what's verified and not merged, and what
    /// waits on it.
    #[test]
    fn it_keeps_to_the_rally_and_starts_from_the_watchmans_last_record() {
        let dir = tempfile::tempdir().unwrap();
        let (private, key) = keypair(dir.path(), "operator");
        let mut chain = Chain::new();
        chain.designate(&private, &key, Some("watch"));
        chain.push(
            json!({"change": "open_rally", "name": "0xA", "projects": ["RAL"],
                          "teams": ["rally"]}),
        );
        // RAL-1 lands in the rally.
        submitted(&mut chain, "RAL-1");
        verified(&mut chain, "watch", "RAL-1");
        chain.push_as(
            "r",
            json!({"change": "merge", "story": "RAL-1", "commit": COMMIT}),
        );
        // RAL-3 is verified and not merged; it's the watchman's last record.
        submitted(&mut chain, "RAL-3");
        verified(&mut chain, "watch", "RAL-3");
        let last_own = chain.records.len() as u64;
        // Since then: RAL-2 submitted, and work outside the rally.
        submitted(&mut chain, "RAL-2");
        submitted(&mut chain, "MAR-1");
        chain.push_as("r", json!({"change": "ready", "story": "MAR-2"}));
        chain.push_as(
            "r",
            json!({"change": "claim", "story": "MAR-2", "team": "marshal"}),
        );

        let ledger = crate::ledger::Ledger::replay(&chain.records).unwrap();
        let out = brief(&ledger, &chain.hashes, Some(&key), 10_000);
        assert_eq!(out["watchman"], json!(["watch"]));
        assert_eq!(out["rally"]["name"], "0xA");
        let since = &out["since_your_last_record"];
        assert_eq!(since["your_last"]["seq"], last_own, "{out}");
        assert_eq!(since["total"], 3, "only RAL-2's three, not MAR's: {since}");
        assert_eq!(
            since["by_change"],
            json!({"claim": 1, "ready": 1, "submit": 1})
        );
        assert_eq!(stories(&since["newest"]), ["RAL-2", "RAL-2", "RAL-2"]);
        let landed = &out["landed"]["per_team"];
        assert_eq!(stories(landed), ["RAL-1"]);
        assert_eq!(landed[0]["team"], "rally");
        assert_eq!(landed[0]["commit"], COMMIT);
        assert_eq!(
            stories(&out["in_the_gate"]["verified_not_merged"]),
            ["RAL-3"]
        );
        assert_eq!(stories(&out["waiting_on_you"]["submissions"]), ["RAL-2"]);
        assert_eq!(out["more"]["claims_outside_rally"], 1);
        assert!(out.to_string().len() < 20_000);
    }

    /// RAL-157: every list is bounded and every note cut short, newest first, with the total.
    #[test]
    fn its_lists_are_bounded_and_its_notes_cut_short() {
        let mut chain = Chain::new();
        chain.push(json!({"change": "open_rally", "name": "x", "projects": ["RAL"]}));
        let long = "word ".repeat(100);
        for i in 0..30 {
            chain.push_as(
                "w",
                json!({"change": "standing", "standing": "hold", "team": "rally",
                       "note": format!("{i} {long}"), "releases": "the operator"}),
            );
        }
        let ledger = crate::ledger::Ledger::replay(&chain.records).unwrap();
        let out = brief(&ledger, &chain.hashes, None, 10_000);
        let rulings = &out["rulings"];
        assert_eq!(rulings["total"], 30);
        let newest = rulings["newest"].as_array().unwrap();
        assert_eq!(newest.len(), ROWS);
        assert!(newest[0]["note"].as_str().unwrap().starts_with("29 "));
        assert!(
            newest
                .iter()
                .all(|r| r["note"].as_str().unwrap().chars().count() <= NOTE_CHARS + 1)
        );
        assert!(out.to_string().len() < 20_000);
    }
}
