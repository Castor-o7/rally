# Rally roadmap

Goal: if Muster is the musculature of the network, Rally is the bones. Rally is the ledger of the
current and past state of the muster's teams. The watchman uses it to verify work, see what each team
has achieved and what it's facing, unblock teams, and plan, while respecting each team. Security of the
information and the environment runs through all of it. A visually appealing web app shows the current
ledger and past ones.

IDs are permanent. Don't reuse or renumber them, so they aren't in numeric order below. A story without
a "done when" line isn't ready to claim.

New entries are appended at the end. This file merges with git's `union` driver (`.gitattributes`, RAL-169):
when two branches each append, a local merge keeps both instead of conflicting. An edit to an existing
entry merged against another edit to the same entry keeps both versions, silently, so after merging main into
a branch that edited an entry, look at it before pushing. GitHub's web merge ignores merge drivers: a pull
request whose roadmap conflicts with main still needs main merged in locally.

Rally tracks itself. Its ledger is the source of truth for every project in `web/projects.json`, Rally's own
included (the operator, 2026-11-06; RAL-24, RAL-9). The hand-kept board is archived in Muster's
`docs/board-archive-2026-11-06.md`, and Muster is frozen while the muster moves to Marshal. So stories follow
the way of working in this repository's CLAUDE.md: claim, submit at a commit, a reader other than the author
verifies at that commit (the watchman, while one is designated), then merge. The roadmap itself is cleared the
same way, as story RAL-0 ([clearance.md](clearance.md)), and a tick means the story is verified and merged.

The operator agreed the design and threat model on 2026-11-02. Later phases' "done when" lines can
still be refined as each story is taken.

## Depends on Muster and Marshal

Muster is frozen (the Muster to Marshal cutover, hold 1859), and Marshal is the line that's moving. Rally
reads and tells through either daemon (RAL-120, RAL-126). What Rally needed from Muster, and where it lives now:

- **MUS-3: Stable identity across restarts.** Done in Muster, and carried into Marshal as MAR-9's stable
  nickname. Rally ties a restarted session to its old nickname with `rally rally-session --succeeds` (RAL-110).
- **MUS-4: Show dead watchers.** Rally reads wake and activity from whichever roster gives them (RAL-7,
  RAL-120, RAL-124), and says when a roster couldn't be read. Rally doesn't wait on Muster for more.
- **MUS-44: Let other programs read the observed roster.** Done. Rally now reads the union of every roster
  source it's given, Muster's and Marshal's (RAL-126), so this no longer depends on Muster alone.
- **MUS-43: Give muster messages a deterministic structure.** Rally doesn't wait on it: RAL-8's digest
  format, approved by the operator on 2026-11-05, is in [digest.md](digest.md), and it's sent through either
  shim (RAL-120).
- **MUS-12: Give the watchman a read-only GitHub token.** Done for the service (RAL-18). It's still a token
  that can't see private repositories such as RootyRoopsDAW, which Rally says rather than guesses (RAL-128).
- **MUS-39: Show that a terminal message came from the operator.** Not in Rally's hands, and whether Marshal
  carries it isn't recorded here. Until something does, it limits what RAL-6 can prove: a relayed approval
  is only as good as its relay, so Rally records how each answer came (`--direct` or `--relayed-by`).

## History: what the first deployment needed

Done. Rally's first record on Murph is from 2026-11-04, the operator retired the hand-kept board on
2026-11-06 (RAL-24, RAL-9), and every project in `web/projects.json` has been tracked by the ledger since. Kept
as written then:

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
- [x] **RAL-114: Write down Rally's design ideals.** From the operator on 2026-11-08, relayed by the watchman,
  mellow-weevil. Done when: [ideals.md](ideals.md) quotes the operator's statement verbatim as its anchor,
  marks everything beyond it as decided (with where it was decided) or proposed, the README links it, it's
  merged, and the operator has signed off on the wording, or the proposed parts are open questions to the
  operator in Rally.

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
- [x] **RAL-7: Detect a stall.** Merged in #132 (f953afb). Depends on MUS-4 and MUS-44. Done when: each row of the stall table in
  the design has a test, a team resting after a story isn't flagged, and the stall of 2026-10-30 would
  have been flagged.
- [x] **RAL-8: Tell the watchman.** Merged in #136 and #137 (2d311b9). Done when: the watchman wakes with one batched digest when something
  changes, and hears nothing when nothing has. Agree the format with the Muster team under MUS-43.
- [x] **RAL-67: Check a story merged by several pull requests, per pull request.** Merged in #140 (2c45e8f). From fiery-snapper's read of
  RAL-8's follow-up (#137) on 2026-11-05. RAL-22 landed in #123 and #127, and its one verify names #127's squash
  commit, not the heads read (7fc4bef and 62d67e2). A second verify on a Verified story is refused, so the digest
  flags it falsely. Done when: a verifier can add commits read to their own verify of an already Verified story with
  an annotation (`rally annotate <verify seq> --also-read <sha>`), accepted only from the verify's own credential;
  each merge's landed head is checked against every commit the verify names or has had added, and a verify at a
  pull request's squash commit counts as covering that pull request's head; RAL-22 is fixed by annotating 7fc4bef
  onto its verify, record 179, and a push after the read still flags, each with a test.
- [x] **RAL-72: Make `rally show` easy to scan, and say what a refused ID or submit needs.** Merged in #143 (8bbd817). From the DiscoSim
  rally's issues log (docs/muster/2026-11-06-discosim-rally-issues.md §2.3, §2.4, §8): a grep for `"state"` in
  `rally show` found a nested "claimed" for stories that were verified, `rally ready DSM-45b` was refused without
  saying why, and plucky-aspen's resubmit after a send-back got a bare 409. Done when: `rally show --brief` gives
  one top-level state per story and no history; `rally show --field state` (or team, submitted_by,
  verified_commit, merged_commit) prints that value alone, and for the list one `ID<tab>value` line per story; a
  refused story ID says the form is capital letters, a hyphen and a number with no leading zero or suffix, and
  points at `rally new-story`; a submit of a sent-back, released or never-claimed story says to re-claim first,
  with the command; each with a test.
- [x] **RAL-70: Gate a branch move on the stories it carries.** Merged in #144 and #148 (11c0579, 6dc4ddd). From the DiscoSim rally's issues log, §2.5: on
  2026-11-06 `main` was moved to f979331 while DSM-42 had been sent back by a peer. Assigned by frank-crocodile.
  Done when: `rally gate <range>` lists every story the range's commits name (subject, a merge's pull request title,
  or a trailer) with its ledger state, and exits non-zero if any is sent back, unverified or claimed, or isn't in
  the ledger under a prefix the ledger uses; `--allow <STORY>` lets one the range only mentions pass, listed; it
  reads only commit messages and runs nothing from the checkout; documented in `docs/gate.md`; with a test of a
  range refused for a sent-back story and passed once it's verified.
- [x] **RAL-75: Flag a heavy-run lock held by a stopped process.** Merged in #147 (eaf2443). From the DiscoSim rally's issues log, §4.4: on
  2026-11-06 a runner shell sat stopped (state T) for about 57 minutes while it held the one-heavy-run lock.
  Assigned by frank-crocodile. Done when: `rally stalls --lock <path>` reads the lock's holder and waiters from
  `/proc/locks`, and gives a `stopped_lock_holder` prompt if the holder, or anything under it, is stopped, or the
  holder is a zombie, naming each process's pid, state, working directory and the session guessed from it; a held
  lock with a running holder gives none; documented in `docs/stalls.md`; with a test that stops a real
  `flock`-held shell and finds it.
- [x] **RAL-79: Multi-part stories.** Merged in #153 (27330ce). From the DiscoSim rally's issues log, §2.1: DSM-37 was verified at part 1
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
- [x] **RAL-24: Bring in other projects.** Merged in #165 (4e7e8c4). Open question 4 in the design. Done when: the decision is
  recorded, and Muster, catapp, ParadoxDetector and any other team on the board can be tracked by it.
  Decided by the operator on 2026-11-06: Rally is the source of truth for every project in `web/projects.json`,
  and the hand-kept board retires (design, open question 4). The ledger already tracks MUS, MAR, CAT and DSM
  stories.
- [x] **RAL-9: Replace the hand-kept table.** Done in Castor-o7/muster#194 (a120efc), verified 2026-11-09 (2200). Depends on RAL-24. The current table is
  `~/Documents/Caster/muster-board.md` on the operator's Mac. Done when (changed by the operator on
  2026-11-06, relayed by the watchman: archive the old table, don't import it): the table is archived
  unchanged in Muster's `docs/board-archive-2026-11-06.md`, with Version 0's table section from
  `docs/board.md`; the watchman no longer needs it; and Muster's `docs/board.md` says Rally's ledger is the
  board, read in the viewer or with `rally show` and `rally brief`. Muster's part is Castor-o7/muster#194. Done
  there: #194 merged on 2026-11-07 as a120efc, with `docs/board-archive-2026-11-06.md` and `docs/board.md`
  saying Rally is the source of truth; submitted to the watchman for its read on 2026-11-09 and verified (2200).

## Phase 4: The web app

Read-only. Changes go through the ledger's commands, so a flaw in the web app can't rewrite the ledger.

- [x] **RAL-25: Give the ledger a read-only API.** Done when: it's documented, it uses RAL-13's access
  control, and a test shows no request through it can change the ledger.
- [x] **RAL-115: The viewer has a modern sci-fi magitech look.** From the operator on 2026-11-08, relayed by
  the watchman: a sleek, reactive dashboard ([ideals.md](ideals.md), P6 to P10). Done when: the viewer is
  restyled and stays reactive and read-only, provenance labels and state colours keep their meaning (RAL-91,
  RAL-93), it works in light and dark and at phone width, the PR carries before and after screenshots, and
  sign-in and the live board are checked in headless Chrome over CDP. Feeds RAL-26.
- [x] **RAL-118: The viewer's look stays light.** Merged in #194 (3f2bb55). From the operator on 2026-11-08: RAL-115's look lagged. Done when:
  nothing on the page uses `backdrop-filter`, nothing that moves has a filter, every endless animation is a transform
  or opacity the compositor runs alone, and scrolling the live board in headless Chrome over CDP holds 60 frames a
  second, with the before and after numbers in the PR.
- [x] **RAL-116: The viewer reads Marshal's roster, falling back to Muster's.** From the watchman on
  2026-11-08, split from RAL-38's findings: Muster is frozen, and the viewer read only `muster-shim roster`.
  Done when: a test feeds each schema and checks the labels and ages come out as the source names them; with
  no sessions on Marshal it falls back, and the page says so; it's checked in headless Chrome over CDP.
  RAL-38's claim, done-when and the cutover of sessions to Marshal's daemon stay with the operator. Done:
  `rally web --roster-from` (or `RALLY_ROSTER_FROM`) lists the commands to try, `marshal-shim,muster-shim` by
  default, and the panel says which it read and why it passed any over. `shape_roster` keeps each source's
  labels (Muster's attested, declared and observed; Marshal's declared and recorded), shows the daemon's
  connection and wait under that label, and adds when the session connected and when it last called a tool.
  `each_roster_keeps_its_own_labels_and_ages` and `the_roster_falls_back_and_says_which_it_read` cover both
  schemas and the fallback, and `the_roster_falls_back_to_musters_and_keeps_each_sources_labels` checks both
  in headless Chrome. The ledger is now shown before the roster is read, since a roster command can take its
  whole 20-second limit to fail before the next is tried; the viewer's tests read no roster unless they pass
  one.
- [x] **RAL-119: A cutover inventory and runbook from Muster's daemon to Marshal's.** Merged in #195 (8d6a2ed). From the watchman on
  2026-11-08, toward the operator's goal of moving with no interruption. Docs only, read-only. Done when:
  [cutover.md](cutover.md) lists every place Muster is wired in on Murph (and what to confirm on a1), what each
  needs on Marshal, what Marshal can't do yet, and a step order that never leaves a session unreachable, with rollback.
- [x] **RAL-117: The viewer tries the roster source that last worked first.** From the watchman on 2026-11-08,
  after RAL-116: on Murph, `marshal-shim roster` took about 10 seconds to fail on every refresh before the
  fallback. Done when: after a fallback the next refresh asks the source that worked first; an earlier source
  is asked again at most every 5 minutes, and is used again once it answers; a unit test covers both with a
  fake runner; and the page still says which source it read. Done: the refresh keeps a `RosterMemory` of the
  command that worked; the ones before it are listed as passed over ("wasn't asked: it failed lately") until
  `ROSTER_RETRY_EARLIER` is up, and are asked after it if it fails.
  `the_roster_source_that_worked_is_asked_first_and_earlier_ones_now_and_then` and
  `a_remembered_source_that_fails_falls_through_to_the_rest` cover it.
- [x] **RAL-120: Rally's commands read and tell through Marshal or Muster.** From the watchman on 2026-11-08:
  moving every session from Muster's daemon to Marshal's, with no interruption, is 0x6's main goal, and `rally
  stalls`, `wake`, `brief` and `digest` called only `muster-shim`. A cutover blocker. Done when: each works
  with only `marshal-shim`, only `muster-shim`, or both with Marshal's daemon empty, with a test per command,
  using fake shims, that checks which shim was read or told and that the report names it; and a shim whose
  `tell` may have delivered isn't followed by a second send. Done: `rally::shims` holds one ordered list,
  `RALLY_ROSTER_FROM` (by default `marshal-shim,muster-shim`, as the viewer reads it). A roster comes from the
  first that gives one, named in `roster.read_with` (`roster_read` in the brief); a message goes through the
  first that knows its recipient, named in `sent_by`, falling back only when the shim says nothing was sent or
  can't be run. `tests/shims.rs` covers all four commands in the three cases, and the stop case.
- [x] **RAL-126: Rally reads every roster source and shows their union.** From the watchman on 2026-11-08:
  once MAR-57 made `marshal-shim roster` answer an empty roster at once, RAL-116 and RAL-120 took it as the
  winner and never read Muster's, where every session was. Side-by-side running needs every source read. Done
  when: the viewer, stalls, wake and brief all show the union; each row names its source; a source that fails
  is listed as passed over; tests with one source empty, both populated, and one failing; and a headless
  Chrome check. Done: `shims::roster` and the viewer's `read_roster_with` ask every command and merge with
  `shims::merge`: each row carries `source`, and a session id on both keeps the more recently active row with
  `also_on` naming the other. A failing command is asked again only every 5 minutes (RAL-117's memory, now per
  command), unless nothing else answers. `tests/shims.rs` runs stalls, wake, brief and digest with one shim
  installed, Marshal empty, both populated and Marshal down;
  `the_roster_is_every_sources_union_and_says_which_were_read` and
  `the_roster_shows_every_source_and_keeps_each_sources_labels` (headless Chrome) cover the viewer. Sending is
  unchanged.
- [x] **RAL-127: cutover.md's gap table is current with Marshal main.** Merged in #206 (7f0a54d). From robust-warbler's survey of
  [cutover.md](cutover.md) against Marshal main `700a1b2`, relayed by the watchman on 2026-11-09: the table still
  said MAR-9, MAR-36 and MAR-43 were "merging" and MUS-47 was "being ported". Docs only. Done when: each gap row
  names its story's state and merge commit as of current Marshal main; step 1 points at Marshal's
  `cutover-test-session.md` (MAR-59) and doctor's project check (MAR-60); step 7 links the no-traffic check
  (MAR-65); and the table says when it was last checked.
- [x] **RAL-122: A design note on carrying unread Muster mail to Marshal at the cutover.** Merged in #199 (bc6c5a2). From the watchman on
  2026-11-08, for [cutover.md](cutover.md)'s "message history carried over" gap. Docs only. Done when:
  [mail-carryover.md](mail-carryover.md) says how mail is kept on both daemons, measures what's unread, sets out the
  options (read it on Muster first, a relay, export and import, or nothing) with their trade-offs, and the choice
  is asked of the operator.
- [x] **RAL-121: The operator's three ideals are written down.** Merged in #197 (597f824). From the operator on 2026-11-08: "the three
  ideals of rally are thusly: accuracy, verifiability, and reactivity". Done when: [ideals.md](ideals.md) quotes it
  as decided, any gloss or grouping beyond the three words is marked proposed and asked of the operator, and it's merged.
- [x] **RAL-123: A verifier can correct its own read.** Merged in #200 (ca22bfe). From the watchman on 2026-11-08: annotating its own
  verification (1432) was refused as "isn't an earlier record that names watchman", and on MAR-10, a multi-part story
  still claimed, no form of re-read could be recorded. Done when: annotate accepts a correction from a verify's or
  send-back's reading team or writer, shown beside it like any annotation, and the refusal says who may annotate it; a
  part's verifier can add `--also-read` while the part is Verified and the story isn't; tests cover a verifier
  annotating its own read, an unrelated team refused, and the part's also-read; and the viewer shows the annotation.
- [x] **RAL-124: The viewer says how long each session has been busy or idle.** Merged in #201 (b4c69a3). From the watchman on 2026-11-08,
  the part of RAL-38 that doesn't wait on the cutover. `activity` had no time of its own, and `last_activity_at` is the
  last tool or prompt, not when a session turned busy or idle. Marshal's side is MAR-58 (the shim declares
  `activity_at`). Done when: the roster carries `activity_at` from either schema, declared, when the shim gives it;
  the page reads "busy for 3m", counting on, and "since unknown" without it; a unit test covers both schemas and a
  headless Chrome test both cases.
- [x] **RAL-125: The viewer fits a phone.** Merged in #203 (b7ad7dd). From the watchman's read of RAL-115 on 2026-11-08: at about 500 pixels a
  panel's edge and the Evidence table's last column looked clipped. Measured over CDP at a 390-pixel viewport on the
  live board, nothing runs past the window outside a table's own scroll box: the look at 500 was the 15-pixel
  scrollbar over a full-page capture, and the table scrolls sideways as RAL-111 intends. Done when: the
  narrow-window browser test also runs at 390 pixels, emulated, and fails if anything outside `.table-wrap` runs past
  the window, shown by breaking it on purpose.
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
- [x] **RAL-28: Show past ledgers.** Merged in #125 (6f55a6a). Depends on RAL-12. Done when: a viewer can pick a past moment and
  see the ledger as it stood then, with corrections made since shown as later, and a test shows that the
  view matches a replay to that moment.
- [x] **RAL-29: Secure the web app.** Merged in #114 (1490e83). Done when: a test shows that a cross-origin page can't read it,
  that markup in recorded text doesn't run, and that an unauthorized viewer is refused; it isn't exposed
  to the open internet; and someone other than its author has reviewed it adversarially, before it's
  reachable beyond the machine it runs on.
- [x] **RAL-69: Show only the projects in the current rally.** Merged in #141 (b88ec91). Requested by the operator on 2026-11-06: a
  project that hasn't been worked on for several rallies shouldn't be in the viewer "unless an active muster team
  has claimed that project", but its progress is kept. Done when: the overview's roadmaps, ledger figures,
  evidence, declared teams, track records and muster totals cover only the projects in which a team on Muster's
  roster holds a story (claimed, in review or sent back), and say so; the other projects keep their records and
  their pages and are listed, collapsed, as not in this rally; when the roster or the ledger can't be read,
  everything is shown with a warning rather than hidden on a guess; and a test covers the rule. Adding a
  project the ledger knows but `projects.json` doesn't (DiscoSim) is RAL-74's.
- [x] **RAL-76: Open and close rallies, with a clean board for each.** Merged in #149 (b6c223e). Requested by the operator on 2026-11-06:
  "When the rally closes from the previous session, the board should be cleared ... we log the old rally, store it,
  and CLEAN the board so we can get a fresh start." Done when: `rally open-rally` records a rally's name, projects
  and teams, and `rally close-rally` ends it, declared, one open at a time; the viewer's overview shows only the open
  rally's projects, teams and what was recorded since it opened; past rallies are listed, each viewable as the ledger
  stood when it closed; nothing is removed from the ledger; and tests cover the records, the service route and the
  board.
- [x] **RAL-82: One command starts, restarts and opens the viewer.** Merged in #154 (69f5768). Requested by the operator on 2026-11-06:
  "I should be able to run one command and just have it open." Done when: `rally view` starts the viewer if it
  isn't running, restarts it when it runs another build or other settings, or on `--restart`, including one started
  by hand; waits until it answers; and opens it signed in. Its settings are remembered, `--stop` stops it, and tests
  cover the settings, the restart rule and finding a viewer by its command line.
- [x] **RAL-77: Rallies, after RAL-76: who may open one, past rallies as their own board, dates, and errors.** Merged in #152 (2e9f3c7). From
  the watchman's read of #149 (RAL-76), filed by plush-tortoise. Done when: with the service's `--rally-openers`
  set, only the credentials it lists, or the credential bound to a signed watchman designation, can open or close
  a rally, and others get a 403 naming the rule; unset, any paired credential may, and each answer and
  `rally rallies` say to set it; a past rally opens as its own board (its projects, the stories recorded for them
  while it was open or held then, and its teams) as the ledger stood at its close, not the whole ledger at that
  record; the overview compares the watchman's writeup's day file with the rally's opening as local dates, not a
  local one with a UTC one; a `/v1/rallies` read that fails for any reason but an older service (a refused token,
  a network error) shows as a warning, not as "an older service"; `docs/service.md` says what `--rally-openers`
  does and can't do; and tests cover each.
- [x] **RAL-78: A watchman's rally starts from the roster.** Merged in #156 (6d59baf). Requested by the operator on 2026-11-06: the watchman
  is told which projects the rally works on, determined by the teams' directories, and sessions in no project (`src`)
  assist the project teams. Done when: `rally brief` shows the open rally with each roster session marked in it or
  not, or with none open, suggests one: the projects whose `sessions` in the project list match a roster session,
  every session as a team, those in no project as assisting, and the `rally open-rally` command to run; it opens
  nothing itself; and tests cover both.
- [x] **RAL-100: `rally view` names the builds it restarts for, and keys them on the commit.** Merged in #168 (459e30a). From the watchman's
  report on 2026-11-06: a run after `--restart` restarted the viewer again, because the two commands ran different
  builds of the same commit, which aren't byte-identical. Done when: a build from a clean checkout embeds its commit
  and `rally view` compares commits, falling back to the binary's SHA-256 without one; the restart note names both
  builds; and a test covers a rebuild of the same commit not restarting.
- [x] **RAL-108: A restarted session stays in the rally under its new nickname.** Merged in #176 (6d6ed46). Requested by the operator on
  2026-11-06, keeping the nicknames: the watchman restarted as mellow-weevil and dropped off rally 0x5's board until
  the rally was closed and reopened (913, 914). The operator ruled who is in a rally: "all the agents started by me,
  the current operator, that are open and available at the start of the rally; unless otherwise specified". Done
  when: while a rally is open, the ledger keeps each team's declared names with their first and last records and
  credentials. A new nickname joins only as a listed one's successor: the listed one is off the roster, and the new
  one recorded for the same team after it, with the same credential. The viewer, the brief and the page use this, and
  the page names each successor beside the one it succeeds. The listed nicknames are unchanged, and there's no new
  record kind. Tests show the restarted watchman rejoining, and a later unrelated session recording for `rally` not
  joining.
- [x] **RAL-110: The operator adds or removes a session in an open rally.** Merged in #179 (a70e313). Requested by the operator on 2026-11-06,
  completing RAL-108's "unless otherwise specified" and settling its shared-credential limit: the ledger can't tell a
  restart from a new agent on the same machine, so the operator decides. Done when: `rally rally-session add|remove
  <nickname> --note` records a declared `rally_session`, only while a rally is open, and the service takes it only
  from those who may open one. An add with `--succeeds <nickname>` confirms a restart. A removal can't confirm one,
  and nobody succeeds themselves. An added session is in and a removed one out, whatever else would take it in, and
  the latest record decides. A detected restart counts at once but shows as unconfirmed until confirmed or removed. A
  confirmed restart stops another being detected for the same session. The page lists what was added and removed.
  `docs/service.md` lists the record as ending the rollback window, and tests cover each.
- [x] **RAL-106: An also-read answers the open read request it covers.** Merged in #173 (56f425f). From RAL-100 on 2026-11-06: the request
  for a read at 4e145b1 (961) stayed open after the watchman added 4e145b1 as also read to the verification in force,
  since a story already Verified can't be verified again, and the merge then flagged it `merged_before_read`, saying
  a merge went past a read that had in fact been done. Done when: adding a commit as also read, or reading a merged
  commit beside the verification (RAL-85), closes every open request whose commit the verification now covers; a
  merge doesn't flag a request whose commit is already covered; replaying an existing ledger changes nothing else;
  and tests cover both.
- [ ] **RAL-34: View from a phone.** Not ready to claim until the operator wants it; they may. Depends
  on RAL-29. Done when: an unpaired device the operator names can read the ledger with a credential of
  its own that can be revoked alone, after its own threat review.
- [x] **RAL-73: A one-time login code survives the browser's preload.** Merged in #142 (cc20189). From the DiscoSim rally issues log of
  2026-11-06, §3.1: Josh's pasted login links kept reporting the code as bad, most likely because Chrome
  preloaded the address and spent the single-use code before the visit. Done when: opening a code's address
  shows a "Sign in" button and spends nothing, and only the button's same-origin POST spends the code; a test
  shows a prefetched GET (`Sec-Purpose: prefetch`, `Purpose: prefetch`) and a plain one leave the code working,
  once; the page's own policy allows its form and nothing else, while every other page still allows no form; it
  shows nothing offered in the address that isn't a code; another site's form POST is refused; and
  `docs/web.md` says so.
- [x] **RAL-83: The Sign in button works in Chrome.** Merged in #155 (3138066). RAL-73's sign-in page set `<meta name="referrer"
  content="no-referrer">`, under which Chrome sends `Origin: null` with the form's POST; the guard refuses
  `Origin: null`, so the button always answered "other sites can't read this page". Found live by the watchman
  on 2026-11-06 and confirmed in headless Chrome. Done when: the sign-in page's referrer policy is `same-origin`
  in its header and its `<meta>`, every other page stays `no-referrer`, `Origin: null` is still refused, a test
  asserts both, and `docs/web.md` says why.
- [x] **RAL-97: A browser test leaves nothing behind, and checks it.** Merged in #170 (a388f03). From 2026-11-06, when every flatpak run of
  RAL-84's script left a headless Chrome and a 170 MB profile in $XDG_RUNTIME_DIR until it filled. robust-warbler
  fixed the leak in #167 (RAL-96); this guards it. Done when: the script names every profile it used, on the host
  and in a flatpak sandbox, and every browser test checks after its run that none is left and no process still runs
  on one; the check fails when the sandbox's profile isn't removed.
- [x] **RAL-84: CI signs in to the viewer through a real headless browser.** Merged in #158 (de33916). From the watchman's finding of
  RAL-83 on 2026-11-06: every sign-in test was curl-driven and sent a real `Origin`, so none caught Chrome's own
  `Origin: null` under a no-referrer page, and the button failed for the operator every time. Done when: a viewer
  test opens a one-time code's address in headless Chrome, presses Sign in, and sees the board and `/api/status`
  served; it fails on a viewer with RAL-83's bug; it runs when `RALLY_TEST_CHROME` names a browser and is skipped
  otherwise; CI installs what it needs and can't skip it; and the README says how to run it.
- [x] **RAL-74: The viewer reads `~/.config/rally/projects.json` by default.** Merged in #146 (15f985d). From the DiscoSim rally issues
  log of 2026-11-06, §3.3: the built-in `web/projects.json` lists RAL, MUS, CAT, CHOMP and PDX only, so a whole
  rally ran with no DiscoSim page until the viewer was restarted with `--projects`. Done when: without
  `--projects`, `rally web` reads `projects.json` beside the pairing (`~/.config/rally/projects.json`, or
  beside `--config`) when it exists, and the built-in list when it doesn't; `--projects` still wins; the first
  line says which list was read; a file that's there but broken is an error, never a quiet fall back; tests
  cover each case and don't read the operator's own config; and `docs/web.md` says so.
- [x] **RAL-96: The viewer keeps what the reader opened, and shows a state once.** From Josh's review of the
  viewer in Chrome on 2026-11-06 (robust-warbler, watched over several refreshes): every 30-second poll rebuilt
  each section, so every open section closed each time (4 of 4, three cycles running), and a verified story's
  state read "verified verified". Done, on RAL-90's ticking ages: the refresh keeps open sections open; it skips
  its redraw while someone types in a field or has text selected, and "updated" moves only when the page is
  drawn; a state shows once, as a dot in its bar colour and its name, labelled declared while in review; and
  `sign-in.mjs` closes Chrome over DevTools and removes its profile even under flatpak, where each run had left a
  headless Chrome and a 173 MB profile in the runtime directory until it filled. A headless test shows an open
  section surviving a refresh, and fails without the fix.
- [x] **RAL-99: The roster says where each team is.** Asked for by Josh on 2026-11-06: the roster no longer
  said which Muster session was in which directory, because the viewer dropped `cwd`, keeping the page to what it
  showed since #24. Done: `shape_roster` passes each session's `cwd`, `git_branch`, `last_tool` and
  `current_task_at` (still not the operator's address), and the roster's columns are Team (the session's
  nickname), Directory (`~` for the home folder, the whole path in its title), Project (and branch), Machine,
  Status (busy or idle, and wake), Working on (and when it was set) and Last action (its age and tool).
- [x] **RAL-109: The viewer encodes the writeup's path in its commit-time read.** Merged in #178 (5823dbc). From mellow-weevil's read of
  RAL-104: `written_at` put the writeup's file name into `gh api repos/R/commits?path=<file>` unencoded. Today
  the name is always `YYYY-MM-DD.md` under a fixed folder, but a name with `&`, `#`, `+`, `%` or a space would
  have ended the value or added a parameter. Done when: the path is percent-encoded as one query value (RFC 3986
  unreserved characters and `/` kept), the commit is checked as hex before the call, and a unit test covers
  names with those characters.
- [x] **RAL-104: The rally report belongs to the open rally and carries its major events.** Merged in #177 (9f1271e). Asked for by Josh
  on 2026-11-06. When "rally 0x5" opened (record 914), the report box still showed the writeup from the rally
  before it. The overview matches the watchman's writeup to a rally by its day file's date, so a rally opened
  later the same day inherits the earlier one's writeup. Josh also wants the report to be more than an end-of-day
  summary: the major events of the rally, as they happen, such as the leaked headless Chrome processes that
  filled `/run/user/1000` and had to be killed, or a team's commit SHA that turned out not to exist. Done when: the
  report box shows nothing from before the open rally opened (a writeup counts only if it was written after the
  opening record, not merely dated the same day), and with no writeup yet it says so rather than showing an old
  one. A new `rally event` record, written by the watchman or any team, has a one-line headline, a severity
  (notice or incident), and what backs it (a record number, PR, commit or file), and is labelled declared like
  any other claim. The report box lists the open rally's events newest first, above the writeup, each with its
  author and age. A closed rally keeps its events in its past-rally view. And tests cover a same-day second
  rally, an event with no backing, and an old rally's events staying out of the new one.

## Phase 5: Rally into Levi

From the rally of 2026-11-04 evening. The operator told the watchman, grand-photon, that ignition-is-go's lead
developer welcomes Rally merging into Levi. Nothing goes to ignition-is-go/levi or any other upstream until the
operator says so; the work is done on a local checkout of Levi at `~/Documents/Gamoire/levi`. RAL-40 is still the
story for Rally reading Levi; these stories are about putting Rally's features into Levi.

- [x] **RAL-112: The viewer page reloads itself when the viewer runs new page code.** From the operator on
  2026-11-06: rally 0x5's report box kept showing the DiscoSim writeup after RAL-104 was deployed, because the tab
  had loaded its code before the restart, and a new window fixed it. Done when: the viewer stamps the page it serves
  with a fingerprint of its page code, and every status carries it. A tab whose status names other code reloads
  itself once, not while you're typing or selecting, and never twice for one code. Tests show the page and status
  agree, and in a real browser that the same code doesn't reload, new code reloads once, and a code already
  reloaded for doesn't again. `docs/web.md` says so.
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
- [x] **RAL-52: Hand a new watchman the peer-verified work.** Merged in #164 (07a6a59). Depends on RAL-51. Built in #99 (`rally
  handover`) and approved by the watchman, apart from the digest, which RAL-8 then built (#136, #137): its
  "Peer-verified, awaiting your re-read (RAL-52)" section lists each story still awaiting the re-read and is left
  out once none is, which a test now shows. Closed out on 2026-11-06 (RAL-52's own record). Done when: when a watchman is
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
- [x] **RAL-63: Designate more than one watchman at once.** Merged in #129 (cd27484). Requested by the operator during the two-watchman
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
- [x] **RAL-68: Don't hand out an ID an open pull request already uses.** Merged in #139 (ecb715e). Proposed by frank-crocodile, filed
  2026-11-05 at fiery-snapper's request, after MUS-95 was written by hand in an open pull request and then handed
  out again. Done when: `rally new-story` also reads the prefix's roadmap in each open pull request, read-only
  through the service's GitHub token, and never hands out a number used there; when GitHub can't be read it says
  so rather than guessing; with a test for an ID in an open pull request and one for an unreadable GitHub.
- [x] **RAL-85: Record a read of the merged commit.** Merged in #157 (c79a2c5). From mellow-weevil's re-read of RAL-81 on 2026-11-06: the
  merged head a4e6f90 took a merge of main after the verify at 6949b93, and `annotate --also-read` takes only the
  original verifier's credential and name, so the successor watchman couldn't record its read and `commit_differs`
  stayed. Done when: `rally read-merged <STORY> --commit <latest merged commit> --note ...` records a verified-kind
  read of its own, beside the verification, by any verifier who isn't an author, the original verifier included;
  it's refused unless the story is Verified and the commit is its latest recorded merge and not already covered;
  `commit_differs` counts it and RAL-67's also-reads; `verified_commits` lists it; it's dropped with the
  verification; `rally verifications` labels it by the designation, gated by nothing; with tests and
  docs/evidence.md.
- [x] **RAL-107: The service's socket test runs without `lsof`.** Merged in #175 (1e16ed6). From §15.1 of the DiscoSim rally's issues log:
  `nothing_listens_but_the_door` failed outright on Murph, which has no `lsof`, and a plain `cargo test` then stopped
  before tests/web.rs, so local runs skipped the browser tests unnoticed. Agreed with mellow-weevil. Done when: the
  test lists the service's internet sockets with `lsof` and, where there's `/proc`, from `/proc/<pid>/fd` and
  `/proc/<pid>/net/{tcp,udp,raw}{,6}`, and checks each reader's list; without `lsof` it uses `/proc` alone; with
  neither it's skipped with a printed reason; `RALLY_TEST_LSOF_REQUIRED`, set in CI, fails it when `lsof` is
  missing; with a test of the `/proc` address parsing, and README.md.
- [x] **RAL-102: The digest says when a merge leaves the base red.** Merged in #169 (87cfe08). From the DiscoSim rally's issues log (§10.3,
  §10.4): Muster's main was red for about a day and Rally's 11c0579 until a team noticed. Picked by mellow-weevil.
  Done when: for each merge the ledger records, the watchman's own included, a digest pass reads the base's CI on
  what landed (RAL-31's lookup, then `gh run list --commit`, read-only) and tells a failed run at once with its
  address, without waiting for the quiet period; a run still going is listed and checked again each pass, and given
  up on, said, after 2 hours; no run is told after 10 minutes; a check that can't be made says so rather than staying
  silent, and is tried again; a pass says nothing; what's pending is kept with the head; with unit tests, a test of
  the digest with a fake `gh`, and docs/digest.md.
- [x] **RAL-103: `rally gate` refuses a head that doesn't contain its base's tip.** Merged in #172 (e1bdc69). From the DiscoSim rally's issues
  log (§10.4): #144 merged at a head without #145, and main went red; #152 merged on 2026-11-06 without #151. Filed
  by mellow-weevil's pick. Done when: `rally gate <base>..<new>` also refuses when `<new>` doesn't contain the
  current tip of `<base>` (after a fetch of `origin/<base>`), saying to merge the base in first; it works without
  GitHub branch protection, which is the operator's setting; with a CLI test on two branches.
- [x] **RAL-105: The digest names verified stories whose open PR has fallen behind its base.** Merged in #174 (6a06f7f). Found in RAL-102's
  work and filed at mellow-weevil's suggestion: a story verified at a head that no longer contains its base's tip
  can't merge until the base is merged in and the new head read, and nothing told its author. Done when: when a
  digest is rendered, each Verified story not yet merged whose open pull request's head is a commit verified, in
  the repository its evidence names, is compared with the pull request's base through GitHub (read-only), and one
  behind is listed with the count, the pull request and what to do; one up to date is not; a read that fails is said;
  at most 10 repositories are read per digest, and the rest counted; with unit tests, a test of the digest with a
  fake `gh`, and docs/digest.md.
- [x] **RAL-87: Say when a watchman's digest has stopped.** Merged in #163 (a64baa3). From the DiscoSim rally's issues log (§2.7) and
  mellow-weevil's digest dying on a 429 on 2026-11-06 with nothing noticing. Done when: each `rally digest` pass
  records when it ran and when a digest last went; `rally brief` and `rally stalls` with `--digest-quiet-after
  <window>` flag each designated watchman, or one named with `--digest-watchman`, whose digest has no pass recorded,
  no pass within the window, or records waiting longer than the window with none sent, and not one whose ledger is
  merely quiet; the window has no default, since it's the operator's to agree; with tests.
- [x] **RAL-80: Viewer sign-ins survive a restart.** Merged in #150 (28bb214). From the DiscoSim rally's issues log (§3.4, 2026-11-06):
  sessions were held only in memory, so adding `--summaries`, or any restart or deploy, signed the operator out.
  Chosen over a SIGHUP reload of the projects and summaries, which would spare only config changes, not a deploy or
  a crash. Done when: a session made by signing in holds across a restart of the viewer; the sessions are kept in
  a file beside the key, readable only by the account, holding only hashes of the sessions and the key; a session
  lasts 14 days from its sign-in, checked on every request and at start, and only the newest 32 are kept; `--new-key`
  still signs every browser out, across a restart too; an unreadable sessions file signs no one in and doesn't stop
  the viewer; with a test.
- [x] **RAL-113: `drawPatched` says a kept node keeps its first listener.** From mellow-weevil's read of RAL-89:
  a node the patch keeps keeps the listeners from its first draw, so a handler that closed over render-time data
  would act on stale data. Done when: a comment above `drawPatched` says a handler in a patched section must read
  what it needs when it runs. Comment only.
- [x] **RAL-89: The viewer refreshes without flashing or closing what's open.** From robust-warbler's visual review
  of the viewer on 2026-11-06, item 1: each poll rebuilt every section with `replaceChildren`. RAL-96 (#167) kept
  open sections open and skipped the redraw while someone types or selects. Done when: a refresh draws each section
  into a detached copy and patches the page's own section to match, so every node that hasn't changed is the same
  node afterwards, and text or attributes that differ are changed in place; a field keeps its focus and what was
  typed; a section's open state survives a draw with no refresh around it; and headless Chrome tests show each,
  failing on the page before it. Item 3 (highlight changed rows, ease bars) isn't part of it.
- [x] **RAL-111: A browser test checks the viewer at a narrow window.** From mellow-weevil's read of RAL-92: with
  every browser test at 1500 pixels, nothing covered a half screen. Done when: a test can ask `sign-in.mjs` for its
  window with `RALLY_BROWSER_WINDOW` (`width,height`, checked, defaulting to 1500,1000); a headless Chrome test at
  800 pixels finds no horizontal page overflow and nothing wider than the window outside a table's own scrolling
  box, with the evidence table on the page; and it fails when the page is made wider than the window. Headless
  Chrome won't make a window narrower than 500 pixels, so a phone's width would need device emulation instead;
  that isn't done here.
- [x] **RAL-92: The viewer's tables are compact: short SHAs, no wrapped IDs, a wider page.** From robust-warbler's
  visual review of the viewer on 2026-11-06, items 6 to 8. Short SHAs with the whole in the title and unwrapped IDs
  and ages came with RAL-90 (#167); the copy button item 6 suggested isn't built. What was left was item 8: at
  `max-width: 960px` about 40% of a 1500-pixel screen sat empty while the tables wrapped heavily. Done when: the
  page uses up to 1200 pixels, never runs wider than the window, and a headless Chrome test on a 1500-pixel window
  shows it, failing at 960.
- [x] **RAL-93: The viewer shows each state once, with provenance in a legend.** From robust-warbler's visual
  review of the viewer on 2026-11-06, item 5: declared, observed and attested chips sat on almost every cell.
  RAL-96 (#167) already shows a state once, as a dot and its name. Done when: a table column whose cells all carry
  the same label shows it once, in its header, and on none of its cells; a column that mixes kinds (or has cells
  with none) keeps each cell's own label, so every fact stays labelled; a legend on the page says what observed,
  declared, verified and attested mean; and a headless Chrome test shows both kinds of column and the legend,
  failing on the page before it.
- [x] **RAL-90: The viewer's ages tick, with a live indicator; Verified green; short commits; no wrapped IDs.** Merged in #167 (d081399).
  From robust-warbler's visual review of the viewer on 2026-11-06 (items 2, 4, 6 and 7), assigned by mellow-weevil.
  Done when: every age on the page counts on between polls without anything being redrawn; a "Live · updated Ns
  ago" line replaces "Updated hh:mm:ss", amber when a poll is late and red when stale or unreachable, announcing
  only changes of state; Verified is green and In review amber, in both schemes; evidence of the form
  `owner/repo@<40 hex>` shows its first 7 with the whole in the title, and other evidence as written; IDs, states
  and ages don't wrap; a headless Chrome test shows each, and fails on the page before it.
- [x] **RAL-94: The watchman's writeup renders its markdown, as text only.** Filed by cobalt-glen, assigned by
  mellow-weevil: the writeup and day summaries were one block of preformatted text. Done when: headings, lists,
  emphasis, code, quotes and tables are drawn, built with el() and text nodes only (no innerHTML, RAL-16); raw HTML in
  the markdown shows as text; only http and https links are links, opened without a referrer or opener, and any other
  address shows as inert text; and a headless Chrome test shows a `<script>`, an `<img onerror>` and a `javascript:`
  link staying inert, and fails on the page before.
- [x] **RAL-91: The viewer's bars are coloured by what they measure.** The open half of RAL-91 (cobalt-glen's "the
  viewer's status colours follow meaning"), assigned by mellow-weevil: every bar's fill was one blue (`--fill`),
  whatever it measured. Done when: each Verified bar and each Progress from the ledger is one bar of Verified green,
  In review amber and Sent back pink over the stories tracked, in RAL-90's state colours, with a title on each
  segment and a legend; ticks, which aren't a ledger state, have a neutral `--ticked` colour of their own, and the
  legend under each tick figure says what it means, so green only ever means Verified; the viewer's data gives the
  in-review and sent-back counts beside Verified; and a headless Chrome test reads every bar's computed colour in
  the light and dark schemes, and fails on the page before.
- [x] **RAL-88: The viewer's cross-site refusal says how to sign in instead.** Merged in #162 (0ab6344). From the DiscoSim rally's issues
  log, §3.2: the Claude-in-Chrome extension's navigation is marked cross-site, so the viewer answered "other sites
  can't read this page", which is right but says nothing about what to do. Assigned by mellow-weevil. Done when:
  the refusal is unchanged (403, same rule) and its text adds: run `rally web --login` in a terminal and open the
  address from there or the address bar, not from another page or an extension; with a test of the text.
- [x] **RAL-86: Long-running digest and wake log a failed pass, back off, and keep going.** Merged in #159 (72d3c8e). From the watchman
  (mellow-weevil), 2026-11-06: `rally digest --every` exited with `refused (429): too many checks and past-state
  reads` right after a successful send. Reserved by silken-beetle, built by eager-cobra. Done when: with `--every`,
  a failed pass that can clear with time (429, 408, 5xx, the service or Muster unreachable) is logged as one JSON
  line and waited out, doubling up to 16 times `--every` and honouring `Retry-After`, and the loop goes on; it exits
  only on bad configuration, no pairing, or a refusal that won't clear; the digest keeps each watchman's head as
  its turn ends, so a pass failing part-way neither sends a digest twice nor skips one; `docs/digest.md` and
  `docs/wake.md` say so; with a test of a fake service answering 429 then 200.
- [x] **RAL-81: Progress from the ledger when a roadmap has no checkboxes.** Merged in #151 (8571623). From the DiscoSim rally's issues log
  (§3.5, 2026-11-06): DiscoSim's roadmap is a table, so the overview read 0 ticks all rally. Done when: a roadmap
  that's read and has no checkboxes shows its progress from the ledger's states, Verified of the stories the ledger
  tracks under its prefix, labelled as the ledger's, on its card and its project page; a roadmap with boxes, one not
  yet read, or a prefix the ledger doesn't track, doesn't; it's kept out of the boxes across projects, which say
  which projects were left out; with a test.
- [x] **RAL-71: Evidence can be corrected.** Merged in #145 (6b7227f). From the DiscoSim rally's issues log (§2.2, 2026-11-06): DSM-41's
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

### A rally that keeps up with its teams

The operator, directly on 2026-11-09, set the goal: Rally stays current with the rally as it happens. A project
team that joins should understand the workflow: it designs a development roadmap and passes it to the
watchman for verification and clearance; once the roadmap is accepted, the watchman coordinates and assigns
its tasks; and the cycle continues, the roadmap revised as needed to reach the project's goal. The design
principles named were durable, accurate and reactive. Found in rally 0x7 by plush-tortoise.

- [x] **RAL-128: The evidence check says a repository it can't see is unseen, not that the commit doesn't
  exist.** Merged in #207 (9a4a958). In rally 0x7 every RootyRoopsDAW submission (records 1650, 1732, 1734) was observed as
  `exists: false`, though each commit exists. The repository is private and the service's token gets a 404 from
  it, and GitHub uses the same 404 for a missing commit. Done when: after a 404 or 422 on the commit, the check
  reads the repository (a GET); only when the token sees it is `exists` recorded false; when it doesn't,
  `exists` is left out, the reason says the repository isn't visible to the token so the commit's existence is
  unknown, and the check isn't retried; a read that fails is retried as before; with a unit test and a test of
  the client's two GETs, and docs/evidence.md.
- [x] **RAL-129: A project's roadmap is submitted to the watchman and cleared at a commit.** Merged in #208 (f7f6664). A project's
  roadmap is its story `<PREFIX>-0`, so submitting, clearing, sending back, waiting and the watchman/peer
  labels are those of any story. Done when: a cleared `<PREFIX>-0`, and no other Verified story, can be claimed
  again to revise it, and goes back for clearance; `rally roadmap [PREFIX...]`, read-only, gives each project's
  roadmap, or each of the open rally's projects', with its stage (none, not started, drafting, awaiting
  clearance, cleared, sent back, revising, revision awaiting clearance), who submitted it and what, who cleared
  it at which commit, the last clearance during a revision, and what to run next; a roadmap that can't be read
  says so and isn't reported as missing; `rally brief` lists the open rally's roadmaps; docs/clearance.md
  describes the cycle; with tests of the ledger rule and of each stage.
- [x] **RAL-130: The watchman assigns a cleared roadmap's stories, and `rally next` offers them first.** Merged in #210 (f9e3e3b).
  Depends on RAL-129. Done when: `rally assign <STORY> --team <team> --note` records, as declared, that the
  story is assigned to the team, taken by the service only from those who may open a rally (the operator or the
  watchman, RAL-77); the assignment claims nothing and confers no authority; `rally show` gives it; `rally next
  --team` offers the team's own sent-back work, then ready and unblocked stories assigned to it, then the
  roadmap order as before, saying which rule picked it; a story assigned to another team is skipped, saying so
  and that it may still be claimed; an assigned story that's blocked says why; an assignment stays until `rally
  assign <STORY> --team <team> --end --note`, a newer assignment of the story, or the story's merge; with tests
  of the ledger, the service's rule and the pick.
- [x] **RAL-131: A team new to the rally is told the cycle: roadmap, clearance, assignment, revision.** Merged in #211 (39e391b).
  Depends on RAL-129 and RAL-130. Done when: `rally next --team` gives `project_roadmap`, and while the
  project's roadmap isn't cleared (and isn't being revised from a clearance) leads with `roadmap_first`, saying
  so and what to run; the session-start briefing names the open rally and its projects, each project's roadmap
  stage, the stories assigned to the team, and the cycle in four lines, escaped like agents' text and within its
  caps, and leaves out what it can't read in time; `rally init`'s block describes the cycle; `rally brief` lists
  the open rally's roadmaps (RAL-129); with tests. With it, from the watchman's read of RAL-130: an assigned
  story waiting on an unfinished one it depends on isn't offered, and says why; docs/clearance.md says the
  openers rule limits by credential, so by machine where sessions share one.
- [x] **RAL-132: An evidence check that finds no CI yet looks again before recording none.** Merged in #209 (84d5921). From
  silken-beetle's watch of the viewer on 2026-11-09: MAR-61 (9a8f5e1), MAR-62 and five other Marshal stories
  showed "CI none" for good, while GitHub has 5 of 5 check runs passed on each. MAR-61's check ran a second after
  its submit (1723, 1724) and its runs started 7 minutes later. Done when: a check that finds no check runs and
  no statuses is looked for again, as pending CI is, until the ceiling (`--evidence-ci-wait`, 30 minutes); runs
  appearing in that time are recorded as they stand; at the ceiling `none` is recorded with a reason saying no CI
  appeared within it; with a unit test and docs/evidence.md.
- [x] **RAL-133: The viewer picks up a change to its projects file without a restart.** Merged in #212 (fada36c). From silken-beetle's
  watch of the viewer on 2026-11-09 (finding 4): rally 0x7 opened with ROO, and ROO was added to
  `~/.config/rally/projects.json` that same second, but the viewer had read `--projects` once, at its start 4
  hours earlier, so it showed "In this rally without a roadmap in the viewer's project list: ROO" and no ROO
  card. Done when: at each refresh the viewer looks at the projects file's modification time and, when it
  changed, reads it again and reads its roadmaps at once, keeping each project's last reading by prefix; a
  version that can't be read or isn't a list keeps the list before, and the page says why; `/api/status`
  gives `projects_file`, observed; with a test of the reload and docs/web.md.
- [x] **RAL-134: A correction of Rally's evidence check shows beside the check on the viewer.** Merged in #213 (3e04f74). From
  silken-beetle's watch of the viewer after RAL-133 went live: the correction notes beside the "no such commit"
  checks (1646, 1650, 1732, 1734) didn't show in the Evidence table, so a reader still saw "no such commit"
  with nothing against it, and checks recorded "CI none" before RAL-132 stay that way. Done when: the story list
  gives each check's corrections (record, who, when, note, declared) beside it, leaving the check as observed;
  the Evidence table's "Rally's check" cell shows each one after the check, as "corrected by <name>
  (declared)"; with a unit test. Alongside, outside the code: each MAR check showing "CI none" whose commit
  GitHub shows with check runs gets a correction note giving the counts (25 notes, 1924 to 1948; MAR-18 and
  MAR-49 have no runs and keep "none"), and ROO-0 and ROO-13 get the RAL-128 note (1949, 1950).
- [x] **RAL-135: A project is added to or removed from the open rally.** Merged in #215 (b3838f0). The operator, 2026-11-09, asking why
  RAL wasn't on rally 0x7's board: rally 0x7 opened with MAR and ROO, and a rally's projects were fixed when it
  opened, so a team joining later could only be shown by closing and reopening the rally. "this is a necessary
  feature since rally is meant to model and display a dynamic development environment, while being a
  verifieable ledger of work for the agent teams that are participating in the rally". Done when: `rally
  rally-project add|remove <PREFIX> --note` records, declared, a project joining or leaving the open rally,
  taken by the service only from those who may open a rally; refused with no rally open, for a prefix already
  in (add) or not in (remove), and for the last project; the rally's `projects` change from that record, so the
  board, brief, roadmaps and session briefing follow it; the rally keeps `opened_with` and each change in
  `project_changes`, through its close; the viewer's rally panel lists the changes, declared; docs/service.md's
  rollback table names it; with ledger, service and headless Chrome tests. Along with it, docs/service.md's
  table gains RAL-129's re-claim and RAL-130's `assign`, which it was missing.
- [x] **RAL-136: The viewer shows the holds, conditions and grants in force.** Merged in #216 (aaabbe2). From the watchman on 2026-11-09,
  during the Marshal soak: the viewer had no sign of a standing record, so MAR-77's deploy hold and the cutover
  hold (1859) were only in `rally brief`. Done when: the service serves `/v1/standings`, a light read of the
  holds, conditions and grants not ended, each as declared with who declared it; the viewer reads it on every
  refresh and shows "Standing in force" beside the rally, holds first, each with what it's on, what releases it
  or its scope, its record and age, labelled declared, and says so when the service is older; an ended one isn't
  shown; with a unit test and a headless Chrome test, and docs/web.md.
- [x] **RAL-137: Viewer: a solid black page background, panels keep their colours.** Merged in #218 (72b8f10). From Josh directly
  (2026-11-09): the page's background is solid black, and the panels and other visual content keep their
  current colour scheme. Done when: in both schemes the page behind the panels is `#000` with no aurora or
  grid, the panels, bars and state colours are unchanged (RAL-91's colours), and text set straight on the
  page (the header, section headings, their muted lines) uses `--on-page` colours that meet WCAG AA on
  black in both schemes; with the viewer's browser tests passing.
- [x] **RAL-138: Viewer: reactive, agentic visual cues for live state.** Merged in #221 (5541ed0). From Josh directly (2026-11-09):
  research visual techniques that read as "reactive/agentic", and use them. From the watchman's scope: every
  cue is tied to a real change in the ledger or roster (a new record arriving, a session going busy or
  idle, a hold in force, a check correction), nothing animates while nothing changes, motion respects
  `prefers-reduced-motion`, and nothing is loaded from elsewhere (the page's CSP). Done when: the PR body
  gives the research and why each cue was chosen; the cues are in the viewer with a browser test that a
  change triggers its cue and an unchanged poll triggers none; and the PR shows them, in screenshots or a
  short recording.
- [x] **RAL-139: Viewer: a redraw updates inline styles (bars no longer freeze at their first draw).** Merged in #219 (a90c3bc). Found
  by silken-beetle watching the live viewer on 2026-11-09: the rally board's state bar drew 4/14/49/49 while
  its legend and its `style` attributes said 6/17/1/51, so a new "In review 1" segment was as wide as
  Verified's 51. `patchNode` (RAL-113) copied `style` with `setAttribute`, which the page's CSP
  (`style-src` without `'unsafe-inline'`) doesn't apply: the attribute's text changed but what was drawn
  didn't, so every patched bar kept its first draw until a reload. Done when: a redraw copies inline styles
  one property at a time with `style.setProperty`, which the CSP applies; a browser test draws the board,
  adds a story, redraws, and finds the bar's computed widths follow the counts (failing on the old
  `patchNode`); and no other attribute `patchNode` copies is one the CSP refuses.
- [x] **RAL-140: Viewer: an Activity strip shows who's working now and what just changed.** Merged in #221 (5541ed0). From Josh directly
  (2026-11-09), following RAL-138's research: presence and attribution are the clearest agentic cues. Done when:
  at the top of the rally's status, "Now" lists the rally's sessions whose activity is busy, each with its
  nickname, how long it's been busy (declared; "since unknown" when `activity_at` is missing) and its
  `current_task` drawn as text, never markup, since it's agent-written; "Just now" lists the 6 latest story
  changes from the ledger, newest first, each with its state, team, record and age; both are drawn only from
  `/api/status`; RAL-138's cues light a changed story or session in the strip, and nothing animates while nothing
  changes; with a browser test of the busy list, text-only tasks, "since unknown", newest-first order and the
  cue on a redraw.
- [x] **RAL-141: A parts story with every part verified says the close is what's next.** Merged in #222 (44ffaab). From the watchman on
  2026-11-09: MAR-23 had parts 2/2 verified and sat Claimed, `read-merged` and a correction were refused with
  "can't make this change from Claimed", and nothing said that the holding team's close (docs/parts.md) was
  what was missing, so the watchman asked whether it was a bug. Done when: a verify, send-back or `read-merged`
  on a claimed parts story whose every declared part is verified is refused saying the close is next and whose
  move it is, with the command; `rally show` gives `close_due` on such a story, in the list and the detail;
  `rally waiting` lists them under `closes_due`, since they wait on a team, not a reader; a merge recorded in
  between changes none of it; docs/parts.md says so; with ledger and view tests.
- [ ] **RAL-142: Viewer: ambient outlines and more magitech motion, kept light.** From Josh directly (2026-11-09):
  "more moving panel outlines, more sleek magic", "reactive, sleek, neofuturism meets magitechnology"; the layout,
  level of detail, colours and function stay as they are, which Josh called successful. Under RAL-26 (the look is
  the operator's to approve). The watchman relaxed RAL-138's "nothing animates while nothing changes" for this story
  alone: the ambient outlines may run, and must cost under a few percent of one core in headless Chrome. Done when:
  every panel has an edge along which two thin lights run, out of phase, as boxes of their own that only translate
  (RAL-118: the compositor alone, no filter on anything that moves); a cue (a new record, a story's state change)
  sends one bright pass in the change's colour along the panel it belongs to, with the colour set by `setProperty`
  (RAL-139); the edge is the panel's first child so a patch keeps it and the motion never restarts on a poll; it's
  hidden from assistive technology and gone under `prefers-reduced-motion`; the meaning colours are byte-identical
  (RAL-91's test) and the CSP is unchanged; a browser test checks one edge per panel, that only its lights run while
  nothing changes and only by `transform`, and that a redraw keeps the same edge; and the PR states the idle CPU
  share and scrolling frame rate measured in headless Chrome, before and after.
- [ ] **RAL-143: A deployed service names who may open or close a rally.** From Josh directly (2026-11-11), after
  the watchman found Murph's service had run with `--rally-openers` unset since RAL-77 (the 11-06 drop-in
  naming `josh,watchman` was shelved and never restored), so any paired credential, every team on Murph
  included, could open or close a rally, and nothing but a line in each answer said so. Josh: fix it so future
  rallies don't start like that. Done when: `rally serve` with `--rally-openers` unset refuses to start unless
  it's given an explicit opt-out (such as `--rally-openers-any`), naming the rule and docs/service.md;
  docs/deploy-murph.md installs the openers setting with the unit, and the deploy's check fails without it;
  `rally rallies` and the viewer show who may open (`who_may_open`) at the top, not only inside answers;
  docs/service.md says so; with a service test that an unset list refuses to start, an opt-out starts with
  the warning, and a listed credential is the only non-watchman one an `open-rally` accepts.
- [ ] **RAL-144: Local CI on Murph for every project; GitHub Actions minutes stop being a bottleneck.** From Josh
  directly (2026-11-11, relayed by the watchman, then confirmed to the lamplighter): "we are out of github actions
  minutes... do away with the bottleneck of Github actions entirely. We can do the CI/CD locally moving forward
  please, for all projects." Josh chose a local runner that tests same-repo branches only and posts results to
  GitHub, over a self-hosted Actions runner: rally, marshal and muster are public, and a self-hosted runner on a
  public repo lets any fork's pull request run code on Murph, where the daemons and their tokens live.
  What was measured before scoping (2026-11-11): Actions still runs today for the public repos (rally#225's
  `check` and `dependency-audit` passed at 18:18Z, on standard runners that bill no minutes); marshal's 11-10 runs
  were cut off mid-job at 01:23Z (`check` and `windows-native-wake` cancelled about 4.5 min in), when the account
  ran out; the required checks are rulesets pinned to GitHub Actions (`integration_id` 15368): `check` on rally
  and muster, `rust` on RootyRoopsDAW; marshal's main has no required check. DiscoSim keeps `dsm-gate`.
  Done when:
  - **Runner.** `ci-local` (in this repo, `scripts/ci-local/`) watches Castor-o7's rally, marshal, muster and
    RootyRoopsDAW: each push to main and each open pull request's head whose head repository is the base
    repository (never a fork; a fork's pull request gets a `local/skipped-fork` status saying a maintainer must
    push the branch to test it). It runs each new sha once, and posts one commit status per job, named
    `local/<job>`, pending then success or failure, with a link-free description and the log path.
  - **Same jobs as the workflows.** Each repo carries `.ci/local.toml` listing its jobs and their commands, one
    for each Linux job in its workflow (rally: check, dependency-audit; muster: tests, conformance, check;
    marshal: check, windows-cross-check, plus a dependency audit; RootyRoopsDAW: rust, compat), with any tool
    the workflow installs pinned to the same version. Jobs Murph can't run say so: muster's `macos` and
    RootyRoopsDAW's macOS leg run on a1 over SSH (Murph→a1 is set up) or are marked `manual`; marshal's
    `windows-native-wake` is `manual`, with `windows-cross-check` still run. A test checks every workflow job is
    either mapped in `.ci/local.toml` or listed as manual, so the two can't drift.
  - **Caps.** Every job runs in `systemd-run --user --scope -p MemoryMax=<cap> -p CPUQuota=<cap>`, with
    `CARGO_TARGET_DIR` and `TMPDIR` under `~/.cache/ci/` (never `/tmp`, a RAM tmpfs on Murph), at most N CI
    jobs at once through CI's own slots (`~/.config/ci-local.slots`), alongside `dsm-heavy`'s rather than
    shared with them, with the memory floor as the guard both share (watchman's read of #227); it pauses new jobs while
    `MemAvailable` is under 6 GB, and while Josh's DiscoSim play window is open it doesn't pause (he plays for
    hours, and a paused queue is the bottleneck this removes) but runs one job at a time at `nice 19` with a
    lower `CPUQuota`.
  - **Rate limits.** The poller states its poll interval, uses conditional requests (ETag / `If-None-Match`, which
    don't count against the limit when nothing changed), reads `X-RateLimit-Remaining`, and backs off before the
    limit, logging each back-off and when it resumes.
  - **One-shot.** `ci-local run <repo> <sha>` runs the same jobs for one sha by hand, writes logs to
    `~/.cache/ci/<repo>/<sha>/<job>.log`, and posts statuses only with `--post`.
  - **No spend.** Josh, 2026-11-11 (to the lamplighter, after the timing API showed a public repo's run billed
    0 ms under his $0 budget): run both on the public repos. rally, marshal and muster keep their hosted
    `push`/`pull_request` triggers and Actions' required `check`, with `local/*` alongside, each declaring
    `hosted_runs` in `.ci/local.toml`; the drift check fails if one of their jobs names a larger (billed) runner.
    The private RootyRoopsDAW's workflows keep only `workflow_dispatch` and lose their `push`/`pull_request`
    triggers. No `runs-on` names a self-hosted label on a public repo (marshal's `public-ci-*` and `release`
    self-hosted jobs reviewed and either dispatch-only or removed, with the reason).
  - **Josh's admin steps, named in the doc** (an agent can't do them): change RootyRoopsDAW's required check
    from the Actions-pinned `rust` to the status context `local/rust` (the public repos keep Actions' `check`); install the
    runner's systemd user unit (auto mode refuses unit files to agents); and confirm the token it posts with
    is limited to commit statuses on those four repos.
  - **docs/ci-local.md** covers how it works, the caps, the one-shot, adding a repo, the admin steps and what
    it doesn't do (forks, Windows), and which repos run both (public) and which run local only (private).
  - **Proof:** a real pull request on each of the four repos shows its `local/*` statuses go green; on
    RootyRoopsDAW with no Actions run started (the Actions runs list for that sha is empty), merging under the
    updated ruleset, and on the public repos beside Actions' green `check`; and
    a deliberately failing commit on a scratch branch shows `local/check` red with its log.
- [ ] **RAL-145: `rally next` and `rally new-story` find a project's own roadmap, tables included.** From the
  watchman (2026-11-11, issue 1.38): in DiscoSim's checkout `rally next --team <t>` failed with "can't read
  docs/roadmap.md", since DiscoSim keeps its roadmap at `docs/design/99-roadmap.md`; given that path, it then
  found "no stories with IDs", because that roadmap is tables, not a checklist. `rally new-story DSM` read no
  written numbers for the same reasons. Done when: without `--roadmap`, both take the projects list's `path` for
  the prefix (or, for `next` without `--prefix`, for the checkout's `origin` repo), falling back to
  `docs/roadmap.md`, and `next` says which in `roadmap_from`; a roadmap with no checklist stories is read from
  its tables' `ID`/`Story`/`Depends on`/`Done when` columns, done coming from the ledger; checklist roadmaps
  pick as before; docs/next.md says so; with unit tests for the path order and for a DiscoSim-shaped table
  roadmap, and `rally next` in the DiscoSim checkout offering a DSM story.
- [ ] **RAL-146: An operator inbox: everything waiting on the operator in one place.** From Josh directly
  (2026-11-13, to the lamplighter): "an 'Operator Inbox' where operator interaction duties and notices of
  Operator type can be stored for Operator Remediation. Like this key, I definitely needed to deal with it, but
  we are working so dynamically that this information got lost in the blitz", "so that I can be more responsive
  to the needs of the team". What prompted it: Rally's own GitHub token stopped working on 2026-11-11 at about
  20:35, every evidence check from record 3034 to record 3486 (110 of them) recorded "401 Bad credentials", and
  the fix was one command only the operator could run; it was said in chat and to the watchman and waited until
  2026-11-13 11:08. On that day the ledger also held 31 open questions to the operator, the oldest from
  2026-11-05 (record 83), with nowhere he'd see them together. Done when:
  - **One list.** `rally inbox` and an "Operator inbox" panel in the viewer show every open item waiting on the
    operator, oldest first within urgent then normal: open questions to the operator (today's blockers on the
    team `operator`, unchanged), and a new declared record kind for the rest, an **operator item**. Each row has
    what's needed in one line, who raised it and when, its age, what it holds up (a story, a team, or the
    service itself), and, where there is one, the exact command or decision asked for. The viewer shows the
    count of open items wherever the board is shown, so an empty inbox and a full one look different at a glance.
  - **Two kinds of item.** `rally inbox add` takes `--action` (something only the operator can do: a token, a
    sudo step, a merge permission, a purchase) or `--notice` (something the operator should know and needn't
    act on), with `--urgent` for what's breaking work now. Any team may add one. Nothing secret goes in an
    item: the threat model's list of what's never recorded applies, and the command refuses text that looks
    like a token or key.
  - **Rally files its own.** When the service observes a fault only the operator can fix, it opens one urgent
    item itself, as an observed record, and doesn't repeat it while it's open: at least an evidence check
    refused for the token's credentials (401 or 403), and the token's expiry within 7 days where GitHub
    reports one. It closes the item itself once the next check succeeds. A rate limit or a missing permission,
    such as RAL-147's check-runs 403, isn't a refusal of the token and files nothing.
  - **Closing, by the operator, by hand** (Josh, 2026-11-13: "I will hand tick the boxes as complete within the
    rally viewer itself. Sort of like a quest log in a video game."). Each item in the viewer's panel has a box;
    ticking it closes the item at once, an action done or a notice seen, and a question to the operator is
    answered as `unblock --direct`. The tick works in any viewer he has open, Murph's included (his choice of
    the three put to him: "I am the only one using this system and at some point we have to assume that is
    trustworthy"), and is the viewer's one write: a POST from a signed-in session of the page only, same-origin,
    with a token tied to the session. The close is recorded as declared, as the operator's, saying it was ticked
    in the viewer, with the machine's credential; signing with the operator's key, to make it verified, stays
    possible and isn't required. `rally inbox done <record>` is the same act at a terminal. Closed items stay in
    the history, and `rally inbox --all` shows them.
  - **Where he looks, nothing pushed** (Josh: "The urgent items should appear at the top of the list in a
    priority queue. The inbox is merely a meaningful method for delivering and storing Operator Action Items").
    Urgent first, then oldest first. `rally digest` and `rally brief` carry the inbox's open count and its
    urgent items, and the session-start block tells a team when an item it raised was closed.
  - **Old records replay.** Questions to the operator keep their records, shelving window and
    `operator_away` rules; the inbox reads them and changes none. docs/service.md's rollback table gains the
    new kind.
  - docs/inbox.md says what belongs in the inbox and what doesn't (a team's own blocker on another team
    doesn't), with unit tests for the ordering, the self-filed token item opening once and closing itself, the
    refusal of secret-looking text and of ticks from anywhere but the page, and a headless-Chrome check that
    the panel shows an added item and loses it when it's ticked.
- [ ] **RAL-147: Check runs the token can't read are recorded at once, read from commit statuses.** From Josh
  through the lamplighter (2026-11-13): Rally's fine-grained GitHub token can't read check runs on private
  repositories (GitHub 403 "Resource not accessible by personal access token"; fine-grained tokens have no Checks
  permission), so every DiscoSim, RootyRoopsDAW and catapp submission was retried until the CI wait ran out and
  recorded "its CI couldn't be read". Done when: that 403 on the check runs is taken as permanent for the token,
  so the check reads the commit's statuses alone, records at once, and says in the reason that check runs can't
  be read with the token and statuses were used; with tests against a stand-in GitHub that (1) the 403 is
  recorded at once, not after the wait; (2) a commit with no statuses and unreadable check runs reads as `none`,
  "no CI reported", never `passed`; (3) a repository whose check runs can be read is checked as before; (4) other
  refusals (a rate-limit 403, another 403, a 401) are still failed reads tried again. docs/evidence.md says so.
- [ ] **RAL-148: The watchman's digest keeps to the open rally and stops repeating its handover list.** From
  the watchman (2026-11-13): every `rally digest`, even for one claim, carried "Peer-verified, awaiting your
  re-read (RAL-52) (353)" with 33 lines. Most of them had been verified by the watchman itself (DSM-100..143,
  records 2774–3120), and some came from projects outside the open rally (CAT-36..83). `rally waiting`'s
  `closes_due` listed MAR-10 and ROO-6/7/10/11 while the open rally's projects were DSM and RAL. The cause:
  the ledger holds one designation (record 3155), so the handover window opened at record 0. A verification
  counted as a peer's if no watchman was in force when it was written, so the watchman's own reads from
  before its designation counted, and only its reads after record 3155 could clear them. Neither list was
  scoped to the open rally, and the digest re-sent the whole list each time. Done when: a verification by the
  watchman (its bound credential, or under a v1 designation its declared name or a name tied to it by
  `rally-session --succeeds`) is never a peer read, and one of its own after a peer's counts as the re-read,
  whenever it was written; with a rally open, `rally handover` marks each row `in_rally`, and the digest's
  handover section and `rally waiting`'s `closes_due` list only that rally's projects (`rally handover` keeps
  every row, and `rally waiting --all` lists every project's closes); a digest whose handover rows are all ones
  the last digest sent says one count line instead of the list; with unit tests for each.
- [ ] **RAL-149: An evidence check a restart cut short is made at the start, whatever the story became.** From
  the watchman and the lamplighter (2026-11-13): after the 20:37 service restart, submissions 3697–3699
  (DSM-214, DSM-209 parts 3 and 4) never got an evidence check. The old build was still retrying their checks
  when the service stopped, and on start `evidence::unchecked` re-queued only submissions still in review, while
  the watchman's verifications (3700–3702) had already moved them on. Done when: on start, every submission with
  no evidence_check record is checked once, whatever its story's state now (verified, sent back, merged), from
  the ledger's first check on, and the record's reason starts with "checked late, after a service restart"
  (in the existing reason text, so the rollback window stays open); a test stops the service between a submit
  and its check, verifies the story, restarts, and finds the record; nothing is checked twice.
- [ ] **RAL-150: Inbox follow-ups: a ticked question says "answered elsewhere"; browser tests leave nothing
  behind.** From the watchman (2026-11-13, after RAL-146's verify), and the lamplighter's report of main
  `ac88000` going red at 21:14 on `tests/web.rs` "a browser still runs on rally-browser-kw0M3y" at load 17-20
  during a DiscoSim sweep. Done when:
  - **A ticked question.** A question to the operator ticked off in the viewer closes as answered directly with
    the note "ticked in the viewer" and no answer, and the inbox marks it `via: viewer`. The asking team's
    session-start line says so plainly: "the operator ticked your question off in the viewer; if you don't have
    the answer, ask the watchman". The viewer's row for a question says that ticking it means it was answered
    elsewhere, and its box is labelled so.
  - **No browser left behind.** `tests/browser/sign-in.mjs` cleans up on every path: a thrown step or a
    rejected promise, a signal, the timeout, and a normal end. Each Chrome process on the run's profile is
    found by the profile's name, asked to stop, then killed, and checked gone, rather than trusting the flatpak
    launcher's exit. Component updates and default apps are off, so a run leaves no `.crx` or fetcher folder in
    the sandbox's `/tmp`, which isn't the profile (1,971 entries, 188 MB had built up by 2026-11-13; flatpak's
    wrapper resets `TMPDIR`, so they can't be sent to the profile). The tests' own check waits for a closed
    browser to go, up to 20 seconds, kills one still there, and fails only if it survives that.
  - **Proof:** the browser tests run 20 times under load (a release build looping beside them), every run green,
    no Chrome left on a test profile, and the sandbox's `/tmp` no larger after than before.
- [ ] **RAL-152: Session start tells a restarted session what it needs to resume.** From the watchman (2026-11-13),
  after issue 1.73: at 19:50 every session ended and came back as a new session under the same nickname, and each
  team sat until the watchman worked out by hand which earlier session had held its nickname and where its
  transcript and scratchpad were (about 15 minutes for six teams; one guessed wrong first). Done when: with
  `--hook`, `rally session-start` ties the session's id to its Muster nickname (its transcript's Muster tag, or the
  roster), and only then tells it, read-only and first in the block: the earlier session that held the nickname,
  found from the head of the project's transcripts (observed from the files, never Muster's word, "can't tell"
  said plainly when none is found), with its transcript and scratchpad paths; what the team holds; what a session
  under the name last recorded and that story's state now; reads waiting; its last note, each declared. A new,
  unrelated session is never told another's claims; a compact or resume isn't called a restart; it writes
  nothing. docs/session-hook.md gives the `settings.json` line for the operator to install. Proof: run by hand as
  each of the six DiscoSim nicknames restarted at 19:55, it names the earlier session the watchman found by hand
  (vibrant-gull c2319bd8, wise-crocodile fa76ef71, sleek-moth e2eb2632, silent-dhole 00baf303, steely-ridge
  e9f8a3ac, deft-pelican 4e31e02c).
- [ ] **RAL-153: Service-level tests of what the service does when GitHub refuses its token.** From the watchman
  (2026-11-13), the gap jaunty-harrier named in RAL-149: `GitHub::new` had no API base override, so no test of
  the running service could show RAL-146's own inbox items. Done when: a stand-in GitHub base is taken for tests
  only (`RALLY_TEST_GITHUB_BASE`, read only by a debug build, only `http://127.0.0.1:<port>`, never under
  `--secure`, and said on standard error when taken; a release build ignores it and says so), and service-level
  tests against a stand-in GitHub show: a 401 files exactly one urgent inbox item across several queued checks
  each tried again, and Rally closes it itself, observed, when a read passes; ten checks made at the start meeting
  a 401 file one item (RAL-149's case); the check-runs 403 files none and the check is recorded; a token expiring
  within 7 days files one. docs/evidence.md says how the override is kept out of a production start.
- [ ] **RAL-154: The watchman's digest loop is started, stopped and restarted by name.** From the watchman
  (2026-11-13, issue 1.75): the digest loop ran inside a bash `while true` wrapper started by hand, found again
  only by searching for its command line, and after RAL-148 the loop needed a restart to pick up the new CLI.
  The wrapper predates RAL-86 and has never had to restart the loop since (no "digest exited" line in three
  days), so it needs no 429 work. Done when: `rally digest --every` holds a pid file (digest.pid beside the
  pairing, or `--pid-file`). The file is written at the loop's start, refused while a live digest holds it, and
  given the exit status and error when the loop ends by itself. `--background` starts the loop on its own,
  logging to digest.log. `--stop` stops the process the file names and removes the file. `--restart` does both,
  with the settings given or the ones remembered, and writes to digest.log why the previous one ended if nobody
  asked it to. No process is found by pattern, and it works without /proc or systemd (a1 is a Mac). Unit tests
  cover the file and the command-line check, and a service test covers start, refusal, restart, a kill -9 the
  restart reports, and stop.
- [ ] **RAL-155: The docs, read as a new operator and a new watchman would, after the night of 2026-11-13.** From the
  watchman: Rally gained an inbox, a restart briefing, late checks and a scoped digest across five pull requests by
  two teams. Done when: README, service.md, web.md, digest.md, inbox.md, session-hook.md and evidence.md say what
  the code does now, with nothing stale (the README's status and "not verified yet" lines, the viewer's "no write
  path"), nothing missing (what `--rally-openers` guards, the service's own inbox items, the session-start block's
  sections), and nothing said twice; docs only; and every place where the code, not the docs, looks wrong is
  listed for the watchman rather than written around.
- [ ] **RAL-156: Rally says what it is.** From the watchman (2026-11-13), after RAL-155's read of the docs found that
  nothing said which commit was live, and that the CLI met an older service with a bare "refused (404): no such
  route". Done when: `rally --version`, the service's start line and `/v1/whoami` carry the build's commit (or say
  there's none); a CLI read meeting a route the service lacks says "the service is older than <story>", from a table
  of the story that added each route that a test holds to the routes the service serves; and `rally init`'s block
  tells agents about `rally inbox add`, when to raise an item and when not (not for what the watchman or the team
  can do itself); and, since this repository's own block had drifted from what `rally init` writes (it had bullets
  for `brief`, `view`, rallies, events and standing records that init didn't), init's block becomes the superset,
  so re-running `rally init` here deletes nothing, with a test that the repository's block equals what init writes.
  service.md's "Keeping it current" says plainly that on Murph the service goes live only once the operator
  restarts it with `sudo`.
- [ ] **RAL-158: An export during an append waits for it instead of reporting a difference.** From the
  watchman (2026-11-13): main went red on 6395487 when hosted CI failed `a_check_a_stop_cut_short_is_made_late_at_
  the_start_whatever_the_story_became` at tests/service.rs:2763 (`unwrap()` on None), just after the service
  printed "writes refused: the service's copy of the ledger in myko was changed past the append path". The cause
  is in the service. `Store::export` compared myko's copy with the kept state without the writer's lock. An append
  puts its record in myko's copy a moment before the kept state has it, so an export in that moment reported a
  difference that wasn't one, and refused. RAL-149's test polls the export while Rally appends its own evidence
  checks, and a loaded runner widens the moment. Done when: the export reads both under the writer's lock, as
  `check` does; a unit test holds an append between myko and the kept state, runs an export, and finds that it
  waits rather than refuses (red without the fix, with the CI log's message); the service test fails on a non-200
  export saying what it was, waits for checks with a 30 s bound and fails loudly on the timeout, and proves
  "nothing checked twice" by waiting for a later submission's check rather than sleeping.
- [ ] **RAL-157: `rally brief` is what a restarted watchman reads first.** From the watchman (2026-11-13): at
  19:58 it rebuilt its picture from a 215 KB handoff file by hand, because `rally brief` was 161 KB (110 KB of it
  the old handover list), not scoped to the open rally (19 of 30 claims from past rallies), and said nothing
  about merges, what changed since the watchman last wrote, or what was being gated. In parts. Part 1, done
  when: `rally brief --watchman` (`/v1/brief?watchman=true`) is under 20 KB, scoped to the open rally, and leads
  with what others wrote since the watchman's own last record, each team's newest merge since the rally opened,
  what's verified and not merged, what waits on the watchman and on the operator, and rulings newest first, then
  each team's assignments and claims and counts with the commands that show more; the full brief counts claims,
  blockers and teams outside the open rally instead of listing them; a roadmap with no `<PREFIX>-0` says where it
  is kept; docs/brief.md says so; with unit tests for each and a service test of the route. Part 2, after a
  note to the watchman on its fields: a declared batch record (the stories and commits gated together, the tip,
  started, landed or failed), shown first in the watchman's brief.
- [ ] **RAL-159: Service tests wait on the thing itself, with a bound, never a fixed-sleep bet.** From the
  watchman and earnest-raccoon (2026-11-13), after RAL-158: tests in tests/service.rs bet that something had
  happened after a fixed sleep, or read once after a wait that carried on silently. Done when: the evidence-check
  test waits up to 30 s and fails saying so; the operator-away test retries each shelve until taken, reads
  "during" inside a 10 s window and says so if the box stalled past it, and waits for the window's end rather than
  sleeping; the connection-cap test waits up to 10 s for a freed place; both hung-Postgres tests wait until a
  probe write is told the service is busy (the service's own word that a write holds the slot) and fail if none is
  within 30 s. The reads test, which passed with nothing stuck, is shown red first in that case.
- [ ] **RAL-161: The digest's small faults from the 0x9 rally.** From the watchman (2026-11-13): an empty range
  printed backwards ("records 3818-3817"); the base-CI section said "no repository in the evidence" for RAL-144,
  a story in parts whose part submits named Castor-o7/rally; every DiscoSim merge printed "no CI run on <sha>,
  10m after the merge was seen", one line each, though DiscoSim has no CI on GitHub and its gate is the
  watchman's. Done when: a digest with no new records says so; a parts story with no closing submit has its
  newest part's repository (`/v1/stories` gives `part_evidence` apart from the story's own evidence); a project
  whose projects-list entry names a `gate` isn't looked for on GitHub, and gets one line per digest naming its
  merges, held until a digest carries it; a merge whose story rode a landed batch says "landed in batch N"
  (`/v1/stories` gives `landed_in_batch`); docs/digest.md says so; with tests for each.
- [ ] **RAL-162: A landed batch carries the gate's own log, hashed and read by Rally.** From the watchman
  (2026-11-14): a DiscoSim landing is the watchman's word ("landed in batch N"). Of the three ways to back it,
  in order: (1) now: `rally batch end --gate-log <path>` hashes the log and reads its result lines by one fixed
  parser, refuses a log without the `# dsm-gate: commit ...` first line, and the service refuses one whose commit
  isn't the batch's tip; recorded as declared ("the log as the watchman kept it, no retention promised"); the
  brief shows it and asks about a landing whose log shows a failure. (2) when the operator grants a token with
  commit-status write on DiscoSim: the gate posts `local/gate` on the tip and Rally reads it as observed.
  (3) documented: rerunning the gate script named in the log on the commit it names. Done when (1) is built
  with tests for the parser, the refusals and the brief, and docs/brief.md says what the hash is worth and what
  it isn't.
- [ ] **RAL-163: `rally stalls` sees the open rally's teams and the watchman's batches.** From the watchman
  (2026-11-14): at 19:58 what told it the state fastest wasn't in Rally. On the live ledger `rally stalls` checked
  the projects list's teams, none of which were tonight's, and flagged only past rallies' blockers. Of tonight's
  facts it caught none: a team idle waiting on the watchman's gate, a hung godot, a claim "stale" while its commit
  sat on a train. In parts. Part 1, done when: with a rally open, stalls checks the rally's teams (named at its
  opening, or with live work in its projects), finding their sessions by nickname and confirmed restarts, scoped
  to the rally, with the projects list's other teams named and not checked; a claim riding a batch still gating
  is never a missed check-in, is named in every-team-waiting as waiting on the watchman's batch N, and prompts the
  watchman (`riding_batch`) only when the batch is past its expected time or built on one that didn't land; a
  landed story with no submit past the check-in prompts the team (`owes_submit`); docs/stalls.md says so; with
  unit tests and a service test. Part 2: `--lock` takes a glob of lock files (dsm-heavy's slots), and for each
  holder's process tree samples CPU time twice a few seconds apart: "possibly hung: <command>, <cwd>, N% CPU over
  5 s, running M min", a prompt and never a finding, with nothing killed.
- [ ] **RAL-160: A browser test that loses Chrome at start shows what Chrome said, and tries once more, said.**
  From the watchman and the lamplighter (2026-11-13/14): headless Chrome aborted at start twice on Murph at load
  15-20 (22:43:22 and 23:58:52), SIGABRT in libsystemd's bus code called from flatpak's zypak spawn helper, before
  the page loaded, and its words were lost because the browser tests discarded its standard error. Done when: on any
  failure, the test's output carries the last 4 KB of Chrome's standard error; an abort at start (exit 134 or
  SIGABRT before the code page loads) is tried once more, and the retry is said on the test process's own,
  uncaptured standard error and in the script's output as `retried_after_abort`, never silently; a second abort, or
  any other exit, fails with what Chrome said each time; each exit is given with what the lamplighter needs to find
  it in the machine's own records: the time to the second, the pid held (saying when it's flatpak's launcher's),
  `/proc/loadavg`, and whether the document portal's mount was there; with tests through a stand-in for Chrome's
  command that aborts once, every time, or exits otherwise.
- [ ] **RAL-164: `rally stalls` as one command line, from the projects list.** From the watchman (2026-11-14): a
  cron line had to spell `--quiet-after`, `--lock` and `--hung-after`, with DiscoSim's lock paths, and stalls and
  wake read the built-in projects list, not the one beside the pairing where DiscoSim is. Done when: stalls and wake
  read projects.json beside the pairing unless given `--projects`; a project entry may carry `stalls` (`locks`, a
  glob, and `hung_after`), used with a rally open and no `--lock`, flags overriding; `--prompts-only` prints
  nothing when there's nothing to ask and succeeds either way; the projects list leaves out a field it doesn't
  know, with a warning, instead of refusing the whole file; docs/stalls.md gives the cron line and the hazard
  for readers older than the field; with tests.
- [ ] **RAL-165: `rally brief --watchman` says what the watchman waits on.** From the watchman (2026-11-14): at
  19:58 it re-derived every team's owed answers from its own file. Messages are Muster's; commitments about stories
  are Rally's, and a blocker already says "X waits on Y". Done when: `brief --watchman` gains "you wait on": the
  open blockers the watchman's team raised (or it raised), oldest first, with the team, story, note and age, scoped
  to the rally; and each landed story with no submit since its newest landing, with its team; docs/brief.md says to
  record an owed thing as a blocker when asked and close it when it comes, and that a question answered in the same
  exchange needs none; with a test.
- [ ] **RAL-166: `rally brief --watchman` says when the service is older than RAL-157.** From the watchman
  (2026-11-14), after RAL-156's route table: every route the service serves names the story that added it, but the
  watchman's brief is a query on an existing route (`/v1/brief?watchman=true`), which a service older than RAL-157
  ignores, answering with the ordinary brief as if it were the watchman's. Done when: the CLI tells the watchman's
  brief by its own sections and, given the ordinary one, says the service is older than RAL-157 and needs updating;
  with a unit test of both answers.
- [ ] **RAL-168: docs/watchman.md, for a watchman starting cold.** From the watchman (2026-11-14): one short page, from
  the commands as they are on `main`, of what the night of 2026-11-13 taught: what to read first (`rally brief
  --watchman`, then the brief, handover and inbox), what a restarted session reads, keeping the digest loop running
  by name, reading and landing work one merge at a time, batches, `rally stalls` from the system's crontab, and what
  only the operator does. Docs only.
- [ ] **RAL-167: `rally brief --watchman` says what's live, staged and merged but not live.** From the watchman
  (2026-11-14): tonight the service was thirteen stories behind main, and nothing in Rally said so; the lamplighter's
  staging left files, not records. Done when: `brief --watchman` has a `builds` section, all observed: live (the
  service's commit from /v1/whoami, or that it's older than RAL-156 and doesn't say), this CLI's commit, main's
  tip, and the stories merged but not live, from GitHub's compare of live and main, mapped by the story ID in each
  commit's message, with the count and the oldest's age; a hint when this CLI is ahead of the service (staged,
  waiting on a restart); no reading of rollout files; with tests.
- [ ] **RAL-171: Conditions shown where they bite, a verified pull request behind its base at session start, and a
  merge's CI noted.** From the watchman (2026-11-14), from jaunty-harrier's note of the night. Done when: (a) a
  condition in force naming a story is printed by `rally merge <STORY>` and under the team's stories at session
  start, "a condition in force names this story: <text>", declared, information and never a block; (b) the team's
  session start says when one of its verified, unmerged pull requests has fallen behind its base, from the same `gh`
  read as the digest's RAL-105 section, within what's left of its 3 seconds, and says so when `gh` doesn't answer in
  time; (c) `rally merge` prints the merged commit's CI, read through `gh` at the merge, observed and recorded
  nowhere, with "running; the digest's RAL-102 section checks again" rather than a verdict while it runs. The CLI
  tests run with a stand-in `gh` first on `PATH`, so none reads GitHub.
- [ ] **RAL-169: The roadmap merges with git's `union` driver.** From the watchman (2026-11-14): on 2026-11-13
  nearly every pull request conflicted on this file, because every team appends its entries at the end, and each
  conflict cost a merge commit and another read. Done when: `.gitattributes` sets `docs/roadmap.md merge=union`;
  the roadmap's header says entries are appended, that an edit to an existing entry needs a look after merging
  main in, and that GitHub's web merge ignores the driver; tested by merging two branches that each append.
- [ ] **RAL-170: `rally rebased <STORY> --head <sha>` says whether a re-based head is the change verified.** From
  the watchman (2026-11-14): on 2026-11-13 it and a team checked by hand, about eight times, that a head with main
  merged in carried the verified change. Done when: observed, recording nothing, it compares the verified commit's
  change against its merge base with main and the head's against its merge base with current main, by patch (not
  by file list); says `same change` only when identical, otherwise names the files and hunks that differ; says
  separately when the head's merges of main resolved a conflict; says `can't tell` (never `same change`) for a
  missing commit or a compare cut short at 250; local git first, GitHub as the fallback; with tests for same,
  changed, conflict-resolved and can't-tell (docs/gate.md).
- [ ] **RAL-172: `rally rebased` counts a final newline, and its GitHub path says modes are unseen.** From the
  watchman's verify of RAL-170 (2026-11-14). Done when: a `\ No newline at end of file` after a changed line is
  kept in the hunk, so a head that differs only by a final newline reads `changed`, and one after context is not;
  the GitHub fallback's answer says file modes can't be seen there; conflict resolutions are the head's own merges
  (first parent), not those main's history brought in, found replaying RAL-163; with tests (docs/gate.md).
- [ ] **RAL-175: A standings read that fails at session start says so; a condition naming no story is said at every
  merge.** From the watchman's verify of RAL-171 (2026-11-14). Done when: at a team's session start, a read of the
  conditions in force that fails or runs out of time gives one line saying it couldn't be checked and why, like the
  behind-main line, and only for a team with an open story; `rally merge` gives, after the conditions naming the
  story, each condition in force naming no story, said as naming none and bearing on every merge; a team's session
  start still lists only those naming its stories; information, never a block; with tests (docs/session-hook.md).
- [ ] **RAL-174: `rally stalls --prompts-only` says what it couldn't see, and fails.** From checking the watchman's
  cron line by hand (2026-11-14): under cron's PATH neither shim nor `gh` was found, the roster couldn't be read,
  two missed check-ins vanished, and the run printed nothing, a silent all clear. Done when: with `--prompts-only`,
  a run that couldn't read the roster, a roadmap or other read, or a lock file (a pattern matching nothing) writes
  `can't see: <what>` on a line each and exits non-zero; the report carries `blind`; docs/watchman.md §6 and
  docs/stalls.md give the cron line with its `PATH=` and say why; tested with no shims on PATH.
- [ ] **RAL-173: `rally rebased --part k` compares a part's verified commit.** From checking the usage sent to the
  watchman (2026-11-14): before its close a story in parts has no verified commit, only each part's, so RAL-163's
  part 2 re-merge couldn't have been read. Done when: `--part k` reads part k's verified commit and the repository
  its evidence names; without it, a story in parts says `can't tell` and names the verified parts; a missing part
  or an unverified one says so; with tests (docs/gate.md).
- [ ] **RAL-176: `rally rebased` remerges with the attributes the merge was made with, and says so.** From the
  watchman's run (2026-11-14): its checkout, on an old branch without `.gitattributes`, remerged a union-merged
  roadmap with the default driver and named it a conflict resolved, where a checkout with the attributes named
  none. Done when: each merge is remerged with its first parent's `.gitattributes` (`--attr-source`), the output
  says which attributes it used, and a test checks a checkout without the attributes gets the head's answer.
