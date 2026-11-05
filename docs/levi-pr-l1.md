# Levi PR L1: the description

The description for the first pull request to ignition-is-go/levi, from `ral47/upstream` at `a4add05`, on Levi
main `aed6027` (v0.9.5). It's built from [levi-writeup.md](levi-writeup.md) and the "Built: RAL-47's review stack"
table in [rally-vs-levi.md](rally-vs-levi.md). Everything below the line is pasted into the PR as it stands.
It carries no attribution line, by Levi's rule. Whether and when it's opened is for the operator to say.

---

## Reviews, a review gate, requested reads, and blockers that aren't tasks

Levi knows what the work is, who's on it, and whether git history shows it done. It doesn't know whether anyone
checked it. This adds review to Levi, in Levi's own model. Each feature is a new event type in
`refs/levi/events`, resolved against git ancestry the way closes are, and synced by the git and hub legs
unchanged.

### What changes if you change nothing

**Nothing**, unless you run one of the new commands or pass `--require-review` to `check-claims`.

- No existing event, field, command or default changes meaning. `next`'s ranking, `init`'s block and what `close`
  requires are untouched.
- `check-claims` without `--require-review` passes and fails exactly as before.
- Older `levi` builds ignore the new events: `materialize` skips unknown item types (its test
  `unknown_item_type_is_ignored`).
- The JSON gains fields on the existing schema versions, and loses none. A reader that ignores unknown fields sees
  no change. **One caveat:** a strict parser that refuses unknown fields would break on them, as it would on any
  new field.

### What's added

- **Reviews.** `levi verify ID [--commit SHA] [-m note]` and `levi send-back ID -m "what's missing"`. A review
  names the commit it read (HEAD by default) and applies on checkouts whose history contains that commit, so a
  verification on a feature branch shows on main once the branch is merged. `show`, `ls` and `next` report
  `review` (`verified`, `sent_back` or `none`) with `review_source: "declared"`. They also report `review_differs`,
  labelled `observed`, when the verified commit isn't the one the task closed at. A sent-back task that's still
  open stays eligible, and `next` gives the note as its reason. Send-back doesn't reopen anything.
- **No reviewing your own work.** A task's latest claimant, or its closer, is refused, matched on (dev, machine,
  worktree) as claims are, so a second session in another worktree on the same machine can review. A closer who
  never claimed the task can only be matched on (dev, machine), so that case warns instead of refusing, and the
  review carries `self_review_warning`. Identity is self-declared in Levi, so this catches mistakes, not
  impostors.
- **A review gate for CI.** `levi check-claims --require-review` also fails unless each task the branch ever
  claimed has, as its newest review applying at the tested commit, a verification of the commit it closed at or
  a later one, and has no open read request.
  - A verification older than the close, or on a parallel branch, fails as `stale_review`, since the commits in
    between were never read.
  - Where git can't say how the two commits are ordered, it fails as `partial`, so the gate never passes on a
    guess.
  - A close made with `--no-anchor` has no commit to compare, so any applying verification counts for it.
- **Requested reads.** `levi request-read ID [--commit SHA]` asks for a read before a merge, and
  `levi withdraw-read ID` takes your own request back. There's one request per requester per task: asking again
  replaces it. A later review by anyone answers it, if it read the requested commit or a later one. If git can't
  place the two commits, the request stays open. `show` lists requests, and marks one `closed_before_read` when a
  close that applies here was made while it was open.
- **Blockers that aren't tasks.** `levi block ID -m reason` prints a `bk-` id, and `levi unblock BK -m why` closes
  it. Anyone can unblock, and the log keeps who opened and who closed each blocker. A task with an open blocker
  leaves `next`, including the recheck inside `next --claim`'s lock. `dep add` still covers a task waiting on a
  task.

### JSON

All changes are additive, on the existing schema versions:

- `levi.ls/1` and `levi.next/1` tasks gain `review`, `review_source`, `review_resolution` and `review_differs`.
  `ls` tasks also gain `blockers` (the open ones).
- `levi.show/1` gains the same review fields, plus `reviews[]`, `read_requests[]` and `blockers[]`, each entry
  with `source: "declared"`.
- `levi.check-claims/1` tasks gain `review`, `review_source`, `review_resolution`, `review_differs`, `open_reads`,
  `stale_review` and `self_review_warning`. They're reported on every run, not only with `--require-review`, so a
  CI log shows why a task would fail the gate before anyone turns it on.

### The hub and the dashboard

- **This branch's `levi-hub`** unwraps the new events into queryable entities. Its test
  `review_log_entry_unwraps_into_queryable_review` sends a `Review` and reads it back.
- **An upstream `levi-hub` (v0.9.5) carries them unchanged.** We checked this with a scratch test at `aed6027`:
  the hub was sent a LogEntry wrapping a `Review`. It logged "Unknown entity type or parse error for ingest:
  Review", kept the LogEntry in its journal, so the sync's pull leg still hands it on unchanged, and went on to
  unwrap the next `Task`. It just can't answer queries about reviews. So a mixed setup, with new clients and an
  old hub, loses nothing.
- **`levi-dash`** is unchanged. It queries only the types it knows, so it doesn't show reviews, except as a
  `Review` line in its activity feed, which lists every LogEntry by type. Showing reviews there is a later change.

### What it doesn't do

- **Prove who reviewed.** Reviews, read requests and blockers are declared events. Anyone with push access to
  `refs/levi/events` can write one as anyone, since a review carries whatever `git config user.email` says.
  That's why they're labelled `declared`, and why only `review_differs`, worked out from git alone, is labelled
  `observed`.
- **Trust clocks.** `created` comes from each machine's clock. Skew can leave a request showing open when it was
  answered, which fails safe, or put a close and a read in the wrong order.
- **Stop a merge by itself.** The gate works only where CI runs `check-claims --require-review` as a required
  check, after fetching `refs/levi/events`.

### How it was tested

- `cargo test --workspace` passes all 177 tests: Levi's 125 and 52 new.
  - **Unit tests** in `levi-core` cover replay, resolution, ranking, the self-review rule and the gate.
  - **CLI integration tests** run against scripted repos, in `levi/tests/cli_reviews.rs` (8), `cli_reads.rs` (5)
    and `cli_blockers.rs` (3), using a second git worktree as the second reader.
  - **The `self_review_warning` path** is covered by a core test only. Every test worktree shares one email and
    hostname, so an end-to-end case can't separate a closer who never claimed the task from one who did.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check` pass, and so do the
  `levi-dash` wasm check and clippy.
- Each commit passes `cargo check --workspace --all-targets`, so the series can be bisected.

### Commits

Fourteen conventional commits on `aed6027`, with `feat(core):` for the core and `feat:` for the CLI. They're
ordered by feature: reviews and the gate, then requested reads, then blockers. There are no merge commits.

| Commit | Change |
|---|---|
| `5bdf2e6` | feat(core): reviews of a task at a commit, resolved by ancestry |
| `a66d81a` | feat(core): warn rather than refuse when a reviewer matches a closer on host only |
| `f9d6da3` | feat(core): the review gate for check-claims --require-review |
| `d61994a` | feat: verify and send back a task at a commit, and gate CI on it |
| `dd3d1f0` | docs: send-back doesn't reopen, and a send-back fails the review gate |
| `45f45f6` | test: isolate the review condition in check-claims --require-review |
| `70bf493` | feat: the review gate requires a read at or after the close |
| `d59675f` | feat(core): requested reads, answered by a later review |
| `7186b7e` | feat(core): an open read request fails the review gate |
| `116c028` | feat: request a read of a task before it merges, or withdraw it |
| `371d3a3` | feat: a read request is answered only by a read of that commit or later |
| `b69df15` | feat(core): blockers that aren't tasks |
| `b772bb0` | feat: block a task on something that isn't a task, and unblock it with why |
| `a4add05` | test: compare next's ids to a vec, for clippy |
