# The brief (RAL-22a, RAL-157)

`rally brief` is what a watchman reads when it starts with no memory: the muster's state, from the ledger, in
one read. It confers no authority: holds, conditions and grants are listed as their writers declared them.

## `rally brief --watchman`: read this first (RAL-157)

A restarted watchman needs the answers in the order it goes looking for them, short enough to take in at once.
`rally brief --watchman` (`GET /v1/brief?watchman=true`) gives them, from the ledger alone, kept to the open
rally's projects, with every list bounded and every note cut short (well under 20 KB):

1. **Since your last record:** what others wrote about the rally's stories after the watchman's own newest
   record, counted by kind of change, with the newest rows. Rally's own evidence checks are counted but not
   listed.
2. **Landed:** each team's newest `rally merge` since the rally opened, with the commit and when. A story that
   landed without a `rally merge` isn't here, so record each landing.
3. **In the gate:** the batches the watchman recorded with `rally batch` (below): those still gating first, each
   with its cars, tip and age, and a prompt when it's past its expected time ("is the gate alive?") or the batch
   it's built on failed; then the newest ended, an abandoned one saying which later batch re-ran its tip, a
   failed one saying which batches built on it are still gating; then, for the newest landed batch, each car
   whose story has no submit since it landed, with its team: the list to chase. Then stories verified and not
   merged, with the commit read.
4. **Waiting on you:** the rally's submissions waiting for a read, and how many parts stories wait on their close.
5. **You wait on** (RAL-165): the open blockers your team raised, oldest first, each with the team it waits on, the
   story and the note; and each landed story in the rally with no submit since it landed, with its team.
   **Waiting on the operator:** the operator's inbox (RAL-146), urgent first.
6. **Rulings:** holds, conditions and grants in force, and the rally's events, newest first. A ruling given
   only in chat isn't here: record it as one of those.
7. **Teams:** each team's assignments and the stories it holds in the rally.
8. **More:** counts of what's outside the rally or past the bounds, with the command that lists each.
9. **Builds** (RAL-167): what's live, by the service's own word (`/v1/whoami`'s commit, or "older than RAL-156:
   the service doesn't say its commit"); this CLI's commit; main's tip; and what's merged but not live, from
   GitHub's compare of live and main: how many commits, and each story named in their messages with the age of its
   oldest commit. GitHub's compare lists at most 250 commits, so a service further behind than that reads short, and
   `reads_short` says so with both numbers. All of it is observed from its own source. Nothing reads the
   lamplighter's rollout files. It needs `gh` signed in, as the digest does, and says so when a read fails.

   The service, the viewer and the CLI are one binary, so a story merged after the service's build isn't thereby
   waiting on the service (RAL-195, after the brief read "8 stories merged, not live" of commits that changed
   only the CLI, the viewer, docs and tests). What's read is every commit on main's own line (its first-parent
   chain) since the service's build: a pull request's merge, or a commit made straight on main. GitHub lists a
   merge's files against its first parent, so these commits' files together are exactly what changed on main, each
   change once. A pull request's commits are covered by its merge whatever their messages say ("address review"
   as much as "RAL-148: ..."), and a merge of main into a branch, which repeats what main already had, isn't read.
   Each commit's files go to the stories it names or brought to main; one that names and brought none is listed by
   its commit in `unnamed` when it may wait on the service, and counted with the stories ("1 story and 1 commit
   naming no story merged, not live on the service"). Each says what it `waits_on`:
   - `service`: a file the service runs changed (`builds::SERVICE_MODULES`, or `Cargo.toml`, `Cargo.lock`,
     `build.rs`), listed in `service_files`. Only these are counted: "2 stories merged, not live on the service;
     the oldest for 6 h", and `count` and `oldest_age_ms` are theirs.
   - `can't tell`: only files the service shares changed, listed in `shared_files`: `src/web.rs` (the viewer,
     and the roadmap parser the service uses) and `src/lib.rs` unless its patch only names modules the service
     doesn't run; or a commit's files couldn't be read. Read the diff. It's never counted as live.
     `src/main.rs` was one of these until RAL-196 moved `serve`'s flags and startup to `src/serve.rs`, which is
     the service's, and the `--db`, `--head-file` and `--db-password-file` flags to the store: `main` now only
     passes that command on and declares none of them, and a test keeps it so.
   - `cli or viewer`: only code the CLI or the viewer runs changed. It's live wherever those are on a build that
     has it: `cli_build` says this CLI's commit, how far behind main it is and which stories it `lacks`, and
     `viewer_build` the build of the viewer `rally view` started on this machine, from its run file.
   - `nothing running`: docs and tests only.

   Past 60 commits landed on main, or when GitHub lists fewer commits than the service is behind, the files
   aren't read: `files_unread` says so, and every story counts as before.
   When this CLI is ahead of the service and a story or commit waits on the service or can't be told, a hint says a build
   looks staged and waiting on a restart: a hint, not a fact. A new file in `src/` must be placed in one of the
   lists in `src/builds.rs`, which a test checks; one this CLI's build doesn't know reads as `can't tell`.
10. **In flight** (RAL-191): the jobs a closed window would end (issues log 1.73, when every session's background
   jobs ended with the shell windows and nothing said what had been running). For each project's `stalls.locks`
   in the projects list beside the pairing, every held lock: its holder's pid, state, command line and working
   directory, from `/proc` (observed), and the owner its lock file names (declared, as `dsm-heavy` writes it). A holder whose
   taker has exited is found by its open descriptor, as stalls finds it (RAL-192).
   Each fair pool's queue comes too, with holders and waiters by owner (RAL-188's reader, see docs/stalls.md).
   Free slots are counted, not listed, and a `summary` says "3 locks held, 1 job waiting for a slot". It's read
   on this machine, by the CLI, wherever the service is, since that's where the jobs run. Read-only. What it
   can't read is in `blind`: an unreadable projects list, no project naming a lock, a lock that can't be read, a
   queue that can't be seen. It never reads as "nothing running". A job that holds no lock isn't seen: a gate or
   probe run outside `dsm-heavy` or a project's locks won't show.

What the ledger doesn't hold is said, not guessed at: what's deployed, a batch not recorded, and whether a gate
process is still running (an open batch past its time is a prompt to look, not a finding).

## Recording a batch (RAL-157 part 2)

Where the watchman lands work by gating several commits together, a "train", as DiscoSim does without pull
requests, it records each batch so a restarted watchman, or anyone, can see what's in flight:

```bash
rally batch start --project DSM --car DSM-214@<sent sha>@<picked sha> --car DSM-209@<sent sha> \
  --base <sha it was built on> --tip <sha the gate runs on> [--on-batch <N>] [--expect 50m] \
  [--log ~/.local/state/dsm-gates/<sha>.log] --note "train 54"        # prints its record number, N
rally batch end --batch <N> --landed <sha main moved to> --note "..."
rally batch end --batch <N> --failed --note "why"
rally batch end --batch <N> --abandoned --note "the gate died with the session"
```

A car is a story, the commit its team sent, and optionally the sha it became on the batch; up to 30. Its story
need only exist: in this way of working a story is submitted after it lands. `--on-batch` names a batch still
gating that this one is built on, as a superset gated in parallel. Only those who may open a rally may record a
batch (the service's `--rally-openers`). It's declared and changes no story's state: record each landing with
`rally merge` as well. A batch is a record kind older binaries can't replay; see docs/service.md.

### The gate's log (RAL-162)

`rally batch end ... --gate-log <path>` records the gate's own log with the end. Rally hashes the file (sha256 and
size; nothing is typed) and reads its result lines with one fixed parser, for DiscoSim's `dsm-gate`: the first
line `# dsm-gate: commit <sha> tree <sha> scenes: ... started <time> script sha256 <hex>`, `QUICK OK`, and each
suite's or scene's `=== N passed, M failed ===` (or `..., K skipped ===`). Since RAL-181 that's every suite: the
QUICK phase's (`== test_sim ==`, `== test_layout (what it covers) ==`) by name as well as the scenes
(`== res://tests/<scene>.tscn ==`), about 112 a gate, where before only the three scenes were. A QUICK suite with
no count line, such as the bench (judged by its own verdict suite), is covered by `QUICK OK` and left out; without
a `QUICK OK`, it's kept as giving no result. At most 256 are recorded, and the service checks that bound too, so
a CLI from RAL-181 on needs a service from RAL-181 on to record a full log. A log without that first line is
refused plainly, and the service refuses a log whose commit isn't the batch's tip: the wrong log for the wrong
batch, or a gate run on a stale tip.

What it is worth, and what it isn't. The hash binds the log the watchman keeps to the one it named when the batch
ended, so a log changed afterwards is caught by anyone who has it. It doesn't show that the gate ran: the
watchman could name any file. So it's recorded as declared, "the log as the watchman kept it ... no retention
promised", with the path as given, and the result lines are the log's own words, not a check. The brief shows
it with the batch, and asks about a batch recorded as landed whose log shows a failure or a scene with no result.

Two stronger checks aren't built yet. The real one is the gate posting a commit status (`local/gate`) on the tip,
as ci-local does for Rally, which Rally would read from GitHub as observed; it needs a token with commit-status
write on DiscoSim, which is the operator's to grant. The one that needs no trust at all is rerunning the gate on
the tip: the log's first line names the commit, the tree and the gate script's hash, so anyone can run that
script on that commit and compare.

### Recording what you're owed (RAL-165)

What the watchman asks a team for and hasn't had, a sha, a frame, an answer that holds up a story, is a
commitment about work, so it goes in Rally: when you ask, record it as a blocker of your own team on theirs, and
close it when it comes.

```bash
rally block --team <your team> --on-team deft-pelican --story DSM-214 --note "owes the landed sha"
rally unblock <N> --note "got it: 3a1f..."
```

`rally brief --watchman` then lists it under "you wait on" until it's closed, so a restarted watchman gets the list
back from the ledger instead of a file. A question answered in the same exchange needs no record. Messages
themselves, and whether one was answered, are Marshal's, not Rally's: Rally doesn't copy them.

## `rally brief`: the whole picture

Without `--watchman`, the brief gives each team's state, open claims (with whether the claimant's session is on
the roster), submissions waiting, open blockers and their chains, questions to the operator, the operator's
inbox, the handover list, holds, conditions and grants, the rally and its sessions, and where each of its
projects' roadmaps stands. With a rally open, claims, blockers and teams outside its projects are counted under
`outside_rally`, not listed (`rally show` and `rally blockers` list them all); questions to the operator are
listed whole. A project whose roadmap has no `<PREFIX>-0` in the ledger says where its roadmap is kept, from
the projects list, rather than reading as if it had none.
