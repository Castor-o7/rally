//! The operator's inbox (RAL-146): everything waiting on the operator in one list, like a quest
//! log. Two sources feed it: operator items (`rally inbox add`, or filed by Rally itself) and open
//! questions to the operator (blockers on the team `operator`, RAL-56), which it reads and changes
//! none of. Urgent items come first, then the rest, oldest first within each, so what's breaking
//! work now is at the top. Nothing is pushed: the list is where the operator looks.

use serde_json::{Value, json};

use crate::ledger::{Blocker, ItemKind, Ledger, OperatorItem};

/// The most rows one read gives, open or closed. The counts are always whole.
pub const LIST_LIMIT: usize = 200;

/// One row of the inbox, before it's written out: whether it's urgent and when it was raised, for
/// the order, and the row itself.
struct Row {
    urgent: bool,
    at_ms: i64,
    seq: u64,
    value: Value,
}

fn age(now_ms: i64, at_ms: i64) -> i64 {
    now_ms.saturating_sub(at_ms).max(0)
}

fn item_row(item: &OperatorItem, now_ms: i64) -> Row {
    let closed = item.closed.as_ref().map(|close| {
        json!({
            "seq": close.seq, "kind": close.kind, "at_ms": close.at_ms,
            "by": close.source.declared_name, "credential": close.source.credential,
            "note": close.note, "via": item.closed_via,
        })
    });
    Row {
        urgent: item.urgent,
        at_ms: item.at_ms,
        seq: item.seq,
        value: json!({
            "seq": item.seq,
            "from": "item",
            "item": item.item,
            "urgent": item.urgent,
            "kind": item.kind,
            "note": item.note,
            "ask": item.ask,
            "holds_up": item.holds_up,
            "raised_by": {
                "name": item.source.declared_name,
                "team": item.team,
                "credential": item.source.credential,
            },
            "at_ms": item.at_ms,
            "age_ms": age(now_ms, item.at_ms),
            "closed": closed,
        }),
    }
}

fn question_row(blocker: &Blocker, now_ms: i64) -> Row {
    let holds_up = match &blocker.story {
        Some(story) => json!({"story": story}),
        None => json!({"team": blocker.team}),
    };
    let closed = blocker.closed.as_ref().map(|close| {
        json!({
            "seq": close.seq, "kind": close.kind, "at_ms": close.at_ms,
            "by": close.source.declared_name, "credential": close.source.credential,
            "note": close.note, "answer": close.answer,
        })
    });
    Row {
        urgent: false,
        at_ms: blocker.at_ms,
        seq: blocker.seq,
        value: json!({
            "seq": blocker.seq,
            "from": "question",
            // A question asks the operator for a decision, so it's something to act on.
            "item": ItemKind::Action,
            "urgent": false,
            "kind": blocker.kind,
            "note": blocker.note,
            "ask": Value::Null,
            "holds_up": holds_up,
            "raised_by": {
                "name": blocker.source.declared_name,
                "team": blocker.team,
                "credential": blocker.source.credential,
            },
            "at_ms": blocker.at_ms,
            "age_ms": age(now_ms, blocker.at_ms),
            "closed": closed,
        }),
    }
}

/// The inbox as of `now_ms`: its open items, urgent first and then oldest first, or with `all`
/// every item, open ones first in that order and then closed ones, newest close first. `open`
/// and `urgent` count every open item, however many rows are shown.
pub fn inbox(ledger: &Ledger, all: bool, now_ms: i64) -> Value {
    let items = ledger
        .operator_items()
        .values()
        .map(|item| (item.closed.is_none(), item_row(item, now_ms)));
    let questions = ledger
        .blockers()
        .values()
        .filter(|blocker| blocker.waits_on_operator())
        .map(|blocker| (blocker.closed.is_none(), question_row(blocker, now_ms)));
    let (mut open, mut closed): (Vec<_>, Vec<_>) = items.chain(questions).partition(|(o, _)| *o);
    open.sort_by_key(|(_, row)| (!row.urgent, row.at_ms, row.seq));
    let urgent = open.iter().filter(|(_, row)| row.urgent).count();
    let open_count = open.len();
    let mut rows: Vec<Value> = open.into_iter().map(|(_, row)| row.value).collect();
    if all {
        closed.sort_by_key(|(_, row)| {
            std::cmp::Reverse(row.value["closed"]["seq"].as_u64().unwrap_or(row.seq))
        });
        rows.extend(closed.into_iter().map(|(_, row)| row.value));
    }
    let total = rows.len();
    rows.truncate(LIST_LIMIT);
    json!({
        "as_of": ledger.last_seq(),
        "open": open_count,
        "urgent": urgent,
        "items": rows,
        "items_total": total,
        "order": "urgent first, then oldest first; closed items, with --all, after them, newest close first",
    })
}

/// How the start of the note of an item Rally files when GitHub refuses its token (RAL-146).
pub const TOKEN_REFUSED: &str = "Rally's GitHub token was refused";

/// How the start of the note of an item Rally files when its token expires within
/// [`EXPIRY_WARNING_MS`] (RAL-146).
pub const TOKEN_EXPIRING: &str = "Rally's GitHub token expires";

/// How far ahead an expiring token is told: 7 days.
pub const EXPIRY_WARNING_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// What the operator is asked to do about the token, in both of Rally's own items.
const TOKEN_ASK: &str = "Make a new read-only fine-grained token (docs/evidence.md, \"The operator's \
     step\"), install it as the service's --github-token-file, and restart rally";

/// What one read of GitHub showed of Rally's token (RAL-146).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenSeen {
    /// GitHub refused it for its credentials: a 401, with GitHub's words.
    Refused(String),
    /// GitHub took it, saying when it expires, if it did.
    Accepted { expires: Option<String> },
}

/// Whether a failed read's text is GitHub refusing the token's credentials (a 401), rather than a
/// rate limit, a missing permission or the network: only the operator can fix that.
pub fn is_credential_refusal(error: &str) -> bool {
    error.starts_with("GitHub answered 401")
}

/// The open item Rally filed itself whose note starts with `start`, if there is one.
fn open_own_item<'a>(ledger: &'a Ledger, start: &str) -> Option<&'a OperatorItem> {
    ledger.operator_items().values().find(|item| {
        item.closed.is_none()
            && item.kind == crate::record::Kind::Observed
            && item.source.credential == crate::store::RALLY_ITSELF
            && item.note.starts_with(start)
    })
}

/// GitHub's expiry text, `YYYY-MM-DD HH:MM:SS UTC` (or with a numeric offset of `+0000`), as
/// milliseconds since 1970, or `None` if it isn't in that form.
pub fn expiry_ms(text: &str) -> Option<i64> {
    let mut parts = text.split_whitespace();
    let date = parts.next()?;
    let time = parts.next()?;
    if !matches!(parts.next(), Some("UTC" | "+0000") | None) {
        return None;
    }
    let mut ymd = date.split('-').map(str::parse::<i64>);
    let (y, m, d) = (ymd.next()?.ok()?, ymd.next()?.ok()?, ymd.next()?.ok()?);
    let mut hms = time.split(':').map(str::parse::<i64>);
    let (h, mi, sec) = (hms.next()?.ok()?, hms.next()?.ok()?, hms.next()?.ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    // Days from 1970-01-01 to the date, by the proleptic Gregorian calendar.
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + mi) * 60_000 + sec * 1000)
}

/// The records Rally writes itself after what one read showed of its token (RAL-146), as bodies
/// of observed records: an urgent item when GitHub refuses it and none is open, or when it expires
/// within 7 days and none is open; and a close of each once what it said has passed. Never two of
/// one kind open at once.
pub fn token_records(ledger: &Ledger, seen: &TokenSeen, now_ms: i64) -> Vec<Value> {
    let item = |note: String| {
        json!({"change": "operator_item", "item": "action", "urgent": true, "team": "rally",
               "note": note, "holds_up": "service", "ask": TOKEN_ASK})
    };
    let close = |item: &OperatorItem, note: &str| json!({"change": "close_item", "item": item.seq, "note": note});
    let mut out = Vec::new();
    match seen {
        TokenSeen::Refused(error) => {
            if open_own_item(ledger, TOKEN_REFUSED).is_none() {
                // GitHub's words, on one line, so the item stays one.
                let error: String = error
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(200)
                    .collect();
                out.push(item(format!(
                    "{TOKEN_REFUSED} ({error}): every evidence check fails until it's replaced"
                )));
            }
        }
        TokenSeen::Accepted { expires } => {
            if let Some(open) = open_own_item(ledger, TOKEN_REFUSED) {
                out.push(close(open, "GitHub took the token again"));
            }
            let expires_at = expires.as_deref().and_then(expiry_ms);
            let soon = expires_at.is_some_and(|at| at - now_ms <= EXPIRY_WARNING_MS);
            match (open_own_item(ledger, TOKEN_EXPIRING), soon) {
                (None, true) => {
                    let at = expires.as_deref().unwrap_or_default();
                    let days =
                        (expires_at.unwrap_or(now_ms) - now_ms).max(0) / (24 * 60 * 60 * 1000);
                    let when = match days {
                        0 => "within a day".to_owned(),
                        1 => "in 1 day".to_owned(),
                        n => format!("in {n} days"),
                    };
                    out.push(item(format!("{TOKEN_EXPIRING} {when}, at {at}")));
                }
                (Some(open), false) => out.push(close(
                    open,
                    &match expires {
                        Some(at) => format!("the token now expires at {at}"),
                        None => "GitHub no longer says the token expires".to_owned(),
                    },
                )),
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{Kind, Record, Source};

    fn record(_seq: u64, at_ms: i64, kind: Kind, body: Value) -> Record {
        let source = Source {
            credential: "murph".into(),
            declared_name: Some("earnest-raccoon".into()),
        };
        Record::new(kind, source, at_ms, body).unwrap()
    }

    fn ledger(records: &[Record]) -> Ledger {
        Ledger::replay(records).unwrap()
    }

    #[test]
    fn urgent_items_come_first_then_the_oldest() {
        let records = [
            record(
                1,
                100,
                Kind::Declared,
                json!({"change": "operator_item", "item": "notice",
                "team": "ral", "note": "old notice"}),
            ),
            record(
                2,
                200,
                Kind::Declared,
                json!({"change": "block", "team": "ral",
                "on_team": "operator", "note": "may we?"}),
            ),
            record(
                3,
                300,
                Kind::Declared,
                json!({"change": "operator_item", "item": "action",
                "urgent": true, "team": "ral", "note": "new urgent", "holds_up": "service"}),
            ),
            record(
                4,
                400,
                Kind::Declared,
                json!({"change": "operator_item", "item": "action",
                "urgent": true, "team": "ral", "note": "newer urgent",
                "ask": "sudo systemctl restart rally"}),
            ),
        ];
        let out = inbox(&ledger(&records), false, 1_000);
        let order: Vec<u64> = out["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["seq"].as_u64().unwrap())
            .collect();
        assert_eq!(order, [3, 4, 1, 2]);
        assert_eq!(out["open"], 4);
        assert_eq!(out["urgent"], 2);
        assert_eq!(out["items"][3]["from"], "question");
        assert_eq!(out["items"][0]["holds_up"], "service");
        assert_eq!(out["items"][1]["ask"], "sudo systemctl restart rally");
        assert_eq!(out["items"][0]["age_ms"], 700);
    }

    #[test]
    fn a_closed_item_leaves_the_list_and_shows_with_all() {
        let records = [
            record(
                1,
                100,
                Kind::Declared,
                json!({"change": "operator_item", "item": "action",
                "team": "ral", "note": "a token"}),
            ),
            record(
                2,
                200,
                Kind::Declared,
                json!({"change": "operator_item", "item": "notice",
                "team": "ral", "note": "fyi"}),
            ),
            record(
                3,
                300,
                Kind::Declared,
                json!({"change": "close_item", "item": 1,
                "note": "done", "via": "viewer"}),
            ),
        ];
        let ledger = ledger(&records);
        let open = inbox(&ledger, false, 1_000);
        assert_eq!(open["open"], 1);
        assert_eq!(open["items"].as_array().unwrap().len(), 1);
        assert_eq!(open["items"][0]["seq"], 2);
        let all = inbox(&ledger, true, 1_000);
        assert_eq!(all["items"].as_array().unwrap().len(), 2);
        assert_eq!(all["items"][1]["closed"]["via"], "viewer");
        assert_eq!(all["items"][1]["closed"]["kind"], "declared");
        assert_eq!(all["items"][1]["closed"]["credential"], "murph");
    }

    fn by_rally(at_ms: i64, body: Value) -> Record {
        let source = Source {
            credential: crate::store::RALLY_ITSELF.into(),
            declared_name: None,
        };
        Record::new(Kind::Observed, source, at_ms, body).unwrap()
    }

    /// Applies what `token_records` asks for, as the service would, and gives the ledger back.
    fn after(mut records: Vec<Record>, seen: &TokenSeen, now_ms: i64) -> (Vec<Value>, Ledger) {
        let bodies = token_records(&ledger(&records), seen, now_ms);
        records.extend(bodies.iter().map(|body| by_rally(now_ms, body.clone())));
        (bodies, ledger(&records))
    }

    #[test]
    fn github_expiry_text_is_read() {
        assert_eq!(expiry_ms("1970-01-02 00:00:00 UTC"), Some(86_400_000));
        assert_eq!(
            expiry_ms("2026-11-13 00:00:00 UTC"),
            Some(1_794_528_000_000)
        );
        assert_eq!(
            expiry_ms("2026-11-13 00:00:00 +0000"),
            Some(1_794_528_000_000)
        );
        assert_eq!(expiry_ms("2026-13-13 00:00:00 UTC"), None);
        assert_eq!(expiry_ms("soon"), None);
    }

    /// RAL-146: a refused token opens one urgent item, however often it's refused, and the next
    /// read GitHub takes closes it.
    #[test]
    fn a_refused_token_opens_one_item_and_closes_it_itself() {
        let refused = TokenSeen::Refused("GitHub answered 401: Bad credentials".into());
        let (bodies, ledger1) = after(Vec::new(), &refused, 1_000);
        assert_eq!(bodies.len(), 1);
        let open = inbox(&ledger1, false, 2_000);
        assert_eq!(open["urgent"], 1);
        assert_eq!(open["items"][0]["kind"], "observed");
        assert_eq!(open["items"][0]["holds_up"], "service");
        assert!(
            open["items"][0]["note"]
                .as_str()
                .unwrap()
                .contains("Bad credentials")
        );
        // Refused again: nothing more.
        let records = vec![by_rally(1_000, bodies[0].clone())];
        assert!(token_records(&ledger1, &refused, 3_000).is_empty());
        // Taken again: closed, by Rally, observed.
        let (closes, ledger2) = after(records, &TokenSeen::Accepted { expires: None }, 4_000);
        assert_eq!(closes.len(), 1);
        assert_eq!(inbox(&ledger2, false, 5_000)["open"], 0);
        let all = inbox(&ledger2, true, 5_000);
        assert_eq!(all["items"][0]["closed"]["kind"], "observed");
        assert!(all["items"][0]["closed"]["via"].is_null());
        // A team's own item that happens to say the same isn't Rally's, and isn't closed.
        let team_item = record(
            0,
            1_000,
            Kind::Declared,
            json!({"change": "operator_item",
            "item": "action", "team": "ral", "note": format!("{TOKEN_REFUSED} again?")}),
        );
        assert!(
            token_records(
                &ledger(&[team_item]),
                &TokenSeen::Accepted { expires: None },
                2_000
            )
            .is_empty()
        );
    }

    /// RAL-146: a token expiring within 7 days opens one urgent item, and a new token that
    /// doesn't closes it; one further off opens nothing.
    #[test]
    fn an_expiring_token_opens_one_item_until_a_new_one_comes() {
        let now = expiry_ms("2026-11-13 00:00:00 UTC").unwrap();
        let far = TokenSeen::Accepted {
            expires: Some("2026-12-31 00:00:00 UTC".into()),
        };
        assert!(token_records(&ledger(&[]), &far, now).is_empty());
        let near = TokenSeen::Accepted {
            expires: Some("2026-11-18 00:00:00 UTC".into()),
        };
        let (bodies, ledger1) = after(Vec::new(), &near, now);
        assert_eq!(bodies.len(), 1);
        let note = bodies[0]["note"].as_str().unwrap();
        assert_eq!(
            note,
            "Rally's GitHub token expires in 5 days, at 2026-11-18 00:00:00 UTC"
        );
        assert!(
            token_records(&ledger1, &near, now + 1_000).is_empty(),
            "only one"
        );
        let records = vec![by_rally(now, bodies[0].clone())];
        let (closes, ledger2) = after(records, &far, now + 2_000);
        assert_eq!(closes.len(), 1);
        assert_eq!(inbox(&ledger2, false, now + 3_000)["open"], 0);
    }
}
