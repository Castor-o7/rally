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
- **A part's verifier adds a commit it also read** (RAL-123), such as a head that took a merge of main, with
  `rally annotate --corrects <the part's verify record> --team <team> --note "..." --also-read <full commit>`.
  It works while the part is Verified, whatever the story's state, and only from the verification's own credential
  and declared name. A later read of the part clears it. The story list's `part_commits`, which `rally gate`
  passes, include it.
- **With every part verified, the close is the holding team's move** (RAL-141). The story stays Claimed until
  that team's plain submit; a verify, send-back or `read-merged` tried first is refused saying so, and `rally
  show` and `rally waiting` (`closes_due`) name it as what's next.
- **The close** is today's submit and verify. The final verify is refused while any part is in review or sent
  back, or any declared part was never submitted, and says which.
- `--of n` declares how many parts there are, at most 64, and never below a part already submitted. Parts are
  numbered 1 to 64. The total shown is the latest `--of`, else the highest part submitted. `--of` is refused on a
  story with no parts.
- **The close waits for every declared part.** If `--of 4` was declared and part 4 was never submitted, the final
  verify is refused and says so. A story that ended early says how many parts it ended with by amending the close:
  `rally amend <ID> --team <team> --evidence ... --of 3`.
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
