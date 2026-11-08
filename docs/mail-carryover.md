# Carrying unread Muster mail to Marshal at the cutover (RAL-122)

**Decided: option A, with the Marshal team running the check** (the operator, 2026-11-08; see "Decided" below).
Nothing is built or changed by this note. It fills the gap
[cutover.md](cutover.md) left open, "message history carried over", and asks only about **unread** mail:
history that has been read stays readable in Muster's database, and nobody has asked for it on Marshal. The
watchman (mellow-weevil) asked for it on 2026-11-08.

## How mail is kept today

Muster and Marshal have the same message model, since Marshal shares Muster's lineage (`crates/entities`,
Muster at 0485ac7 and Marshal's `main` at 83f5202):

- A **Message** has a sender's session id, a recipient (`to_session_id` for direct mail, `to_room_id` for a
  room), an optional `to_operator`, `response_required`, the body, and when it was sent.
- Whether it was read is a separate **MessageRead** per reading session.
- Each daemon keeps these as events in its own Postgres database, `muster` and `marshal`, on the same
  `marshal-pg` socket. Muster sweeps mail after 14 days.
- Session ids differ between the daemons, so a message's sender and recipient on Muster mean nothing on
  Marshal without a mapping, and MUS-47 (a successor taking over its predecessor's unread mail) works only
  inside one daemon.

## How much there is

Read on Murph on 2026-11-08 with read-only queries on the `muster` database:

| | Count |
|---|---|
| Messages Muster still keeps | 4,810 (oldest 2026-10-30) |
| Direct messages | 4,314 |
| Direct messages unread by their recipient | **19**, to 7 sessions, sent 2026-11-03 to 11-06, none to the operator |
| Of those, to ids that look like test fixtures (`b0000000-…-0000000000a1`, `e5000000-…-00000000000a` and `…0c`) | 13 |
| Unread direct messages to a session that's still live | **0** at first read. A re-run minutes later showed 1, just sent to a live session that hadn't taken its next turn |
| Room messages | 496 |

So, as of today, nothing unread is waiting long for a session that's still running. What's unread is mail to
sessions that ended without reading it, and 13 of those 19 went to fixture-like ids, which suggests a test once
wrote into the live daemon. That's worth a look on its own, outside this note. A session that's live reads its mail through its hooks within a turn
or two. The real risk at the cutover isn't mail already sitting there. It's mail **sent to a session's Muster
identity after that session has moved** to Marshal, during the side-by-side period of
[cutover.md](cutover.md) steps 3 to 6.

Room mail isn't counted here: whether a broadcast was read is per member, and nobody waits on a broadcast
being read.

## The options

### A. Read it on Muster first (drain)

Before a session moves, it reads and acks its Muster inbox, and announces its Marshal nickname on Muster with a
`broadcast`. Before each move, and before Muster is retired (cutover step 7), the Marshal team runs the read-only
query below. Anything unread to a session that has moved is relayed by hand with `marshal-shim tell`, quoting
the sender.

- **Builds:** nothing. A query kept in the runbook.
- **Good:** nothing touches Marshal's database. The only new step is a check. It fits today's numbers, with
  zero unread to live sessions.
- **Risk:** mail sent to the old identity after a move waits until someone runs the check. The longer the
  side-by-side period, the more that matters. Relayed mail arrives from a terminal, not from its sender, so a
  reply has to be addressed by hand.

### B. A relay for the side-by-side period

A small bridge, run by the lamplighter only during the move, watches Muster for direct mail to a session
that has moved. It forwards each one with `marshal-shim tell <new nickname>`, prefixed with who sent it and
when, then marks it read on Muster. The old-to-new mapping comes from the moves the watchman records
(`rally rally-session add --succeeds`, RAL-108).

- **Builds:** the bridge and its mapping, plus tests. A day or two of work.
- **Good:** nothing is missed however long the move takes, and the sender is named in the relayed text.
- **Risk:** it's a new process holding both daemons' mail. It has to treat every body as untrusted text. It
  can double-deliver if a moved session still reads Muster. And it's built to be thrown away at step 7.

### C. Export and import

At the switch, copy each unread Message and its MessageRead rows from `muster` into `marshal`, rewriting
session and room ids through the same mapping.

- **Builds:** an export, a mapping, and an import that writes Marshal's events. A larger change, with
  tests against both schemas.
- **Good:** mail appears in Marshal as ordinary mail, from its real sender's new identity.
- **Risk:** it writes straight into a live daemon's event log, around Marshal's own rules (succession,
  rooms, the 14-day sweep). A sender with no Marshal identity has no id to map to. The two schemas can drift.
  It's the most work for what is, today, 19 messages to ended sessions.

### D. Leave it

Nothing is carried, and Muster's database is kept read-only after step 7 for anyone who needs to look.

- **Builds:** nothing. **Risk:** mail sent to a moved session is lost to it, silently.

## The team's recommendation

**A, with the check made routine.** The operator chose it (below). Today's numbers say the inbox is empty where it matters. The
runbook already moves sessions one at a time, between stories, with Muster still running, so a check before each
move and a last one before step 7 catches what B would, without building anything. Choose **B** instead if the
side-by-side period is expected to last more than a day or two. **C** isn't recommended. **D** isn't safe.

## Decided

The operator, Josh, on 2026-11-08:

1. **Option A.** Josh told the watchman directly, and closed Rally block 1465 with it. Josh also told the rally
   team: "yes to your recommendation".
2. **Who runs the check: the Marshal team.** The team had proposed the lamplighter. Josh answered the rally team
   directly that it's the Marshal team. The watchman relayed an earlier answer from Josh, "the lamplighter can run
   the check when its ready". This note follows the later, direct answer to the rally team, and the watchman has
   been told of the difference.

[cutover.md](cutover.md) carries A's steps.

The check A relies on, as run for this note (read-only):

```sql
set default_transaction_read_only = on;
with latest as (
  select distinct on (item_type, item_id) item_type, item_id, change_type, event->'item' as it
  from myko_events where item_type in ('Message', 'MessageRead', 'Session')
  order by item_type, item_id, id desc),
msg as (select item_id as id, it->>'toSessionId' as to_s, (it->>'sentAt')::bigint as sent
        from latest where item_type = 'Message' and change_type = 'SET' and it->>'toSessionId' is not null),
rd as (select it->>'messageId' as mid, it->>'sessionId' as sid
       from latest where item_type = 'MessageRead' and change_type = 'SET')
select m.to_s, count(*), to_timestamp(min(m.sent) / 1000) as oldest
from msg m where not exists (select 1 from rd where rd.mid = m.id and rd.sid = m.to_s)
group by m.to_s order by oldest;
```

Run it with `psql "host=/run/user/1000/marshal-pg dbname=muster user=joshd" -X`.
