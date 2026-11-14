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
5. **Waiting on the operator:** the operator's inbox (RAL-146), urgent first.
6. **Rulings:** holds, conditions and grants in force, and the rally's events, newest first. A ruling given
   only in chat isn't here: record it as one of those.
7. **Teams:** each team's assignments and the stories it holds in the rally.
8. **More:** counts of what's outside the rally or past the bounds, with the command that lists each.

What the ledger doesn't hold is said, not guessed at: what's deployed, a batch not recorded, and whether a gate
process is still running (an open batch past its time is a prompt to look, not a finding).

## Recording a batch (RAL-157 part 2)

Where the watchman lands work by gating several commits together, a "train", as DiscoSim does without pull
requests, it records each batch so a restarted watchman, or anyone, can see what's in flight:

```bash
rally batch start --project DSM --car DSM-214@<sent sha>@<picked sha> --car DSM-209@<sent sha> \
  --base <sha it was built on> --tip <sha the gate runs on> [--on-batch <N>] [--expect 50m] \
  [--log ~/.cache/watchman/train54.out] --note "train 54"        # prints its record number, N
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
scene's `=== N passed, M failed ===`. A log without that first line is refused plainly, and the service refuses a
log whose commit isn't the batch's tip: the wrong log for the wrong batch, or a gate run on a stale tip.

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

## `rally brief`: the whole picture

Without `--watchman`, the brief gives each team's state, open claims (with whether the claimant's session is on
the roster), submissions waiting, open blockers and their chains, questions to the operator, the operator's
inbox, the handover list, holds, conditions and grants, the rally and its sessions, and where each of its
projects' roadmaps stands. With a rally open, claims, blockers and teams outside its projects are counted under
`outside_rally`, not listed (`rally show` and `rally blockers` list them all); questions to the operator are
listed whole. A project whose roadmap has no `<PREFIX>-0` in the ledger says where its roadmap is kept, from
the projects list, rather than reading as if it had none.
