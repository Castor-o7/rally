# Multi-part stories (RAL-79)

From the DiscoSim rally's issues log, §2.1: DSM-37 was verified at part 1, and Rally had no way to record parts 2–4.
A verify of a Verified story is refused, and a resubmit is refused while the story is in review. The workaround, "submit
a multi-part story once, at its last part", left long stories invisible on Rally for hours.

```sh
rally submit DSM-37 --team discosim --evidence Castor-o7/discosim@<p1> --part 1 --of 4
rally verify DSM-37 --part 1 --commit <p1> --note "read the long bar"       # anyone but the submitter
rally submit DSM-37 --team discosim --evidence Castor-o7/discosim@<p2> --part 2
rally send-back DSM-37 --part 2 --commit <p2> --note "no test for the ATM"
rally submit DSM-37 --team discosim --evidence Castor-o7/discosim@<p2b> --part 2   # under the same claim
rally amend DSM-37 --part 2 --team discosim --evidence Castor-o7/discosim@<p2c>   # while it's in review
...
rally submit DSM-37 --team discosim --evidence Castor-o7/discosim@<last>   # the close, as today
rally verify DSM-37 --commit <last> --note "all parts read"                 # refused while a part is open
```

## How a part moves

- **The story stays Claimed** while its parts are read. A part is the holding team's work in progress, so it's
  submitted only while the team holds the claim.
- **Each part** goes: submitted (in review) → verified, or sent back → submitted again. Reading it follows the
  story's rules: never by anyone who claimed or submitted the story (RAL-51), with the commit read (RAL-31).
  A verified part can't be submitted again. A part already in review is amended, not resubmitted.
- **Amending a part** (RAL-71) is for its submitter, while that part is in review. Once a part is amended, a read
  of it that names a commit must name the amended evidence's. A story-level `amend` still covers only the story's
  own submission.
- **The close** is today's submit and verify. The final verify is refused while any part is in review or sent
  back, and says which.
- `--of n` declares how many parts there are. The total shown is the latest `--of`, else the highest part
  submitted.
- **A story with no parts** behaves exactly as before, and its records are written as before.

## Where parts show

- `rally show <ID>`: `parts` ("2/4 verified") and `part_list`, each part's state, evidence, submitter and the
  commit read. `rally show --brief` and `--field parts` give the count.
- `rally show` (the list): `parts`, `parts_in_review`, and `part_commits` (the commits verified parts' readers read).
- `rally waiting` and the watchman's digest list each part in review as `DSM-37 part 2/4`.
- Rally checks each part's evidence as it checks a submission's (RAL-5), and again after a restart.
- The viewer's board counts a story with a part in review as held.
- `rally gate` passes a commit a verified part's read covers (`docs/gate.md`).

Rollback: a record with `part` can't be replayed by a binary older than RAL-79 (`docs/service.md`).
