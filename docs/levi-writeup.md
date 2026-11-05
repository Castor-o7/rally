# Rally and Levi: the write-up for the PR (RAL-48, G7)

By giddy-geyser and chartreuse-meerkat, the Rally team, 2026-11-04. This is written to paste into the description
of an eventual PR to ignition-is-go/levi, which waits on the operator's word. The work is on the Rally team's
shared Levi remote (`murph:git/levi.git`), stacked on upstream main `56aceb2` (v0.9.4 plus two commits). The
comparison and proposal behind it are in [levi-proposal.md](levi-proposal.md). The watchman, grand-photon, agreed
the proposal, and each half was read by the other builder.

The watchman approved the stack (RAL-47) at `baec03a`. It read the core in full, and the CLI through the two
builders' reads of each other's halves.


**The branch for the PR** is `ral47/linear` on `murph:git/levi.git` at `44c886b`: a linear rebuild, by
chartreuse-meerkat, of the stack the watchman approved. The working branches with merge commits are left as they
are. Which one goes up, and when, is for the operator to say.
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
  has, as its newest review applying at the tested commit, a verification of the commit it closed at or a later
  one, and has no open read request. A verification strictly older than the close, or on a parallel branch, fails
  (`stale_review`), since the commits in between were never read. Where git can't say how the two commits are
  ordered, it fails as `partial`, so the gate never passes on a guess. A close made with `--no-anchor` has no
  commit to compare, so for it any applying verification counts.
- **Requested reads.** `levi request-read ID [--commit SHA]` asks for a read before a merge, and
  `levi withdraw-read ID` takes your own back. One request per requester per task: asking again replaces it. A
  later review by anyone answers it, provided it read the requested commit or a later one. If git can't place the two
  commits, the request stays open. `show` lists requests, and marks one `closed_before_read` when a close that
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
- `levi.check-claims/1` tasks gain `review`, `review_source`, `review_resolution`, `review_differs`, `open_reads`,
  `stale_review` and `self_review_warning`, on every run, not only with `--require-review`, so that
  a CI log shows why a task would fail the gate before anyone turns it on.

## What it doesn't do, and its limits

- **Prove who reviewed.** Reviews, read requests and blockers are declared events. Anyone with push access to
  `refs/levi/events` can write one as anyone, since a review carries whatever `git config user.email` says. That's
  why they're labelled `declared`, and why only `review_differs`, worked out from git alone, is labelled
  `observed`.
- **Trust clocks.** `created` comes from each machine's clock. Skew can leave a request showing open when it was
  really answered, which fails safe, or put a close and a read in the wrong order.
- **Check staleness for an unanchored close.** A close made with `--no-anchor` has no commit to compare, so any
  applying verification counts for it.
- **Stop a merge by itself.** The gate works only where CI runs `check-claims --require-review` as a required check,
  after fetching `refs/levi/events`.
- **Touch the hub or the dashboard.** The hub carries the new events in `LogEntry` like any other, with a test that a
  `Review` round-trips. The dashboard doesn't show reviews yet; that's a later change.
- **Bring in the rest of Rally.** Rally's hash-chained ledger, authenticated service, team state and backups stay in
  Rally. They're about coordinating teams of agents across machines, or they'd fight Levi's union-merged, offline-first
  model.

## How it was tested

- `cargo test --workspace`: Levi's 125 tests, plus the new ones. There are unit tests in `levi-core` for replay,
  resolution, ranking, the self-review rule and the gate. There are CLI integration tests against scripted repos
  in `levi/tests/cli_reviews.rs` (8), `cli_reads.rs` (5) and `cli_blockers.rs` (3), using a second git worktree as
  the second reader. 177 pass at `baec03a` (Levi's 125 and 52 new), on two machines. The `self_review_warning` path is covered by a core
  test only: every test worktree shares one email and hostname, so an end-to-end case can't separate a closer who
  never claimed the task from one who did.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check` pass. The `levi-dash` wasm
  check and clippy pass too.

## Commits

Fourteen conventional commits on `56aceb2`, with `feat(core):` for the core and `feat:` for the CLI, ordered by
feature: reviews and the gate, then requested reads, then blockers. There are no merge commits, and the tree is
identical to the reviewed stack (`baec03a`, tree `d78a6d0`). Each commit passes `cargo check`, and the last passes
the full suite.

| Commit | Change |
|---|---|
| `74cd68b` | feat(core): reviews of a task at a commit, resolved by ancestry |
| `7cefcf8` | feat(core): warn rather than refuse when a reviewer matches a closer on host only |
| `7f5b4dc` | feat(core): the review gate for check-claims --require-review |
| `a58b795` | feat: verify and send back a task at a commit, and gate CI on it |
| `3e37578` | docs: send-back doesn't reopen, and a send-back fails the review gate |
| `bd1d1f3` | test: isolate the review condition in check-claims --require-review |
| `e1da8ed` | feat: the review gate requires a read at or after the close |
| `7d8026f` | feat(core): requested reads, answered by a later review |
| `7adc8bc` | feat(core): an open read request fails the review gate |
| `ee5561b` | feat: request a read of a task before it merges, or withdraw it |
| `ca54b8e` | feat: a read request is answered only by a read of that commit or later |
| `65c126a` | feat(core): blockers that aren't tasks |
| `e5dcca5` | feat: block a task on something that isn't a task, and unblock it with why |
| `44c886b` | test: compare next's ids to a vec, for clippy |
