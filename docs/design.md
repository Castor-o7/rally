# Rally design

**Draft, 2026-11-02.** This is the starting point for [RAL-1](roadmap.md), not a settled
design. The team that owns Rally can change anything here except the decisions the operator
has made, which are listed first.

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

## Open questions

Answers marked "proposed" are for RAL-1 and need the operator's agreement.

1. **Where does Rally run, and how do both machines reach it?** Answered by RAL-4 above.
2. **How is Rally's state stored?** Proposed: an append-only log of events in SQLite on the host, written
   by Rally's Rust service, not myko. It gives the history the Version 0 table lacks, lets a wrong entry
   be corrected without erasing it, and can rebuild the past ledgers the web app has to show. A single
   writer and a plain file also keep replay and backups simple. Muster's decision to stay on myko doesn't
   bind Rally.
3. **What does "someone other than the author" mean when identity can't be proved?** Any process on
   an account can act as that machine, so this rule catches mistakes rather than a determined
   forgery. Say so plainly, and revisit it when Muster has something stronger (MUS-39). Agreed for
   Version 1 under RAL-4: the check compares declared names, and authors show as a credential declaring
   a name, for example "a1 credential, declaring gleaming-tench".
4. **How do other projects join?** Proposed: stories stay in each project's roadmap file, which its team
   owns, and Rally tracks them by ID. Rally records their state and evidence, not their text.
5. **Who can see what, and how?** Rally's own web app, viewable from paired machines only at first
   (RAL-4). Proposed: it's read-only, and phone viewing waits for RAL-34. A Rally view inside Muster would
   break the one-way dependency.
6. **Should Rally's notifications use structured messages?** Proposed: deferred until the MUS-43 design
   meeting, which isn't scheduled. Rally's digests are a good test case, and RAL-8 waits for it or starts
   with plain text.
