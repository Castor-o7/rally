# Rally design

**Agreed by the operator on 2026-11-02 ([RAL-1](roadmap.md)).** The team that owns Rally can change
anything here except the decisions the operator has made, which are listed first, but agreed points
should go back to the operator if they change.

## Decided by the operator

- Rally is a separate project from Muster, in its own repository.
- Muster facilitates secure coordination between agents. Rally is the ledger of progress, and
  of where and how each team is.
- When the watchman sees a team has stalled, the watchman coordinates with that team to find
  out whether there's other work it can do or whether it's stuck on a real problem.
- If Muster is the musculature of the network, Rally is the bones: the deterministic ledger of the
  current state of the muster's teams, and the center for recording and verifying work.
- The watchman uses the ledger to verify each team's successes and challenges against its roadmap, to
  unblock teams, and to record and plan so as to maximize progress while respecting each team.
- Rally maintains a relationship to security, of the information and of the environment.
- A visually appealing web app showing the current ledger and past ledgers is a goal.
- Rally should stay a myko service if it can, for uniformity with Muster (2026-11-02). This replaces the
  earlier agreement on SQLite written by Rally's own service. See "Rally as a myko service" below.
- Rally should interface with [Levi](https://github.com/ignition-is-go/levi) (2026-11-02). See "Levi" below.
- Rally's licence is AGPL-3.0-or-later, matching myko (2026-11-02).
- Claims live in Levi (2026-11-02). See "Levi" below.

## Carried over from Muster's roadmap

These come from Muster's MUS-23, MUS-28 and MUS-32 and its board process. They aren't
operator decisions, but they've held up so far:

- Rally records and informs. It doesn't command, and agents and the operator can correct it.
- It keeps track records as plain facts: stories verified, stories sent back and what was
  missing, and problems reported. There are no points, scores or weights.
- It wakes the watchman with batched digests, and only when something changes.
- Merged and Verified are independent facts.

## What Rally can and can't determine

The ledger itself is deterministic: the same records always produce the same state. Its contents
aren't all facts, though, so "the ledger says a team is working" means a team declared that, not that
it's true. Rally can decide some things deterministically and has to label the rest. Each record has
one of three kinds:

| Kind | Meaning | Examples |
|---|---|---|
| Observed | Came from a system, not from an agent's account of itself | Commit exists, CI passed, session can be woken |
| Declared | An agent or person said so | "Working on MUS-4", "blocked on a review" |
| Verified | Someone other than the author checked it | The watchman confirmed "done when" |

Each record should also say who or what it came from and when, so a reader can judge how
fresh it is.

- **Deterministic:** whether a state change is valid, and whether a commit exists, names its
  story and passed CI.
- **Not deterministic:** whether the work meets its "done when" line. That takes a reader, and
  sometimes a real run. Rally records who judged it and on what evidence.

## Story states

Starting from Muster's Version 0 board (`docs/board.md` in the Muster repository):

| State | Meaning | Who moves it here |
|---|---|---|
| Ready | Has a "done when" line | The team |
| Claimed | Someone is working on it | The team |
| In review | Pushed, with the evidence submitted | The team |
| Verified | Checked against "done when" | Someone other than the author |
| Sent back | Something's missing, and the note says what | Someone other than the author |

Merged is a separate field. Blocked is what a team declares about itself, not a story state,
though a team could name the story it's blocked on.

### Commits

Heads move while work waits: muster#34 changed three times in review on 2026-11-02. So a verification
names the full hash of the commit the verifier read (RAL-31), and a correction that sets Verified does
too. Verifications recorded before RAL-31 replay without one. A sent-back story may name the commit read.

A merge is recorded as a commit (RAL-33): `merge` is allowed in any state and moves none, so Merged
and Verified stay independent, and a story merged before its read, as catapp#78 was, is recorded as
it happened. The record names no merger, because every session shares one GitHub account; its source
says only who recorded it, and until RAL-5 that's a declaration. When a story has both a verified and a
merged commit and they differ, Rally shows `commit_differs` with both, labelled observed, because it's
worked out from the ledger alone rather than claimed by anyone. A later merge of the verified commit
clears it.

### Requested reads

catapp#78 merged before a requested read arrived. So anyone can ask for a read of a story at a commit
(`request_read`, RAL-32), in any state, since the ask can come before the submission. The request stays
on the story, visible to whoever merges, until a verify or send-back answers it, a verifier's correction
into Verified or Sent back answers it, or the requester or a verifier withdraws it. A merge recorded while
a request is open doesn't close it: the request then shows the commit that was merged before its read,
so the story carries what happened.

An open request blocks a merge, and the read must cover the head being merged: the muster's rule in
Muster's `docs/board.md`, which the operator told Rally to record on 2026-11-04. Rally can't stop a
merge on GitHub, so a merge recorded while a request was open isn't refused, which would hide that it
happened; it's recorded with `merged_before_read` on the request, as the rule broken.

A read at a different commit than the one requested still answers the request, since the head has
moved on and the request's commit is no longer what would merge; the history keeps both commits.
Whether the read covered what was merged is then `commit_differs`, once the merge is recorded.

### Who verifies: watchman or peer (RAL-51, RAL-52)

The operator's rulings of 2026-11-04, relayed by the watchman:

- While the operator has designated a watchman, the watchman reads every PR at the head being merged, as before.
- While none is designated, a verification by any team other than the author's, or by the operator, counts. Rally
  labels each verification `watchman` or `peer` by whether a watchman was designated when it was recorded.
- Designating a watchman, and ending a designation, is the operator's declaration, accepted only from the
  operator's own credential.
- A newly designated watchman re-reads every story verified by a peer since the last designation, merged or not,
  and decides with critical judgement whether to keep it, send it back or ask for fixes. Nothing stands on a peer's
  verification alone. Its decisions are its own verifications or corrections, and the peer's records stay.

## Stalls

A stall is worked out from both systems. Rally reads Muster's observed roster and combines it
with its own records:

| Muster says | Rally says | Likely meaning | What the watchman does |
|---|---|---|---|
| Can't be woken | Has an open claim | Agent is gone | Reassign the work |
| Idle, can be woken | Open claim, missed check-in | Possibly stuck | Ask the team |
| Any | Declared blocked | Real blocker | Help or escalate |
| Idle, can be woken | No claim, stories ready | Out of assigned work | Offer more |

These are prompts for the watchman, not conclusions. Resting after a story is normal under the
board process, so the rules mustn't treat every idle team as a problem. Agree with the operator
how long a check-in lasts, and test the rules against the stall of 2026-10-30.

## How Rally uses Muster

Rally doesn't need Muster. It's a ledger reached over its own service: teams record and read their work
there whatever they use to talk. Muster is one way for sessions to reach each other, and Rally uses it where
it's there. Another is a harness's own messaging. Prime Agent's sessions, for example, can message and steer
each other through Prime Agent's own daemon, on one machine, so a Prime Agent session using Rally is told that
its built-in messaging is a fine substitute for Muster, which it needs only to reach other harnesses' sessions
or other machines (RAL-59). `rally wake` (RAL-54) sends its digests through Muster when Muster is there, and
otherwise only reports; its sender is a small trait, so a harness's own messaging can be a sender too. The
session says it's Prime Agent explicitly, with `--harness prime-agent` or
`RALLY_HARNESS=prime-agent`. Prime Agent documents no variable or header that identifies it, so Rally doesn't
guess.

- **Reading the roster.** Muster's roadmap has MUS-44 for a read-only way for a local program
  to read observed session facts, and MUS-4 for whether a session can be woken. Neither exists
  yet. Today the roster is available to a session as `muster://roster`, and it doesn't separate
  observed facts from declared ones.
- **Notifying the watchman.** `muster-shim tell <team> "<message>"` already sends a direct message
  from a terminal and wakes an idle session. The watchman can't reply to a message sent that way,
  and any process on the account can send one.
- **Identity.** Nicknames change when a session restarts until MUS-3 lands, so claims recorded
  against a nickname go stale. Key claims on the team and story, and record the session only as
  who was acting at the time.

### What Muster's roster can tell Rally

From the Muster team, 2026-11-02, for RAL-7. `muster-shim roster` (muster#40) isn't merged or rolled
out yet, so nothing should be built against it until it is.

- **`observed` fields** (`bound_to_connection`, `waiting`, `waiting_until`) show that a connection or wait
  exists, not that a session is working. `bound_to_connection` can stay true after a drop until the row
  is reaped.
- **`waiting` means "probably wakeable".** It can be falsely true for minutes after a wait times out or a
  daemon restarts, and clock skew between machines shifts its expiry.
- **`attested` fields** (`machine`, `operator`) are written by the gate for paired machines, but sessions
  on the daemon's own host only claim them, and the row doesn't say which case applies.
- **Everything else is declared,** including `last_activity_at`, `activity` and `current_task`. So
  `last_activity_at` isn't proof of life, and liveness can't be read directly through the gate.
- **`wake: none`** means no sign that a message would wake the session, not that it's dead. An empty roster
  comes back as an error rather than an empty list.
- All of this holds against paired machines. A process on the daemon's host can write anything until
  MUS-7.

So the stall table's "can't be woken" row is a prompt to ask, never a conclusion that an agent is gone,
and Rally labels these facts with Muster's own categories rather than calling them all observed.

## RAL-4: where Rally runs

**Agreed by the operator on 2026-11-02,** confirming the recommendations put to them: this design, Murph
as host, the web app viewable from paired machines only at first, and a record's author being a machine's
credential for Version 1. Reserving Murph's address on the router is still to do.

- **One service on one machine.** It's the single writer, so there's one order of records and replay
  stays deterministic. Both machines reach it through a `rally` command that talks to the service.
- **Transport and access, following Muster's gate.** TLS with a pinned certificate and a separate token
  for each machine, stored as a hash on the host. These are Rally's own credentials, so Rally doesn't
  need Muster's tokens and a leak of one doesn't expose the other. It listens on a LAN or mesh address,
  never the open internet.
- **Who may connect.** Only machines the operator has paired with Rally, a1 and Murph to start with. A
  write records which machine's credential made it. The web app gets its own read-only credential.
- **When the service is down,** a write fails with a clear error rather than queueing silently, reads
  fail too, and teams fall back to Muster's `docs/board.md` process, with the watchman keeping the
  fallback board. Muster isn't affected. Outage entries are backfilled afterwards with their real time
  and who entered them, so the ledger never looks continuous when it wasn't (RAL-30).
- **Not through Muster messages.** That would tie the ledger to message delivery, which is still being
  hardened (MUS-34), and would make Muster the ledger's transport rather than a neighbor.

**Which machine:** Murph, which already runs the Muster daemon. One machine failing takes out both, but
stall detection needs Muster's roster anyway, so that costs less than it looks. Murph needs a router
reservation first. Backups go to a1, a laptop that's only available with its lid open, so they catch up
afterwards and alert when overdue, and a restore on a1 has to be tested (RAL-15).

**What we know so far, reported by the watchman on 2026-11-02:** a1's Muster connections go to the
private LAN address 192.168.1.241:6255. a1 has `Tailscale.app` installed but no `tailscale` or `wg`
command, and has `utun` interfaces nobody has identified, so a VPN isn't ruled out. a1 has been up since
10-13.

**Reported by vibrant-mink from Murph, the same day:** no Tailscale or WireGuard in use as far as it
could tell (`tailscaled` inactive, no such interfaces). Its LAN address, 192.168.1.241, comes from DHCP,
so it could change unless the router reserves it; the router wasn't checked. Murph has been up since
10-15, with crash entries on 09-26 and other reboots on 09-26, 10-12 and 10-15, reasons unknown. It
didn't reboot on 11-01. Power and network outages that didn't reboot the host weren't checked, because
nothing monitors them.

**The 11-01 crash wasn't Murph.** The operator told the watchman that a cat closed a1's lid, so a1's
terminals went unresponsive and were restarted. The crash entries on 09-26 are still unexplained.

So Rally's first transport is the LAN, protected by its own TLS and tokens rather than a mesh.

## Rally as a myko service

**Built under RAL-39, waiting to be verified.** This was the proposal, and "What was built" below says what
came of each point. Before it, Rally used SQLite and its own service. Nothing was deployed, so nothing needed
migrating.

- **Keep:** the record format, state rules, replay, hash chain, head checks, text limits and access rules.
  None depends on SQLite. The ledger's logic stays pure, so it stays deterministic.
- **Change:** each record becomes its own myko entity, appended only through a command that runs the same checks.
  Reads become myko queries. Muster's and Levi's hub are the patterns to follow.
- **One door, chosen by the team with the operator's trust (2026-11-02).** No client talks to myko. Rally's TLS and token
  API stays the only way in, with myko behind it on loopback. There's no frame filter to get wrong, which
  is where Muster's bypasses came from. The cost: other myko clients can't subscribe to Rally directly yet. An
  allowlisted read-only subscription could come later, with its own review.
- **The loopback port is the real hole** (avoided as built, since myko doesn't listen at all). myko-server 6.9.0 only binds TCP, so a Unix socket with file permissions
  isn't available without changing myko, and its port has no authentication. Any process on the host can reach
  it and write past the checks, as with MUS-7. The chain detects that but doesn't prevent it, and the docs
  must say so.
- **Raw events are the main risk** (they can't arrive as built, since nothing listens; the gate below would be
  needed only if a myko listener were ever opened). myko's WebSocket handler passes a client's events straight to the store,
  and it has no authorization hook (Muster's `docs/session-binding.md`, MUS-41). So every guarantee Rally has
  would be bypassed unless something refuses them. Rally needs the equivalent of Muster's gate in front of it,
  with a test that a raw event is refused. MUS-41 has no direction yet and myko 6.9.0 has no hook, so Rally can't
  wait for one. Muster's bypasses came from parsing frames separately from myko, so the gate should refuse by
  default, allow no client events at all, and refuse anything ambiguous (NUL in keys or values, positional bodies,
  unparseable frames). `crates/daemon/src/gate_filter.rs` in Muster shows these. myko's internal loopback port
  has no authentication (MUS-7), so a process on the host gets past the gate; that has to be bound and stated.
- **The chain must stay checkable from outside myko,** with `rally check --expect` against a head saved on a1.
  Otherwise RAL-14's guarantee is only as good as the store's access control.
- **History:** from its source, myko's Postgres persister inserts events into a table and replays them, which suits
  a record per entity. Muster calls this latest-state-per-entity, so it has to be shown rather than assumed. The
  deciding test: append three records, save the head, restart, replay, and `rally check --expect` passes.
- **Durability:** myko without Postgres keeps state in memory only, per Muster's and Levi's docs, and Postgres is
  the only durable persister that ships with myko-server 6.9.0. `Persister` is a public trait, but nobody has
  checked how replay would work with another one (MUS-17). So plan on Postgres on Murph. The backup story,
  the size limits and the disk floor (RAL-15, RAL-36) have to be redone for that. Replay memory (RAL-35)
  may improve.
- **Order:** myko applies events in arrival order, and the chain needs one writer. Appending through one
  command on one service keeps that, but it has to be tested.
- **Licence:** decided: Rally is AGPL-3.0-or-later, as myko and Levi are.

### What the spike showed

`spikes/myko-ledger` ran myko 6.9.0 against a scratch Postgres on 2026-11-02. **myko can hold this ledger, on the
conditions below.** It's a throwaway proof with arbitrary text, not Rally's real records or `rally check`.

- **Records survive.** Three records and the saved head came back identical after both SIGTERM and SIGKILL restarts.
- **The store holds only the latest version of each record.** The events table keeps earlier versions, but replay
  doesn't use them, and the table can't say who wrote a row.
- **Anyone who reaches the port can rewrite the ledger.** A raw event or the generated `DeleteLedgerRecord` command
  writes past the append path, with no refusal. So blocking raw events alone isn't enough.
- **A saved head is required.** Re-signing the rest of the chain, or deleting the newest record, passes a chain check
  with no saved head. A head saved elsewhere catches both. So does a later honest append that reuses the number.

So what's built has to do this:

1. **No listener, if it holds up.** The proof ran with no listening port, by not calling `run()`, and reached records
   through a command handler directly. That removes the loopback hole rather than stating it. It skips myko's search
   index, relations, sagas and health check, nothing documents it as supported, and it ran only briefly, so it needs
   its own check. If it fails, the fallback is loopback with the limit stated.
2. **Wait for Postgres before acknowledging an append.** myko acknowledges first. With Postgres stopped, two appends
   returned success and were lost. `PersistHealth.queued` told the truth, but it's one counter for the whole queue.
3. **One writer lock.** Without one, 8 concurrent clients got success for overwritten records and a broken chain.
4. **Never set a fixed host id.** With one, replay silently returned an empty ledger while the table held every row.
5. **Sort by numeric sequence.** The store doesn't provide an order.

Not tested: `/myko/mcp`, direct SQL tampering, Postgres restarting under a running server, backups, and replay beyond
5,000 records, which took about 160 to 270 ms and about 13 MiB in the server process.

### What was built

`src/store.rs` and `src/entity.rs`, from 2026-11-03. Each record is a `LedgerRecord` entity keyed by its sequence
number, in myko's own events table (`rally_events`) in Postgres, and myko's replay rebuilds the store from it. Records
are appended only through the `AppendLedgerRecord` command, which runs the same checks as before. `record.rs`,
`ledger.rs`, `views.rs`, `access.rs` and the chain hash didn't change. Each of the spike's conditions, with what shows
it:

1. **No listener: chosen, because it held up.** Rally builds a `MykoServer` with Postgres and never calls `run()`, so
   myko binds nothing. The service's test `nothing_listens_but_the_door` reads the running service's sockets with
   `lsof`: one listening socket, the TLS door, and otherwise only connections out to Postgres. So there's no loopback
   port to state beside MUS-7. What `run()` would have done, and what skipping it costs:
   - *Replay from Postgres:* needed, and Rally runs it: `init_postgres_and_wait`, the same catch-up `run()` waits for.
   - *Persister health check:* it only checks that the table and channel names are valid identifiers. Rally's names
     are fixed, and its own schema statements fail first if they aren't.
   - *Search index:* only myko's search queries use it, and Rally has none. Records set later are indexed anyway.
   - *Relations, server ownership, sagas:* `LedgerRecord` declares no relations and isn't server-owned, and neither
     Rally nor myko 6.9.0 registers a saga.
   - *Periodic loggers and telemetry:* lost, and not needed.

   It held up past the spike's brief run: `many_appends_and_repeated_restarts_hold_up` makes 1,000 appends through the
   service with five restarts, alternating SIGKILL and SIGTERM, and checks against a saved head after each. Replay was
   also measured at 64,001 and 256,501 records (below). This mode isn't documented by myko as supported, so a myko
   upgrade has to rerun these tests, and the version stays pinned at `=6.9.0`.

   **How `rally check` and the viewer reach records with no port:** from other machines, through the service, as
   before; on the host, read-only straight from Postgres with `--db`, not through myko. That second way doesn't share
   the store it's checking, needs no lock, so it works while the service runs, and its connection is read-only in
   Postgres too.
2. **An append isn't acknowledged until Postgres has it.** myko's own producer queues events and acknowledges first,
   and `PersistHealth.queued` is one counter for the whole queue, so it can't say which event is durable. It isn't
   reliable enough for this, so Rally doesn't use it: the command commits the record's row itself, in myko's own
   format, before the record enters myko's store or the write is acknowledged. myko's persister for every entity is
   a blackhole, so it never writes a second copy. Tests stop a scratch Postgres under a running writer and a running
   service: writes fail promptly (the tests allow 30 s), with no success reply, myko's copy doesn't take the record, and after Postgres
   comes back the writer reconnects, retakes its lock and carries on, with every acknowledged record present after a
   restart. If the connection dies after the commit is sent, Postgres may have the record without its being
   acknowledged, so that write reports the outcome as unknown, never as nothing recorded (a test ends the backend
   between the INSERT and the COMMIT). If it was recorded, the next append finds Postgres ahead of myko's copy and
   refuses until a restart replays it. So the recovery is a restart of the service; a writer stuck this way refuses
   for good otherwise.
3. **One writer lock:** a Postgres advisory lock, held by the service or a `--db` writer for as long as it's open, so it
   works across processes, and Rally's own lock inside one. Tests run eight writers each holding its own store, eight
   threads sharing one, six `rally` processes, and eight machines writing through the service: every success has its
   own number and the chain holds. Before each append the writer also checks that Postgres's newest record is the one
   it replayed. The table has a unique index on a record's number, so Postgres itself refuses a second version.
4. **No fixed host id.** Each run gets a new one; a test checks that replay returns every record across restarts.
5. **Sorted by number.** Ids must be canonical decimals, and reads sort by their value; a test with 14 records checks
   order across restarts. Numbers aren't reused: an append takes the next number after the replayed chain, and the
   unique index refuses a repeat even if someone writes the row by hand.
6. **The deciding test passes,** with real records and the real command: three appends with `rally`, the head saved
   from `rally check`, a real restart with SIGTERM and then SIGKILL, replay, then `rally check --expect` through the
   service and on the host. A rewrite and a truncation written as myko events would leave them (a second version of
   a record, or a deletion) are caught by the saved head; while the old rows remain, `check` also reports the record
   has more than one version, and a writer refuses to start over it. With those rows removed too, only the saved head
   catches the rewrite, as RAL-14 said.
7. **Generated `DeleteLedgerRecord` commands and raw events can't arrive,** because nothing listens and the door has no
   route to myko: `myko_cant_be_reached_through_the_door`. Run inside the process anyway, as a test does, they change
   only myko's copy, never Postgres, and `check` reports the difference.

The table also refuses updates, deletes, `TRUNCATE` and deletion events, and a check constraint makes each row's
`item_type`, `item_id` and `change_type` columns equal its event's `itemType`, `item.id` and `changeType`. Without it,
a row with other columns and a `LedgerRecord` event would pass every protection, since they key on the columns, while
myko loaded it by the event. Reads and checks go by the event too, and report any row that doesn't match. That
catches mistakes, not someone who owns the database, who can lift it.

**The saga channel.** `server.ctx()` gives a context that sends every change to myko's saga channel, which only
`run()` drains. With no listener, that kept a second copy of every appended record for as long as the process ran:
25.6 KB per 12.2 KB record, counted by the allocator. Rally builds its own context with no saga sink, and the same
count is 12.4 KB, the store's own copy; `tests/memory.rs` fails if an append keeps more than 1.5 times the record.

**Each write's own outcome, and a consistent check.** A third read found that concurrent appends could swap
their typed errors through one shared slot, and that `check` read Postgres and myko's copy at different moments,
so an append in between looked like tampering. Each append now gets its own error by its request's id, and `check`
reads both under the writer's lock; tests that widen those gaps on purpose failed before and pass now. The same
read found myko quoting the address, password included, in its errors, so myko now gets a URL it hides the password
in, and Rally removes it from anything myko says.

**Reads and a hung Postgres.** Reads come from myko's copy without the writer's lock, so a write waiting on a hung
Postgres doesn't block them; a test stops the scratch Postgres's processes with SIGSTOP mid-write. Before, the read
waited behind the write for as long as Postgres stayed stopped. The write itself has no deadline of Rally's own; see
`docs/service.md`.

**What this does to RAL-15 and RAL-35.** Neither is done. For RAL-15, a backup is now the Postgres database (for
example `pg_dump` of `rally_events`) together with the head file, which lives outside Postgres, and a restore has to
rebuild both and pass `rally check --expect`; none of that is built. For RAL-35, reads still replay the whole ledger,
now from myko's in-memory copy rather than a file, and the service now holds every record in memory, about once, for as long as
it runs. Measured on a1 with a release build, at the 64 MiB limit on the table's size: 64,001 typical 300-character
records take 1.1 s to replay at start, a read through the service 0.2 s and a check 0.3 s, and the service holds
about 150 to 230 MiB. 4,751 worst-case records of 4,000 random CJK characters (about 12 KB) take 0.6 s and about
130 to 150 MiB in the service, and a direct `--db` write peaks at 244 MiB. At 256 MiB (256,501 typical records) it's
4.1 s, 0.9 s, 1.7 s and about 520 to 730 MiB, so growth is linear. The service's memory now stays in use between
requests rather than rising and falling. An append through the running service, at the 64 MiB limit with typical
records, took 0.22 to 0.29 s end to end from a fresh `rally` process, including its start and the TLS connection,
with the service at 143 MiB. Worst-case records weren't timed for appends.

**What three reads changed.** RAL-39 was read three times before merging, by `plush-geyser` and `dauntless-peacock`,
and each round found real problems:

1. **First read:** a row whose columns didn't match its event JSON got past every protection and fooled `--db check`; a
   commit with an unknown outcome was reported as "nothing was recorded"; the documented password setup couldn't work;
   redaction missed some address forms; `adopt-head` raced the service; myko's saga channel kept a second copy of every
   record; reads hung behind a hung Postgres; and the test guard could reach port 5432.
2. **Second read:** the fixes for those opened three new problems: concurrent appends shared one error slot and could
   swap replies; myko's own error text could carry the password; and a check during an append could report a false
   difference.
3. **Third read:** a stray password-file setting blocked commands that never use the database.

Each was shown by a failing test before its fix, except the port guard, which couldn't be tested without touching the
operator's Postgres. The team's lesson: a fix round can introduce regressions, so concurrent tests belong in the first
round, not the second.

## Levi

[Levi](https://github.com/ignition-is-go/levi) is an agent-first issue tracker whose events travel with a
repository in `refs/levi/events`. A task's status is a function of git history, it has advisory claims, every
read has stable `--json`, and its optional hub is a myko server. It overlaps with Rally on stories and claims.

**Proposal, for RAL-40:** Levi says what the work is and what git history shows, and Rally stays the ledger of
verification, evidence, team state and history. Rally reads Levi from a checkout with `levi --json`, labelling each fact as declared unless it comes from git,
and doesn't write to it at first. It doesn't trust a Levi hub, which by its own warning has no authentication. "Closed at
a commit that's an ancestor of HEAD" is the kind of observed fact Rally wants, and could help RAL-5.
What's in a Levi task is written by agents, so it's untrusted text, and a Levi claim is declared, not proof.
**Claims live in Levi,** on the operator's word, so there's one source of truth for them. **Built for RAL-40:**
`rally levi` reads a checkout's tasks this way, read-only and without the service; see [levi.md](levi.md). RAL-40
settled the claims: projects not on Levi, which on 2026-11-05 was all of rally, muster and catapp, keep Rally's
`claim`, `release` and holder check unchanged. On a project that moves to Levi the claim is Levi's (`levi start`,
`levi drop`), and `rally claim` becomes the team's record of the Levi claim it holds, which the holder check on
`submit` still needs since the service has no checkout. That part isn't built until a project moves. Which of Rally's
features would be additive for Levi, which would change its defaults, and which conflict with it is in
[rally-vs-levi.md](rally-vs-levi.md).

## Open questions

1. **Where does Rally run, and how do both machines reach it?** Answered by RAL-4 above.
2. **How is Rally's state stored?** Revisited under RAL-39, on the operator's word: an append-only log of records
   as myko entities in Postgres on the host, written only by Rally's service through one command. See "What was
   built" above. It gives the history the Version 0 table lacks, lets a wrong entry be corrected without erasing
   it, and can rebuild the past ledgers the web app has to show. It replaced a log in SQLite.
3. **What does "someone other than the author" mean when identity can't be proved?** Any process on
   an account can act as that machine, so this rule catches mistakes rather than a determined
   forgery. Say so plainly, and revisit it when Muster has something stronger (MUS-39). Agreed for
   Version 1 under RAL-4: the check compares declared names, and authors show as a credential declaring
   a name, for example "a1 credential, declaring gleaming-tench".
4. **How do other projects join?** Stories stay in each project's roadmap file, which its team
   owns, and Rally tracks them by ID. Rally records their state and evidence, not their text.
5. **Who can see what, and how?** Rally's own web app, viewable from paired machines only at first
   (RAL-4). It's read-only, and phone viewing waits for RAL-34. A Rally view inside Muster would
   break the one-way dependency.
6. **Should Rally's notifications use structured messages?** Deferred until the MUS-43 design
   meeting, which isn't scheduled. Rally's digests are a good test case, and RAL-8 waits for it or starts
   with plain text.
