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

**Proposal, for RAL-39.** What's built today uses SQLite and its own service, and it isn't deployed, so
changing costs little now.

- **Keep:** the record format, state rules, replay, hash chain, head checks, text limits and access rules.
  None depends on SQLite. The ledger's logic stays pure, so it stays deterministic.
- **Change:** records become a myko entity, appended only through a command that runs the same checks. Reads
  become myko queries. Muster's and Levi's hub are the patterns to follow.
- **Raw events are the main risk.** myko's WebSocket handler passes a client's events straight to the store,
  and it has no authorization hook (Muster's `docs/session-binding.md`, MUS-41). So every guarantee Rally has
  would be bypassed unless something refuses them. Rally needs the equivalent of Muster's gate in front of it,
  with a test that a raw event is refused, or a hook upstream. Muster's filter had three bypasses, so RAL-39
  should prefer whatever MUS-41 decides.
- **Durability:** myko without Postgres keeps state in memory only, per Muster's and Levi's docs. A ledger
  has to survive a restart, so Rally needs Postgres on Murph, or a myko persister that fits. The backup story,
  the size limits and the disk floor (RAL-15, RAL-36) have to be redone for that. Replay memory (RAL-35)
  may improve.
- **Order:** myko applies events in arrival order, and the chain needs one writer. Appending through one
  command on one service keeps that, but it has to be tested.
- **Licence:** myko is AGPL-3.0-or-later, and Rally is MIT OR Apache-2.0. **The operator should decide** what
  Rally's licence should be before this merges. Levi is AGPL to match, and Muster declares MIT OR Apache-2.0.

## Levi

[Levi](https://github.com/ignition-is-go/levi) is an agent-first issue tracker whose events travel with a
repository in `refs/levi/events`. A task's status is a function of git history, it has advisory claims, every
read has stable `--json`, and its optional hub is a myko server. It overlaps with Rally on stories and claims.

**Proposal, for RAL-40:** Levi says what the work is and what git history shows, and Rally stays the ledger of
verification, evidence, team state and history. Rally reads Levi and doesn't write to it at first. "Closed at
a commit that's an ancestor of HEAD" is the kind of observed fact Rally wants, and could help RAL-5.
What's in a Levi task is written by agents, so it's untrusted text, and a Levi claim is declared, not proof.
Whether claims live in Levi or Rally is **for the operator**, since two sources of truth for a claim would
disagree. No Rally project has been checked for Levi use yet.

## Open questions

1. **Where does Rally run, and how do both machines reach it?** Answered by RAL-4 above.
2. **How is Rally's state stored?** Being revisited under RAL-39, on the operator's word. Built so far: an
   append-only log of events in SQLite on the host, written by Rally's Rust service, not myko. It gives the history the Version 0 table lacks, lets a wrong entry
   be corrected without erasing it, and can rebuild the past ledgers the web app has to show. A single
   writer and a plain file also keep replay and backups simple. Muster's decision to stay on myko doesn't
   bind Rally.
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
