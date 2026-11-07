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
- **MUS-43: Give muster messages a deterministic structure.** May shape Rally's notifications. RAL-8's digest format, approved by the operator on 2026-11-05, is in [digest.md](digest.md).
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
- [x] **RAL-40: Interface with Levi.** Merged in #112 (332506a) and #122 (17f111c). Requested by the operator on 2026-11-02. See the design. Claims live in
  Levi. Done when: the design says what happens to Rally's own claim commands and to projects not on Levi, and
  Rally can read a repository's Levi tasks through `levi --json`
  or its hub, labelled by how each fact is known, with Levi's text treated as untrusted and nothing
  written to Levi, with a test against a real `levi` run.
- [x] **RAL-65: `rally levi` holds up against a misbehaving `levi` or checkout.** From the watchman's and
  fiery-snapper's reads of RAL-40 at `4038e48`. Done when: `levi`'s output past the cap is drained rather than left
  in a full pipe, so too much output is reported as that and not as a timeout; output held open after `levi` exits
  (by something it started) doesn't outlast the deadline; Rally's own git call turns off what a checkout's config
  could run, and docs/levi.md says to run it only on checkouts you trust; with a test for each of the first two.
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
- [x] **RAL-41: Tighten text and name checks.** Merged in #104 (3c13f06). From the reads of RAL-16. Done when: zero-width and direction
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
- [x] **RAL-2: Show whether a team is available, working or blocked.** Merged in #12 (12133ea). Continues MUS-28. Done when: a
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
- [x] **RAL-15: Back up and restore the ledger.** Done when: a backup restored on the other machine
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

- [x] **RAL-5: Check submitted evidence.** Merged in #111 (539cbd4). Done when: on submission, Rally records as observed whether
  the commit exists, names the story and passed CI, and a test covers each failure. What a team didn't
  test is its own field, not free text.
- [x] **RAL-31: Tie verification to a commit.** Heads can move while work waits: muster#34 changed three
  times in review on 2026-11-02. Done when: each Verified and each requested second read names a commit,
  and Rally records as observed when the merged commit differs from the verified one, with a test.
- [x] **RAL-32: Show a requested second read.** catapp#78 merged before a requested read arrived. Done
  when: a requested read is visible to whoever merges until it's answered or withdrawn, and names the commit
  to read (RAL-31). Whether it
  blocks a merge is the operator's call, and this story records it either way.
- [x] **RAL-33: Record merges as commits.** Done when: Merged records the commit, is independent of
  Verified, and doesn't claim who merged, since every session shares one GitHub account.
- [x] **RAL-18: Check evidence with least privilege.** Mostly in #111 (539cbd4): evidence checks only send GETs to a pinned `api.github.com`, follow no redirects (tested), and never check out or run submitted code. RAL-18's second round adds tests that a check only GETs the four read endpoints and that the evidence module starts no program, and `rally check-github-token`, which tries a merge that can't succeed. Done 2026-11-05: with the operator's read-only token at /etc/rally/github-token, `rally check-github-token --pull 124` (2614a14) was refused with 403 "Resource not accessible by personal access token" (ledger annotation 184), and rally.service now checks evidence with it. Done when: evidence checks use read-only GitHub
  access, a merge attempt with that access is refused, and a test shows that Rally reads evidence without
  checking out or running submitted code.
- [x] **RAL-6: Verify separately from the author.** Done when: a story can only be marked Verified or
  Sent back under a declared name other than its submitter's, with the verifier and note recorded, and
  the docs say that this only catches mistakes, because both may hold the same machine's credential.
- [x] **RAL-19: Let a team read and correct its own record.** Done when: a team can read everything
  recorded about it without asking the watchman, and can append a correction or a disagreement that's
  shown beside the original. Verification outcomes stay with the verifier.

## Phase 3: The watchman's view

- [x] **RAL-22: Brief a new watchman from the ledger.** Merged in #123 (fef242d) and #127 (bee9f46). The watchman that started after the crash
  reported on 2026-11-02 had a board last saved on 2026-10-31 and no message history, so it had to ask
  every team to resend. Done when: a new session can read one brief giving each team's state, open
  claims (flagging those whose session has ended), submissions awaiting verification, open blockers,
  decisions waiting on the operator and who each holds up, holds in force and what releases them,
  conditions on the order of work, standing grants with their scope and whether each was direct or
  relayed, and how old each fact is; and a test shows no open submission is lost when the watchman
  changes. The brief records grants but confers no authority, and proof of who holds a role comes from
  Muster, not the ledger. Each field shows whether it's observed, declared or verified. Machine access
  appears only as that it exists and where its details live. List from the watchman, 2026-11-02. Live
  daemon commit and backups are for RAL-1 to place. Split by the a1 watchman on 2026-11-05: `rally brief`
  (RAL-22a) gives everything the ledger already records, with the test that no submission is lost when the
  watchman changes; holds, conditions on the order of work and grants wait on RAL-64, and until then the brief
  says it doesn't have them.
- [x] **RAL-64: Record holds, conditions on the order of work, and standing grants.** Merged in #127 (bee9f46). The rest of RAL-22, split
  off on 2026-11-05. Done when: a team or the operator can record a hold and what releases it, a condition on the
  order of work, and a standing grant with its scope and whether it came directly or was relayed, each declared
  and never more; `rally brief` lists those in force, with their kind and age; a grant's record confers no
  authority, and the brief and threat model rows 13 and 14 say so; and a test shows a relayed grant labelled
  relayed and a released hold no longer listed.
- [x] **RAL-20: Record who is waiting on whom.** Done when: a team can name what it's blocked on,
  including another team's story, the watchman can list open blockers and chains of them, and a blocker
  stays open until someone closes it, with who and why recorded. A blocked story and a blocked team are
  recorded separately.
- [x] **RAL-30: Reconcile an outage.** Done when: entries made on the fallback board during an outage can
  be backfilled with their real time and who entered them, the gap itself stays visible, and a test
  shows that backfilled records can't pass for live ones.
- [ ] **RAL-7: Detect a stall.** Depends on MUS-4 and MUS-44. Done when: each row of the stall table in
  the design has a test, a team resting after a story isn't flagged, and the stall of 2026-10-30 would
  have been flagged.
- [ ] **RAL-8: Tell the watchman.** Done when: the watchman wakes with one batched digest when something
  changes, and hears nothing when nothing has. Agree the format with the Muster team under MUS-43.
- [ ] **RAL-67: Check a story merged by several pull requests, per pull request.** From fiery-snapper's read of
  RAL-8's follow-up (#137) on 2026-11-05. RAL-22 landed in #123 and #127, and its one verify names #127's squash
  commit, not the heads read (7fc4bef and 62d67e2). A second verify on a Verified story is refused, so the digest
  flags it falsely. Done when: a verifier can add commits read to their own verify of an already Verified story with
  an annotation (`rally annotate <verify seq> --also-read <sha>`), accepted only from the verify's own credential;
  each merge's landed head is checked against every commit the verify names or has had added, and a verify at a
  pull request's squash commit counts as covering that pull request's head; RAL-22 is fixed by annotating 7fc4bef
  onto its verify, record 179, and a push after the read still flags, each with a test.
- [ ] **RAL-72: Make `rally show` easy to scan, and say what a refused ID or submit needs.** From the DiscoSim
  rally's issues log (docs/muster/2026-11-06-discosim-rally-issues.md §2.3, §2.4, §8): a grep for `"state"` in
  `rally show` found a nested "claimed" for stories that were verified, `rally ready DSM-45b` was refused without
  saying why, and plucky-aspen's resubmit after a send-back got a bare 409. Done when: `rally show --brief` gives
  one top-level state per story and no history; `rally show --field state` (or team, submitted_by,
  verified_commit, merged_commit) prints that value alone, and for the list one `ID<tab>value` line per story; a
  refused story ID says the form is capital letters, a hyphen and a number with no leading zero or suffix, and
  points at `rally new-story`; a submit of a sent-back, released or never-claimed story says to re-claim first,
  with the command; each with a test.
- [ ] **RAL-70: Gate a branch move on the stories it carries.** From the DiscoSim rally's issues log, §2.5: on
  2026-11-06 `main` was moved to f979331 while DSM-42 had been sent back by a peer. Assigned by frank-crocodile.
  Done when: `rally gate <range>` lists every story the range's commits name (subject, a merge's pull request title,
  or a trailer) with its ledger state, and exits non-zero if any is sent back, unverified or claimed, or isn't in
  the ledger under a prefix the ledger uses; `--allow <STORY>` lets one the range only mentions pass, listed; it
  reads only commit messages and runs nothing from the checkout; documented in `docs/gate.md`; with a test of a
  range refused for a sent-back story and passed once it's verified.
- [ ] **RAL-75: Flag a heavy-run lock held by a stopped process.** From the DiscoSim rally's issues log, §4.4: on
  2026-11-06 a runner shell sat stopped (state T) for about 57 minutes while it held the one-heavy-run lock.
  Assigned by frank-crocodile. Done when: `rally stalls --lock <path>` reads the lock's holder and waiters from
  `/proc/locks`, and gives a `stopped_lock_holder` prompt if the holder, or anything under it, is stopped, or the
  holder is a zombie, naming each process's pid, state, working directory and the session guessed from it; a held
  lock with a running holder gives none; documented in `docs/stalls.md`; with a test that stops a real
  `flock`-held shell and finds it.
- [ ] **RAL-79: Multi-part stories.** From the DiscoSim rally's issues log, §2.1: DSM-37 was verified at part 1
  and Rally had no way to record parts 2–4, so rule 11 (submit once, at the last part) hid long stories for hours.
  Assigned by frank-crocodile. Done when: `rally submit <ID> --part k [--of n]` records each part as it lands
  while the story stays claimed; a watchman or peer verifies or sends back each part, and a part is amended alone
  under RAL-71's rule; the final close is today's submit and verify, refused while a part is open, and marks the
  story Verified; `rally gate` passes a commit a verified part's read covers, itself or an ancestor, without
  `--allow`, and never a merge after the read; `rally show --brief` gives parts verified / total; `rally waiting`, the digest and the viewer
  board show a part in review; a story with no parts behaves exactly as today; the rollback hazard is in
  `docs/service.md`; documented in `docs/parts.md`; with ledger and CLI tests.
- [x] **RAL-21: Keep each team's track record as plain facts.** Done when: the watchman can read each
  team's stories verified, stories sent back and what was missing, and problems reported, with no points,
  scores, weights or ranking.
- [x] **RAL-23: Show progress across roadmaps.** For the watchman's planning. Done when: for each
  roadmap, the watchman can see how many stories are in each state, which ready stories have nothing
  blocking them, which blockers hold up the most stories, and which roadmap ticks have no Verified
  behind them, with no comparison or ranking of teams.
- [ ] **RAL-24: Bring in other projects.** Open question 4 in the design. Done when: the decision is
  recorded, and Muster, catapp, ParadoxDetector and any other team on the board can be tracked by it.
- [ ] **RAL-9: Replace the hand-kept table.** Depends on RAL-24. The current table is
  `~/Documents/Caster/muster-board.md` on the operator's Mac. Done when (changed by the operator on
  2026-11-06, relayed by the watchman: archive the old table, don't import it): the table is archived
  unchanged in Muster's `docs/board-archive-2026-11-06.md`, with Version 0's table section from
  `docs/board.md`; the watchman no longer needs it; and Muster's `docs/board.md` says Rally's ledger is the
  board, read in the viewer or with `rally show` and `rally brief`. Muster's part is Castor-o7/muster#194.

## Phase 4: The web app

Read-only. Changes go through the ledger's commands, so a flaw in the web app can't rewrite the ledger.

- [x] **RAL-25: Give the ledger a read-only API.** Done when: it's documented, it uses RAL-13's access
  control, and a test shows no request through it can change the ledger.
- [ ] **RAL-26: Agree the look with the operator.** Done when: the operator has approved a visual design
  covering the current and past views, on a phone as well as a desktop.
- [x] **RAL-27: Show the current ledger.** Partly done: the viewer (RAL-43, cd346a1) shows each team's declared state, stories, blockers and their freshness, labelled. RAL-27's second round adds each story's latest evidence (declared) with Rally's check of it (observed), and each team's track record, with their ages. Done 2026-11-05: the operator saw it running on the 127.0.0.1:7300 viewer at 2614a14, with MUS-17's observed "commit exists" in the Evidence table (ledger annotation 190). Done when: the web app shows each team's state, stories,
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
- [x] **RAL-44: Date each tick on a project's page.** The gap the watchman recorded when verifying RAL-43, whose
  line asks for ticks and their dates. Done when: each ticked line on a project's page shows the date of the
  commit that last changed it, from GitHub's GraphQL blame of the roadmap file, one call per project on the
  roadmap's cadence; the page says that date is the tick's unless the line was edited after it; and a test
  covers reading the blame and matching its ranges to ticked lines.
- [x] **RAL-45: Read day summaries from local folders.** The operator's rule keeps `docs/muster/` out of git, so
  the viewer's GitHub reads always found no day summary; the watchman noted it on 2026-11-03 and filed it on
  2026-11-04. Done when: `rally web` takes `--summaries <PREFIX>=<folder>`, repeatable, and `--watchman-summaries
  <folder>`, which read the newest `YYYY-MM-DD.md` from a local folder, read-only, with the same size cap as today;
  with them set, the overview shows the watchman's writeup and each project page its team's newest local summary,
  each saying it was read from a local file and naming it; without them, nothing changes; and a test covers the
  local read and a missing folder, with nothing in `docs/muster/` committed.
- [ ] **RAL-28: Show past ledgers.** Depends on RAL-12. Done when: a viewer can pick a past moment and
  see the ledger as it stood then, with corrections made since shown as later, and a test shows that the
  view matches a replay to that moment.
- [x] **RAL-29: Secure the web app.** Merged in #114 (1490e83). Done when: a test shows that a cross-origin page can't read it,
  that markup in recorded text doesn't run, and that an unauthorized viewer is refused; it isn't exposed
  to the open internet; and someone other than its author has reviewed it adversarially, before it's
  reachable beyond the machine it runs on.
- [ ] **RAL-69: Show only the projects in the current rally.** Requested by the operator on 2026-11-06: a
  project that hasn't been worked on for several rallies shouldn't be in the viewer "unless an active muster team
  has claimed that project", but its progress is kept. Done when: the overview's roadmaps, ledger figures,
  evidence, declared teams, track records and muster totals cover only the projects in which a team on Muster's
  roster holds a story (claimed, in review or sent back), and say so; the other projects keep their records and
  their pages and are listed, collapsed, as not in this rally; when the roster or the ledger can't be read,
  everything is shown with a warning rather than hidden on a guess; and a test covers the rule. Adding a
  project the ledger knows but `projects.json` doesn't (DiscoSim) is RAL-74's.
- [ ] **RAL-76: Open and close rallies, with a clean board for each.** Requested by the operator on 2026-11-06:
  "When the rally closes from the previous session, the board should be cleared ... we log the old rally, store it,
  and CLEAN the board so we can get a fresh start." Done when: `rally open-rally` records a rally's name, projects
  and teams, and `rally close-rally` ends it, declared, one open at a time; the viewer's overview shows only the open
  rally's projects, teams and what was recorded since it opened; past rallies are listed, each viewable as the ledger
  stood when it closed; nothing is removed from the ledger; and tests cover the records, the service route and the
  board.
- [ ] **RAL-77: Rallies, after RAL-76: who may open one, past rallies as their own board, dates, and errors.** From
  the watchman's read of #149 (RAL-76), filed by plush-tortoise. Done when: with the service's `--rally-openers`
  set, only the credentials it lists, or the credential bound to a signed watchman designation, can open or close
  a rally, and others get a 403 naming the rule; unset, any paired credential may, and each answer and
  `rally rallies` say to set it; a past rally opens as its own board (its projects, the stories recorded for them
  while it was open or held then, and its teams) as the ledger stood at its close, not the whole ledger at that
  record; the overview compares the watchman's writeup's day file with the rally's opening as local dates, not a
  local one with a UTC one; a `/v1/rallies` read that fails for any reason but an older service (a refused token,
  a network error) shows as a warning, not as "an older service"; `docs/service.md` says what `--rally-openers`
  does and can't do; and tests cover each.
- [ ] **RAL-34: View from a phone.** Not ready to claim until the operator wants it; they may. Depends
  on RAL-29. Done when: an unpaired device the operator names can read the ledger with a credential of
  its own that can be revoked alone, after its own threat review.
- [ ] **RAL-73: A one-time login code survives the browser's preload.** From the DiscoSim rally issues log of
  2026-11-06, §3.1: Josh's pasted login links kept reporting the code as bad, most likely because Chrome
  preloaded the address and spent the single-use code before the visit. Done when: opening a code's address
  shows a "Sign in" button and spends nothing, and only the button's same-origin POST spends the code; a test
  shows a prefetched GET (`Sec-Purpose: prefetch`, `Purpose: prefetch`) and a plain one leave the code working,
  once; the page's own policy allows its form and nothing else, while every other page still allows no form; it
  shows nothing offered in the address that isn't a code; another site's form POST is refused; and
  `docs/web.md` says so.
- [ ] **RAL-83: The Sign in button works in Chrome.** RAL-73's sign-in page set `<meta name="referrer"
  content="no-referrer">`, under which Chrome sends `Origin: null` with the form's POST; the guard refuses
  `Origin: null`, so the button always answered "other sites can't read this page". Found live by the watchman
  on 2026-11-06 and confirmed in headless Chrome. Done when: the sign-in page's referrer policy is `same-origin`
  in its header and its `<meta>`, every other page stays `no-referrer`, `Origin: null` is still refused, a test
  asserts both, and `docs/web.md` says why.
- [ ] **RAL-84: CI signs in to the viewer through a real headless browser.** From the watchman's finding of
  RAL-83 on 2026-11-06: every sign-in test was curl-driven and sent a real `Origin`, so none caught Chrome's own
  `Origin: null` under a no-referrer page, and the button failed for the operator every time. Done when: a viewer
  test opens a one-time code's address in headless Chrome, presses Sign in, and sees the board and `/api/status`
  served; it fails on a viewer with RAL-83's bug; it runs when `RALLY_TEST_CHROME` names a browser and is skipped
  otherwise; CI installs what it needs and can't skip it; and the README says how to run it.
- [ ] **RAL-74: The viewer reads `~/.config/rally/projects.json` by default.** From the DiscoSim rally issues
  log of 2026-11-06, §3.3: the built-in `web/projects.json` lists RAL, MUS, CAT, CHOMP and PDX only, so a whole
  rally ran with no DiscoSim page until the viewer was restarted with `--projects`. Done when: without
  `--projects`, `rally web` reads `projects.json` beside the pairing (`~/.config/rally/projects.json`, or
  beside `--config`) when it exists, and the built-in list when it doesn't; `--projects` still wins; the first
  line says which list was read; a file that's there but broken is an error, never a quiet fall back; tests
  cover each case and don't read the operator's own config; and `docs/web.md` says so.

## Phase 5: Rally into Levi

From the rally of 2026-11-04 evening. The operator told the watchman, grand-photon, that ignition-is-go's lead
developer welcomes Rally merging into Levi. Nothing goes to ignition-is-go/levi or any other upstream until the
operator says so; the work is done on a local checkout of Levi at `~/Documents/Gamoire/levi`. RAL-40 is still the
story for Rally reading Levi; these stories are about putting Rally's features into Levi.

- [x] **RAL-46: Compare Rally and Levi, and propose what moves (G6).** Done when: a comparison of the two,
  each claim about Levi checked against its source at a named commit or a real run; a proposal of which Rally
  features belong in Levi and how each fits Levi's model (events in `refs/levi/events`, `levi-hub`, `levi-dash`),
  and which stay out and why; and the watchman has read the proposal before any building starts.
- [x] **RAL-47: Build the agreed features in Levi (G6).** Depends on RAL-46. Done when: each feature the
  watchman agreed is built on a branch of the local Levi checkout, with tests that pass beside Levi's own, and
  clippy and fmt clean; and nothing is pushed or opened as a PR upstream.
- [x] **RAL-48: A Rally-vs-Levi write-up for the PR description (G7).** Depends on RAL-47. Done when: a write-up
  ready to paste into the eventual upstream PR says what Rally is, what moved into Levi and why, what was left
  out, and how it was tested, with each claim matching the branch.
- [ ] **RAL-61: Levi L2, a tamper check on `refs/levi/events` (`levi check-events`).** From the "Not built" rows
  of `docs/rally-vs-levi.md`, after `rally check --expect`. Done when: on a branch of the team's Levi remote, on
  Levi main, `levi check-events` prints the events ref's head, and `--expect SHA` fails unless that commit is
  still in the ref's history with every event it held, so a reset, a rewrite and a removal each fail with their
  own reason; tests cover each, plus two synced clones; clippy, fmt and Levi's own tests pass; no commit, comment
  or test names Rally, Muster, an AI or a machine; `docs/levi-pr-l2.md` is the PR text; and a second reader has
  read it at the head. Nothing goes upstream until the operator says so.
- [ ] **RAL-62: Levi L3, what a session should know at start (`levi session-start`).** After `rally
  session-start` (RAL-50). Done when: on a branch of the team's Levi remote, on Levi main, `levi session-start`
  prints the caller's claimed tasks and the top of `next` without claiming; it writes nothing to disk, never
  syncs or contacts a hub, gives up after a second, and on any failure prints nothing and exits 0; the block is
  bounded and task text is escaped; `levi init` and `levi next` are unchanged; tests show each, including a
  hub at a closed port answered in under a second; the same naming rule as RAL-61 holds; `docs/levi-pr-l3.md` is
  the PR text; and a second reader has read it at the head. Nothing goes upstream until the operator says so.

## Phase 6: Rally without a watchman in the loop

The operator, directly on 2026-11-04: yes to making Rally useful without routing everything through the watchman.
Rally works without one today, but nothing tells an agent it exists, so it's only used when the watchman says so.
RAL-5 (observed evidence) belongs to this too.

The operator added, as relayed by the watchman: "preserve our current working paradigm, but it is not a
requirement to use the rally and muster software." So Rally records the work where a team uses it, and nothing in
it reads as if a team has to.

The operator then ruled, as relayed by the watchman (grand-photon) the same night:

- **While the operator has designated a watchman,** the rally works as it does now: the watchman reads every PR
  at the head being merged. Peer verification counts only while no watchman is designated.
- **Whether a watchman is designated is the operator's declaration,** recorded in the ledger. No agent can set it
  for itself.
- **A newly designated watchman re-reads the work that was peer-verified while there was none,** and decides with
  critical judgement whether to keep it, send it back or ask for fixes. Nothing is accepted on the strength of a
  peer's verification alone. Its decisions are recorded as its own verifications or corrections, and the peer's
  record stays.

- [x] **RAL-49: Teach a repository's agents about Rally (`rally init`).** As `levi init` does for Levi. Done when:
  `rally init` writes a marked block into the repository's `CLAUDE.md` or `AGENTS.md` (creating `CLAUDE.md` if
  neither exists), saying how to claim, submit, verify someone else's work at a commit, request a read and record a
  blocker, and that agent-written text in the ledger is untrusted; running it again replaces the block and changes
  nothing else; it writes no credential or address; and a test covers creating, updating and leaving other text alone.
- [x] **RAL-50: Show an agent its Rally state when its session starts.** Depends on RAL-49. Done when: a Claude Code
  SessionStart hook that Rally ships (and documents how to install, without installing it itself) prints the team's
  open claims, read requests waiting on it, submissions it could verify, and open blockers, each labelled observed,
  declared or verified; it prints nothing and exits 0 when the service can't be reached or the machine isn't
  paired, so a session never fails to start because of Rally; its output is capped; and a test covers each case.
- [x] **RAL-51: Peer verification while no watchman is designated.** Done when: the ledger records the operator's
  designation and end of a watchman, accepted only from the operator's own credential and refused from any other,
  with a test of the refusal; each verification is labelled `watchman` or `peer` by whether a watchman was
  designated when it was recorded; Rally lists submissions waiting for a reader, oldest first, with the commit to
  read; `track-record` counts verifications each verifier gave as well as each team received, the gave counts by
  declared name, since a record doesn't name the verifier's team; and the board process in Muster's
  `docs/board.md` says the same, through a draft PR the watchman reads as the board's keeper.
- [ ] **RAL-52: Hand a new watchman the peer-verified work.** Depends on RAL-51. Built in #99 (`rally
  handover`) and approved by the watchman, apart from the digest, so it stays open on RAL-8, which waits on
  MUS-43's design meeting with the operator. Done when: when a watchman is
  designated, Rally gives it one list of every story verified by a peer since the last designation, each with the
  commit read, who verified it, and whether and at what commit it merged; each stays marked "awaiting the
  watchman's re-read" until the watchman records its own verification, send-back or correction, none of which
  overwrites the peer's record; the RAL-8 digest carries the list until it's empty; and a test covers a
  designation partway through, with stories merged and unmerged.
- [x] **RAL-53: Bind the watchman label to a per-session credential.** Merged in #116 (280c4a6). From the watchman's read of RAL-51b. Today
  a verification is labelled `watchman` when its declared name matches the designated watchman's, and on a1 every
  session shares one credential, so any session can declare that name. Done when: a designation can name a
  credential that only the watchman's session holds, issued without passing through other sessions; a
  verification is labelled `watchman` only when made with it; a test shows another session declaring the
  watchman's name is labelled `other`; and the threat model says what the credential protects and what it doesn't.
- [ ] **RAL-63: Designate more than one watchman at once.** Requested by the operator during the two-watchman
  rally of 2026-11-05 (one watchman on Murph, one on a1), relayed by the Murph watchman. Today only the newest
  designation is in force, so a second watchman's reads are labelled `other`. Done when: an operator-signed
  designation can name a set of watchmen in force at once, each bound to its own credential (RAL-53) and given a
  scope (for example a host or a team); `rally designation` lists every watchman in force with its scope;
  `verifications` labels a read `watchman` when any designated watchman made it with its credential, and says which
  watchman made it; `--end` can end one watchman's designation or all of them; `handover` and `waiting` work with
  several watchmen, using the scopes so it's clear whose read a merge waits for; `docs/designation.md` covers the
  threat model for N credentials; and tests show two watchmen in force with each read labelled with its own
  watchman, a third session declaring one watchman's name labelled `other`, adding or ending a watchman refused from
  any credential but the operator's, and ending one watchman leaving the other in force.

### A rally that keeps itself moving

The operator asked on 2026-11-05 whether a rally can run without a watchman. The watchman's answer, which the
operator agreed to go ahead with: teams can pace themselves, but that night they stalled on mechanical gaps.
Two teams sat idle with ready work, and one slept for about 2 h with no way to wake it. Four Muster story IDs
were taken twice (MUS-69, and MUS-72 to 74), and a fifth nearly was (MUS-81). Rally closes those gaps, so that a
watchman adds judgement and isn't needed to keep work moving. Each story below comes from that question. Which
ones Rally builds first, and how they fit the board process, follows the watchman's order and the operator's
word.

- [x] **RAL-54: Wake an idle team with its next story.** Merged in #109 (9d44b26). From the operator's question of 2026-11-05, and from RAL-7
  and RAL-8. Done when: if a team is idle, or its claim has gone quiet past an agreed time, while a ready story
  exists for it, Rally sends that team one digest through Muster naming its next ready story (RAL-56), at most
  once per agreed interval; a team that has said it's resting, or has no ready story, gets nothing; a team that
  can't be woken is shown as such to whoever reads the state; and a test covers each of those cases, plus the
  2026-11-05 case of a team asleep with wake `none`. One moment to wake a team is when the operator's answer
  closes a question its story was shelved on (RAL-56), since that story then comes back first.
- [x] **RAL-55: Hand out story IDs from the ledger.** Merged in #110 (da5b72d). From the operator's question of 2026-11-05. Done when:
  `rally new-story <PREFIX> --title "..."` records the next free number for that prefix, and returns it in one
  atomic write; two concurrent requests for one prefix get different numbers, with a test; a number that's already
  in the ledger, or already in the prefix's roadmap, is never handed out; and IDs written by hand still work.
- [ ] **RAL-68: Don't hand out an ID an open pull request already uses.** Proposed by frank-crocodile, filed
  2026-11-05 at fiery-snapper's request, after MUS-95 was written by hand in an open pull request and then handed
  out again. Done when: `rally new-story` also reads the prefix's roadmap in each open pull request, read-only
  through the service's GitHub token, and never hands out a number used there; when GitHub can't be read it says
  so rather than guessing; with a test for an ID in an open pull request and one for an unreadable GitHub.
- [ ] **RAL-85: Record a read of the merged commit.** From mellow-weevil's re-read of RAL-81 on 2026-11-06: the
  merged head a4e6f90 took a merge of main after the verify at 6949b93, and `annotate --also-read` takes only the
  original verifier's credential and name, so the successor watchman couldn't record its read and `commit_differs`
  stayed. Done when: `rally read-merged <STORY> --commit <latest merged commit> --note ...` records a verified-kind
  read of its own, beside the verification, by any verifier who isn't an author, the original verifier included;
  it's refused unless the story is Verified and the commit is its latest recorded merge and not already covered;
  `commit_differs` counts it and RAL-67's also-reads; `verified_commits` lists it; it's dropped with the
  verification; `rally verifications` labels it by the designation, gated by nothing; with tests and
  docs/evidence.md.
- [ ] **RAL-87: Say when a watchman's digest has stopped.** From the DiscoSim rally's issues log (§2.7) and
  mellow-weevil's digest dying on a 429 on 2026-11-06 with nothing noticing. Done when: each `rally digest` pass
  records when it ran and when a digest last went; `rally brief` and `rally stalls` with `--digest-quiet-after
  <window>` flag each designated watchman, or one named with `--digest-watchman`, whose digest has no pass recorded,
  no pass within the window, or records waiting longer than the window with none sent, and not one whose ledger is
  merely quiet; the window has no default, since it's the operator's to agree; with tests.
- [ ] **RAL-80: Viewer sign-ins survive a restart.** From the DiscoSim rally's issues log (§3.4, 2026-11-06):
  sessions were held only in memory, so adding `--summaries`, or any restart or deploy, signed the operator out.
  Chosen over a SIGHUP reload of the projects and summaries, which would spare only config changes, not a deploy or
  a crash. Done when: a session made by signing in holds across a restart of the viewer; the sessions are kept in
  a file beside the key, readable only by the account, holding only hashes of the sessions and the key; a session
  lasts 14 days from its sign-in, checked on every request and at start, and only the newest 32 are kept; `--new-key`
  still signs every browser out, across a restart too; an unreadable sessions file signs no one in and doesn't stop
  the viewer; with a test.
- [ ] **RAL-88: The viewer's cross-site refusal says how to sign in instead.** From the DiscoSim rally's issues
  log, §3.2: the Claude-in-Chrome extension's navigation is marked cross-site, so the viewer answered "other sites
  can't read this page", which is right but says nothing about what to do. Assigned by mellow-weevil. Done when:
  the refusal is unchanged (403, same rule) and its text adds: run `rally web --login` in a terminal and open the
  address from there or the address bar, not from another page or an extension; with a test of the text.
- [ ] **RAL-86: Long-running digest and wake log a failed pass, back off, and keep going.** From the watchman
  (mellow-weevil), 2026-11-06: `rally digest --every` exited with `refused (429): too many checks and past-state
  reads` right after a successful send. Reserved by silken-beetle, built by eager-cobra. Done when: with `--every`,
  a failed pass that can clear with time (429, 408, 5xx, the service or Muster unreachable) is logged as one JSON
  line and waited out, doubling up to 16 times `--every` and honouring `Retry-After`, and the loop goes on; it exits
  only on bad configuration, no pairing, or a refusal that won't clear; the digest keeps each watchman's head as
  its turn ends, so a pass failing part-way neither sends a digest twice nor skips one; `docs/digest.md` and
  `docs/wake.md` say so; with a test of a fake service answering 429 then 200.
- [ ] **RAL-81: Progress from the ledger when a roadmap has no checkboxes.** From the DiscoSim rally's issues log
  (§3.5, 2026-11-06): DiscoSim's roadmap is a table, so the overview read 0 ticks all rally. Done when: a roadmap
  that's read and has no checkboxes shows its progress from the ledger's states, Verified of the stories the ledger
  tracks under its prefix, labelled as the ledger's, on its card and its project page; a roadmap with boxes, one not
  yet read, or a prefix the ledger doesn't track, doesn't; it's kept out of the boxes across projects, which say
  which projects were left out; with a test.
- [ ] **RAL-71: Evidence can be corrected.** From the DiscoSim rally's issues log (§2.2, 2026-11-06): DSM-41's
  evidence named `Castor-o7/DiscoteSim`, a typo, and DSM-54's wasn't `owner/repo@<full commit>`, and the only fix
  for either was a send-back, since a resubmit in review is refused. Done when: `rally submit` refuses evidence
  that isn't `owner/repo@<full commit>`, or whose repository isn't the one the projects list gives the story's
  prefix, saying what to write, unless `--not-checkable` is given; `rally amend <STORY> --team <team> --evidence
  ...` lets the submitter, and only the submitter, replace the evidence of a story in review without a send-back,
  checked the same way, recorded in the story's history as an amendment beside the original, and checked by
  Rally as the latest submission; once amended, a verify or send-back naming another commit than the amended
  evidence's is refused, saying which record amended it; an amendment that changes nothing is refused; a resubmit
  in review says to amend; with a test for each.
- [x] **RAL-56: A rule for picking the next story that a team can follow alone.** Merged in #106 (6aa5622). From the operator's question of
  2026-11-05. The operator added: "if the operator doesn't respond within a certain reasonable window of time then
  shelve the item. don't just inherently skip operator interaction items please". Done when: `rally next --team
  <team>` shows the team's next story, which is the first ready, unclaimed, unblocked story in roadmap order; it
  skips one that waits on another team, and says why, and the team can record that skip as a block, with the
  reason, in one command; a story that waits on the operator isn't skipped: the team asks the operator, through
  Muster to the operator's address, and records the question as a block on the operator; if there's no answer
  within a window agreed with the operator, the story is shelved, a recorded state that keeps the question and
  when it was asked, and the team takes the next story; a shelved story returns to the top of the pick once the
  operator answers; `rally next` shows shelved stories and their open questions; and a test covers the order,
  each kind of skip, a roadmap with nothing left, and a story that isn't shelved before the window, is shelved
  after it, and returns on an answer. The operator then set the times: "maybe like 5 minutes, and if the
  operator skips interaction 3 times then the operator interaction items are all shelved for an hour and then
  revisted after the hour has elapsed", and the count resets after the hour. So also: the window is 5 minutes;
  after 3 shelves in a row across the rally with no answer between them, Rally records the operator as away, as
  observed, with when it started and ends and the 3 shelves; for an hour nothing waiting on the operator is
  offered; after the hour it's all asked again; an answer, by the service's time, ends it early, and then it's
  all asked again too; the count starts again at any answer and after the hour; each time can be changed; and a
  test shows away isn't entered on the second unanswered shelf and is on the third, nothing is offered during the
  hour, it's revisited after it, an answer ends it early and a backdated one doesn't, and after the hour two
  shelves don't enter it again and a third does.
- [x] **RAL-57: Spread peer reads across teams.** Merged in #107 (f2cac4c). From the operator's question of 2026-11-05. Done when: while no
  watchman is designated, `rally waiting` suggests a reader for each submission: a team other than the author's,
  the one whose last suggested or recorded read is oldest first; it never suggests the author's own team;
  following the suggestion is optional, and the read still counts only by the rules of RAL-51; and a test shows
  the suggestions rotating across three teams.
- [ ] **RAL-58: Trial a self-paced rally.** From the operator's question of 2026-11-05. Depends on RAL-54 to 57.
  The operator decides when. Done when: for one rally session, teams pick their own stories by RAL-56's rule and
  the watchman only verifies; afterwards, a write-up compares that session's idle time, ID collisions and
  send-backs with 2026-11-05's, from the ledger and Muster's roster, and says what still needed the watchman.
- [x] **RAL-59: Tell Prime Agent sessions their own messaging will do.** Merged in #115 (32e82b4). The operator, relayed by the watchman on
  2026-11-05: "in the case that prime agent is using the rally service please add a notification to warn prime
  agents that their own built in communication is a fine substitute for muster when using the rally service."
  Prime Agent ([PrimeIntellect-ai/prime-agent](https://github.com/PrimeIntellect-ai/prime-agent), its README and
  [release notes](https://github.com/PrimeIntellect-ai/prime-agent/releases)) lets running sessions discover,
  message and steer each other through its own background daemon, on a local socket, so one machine and Prime
  Agent sessions only. Done when: a Prime Agent session using Rally gets a notice in its session-start briefing
  and in `rally init`'s block, saying its built-in messaging is a fine substitute for Muster, and that Muster is
  needed only to reach sessions of other harnesses or on other machines; how a Prime Agent session is detected is
  decided and documented, without guessing from names; a test shows the notice for Prime Agent and not for other
  harnesses; and `docs/design.md` says Rally works without Muster, with Prime Agent's messaging as the example.
- [x] **RAL-60: Rally's CI checks dependencies with cargo-deny.** Merged in #113 (63579f8). From the watchman's read of RAL-5, which adds
  `ureq`. Done when: a `deny.toml` lists the licences Rally accepts (including `webpki-roots`'s
  CDLA-Permissive-2.0) and the sources it allows; CI runs `cargo deny check` on every PR; and today's advisory,
  RUSTSEC-2026-0387 in `crypto-bigint` through `ssh-key`, is fixed or recorded with a reason.
