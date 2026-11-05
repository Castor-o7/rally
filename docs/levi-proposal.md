# Rally and Levi: comparison and proposal (RAL-46, G6)

By giddy-geyser, the Rally team on a1, 2026-11-04. Levi is read at upstream main `56aceb2` (v0.9.4 plus two
commits), cloned to `~/Documents/Gamoire/levi` on a1, built and installed there. Facts about Levi come from its
source at that commit and from chartreuse-meerkat's [field guide](levi-field-guide.md) (#86), which was written from
a real build and run on Murph. Facts about Rally come from `design.md` and `rally --help` at `52d9c8a`. Nothing here
has been built yet. The watchman reads this before anything is.

## The two side by side

| | Rally | Levi |
|---|---|---|
| What it is | The ledger of the muster's progress: who's on what, what was checked and by whom, and where each team stands | An issue tracker for agents: what the work is, who's on it, and whether git history shows it done |
| Where state lives | An append-only, hash-chained ledger in Postgres, behind one myko service on Murph, with a head file kept apart and backups pulled to a1 | Events as CBOR blobs in `refs/levi/events` inside each repository, union-merged, with an optional myko hub to relay them |
| Unit of work | A story (`RAL-n`) from a roadmap, with a "done when" line | A task (`lv-xxxx`) with title, body, priority and labels |
| Status | Ready, Claimed, In review, Verified, Sent back; Merged is a separate fact | Open or closed, worked out per checkout from close and reopen events anchored at commits |
| What "done" means | Someone other than the author verified it against "done when", at a named commit | The closing commit is an ancestor of HEAD |
| Claims | `claim` and `release`, though the operator ruled on 2026-11-02 that claims live in Levi | Advisory, keyed to (dev, machine, worktree), expire after 24 h, atomic on one machine |
| Review | `submit`, `verify` (naming the commit read), `send-back` (saying what's missing), `request-read`, `withdraw-read`, and `commit_differs` when what merged isn't what was read | None. A closed task wasn't necessarily checked by anyone |
| Blockers | `block` with a reason, on anything: a story, another team's story, or the operator; `unblock` with a reason; chains | `Dependency`, task on task, across projects with a `via` note; `next` skips blocked tasks |
| Teams | Declared team state (available, working, blocked) and its history; track records as plain facts | No teams; `dev` is the git email, `machine` the hostname |
| How each fact is known | Every record is labelled observed, declared or verified, with its source | `resolution` grades ancestry answers (`exact`, `facts`, `squashed`, `partial`); everything else is unlabelled |
| Identity | A per-machine credential from pairing, recorded by the service; names are declared | Self-declared git email; anyone with push access can write events as anyone |
| Integrity | `check --expect` finds removed or changed records against a head saved elsewhere | Events are content-addressed and can't be changed, but they can be added by anyone, and there's no single head to pin |
| Offline | Needs the service, with a fallback board and backfill (RAL-30) for outages | Fully offline from a clone |
| Viewer | `rally web`, a read-only loopback page of projects, roadmaps, ticks and day summaries | `levi-dash`, a wasm app over the hub: overview, claims in flight, a branch-aware browser, the blocking graph |
| CI | None of its own | `check-claims --git-ref`, and a GitHub Action |
| Licence | AGPL-3.0-or-later | AGPL-3.0-or-later |

In short, Levi knows **what the work is and whether git shows it done**, and Rally knows **whether someone
checked it, and what's holding it up**. The gap in Levi that Rally fills is review: a closed Levi task is a claim
by its closer, backed by a commit, and nothing more.

## The proposal: what moves into Levi

Each feature is a new event type, plus commands and `--json` fields, so it travels in `refs/levi/events` like
everything else, syncs through the git and hub legs unchanged, and is ignored by older `levi` builds:
`materialize` drops unknown item types, and its own test `unknown_item_type_is_ignored` covers it. No existing
event, field or JSON shape changes. Fields keep clear of `tx` and `createdAt`, per Levi's `CLAUDE.md`.

### 1. Reviews: verify and send back at a commit

From Rally's `verify` and `send-back` (RAL-31). This is the core of the proposal.

- **Event `Review`:** `project_id`, `task_id`, `verdict` (`Verified` or `SentBack`), `commit` (the full sha
  read), `note` (what was checked, or what's missing), `by_dev`, `by_machine`, `created`.
- **Commands:** `levi verify <id> --commit <sha> [-m note]` and `levi send-back <id> -m "what's missing"
  [--commit <sha>]`. `--commit` defaults to HEAD. Refused when `by_dev` and `by_machine` match the task's closer
  or its latest claimant, as Rally refuses a verifier verifying their own submission. Levi's identity is
  declared, so this stops mistakes, not impostors, and the docs will say so.
- **Resolution, the Levi way:** a review **applies here** when its commit is an ancestor of HEAD, using the same
  ancestry machinery and the same `exact`, `facts` and `squashed` grades as `StatusChange`. So a verification on
  a feature branch shows on that branch, and on main once it's merged, just as a close does.
- **Output:** `show --json` gains `reviews[]` (each with `verdict`, `commit`, `note`, `by`, `at`, `applies_here`).
  `ls` and `next` gain a `review` field per task: `verified`, `sent_back` or `none`, from the newest review that
  applies here. And, from Rally's `commit_differs`: when a task is closed here and its newest applying
  verification isn't at the close's anchor commit, `review_differs: {verified, closed}`. That's worked out from
  git alone, so it's labelled observed.
- **`next`:** a task whose newest applying review is `SentBack` and that's still open ranks as eligible again,
  with the reason "sent back: <note>".

### 2. Requested reads

From Rally's `request-read` and `withdraw-read` (RAL-32), and the muster's rule that an open request blocks a
merge.

- **Event `ReadRequest`:** `project_id`, `task_id`, `commit`, `by_dev`, `created`. Its id is the task id plus the
  requester, so one open request per requester per task. `withdraw-read` deletes it (`DEL`), as `drop` deletes a
  claim. A `Review` created after it answers it.
- **Commands:** `levi request-read <id> [--commit <sha>]` and `levi withdraw-read <id>`.
- **Output:** `show --json` lists open requests. If a close that applies here was made while a request was open,
  the request carries `closed_before_read`, Levi's equivalent of Rally's `merged_before_read`.

### 3. A review gate for CI

`check-claims` already fails a branch whose claimed tasks aren't closed at the tested commit. With
**`--require-review`**, it also fails unless each of those tasks has a `Verified` review that applies at the
tested commit and no open read request. The result stays `levi.check-claims/1` with one added field per task,
`review`, so existing readers are unaffected. This is what Rally can't do on its own: it can't stop a merge on
GitHub, but a required CI check can.

### 4. Blockers that aren't tasks

From Rally's `block`, `unblock` and `blockers` (RAL-20). Levi's `Dependency` covers a task waiting on a task. It
can't say "waiting on the operator to open port 6300", which was most of Rally's real blockers on 2026-11-04.

- **Event `Blocker`:** `project_id`, `task_id`, `reason`, `by_dev`, `created`, and when closed, `closed_by`,
  `closed_reason`, `closed_at` (a later `SET` of the same id, so last-writer-wins keeps the history in the log).
- **Commands:** `levi block <id> -m reason` and `levi unblock <blocker-id> -m why`. `next` treats a task with an
  open blocker as ineligible, with the reason. `ls --json` and `show --json` include open blockers.

### 5. Track record as plain facts

From Rally's `track-record` (RAL-21). A read command over the events above, with no new event type:
**`levi record [--dev <email>] --json`** (`levi.record/1`) gives, per dev, the verifications they gave,
the send-backs they received with each note, and the blockers they raised, as counts and dates. No scores and
no ranking, as in Rally. It's keyed by `dev` because Levi has no teams, and it says so: on a1, every session
shares one git email, so the record is per checkout unless sessions set their own.

### Order of building (RAL-47)

1, then 3 (they're the point and depend on each other), then 2, 4 and 5. Each is its own conventional commit
(`feat:`) on a branch of the local checkout, `rally/reviews`, with tests in Levi's style (CLI integration against
scripted repos, as `levi/tests/cli_git_state.rs` does), and `cargo test --workspace`, clippy with `-D warnings`
and fmt clean. Per Levi's `CLAUDE.md`, those commits carry no AI attribution. Nothing is pushed upstream.
`levi-dash` changes (a review column in the browser) are left for a later story unless the watchman wants them
now.

## What stays in Rally, and why

- **The hash-chained ledger, head file, `check --expect`, backup and restore.** Levi's integrity comes from git's
  content addressing and union merge, and that's a deliberate design. Grafting a single-head chain onto it would
  fight the union merge.
- **Credentials, pairing and the authenticated service.** Levi's hub has no authentication, by its own warning,
  and adding it is a larger change to Levi's model that's for Levi's maintainers to want. It's worth a separate
  conversation, not this branch.
- **Team state, stalls and the Muster roster.** These are about the muster, not about issues. Levi has no teams,
  and Muster is Rally's concern.
- **Outage backfill, day summaries and the watchman's writeups.** Levi is offline-first, so there's no outage to
  backfill, and the summaries are the muster's.
- **Claims.** These are already Levi's, on the operator's ruling. What happens to Rally's own `claim` and
  `release` is still RAL-40's to settle.
- **Labelling every fact observed, declared or verified.** Levi labels ancestry answers only. Building 1 and 3
  adds `verified`. Labelling every field would change every JSON shape, so it's left out.

## Questions for the watchman

1. Is the scope right: 1 and 3 as the core, with 2, 4 and 5 after?
2. Should `levi-dash` show reviews in this round?
3. The self-review refusal checks the closer and the latest claimant by (dev, machine). Is that enough, given
   Levi's declared identity?
