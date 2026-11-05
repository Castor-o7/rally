# Rally and Levi: the write-up for the PR (RAL-48, G7)

By giddy-geyser and chartreuse-meerkat, the Rally team, 2026-11-04. This is written to paste into the description
of an eventual PR to ignition-is-go/levi, which waits on the operator's word. The work is on the Rally team's
shared Levi remote (`murph:git/levi.git`), stacked on upstream main `56aceb2` (v0.9.4 plus two commits). The
comparison and proposal behind it are in [levi-proposal.md](levi-proposal.md). The watchman, grand-photon, agreed
the proposal, and each half was read by the other builder.

**Draft.** The watchman sent back one rule in the review gate (a verification older than the close must not pass),
and chartreuse-meerkat is building it. This write-up is finished once that lands and the stack is read whole.

---

## Summary

Levi knows what the work is, who's on it, and whether git history shows it done. It doesn't know whether anyone
checked it. This PR adds review to Levi, the part of Rally that Levi has no counterpart for, in Levi's own model.
Each feature is a new event type in `refs/levi/events`, resolved against git ancestry the way closes are. It syncs
through the git and hub legs unchanged, and older `levi` builds ignore it (`materialize` drops unknown item types;
see its test `unknown_item_type_is_ignored`). No existing event, field or command changes meaning.

## What's added

- **Reviews.** `levi verify ID [--commit SHA] [-m note]` and `levi send-back ID -m "what's missing"`. A `Review`
  names the commit read (HEAD by default) and applies on checkouts whose history contains it, so a verification on
  a feature branch shows on main once that's merged. `show`, `ls` and `next` report `review` (`verified`,
  `sent_back` or `none`) with `review_source: "declared"`. They also report `review_differs`, labelled `observed`,
  when the verified commit isn't the commit the task closed at. A sent-back task that's still open stays eligible,
  and `next` gives the note as the reason. Send-back doesn't reopen anything.
- **No reviewing your own work.** A task's latest claimant, or its closer, is refused, matched on
  (dev, machine, worktree) as claims are, so a second session in another worktree on the same machine can review.
  A closer who never claimed the task can only be matched on (dev, machine), so they're warned instead, and the
  review carries `self_review_warning`. Levi's identity is self-declared, so this catches mistakes, not impostors.
- **A review gate for CI.** `levi check-claims --require-review` also fails unless each task the branch ever claimed
  has, as its newest review applying at the tested commit, a verification, and has no open read request.
- **Requested reads.** `levi request-read ID [--commit SHA]` asks for a read before a merge, and
  `levi withdraw-read ID` takes your own back. One request per requester per task: asking again replaces it. A
  later review by anyone answers it. `show` lists requests, and marks one `closed_before_read` when a close that
  applies here was made while it was open.
- **Blockers that aren't tasks.** `levi block ID -m reason` prints a `bk-` id, and `levi unblock BK -m why` closes
  it. Anyone can unblock, and the log keeps who opened and who closed each blocker. A task with an open blocker
  leaves `next`, including the recheck inside `next --claim`'s lock. `dep add` still covers a task waiting on a
  task.

## JSON

All changes are additive, on the existing schema versions:

- `levi.ls/1` and `levi.next/1` tasks gain `review`, `review_source`, `review_resolution` and `review_differs`.
  `ls` tasks also gain `blockers` (the open ones).
- `levi.show/1` gains the same review fields, plus `reviews[]`, `read_requests[]` and `blockers[]`. Each entry has
  `source: "declared"`.
- `levi.check-claims/1` tasks gain `review`, `review_source`, `review_resolution`, `review_differs` and
  `open_reads`, on every run, not only with `--require-review`. The proposal named one added field; this adds five,
  so that a CI log shows why a task would fail the gate before anyone turns it on.

## What it doesn't do

- **Prove who reviewed.** A review is a declaration by whoever's `git config user.email` it carries. That's why it's
  labelled `declared`, and why only `review_differs`, worked out from git alone, is labelled `observed`.
- **Stop a merge by itself.** The gate works only where CI runs `check-claims --require-review` as a required check,
  after fetching `refs/levi/events`.
- **Touch the hub or the dashboard.** The hub carries the new events in `LogEntry` like any other, with a test that a
  `Review` round-trips. The dashboard doesn't show reviews yet; that's a later change.
- **Bring in the rest of Rally.** Rally's hash-chained ledger, authenticated service, team state and backups stay in
  Rally. They're about the muster, or they'd fight Levi's union-merged, offline-first model. The proposal says why
  for each.

## How it was tested

- `cargo test --workspace`: Levi's 125 tests, plus the new ones. There are unit tests in `levi-core` for replay,
  resolution, ranking, the self-review rule and the gate. There are CLI integration tests against scripted repos
  in `levi/tests/cli_reviews.rs` (7), `cli_reads.rs` (4) and `cli_blockers.rs` (3), using a second git worktree as
  the second reader. 167 pass at `8fff9ea`.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check` pass. The `levi-dash` wasm
  check and clippy are run on Murph.

## Commits

Conventional commits, with `feat(core):` for the core and `feat:` for the CLI. Following Levi's `CLAUDE.md`, and the
operator's ruling for this PR, they carry no AI attribution. The stack has merge commits where the two builders'
branches met. Whether to squash or rebase it into a linear history before the PR is for the operator to say.
