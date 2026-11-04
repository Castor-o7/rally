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

## What a first deployment needs

Rally can run on Murph, as the ledger of record for Rally's own stories first, once these are done:

1. RAL-39: Rally as a myko service, first, so that the items below are only built once. RAL-13: the service
   and its access control, so both machines can reach it. RAL-36, so it can't fill the
   disk Muster's daemon needs, and a check of Murph's free disk before rollout.
2. RAL-15: backups to a1, with a real restore, so nothing is lost.
3. RAL-14, RAL-16 and RAL-17: tamper detection, untrusted text and secrets, because an append-only ledger
   can't simply forget what it took in.
4. RAL-6: verification limits written down, because teams will read Verified as meaning something.
5. The operator: a router reservation for Murph, and approval of the rollout.

The trial keeps the watchman's table and Rally in parallel for Rally's own stories, comparing the two.
Muster's `docs/board.md` stays in force throughout. Before the watchman relies on the ledger, and the
other teams join (RAL-24) or the table is retired (RAL-9), it also needs:

- RAL-15's tested restore on a1, with the catch-up and overdue alert in use.
- RAL-30's outage backfill used once for real.
- RAL-31's warning when a merged commit differs from the verified one.
- RAL-7 reading roster facts from `muster-shim roster`, once that has rolled out.
- RAL-35's limits on load, with read time measured as records grow, before other teams join.

The web app comes after this.

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

- [x] **RAL-39: Make Rally a myko service.** Requested by the operator on 2026-11-02. See the design. Done
  when: records are myko entities appended through one command with today's checks; every existing guarantee
  still has a passing test; a test shows a raw event from a client is refused; `rally check --expect` can check
  the chain from outside myko; records survive a restart; an append isn't acknowledged until Postgres has it, with
  a test that stops Postgres; concurrent appends keep one chain; and the design's choice of no listener or loopback
  is recorded with its evidence. The licence is AGPL-3.0-or-later, decided by the operator.
- [ ] **RAL-40: Interface with Levi.** Requested by the operator on 2026-11-02. See the design. Claims live in
  Levi. Done when: the design says what happens to Rally's own claim commands and to projects not on Levi, and
  Rally can read a repository's Levi tasks through `levi --json`
  or its hub, labelled by how each fact is known, with Levi's text treated as untrusted and nothing
  written to Levi, with a test against a real `levi` run.
- [x] **RAL-42: Clear the source review's smaller findings.** From `dauntless-peacock`'s review of `a4dc39e`, all
  predicted from reading, not run. After RAL-39, since some concern the store. Done when: each is fixed, shown not
  to apply, or recorded as accepted: reads from a replaced ledger file; a submitter's verified annotation on their
  own story; an empty `cert.pem` passing; write failures reported as "can't be read"; leftover temporary head files;
  no overall deadline or reply cap in the client; unbounded command output; roadmap code-fence edge cases; story
  IDs with leading zeros; names `add-machine` accepts that the ledger refuses; and other loopback addresses. From the
  second and third reads of RAL-39: a write to a Postgres that hangs rather than stops has no deadline of
  Rally's own, and later writes and checks queue behind it, each holding a blocking thread, so reads last only
  until the pool runs out; the address given to myko keeps only host, port, user, database and password; the
  password file is checked then read, with no check of its directory.
- [ ] **RAL-41: Tighten text and name checks.** From the reads of RAL-16. Done when: zero-width and direction
  characters are refused in names, so `alice` and a look-alike can't pass as different people; `/v1/check`
  carries the untrusted-text notice; and limits don't apply on replay in a way that makes an old ledger
  unreadable if they tighten, with a test for each. Decide this before deployment.
- [x] **RAL-11: Define the ledger's record format.** Done when: every record has a versioned format
  giving its kind (observed, declared or verified), source and time, and a record missing any of them
  is refused, with a test for each.
- [x] **RAL-3: Record stories and their states.** Continues MUS-23. Done when: a team can claim a
  story and submit it for review with a command, an invalid state change is refused, and each change
  keeps its history, including corrections.
- [x] **RAL-12: Make replay deterministic.** Done when: replaying the same records always produces
  identical state, the state as of any past record can be rebuilt, and a test shows both. Replay reads
  no clock or outside system.
- [ ] **RAL-2: Show whether a team is available, working or blocked.** Continues MUS-28. Done when: a
  team can set its state, the watchman can read it without sending a message, and it's labelled as
  declared.
- [x] **RAL-13: Control who can write and read.** Depends on RAL-4 and RAL-10. Done when: an
  unauthorized write or read is refused, shown by a test for each route into the service, and the docs
  say what an accepted write does and doesn't prove about who sent it.
- [x] **RAL-35: Limit load on the service.** From the second read of RAL-13, and now required before the trial,
  because a source review predicts one read-only credential could stall everything and pin gigabytes on a host
  shared with Muster. Measure these first; they're predictions. Done when: response memory is bounded, replay
  isn't quadratic in a story's corrections, the read limit fits what the store can serve, and connections are
  capped and time out before they authenticate, failed token attempts are limited, and reads don't replay
  the whole ledger under the store's lock or block the service's workers, with a test for each, and with
  memory and time measured at the ledger's limit before that limit is raised. Since RAL-39, the starting point
  measured on a1 is 143 to 230 MiB held for as long as the service runs at the 64 MiB limit, beside Muster's
  daemon on Murph. Also: a check holds the writer lock for a full Postgres read, so it stalls every write, and a
  read-only credential can trigger it.
- [x] **RAL-36: Stop before the disk fills.** Rally shares Murph with Muster's daemon. Required before the
  trial. Done when: the service refuses writes while free space on the ledger's disk is below a set floor,
  reads carry on, the error says why, and a test shows it. The docs say what isn't covered.
- [x] **RAL-14: Make tampering detectable.** Done when: a check reports any altered, removed or
  reordered record, including the newest records being removed, which doesn't show as a gap in the
  sequence; a test covers each; and the docs say what this can't catch, such as someone with full access
  rewriting the whole ledger.
- [ ] **RAL-15: Back up and restore the ledger.** Done when: a backup restored on the other machine
  passes the RAL-14 check and rebuilds the same state, shown by a real restore; backups get the same
  protection as the ledger; and because a1 is only available with its lid open, backups catch up
  afterwards and alert when overdue. Each backup keeps the ledger and its head file together, also saves the ledger's head on a1, and the watchman can
  run `rally check --expect` against it (RAL-14), so that a rewrite on the host shows.
- [x] **RAL-16: Treat recorded text as untrusted.** Teams write text that the watchman and the web app
  later read. Done when: free text has a length limit, is marked as untrusted wherever it's shown to an
  agent, and a test shows that text written as instructions or markup is displayed only as text.
- [x] **RAL-17: Keep secrets out of the ledger.** An append-only ledger can't simply forget one. Done
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
- [x] **RAL-6: Verify separately from the author.** Done when: a story can only be marked Verified or
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

- [x] **RAL-25: Give the ledger a read-only API.** Done when: it's documented, it uses RAL-13's access
  control, and a test shows no request through it can change the ledger.
- [ ] **RAL-26: Agree the look with the operator.** Done when: the operator has approved a visual design
  covering the current and past views, on a phone as well as a desktop.
- [ ] **RAL-27: Show the current ledger.** Done when: the web app shows each team's state, stories,
  evidence, blockers and track record, says for each whether it's observed, declared or verified and how
  fresh it is, and the operator has seen it running.
- [x] **RAL-37: Show each roadmap's progress.** Requested by the operator on 2026-11-02. Done when: the
  web app shows ticks from each roadmap file as a percentage, labelled with what a tick means there and
  with the file, commit and read time; Verified is a separate figure from the ledger, or "not available
  yet"; the two are never merged; and a test covers the counting.
- [ ] **RAL-38: Show what each team is working on.** Requested by the operator on 2026-11-02. Depends on
  the rollout of MUS-44. Done when: the web app shows each team's declared state and Muster's roster with
  Muster's own labels and each fact's age, and the operator has seen it with live data.
- [x] **RAL-43: Give each project an info page.** Requested by the operator on 2026-11-03: "all of this
  information should be viewable under each muster team's project info page, which should be accessible by
  clicking on the project in the rally viewer app." Done when: in `rally web`, each project in the roadmap list
  links to a page of its own showing (1) the roadmap's progress and its stories by state, counted as the
  overview counts them; (2) the ledger's records for that project, once the service is deployed (RAL-27); (3)
  the team's latest day summary, read from `docs/muster/<date>.md` in that project's repository, shown as text
  only under RAL-16's rules, with its file, commit and read time like the roadmap's; and (4) the facts the
  viewer can source from that repository, such as merged PRs, ticks and their dates, never from a message.
  Everything is read the way the roadmaps are, through `gh`, and the page keeps the viewer's loopback,
  read-only, no-markup rules, with a test for each part and for hostile text in the summary. Also, from the
  operator the same evening: "the summary for the total rally progress should be viewable at the bottom of the
  rally summary page please". So the main page ends with the totals across projects, and the watchman's
  writeup of the whole muster from `docs/muster/watchman/<date>.md` in Rally's own repository, under the same rules.
- [ ] **RAL-44: Date each tick on a project's page.** The gap the watchman recorded when verifying RAL-43, whose
  line asks for ticks and their dates. Done when: each ticked line on a project's page shows the date of the
  commit that last changed it, from GitHub's GraphQL blame of the roadmap file, one call per project on the
  roadmap's cadence; the page says that date is the tick's unless the line was edited after it; and a test
  covers reading the blame and matching its ranges to ticked lines.
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
