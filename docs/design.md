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

## Carried over from Muster's roadmap

These come from Muster's MUS-23, MUS-28 and MUS-32 and its board process. They aren't
operator decisions, but they've held up so far:

- Rally records and informs. It doesn't command, and agents and the operator can correct it.
- It keeps track records as plain facts: stories verified, stories sent back and what was
  missing, and problems reported. There are no points, scores or weights.
- It wakes the watchman with batched digests, and only when something changes.
- Merged and Verified are independent facts.

## What Rally can and can't determine

Rally can decide some things deterministically and has to label the rest. Each record has one
of three kinds:

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

## Open questions

1. **Where does Rally run, and how do both machines reach it?** Decide this first (RAL-4). The
   muster spans a Mac (a1) and a Linux box (Murph). A CLI writing to a local file only works on
   one machine, so the first idea of a small CLI with an append-only log isn't enough by itself.
   Options include a single service on one machine, or sending writes through Muster messages. A
   service needs its own answer to who may connect.
2. **How is Rally's state stored?** An append-only log of events is the leading idea, because it
   gives the history the Version 0 table lacks and lets a wrong entry be corrected without erasing
   it. SQLite is one candidate. Whether Rally should use myko is also open; Muster's decision to
   stay on myko doesn't bind Rally.
3. **What does "someone other than the author" mean when identity can't be proved?** Any process on
   an account can act as that machine, so this rule catches mistakes rather than a determined
   forgery. Say so plainly, and revisit it when Muster has something stronger (MUS-39).
4. **How do other projects join?** The board already covers Muster, catapp and ParadoxDetector, each
   with its own roadmap file. Decide whether stories stay in those files with Rally tracking them, or
   Rally becomes their home.
5. **Who can see what, and how?** Candidates are a CLI, Muster's TUI or web UI, or a generated table. A
   Rally view inside Muster would break the one-way dependency.
6. **Should Rally's notifications use structured messages?** Muster's MUS-43 is designing those, so
   Rally's digests are a good test case. Talk to the Muster team before settling a format.
