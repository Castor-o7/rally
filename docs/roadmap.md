# Rally roadmap

Goal: a ledger that shows the progress of a muster and where each team is, so that the watchman
can spot a stalled team and find out why.

IDs are permanent. Don't reuse or renumber them. A story without a "done when" line isn't ready to
claim. Until Rally can track itself, stories follow the process in Muster's `docs/board.md`.

## Depends on Muster

- **MUS-3: Stable identity across restarts.** Claims shouldn't go stale when a session restarts.
- **MUS-4: Show dead watchers.** Needed to tell a gone agent from a quiet one.
- **MUS-44: Let other programs read the observed roster.** Rally's way to read session facts.
- **MUS-43: Give muster messages a deterministic structure.** May shape Rally's notifications.

## Design

- [ ] **RAL-1: Agree the design.** Continues MUS-32. Start from [design.md](design.md). Done when:
  the operator has agreed the design, its open questions are answered or deliberately deferred, and
  follow-up stories are filed here.
- [ ] **RAL-4: Decide where Rally runs.** Open question 1 in the design, and the first to settle.
  Done when: the design records how both machines read and write the ledger and who may connect, and
  the operator has agreed it.

## Version 1

Refine these under RAL-1. Their "done when" lines are a first draft.

- [ ] **RAL-3: Record stories and their states.** Continues MUS-23. Done when: a team can claim a
  story and submit it for review with a command, an invalid state change is refused, and each change
  keeps its history, including corrections.
- [ ] **RAL-2: Show whether a team is available, working or blocked.** Continues MUS-28. Done when: a
  team can set its state, the watchman can read it without sending a message, and it's labelled as
  declared.
- [ ] **RAL-5: Check submitted evidence.** Done when: on submission, Rally records as observed whether
  the commit exists, names the story and passed CI, and a test covers each failure.
- [ ] **RAL-6: Verify separately from the author.** Done when: a story can only be marked Verified or
  Sent back by someone other than its submitter, with the verifier and note recorded, and the docs say
  what this does and doesn't prove.
- [ ] **RAL-7: Detect a stall.** Depends on MUS-4 and MUS-44. Done when: each row of the stall table in
  the design has a test, a team resting after a story isn't flagged, and the stall of 2026-10-30 would
  have been flagged.
- [ ] **RAL-8: Tell the watchman.** Done when: the watchman wakes with one batched digest when something
  changes, and hears nothing when nothing has.
- [ ] **RAL-9: Replace the hand-kept table.** The current table is
  `~/Documents/Caster/muster-board.md` on the operator's Mac. Done when: its contents are imported, the
  watchman no longer needs it, and Muster's `docs/board.md` points to Rally.
