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
3. **In the gate:** stories verified and not merged, with the commit read. Which of them the watchman is gating
   together isn't in the ledger yet (RAL-157 part 2 records it).
4. **Waiting on you:** the rally's submissions waiting for a read, and how many parts stories wait on their close.
5. **Waiting on the operator:** the operator's inbox (RAL-146), urgent first.
6. **Rulings:** holds, conditions and grants in force, and the rally's events, newest first. A ruling given
   only in chat isn't here: record it as one of those.
7. **Teams:** each team's assignments and the stories it holds in the rally.
8. **More:** counts of what's outside the rally or past the bounds, with the command that lists each.

What the ledger doesn't hold is said, not guessed at: what's deployed, and which commits the watchman is gating
together.

## `rally brief`: the whole picture

Without `--watchman`, the brief gives each team's state, open claims (with whether the claimant's session is on
the roster), submissions waiting, open blockers and their chains, questions to the operator, the operator's
inbox, the handover list, holds, conditions and grants, the rally and its sessions, and where each of its
projects' roadmaps stands. With a rally open, claims, blockers and teams outside its projects are counted under
`outside_rally`, not listed (`rally show` and `rally blockers` list them all); questions to the operator are
listed whole. A project whose roadmap has no `<PREFIX>-0` in the ledger says where its roadmap is kept, from
the projects list, rather than reading as if it had none.
