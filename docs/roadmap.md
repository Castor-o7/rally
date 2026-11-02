# Rally roadmap

Goal: if Muster is the musculature of the network, Rally is the bones. Rally is the ledger of the
current and past state of the muster's teams. The watchman uses it to verify work, see what each team
has achieved and what it's facing, unblock teams, and plan, while respecting each team. Security of the
information and the environment runs through all of it. A visually appealing web app shows the current
ledger and past ones.

IDs are permanent. Don't reuse or renumber them, so they aren't in numeric order below. A story without
a "done when" line isn't ready to claim. Until Rally can track itself, stories follow the process in
Muster's `docs/board.md`.

The operator agreed the design and threat model on 2026-11-02. Later phases' "done when" lines can
still be refined as each story is taken.

## Depends on Muster

- **MUS-3: Stable identity across restarts.** Claims shouldn't go stale when a session restarts.
- **MUS-4: Show dead watchers.** Needed to tell a gone agent from a quiet one.
- **MUS-44: Let other programs read the observed roster.** Rally's way to read session facts.
- **MUS-43: Give muster messages a deterministic structure.** May shape Rally's notifications.
- **MUS-12: Give the watchman a read-only GitHub token.** Relevant to RAL-18.
- **MUS-39: Show that a terminal message came from the operator.** Limits what RAL-6 can prove today.

## Phase 0: Design

- [x] **RAL-4: Decide where Rally runs.** Open question 1 in the design, and the first to settle.
  Done when: the design records how both machines read and write the ledger and who may connect, and
  the operator has agreed it.
- [x] **RAL-1: Agree the design.** Continues MUS-32. Start from [design.md](design.md). Done when:
  the operator has agreed the design, its open questions are answered or deliberately deferred, and
  follow-up stories are filed here.
- [x] **RAL-10: Write the threat model.** Before any service exists. Done when: `docs/` says what the
  ledger holds, who could want to read or alter it, how the service and web app could be attacked, what
  must never be recorded, and what Rally doesn't defend against, and the operator has agreed it.

## Phase 1: The ledger

The bones. Everything later reads from this.

- [x] **RAL-11: Define the ledger's record format.** Done when: every record has a versioned format
  giving its kind (observed, declared or verified), source and time, and a record missing any of them
  is refused, with a test for each.
- [ ] **RAL-3: Record stories and their states.** Continues MUS-23. Done when: a team can claim a
  story and submit it for review with a command, an invalid state change is refused, and each change
  keeps its history, including corrections.
- [ ] **RAL-12: Make replay deterministic.** Done when: replaying the same records always produces
  identical state, the state as of any past record can be rebuilt, and a test shows both. Replay reads
  no clock or outside system.
- [ ] **RAL-2: Show whether a team is available, working or blocked.** Continues MUS-28. Done when: a
  team can set its state, the watchman can read it without sending a message, and it's labelled as
  declared.
- [ ] **RAL-13: Control who can write and read.** Depends on RAL-4 and RAL-10. Done when: an
  unauthorized write or read is refused, shown by a test for each route into the service, and the docs
  say what an accepted write does and doesn't prove about who sent it.
- [ ] **RAL-14: Make tampering detectable.** Done when: a check reports any altered, removed or
  reordered record, including the newest records being removed, which doesn't show as a gap in the
  sequence; a test covers each; and the docs say what this can't catch, such as someone with full access
  rewriting the whole ledger.
- [ ] **RAL-15: Back up and restore the ledger.** Done when: a backup restored on the other machine
  passes the RAL-14 check and rebuilds the same state, shown by a real restore; backups get the same
  protection as the ledger; and because a1 is only available with its lid open, backups catch up
  afterwards and alert when overdue.
- [ ] **RAL-16: Treat recorded text as untrusted.** Teams write text that the watchman and the web app
  later read. Done when: free text has a length limit, is marked as untrusted wherever it's shown to an
  agent, and a test shows that text written as instructions or markup is displayed only as text.
- [ ] **RAL-17: Keep secrets out of the ledger.** An append-only ledger can't simply forget one. Done
  when: submissions matching common token and key formats are refused, a test covers each format, and
  the docs say how a secret that gets through is handled and that this check isn't complete.

## Phase 2: Verification

- [ ] **RAL-5: Check submitted evidence.** Done when: on submission, Rally records as observed whether
  the commit exists, names the story and passed CI, and a test covers each failure. What a team didn't
  test is its own field, not free text.
- [ ] **RAL-31: Tie verification to a commit.** Heads can move while work waits: muster#34 changed three
  times in review on 2026-11-02. Done when: each Verified and each requested second read names a commit,
  and Rally records as observed when the merged commit differs from the verified one, with a test.
- [ ] **RAL-32: Show a requested second read.** catapp#78 merged before a requested read arrived. Done
  when: a requested read is visible to whoever merges until it's answered or withdrawn. Whether it
  blocks a merge is the operator's call, and this story records it either way.
- [ ] **RAL-33: Record merges as commits.** Done when: Merged records the commit, is independent of
  Verified, and doesn't claim who merged, since every session shares one GitHub account.
- [ ] **RAL-18: Check evidence with least privilege.** Done when: evidence checks use read-only GitHub
  access, a merge attempt with that access is refused, and a test shows that Rally reads evidence without
  checking out or running submitted code.
- [ ] **RAL-6: Verify separately from the author.** Done when: a story can only be marked Verified or
  Sent back under a declared name other than its submitter's, with the verifier and note recorded, and
  the docs say that this only catches mistakes, because both may hold the same machine's credential.
- [ ] **RAL-19: Let a team read and correct its own record.** Done when: a team can read everything
  recorded about it without asking the watchman, and can append a correction or a disagreement that's
  shown beside the original. Verification outcomes stay with the verifier.

## Phase 3: The watchman's view

- [ ] **RAL-22: Brief a new watchman from the ledger.** The watchman that started after the crash
  reported on 2026-11-02 had a board last saved on 2026-10-31 and no message history, so it had to ask
  every team to resend. Done when: a new session can read one brief giving each team's state, open
  claims (flagging those whose session has ended), submissions awaiting verification, open blockers,
  decisions waiting on the operator and who each holds up, holds in force and what releases them,
  conditions on the order of work, standing grants with their scope and whether each was direct or
  relayed, and how old each fact is; and a test shows no open submission is lost when the watchman
  changes. The brief records grants but confers no authority, and proof of who holds a role comes from
  Muster, not the ledger. Each field shows whether it's observed, declared or verified. Machine access
  appears only as that it exists and where its details live. List from the watchman, 2026-11-02. Live
  daemon commit and backups are for RAL-1 to place.
- [ ] **RAL-20: Record who is waiting on whom.** Done when: a team can name what it's blocked on,
  including another team's story, the watchman can list open blockers and chains of them, and a blocker
  stays open until someone closes it, with who and why recorded. A blocked story and a blocked team are
  recorded separately.
- [ ] **RAL-30: Reconcile an outage.** Done when: entries made on the fallback board during an outage can
  be backfilled with their real time and who entered them, the gap itself stays visible, and a test
  shows that backfilled records can't pass for live ones.
- [ ] **RAL-7: Detect a stall.** Depends on MUS-4 and MUS-44. Done when: each row of the stall table in
  the design has a test, a team resting after a story isn't flagged, and the stall of 2026-10-30 would
  have been flagged.
- [ ] **RAL-8: Tell the watchman.** Done when: the watchman wakes with one batched digest when something
  changes, and hears nothing when nothing has. Agree the format with the Muster team under MUS-43.
- [ ] **RAL-21: Keep each team's track record as plain facts.** Done when: the watchman can read each
  team's stories verified, stories sent back and what was missing, and problems reported, with no points,
  scores, weights or ranking.
- [ ] **RAL-23: Show progress across roadmaps.** For the watchman's planning. Done when: for each
  roadmap, the watchman can see how many stories are in each state, which ready stories have nothing
  blocking them, which blockers hold up the most stories, and which roadmap ticks have no Verified
  behind them, with no comparison or ranking of teams.
- [ ] **RAL-24: Bring in other projects.** Open question 4 in the design. Done when: the decision is
  recorded, and Muster, catapp, ParadoxDetector and any other team on the board can be tracked by it.
- [ ] **RAL-9: Replace the hand-kept table.** Depends on RAL-24. The current table is
  `~/Documents/Caster/muster-board.md` on the operator's Mac. Done when: its contents are imported, the
  watchman no longer needs it, and Muster's `docs/board.md` points to Rally.

## Phase 4: The web app

Read-only. Changes go through the ledger's commands, so a flaw in the web app can't rewrite the ledger.

- [ ] **RAL-25: Give the ledger a read-only API.** Done when: it's documented, it uses RAL-13's access
  control, and a test shows no request through it can change the ledger.
- [ ] **RAL-26: Agree the look with the operator.** Done when: the operator has approved a visual design
  covering the current and past views, on a phone as well as a desktop.
- [ ] **RAL-27: Show the current ledger.** Done when: the web app shows each team's state, stories,
  evidence, blockers and track record, says for each whether it's observed, declared or verified and how
  fresh it is, and the operator has seen it running.
- [ ] **RAL-28: Show past ledgers.** Depends on RAL-12. Done when: a viewer can pick a past moment and
  see the ledger as it stood then, with corrections made since shown as later, and a test shows that the
  view matches a replay to that moment.
- [ ] **RAL-29: Secure the web app.** Done when: a test shows that a cross-origin page can't read it,
  that markup in recorded text doesn't run, and that an unauthorized viewer is refused; it isn't exposed
  to the open internet; and someone other than its author has reviewed it adversarially, before it's
  reachable beyond the machine it runs on.
- [ ] **RAL-34: View from a phone.** Not ready to claim until the operator wants it; they may. Depends
  on RAL-29. Done when: an unpaired device the operator names can read the ledger with a credential of
  its own that can be revoked alone, after its own threat review.
