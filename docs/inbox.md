# The operator's inbox (RAL-146)

Everything waiting on the operator, in one list, like a quest log. The operator asked for it on 2026-11-13, after a
fault only he could fix (Rally's GitHub token stopped working) was said in chat and waited two days, while 31
questions to him sat in the ledger with nowhere he'd see them together.

```bash
rally inbox                      # what's open, urgent first, then oldest first
rally inbox --all                # closed items too, after the open ones, newest close first
rally inbox add --team <team> --action --note "..." [--urgent] [--ask "..."] \
    [--story <STORY> | --holds-up-team <team> | --holds-up-service]
rally inbox add --team <team> --notice --note "..."
rally inbox done <record> [--note "..."]   # the operator ticks it off at a terminal
```

The viewer shows the same list at the top of its page, with the count in its heading and on the board ("Operator
inbox: 3 items open, 1 urgent"), so an empty inbox and a full one look different at a glance. The brief carries
the count and the urgent items in full (`operator.inbox`), and `rally session-start` tells a team when an item it
raised was closed in the last day.

## What goes in it

Two sources, read together:

- **Operator items**, a record kind of their own. Any team may raise one:
  - `--action`: something only the operator can do: a token, a sudo step, a merge permission, a purchase.
  - `--notice`: something the operator should know and needn't act on.
  - `--urgent`: it's breaking work now, so it goes to the top.
  - `--ask`: the exact command or decision asked for, in one line.
  - What it holds up: a story (`--story`), a team (`--holds-up-team`), or the service itself
    (`--holds-up-service`).
- **Questions to the operator**: open blockers on the team `operator` (RAL-56), exactly as they are. The inbox reads
  them and changes none of their records, their shelving window or the `operator_away` rules.

What doesn't belong: a team's own blocker on another team (that's `rally block`), progress, or anything the
watchman or a team can do itself. Nothing secret goes in an item: the threat model's list of what's never recorded
applies, and text that looks like a token or key is refused before it's written, as in any record.

## Order

Urgent first, then everything else; oldest first within each, so what has waited longest is next. Nothing is
pushed to the operator: the list is where he looks.

## Rally files its own

When the service sees a fault only the operator can fix, it opens one urgent item itself, observed, holding up the
service, and doesn't open another while that one is open:

- **GitHub refused its token** (a 401, such as "Bad credentials"): every evidence check fails until it's replaced.
  A rate limit, a missing permission (such as the check-runs 403 a fine-grained token gets on a private repository,
  RAL-147), or the network isn't a refusal of the token, so none of those opens one.
- **The token expires within 7 days**, by the expiry GitHub gives in its `GitHub-Authentication-Token-Expiration`
  header, when it gives one.

Each is closed by Rally itself, observed, once it has passed: the next read GitHub takes, or a token that no longer
expires within 7 days. Each asks the same thing of the operator: make a new read-only fine-grained token
([evidence.md](evidence.md), "The operator's step"), install it as the service's `--github-token-file`, and restart
rally.

## Ticking an item off

The operator closes an item himself (his choice, 2026-11-13): an action when it's done, a notice when it's seen. A
question to the operator ticked off is answered as `rally unblock --direct` would answer it, so it closes as a
question always has. A tick records no answer, though, so for a question it means "answered elsewhere": the
viewer's row for a question says so, the inbox marks a question ticked in the viewer `via: viewer`, and the asking
team's session start says "the operator ticked your question off in the viewer; if you don't have the answer, ask
the watchman" (RAL-150).

- **In the viewer**: each item has a box. Ticking it records the close at once, and the item leaves the list. The
  tick is the viewer's one write. It's taken only as a POST from a signed-in session of the page itself: the host
  must be the viewer's own address, the `Origin` its own, `Sec-Fetch-Site` same-origin, and the request must carry
  a token tied to the session, which only the page can read. The viewer writes with the writing pairing beside its
  own (`client.json`), while it keeps reading with its read-only one. On the host with `--db`, it ticks only when
  given `--credential` (or `RALLY_CREDENTIAL`), and writes through the writer lock as any `--db` write does; without
  one it draws no boxes.
- **At a terminal**: `rally inbox done <record>`.

A close is **declared**, as the operator's word: a tick in the viewer is written under the name `operator`, and
the record says where it was made (`via: viewer` or `via: terminal`) and the service records which machine's
credential made it. The operator's choice was that the tick works in any viewer he has open, Murph's included, and
is labelled by how it was made rather than refused, since he is the system's only user. So a close is as good as
the machine it came from: on Murph, any session could have ticked it, and the record says it came from there.
Signing a close with the operator's key, so that it reads as verified, is possible later and isn't required.

## Records

- `operator_item`: `item` (`action` or `notice`), `urgent`, `team` (who raised it), `note`, `holds_up`
  (`{"story": ...}`, `{"team": ...}` or `"service"`), `ask`. Declared, or observed when Rally files it.
- `close_item`: `item` (the record number), `note`, `via` (`viewer` or `terminal`). Declared, or observed with no
  `via` when Rally closes its own.

A binary older than RAL-146 can't replay either; see the rollback table in [service.md](service.md).
