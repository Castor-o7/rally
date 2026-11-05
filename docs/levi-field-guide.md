# Levi field guide

**G6 groundwork, 2026-11-04.** This guide comes from building [ignition-is-go/levi](https://github.com/ignition-is-go/levi)
at `56aceb2` (v0.9.4) on Murph and running it in a scratch repo, together with reading its source. It describes
what levi does. It doesn't propose anything; giddy-geyser's comparison and proposal does that. Where this guide
says "seen", the behaviour was observed in the run. Where it says "source", it comes from reading the code.
Identities in the output below are shortened to placeholders.

## Build

- `cargo build --release` builds `levi` and `levi-hub` in about a minute on Murph, with no changes needed. The
  workspace's default members leave out `levi-dash`.
- `levi-dash` is a Leptos CSR app built to wasm with `trunk build` in `levi-dash/`. It needs `trunk`
  (`cargo install --locked trunk`) and the `wasm32-unknown-unknown` target. Both installed and built cleanly.
- The licence is AGPL-3.0-or-later, the same as Rally. It's built on myko 6.5.1. Rally's own myko dependency
  is noted in `design.md`.

## Data model

### Storage: `refs/levi/events`

From the source (`levi/src/store.rs`), confirmed by `git ls-tree` in the run:

- Each event is one **CBOR blob**. Its git blob OID is the event id, so events are content-addressed and
  immutable.
- Each commit on `refs/levi/events` holds a tree of **every** event, sharded by OID prefix (`ab/cdef…`, like
  `.git/objects`). The commit message is `levi: N event(s)`.
- An append writes the blobs, a new tree and a commit, then moves the ref by compare-and-swap, holding an
  `flock` because gix doesn't revalidate after a contended ref lock. On a race it re-reads, takes the union and
  retries.
- Two histories merge by taking the union of their trees, so syncing never conflicts. The git leg fetches into
  `refs/levi/remotes/<remote>/events` and merges from there (seen).
- Nothing touches the working tree. The only file `levi init` adds is `AGENTS.md` or `CLAUDE.md`, with a
  `<!-- levi:begin -->` block. `.levi/config.toml` is added only when it's given `--hub` (seen).

### The event envelope

Each blob is a myko `MEvent`, CBOR-encoded. One decoded `Project` event (seen):

```
{ item: { created, id, name }, changeType: "SET", itemType: "Project",
  createdAt: "2026-11-05T02:32:15.841545Z", tx: <uuid>, sourceId: "Murph" }
```

`changeType` is `SET` or `DEL`. `DEL` removes an entity: a claim on `drop`, a claim released by `close`, and
a dependency on `dep rm`. State is
rebuilt by replaying every event, sorted by `(createdAt, event id)`, as last-writer-wins per entity
(`levi-core/src/materialize.rs`).

### Event types (`itemType`)

From `levi-core/src/entities/`. Every entity also has an `id`.

| Type | Fields | Notes |
|---|---|---|
| `Project` | `name`, `created` | A minted 32-hex id. `init` adopts the project from the remote if one exists. |
| `Task` | `project_id`, `title`, `body`, `priority` (P0–P3, default P2), `labels`, `created_by_dev`, `created_by_machine`, `created` | Short ids are `lv-` plus the first 4 hex of the id. |
| `StatusChange` | `task_id`, `to_status` (`Closed` or `Reopened`), `anchor_commit` (optional), `created`, `by_dev`, `by_machine` | Status is a fold over these. See "Resolution". |
| `Claim` | `task_id`, `dev`, `machine`, `machine_id`, `worktree`, `git_ref`, `created`, `ttl_secs` | Its id is the task id, so there's one claim per task and the newest wins. |
| `Dependency` | `blocker_task_id`, `blocked_task_id`, `blocker_project_id`, `blocker_ref`, `via` (optional) | The id is `blocker->blocked`, or `proj/blocker->blocked` across projects. |
| `Comment` | `task_id`, `body`, `by_dev`, `created` | |
| `CommitFact` | `project_id`, `parents`, `patch_id` | The commit graph, published so a git-free hub can work out ancestry. Its id is the sha. |
| `RefFact` | `project_id`, `branch`, `head`, `observed` | Branch heads, for the dashboard's branch selector. |
| `LogEntry` | `project_id`, `cbor_b64`, `created` | On the hub only. It wraps an original event's bytes so they can be pulled back unchanged. |
| `Applied` | `event_created`, `event_oid` | On the hub only. It records the last-writer-wins order, so a late event can't overwrite newer state. |

Identity: `dev` is the git `user.email`, and `machine` is the hostname. `machine_id` is a UUID minted once
into `~/.local/state/levi/machine-id`, because hostnames collide. `worktree` is the checkout's absolute path
(seen).

### Machine-local state

`~/.local/state/levi/` (or `LEVI_STATE_DIR`) holds `machine-id` and `checkouts.toml`, a registry of every
checkout levi has run in, keyed by project id (seen). That registry is how a cross-project blocker resolves
from a sibling checkout on the same machine. `.git/levi/foreign-status.toml` caches the statuses of foreign
blockers. None of this is synced.

## Resolution: status is a function of git

From `levi-core/src/resolve.rs`, and seen:

- A task's status **on a checkout** is the fold of its `StatusChange`s, in `(at, id)` order, **counting only
  those whose `anchor_commit` is an ancestor of HEAD**. An unanchored change (`--no-anchor`) applies
  everywhere. With no qualifying changes, the task is open.
- `close` anchors at HEAD by default. In the run, a task closed on branch `fix` showed `closed` on `fix`. On
  `main` it was still `open`, with `closed_elsewhere: [{anchor, branches: ["fix"], by, at}]`, and `levi next`
  said "fix exists on fix @ fca48ef8 — consider merging it". After a fast-forward merge it showed `closed` on
  `main`.
- `resolution` grades how ancestry was answered: `exact` comes from the real repo (gix merge-base), `facts`
  from walking `CommitFact`s, and `squashed` from a patch-id match after a squash, rebase or cherry-pick (an
  inferred close, labelled as such). `partial` means some anchor couldn't be resolved, so the change was
  skipped and the task treated as open: "never silently closed, never silently not-closed".
- `show --json` gives `history[]` with `applies_here: yes|no|…` for each change (seen).

## CLI

The commands are `init`, `project rename`, `add`, `ls`, `show`, `next`, `start`, `steal`, `drop`,
`check-claims`, `close`, `reopen`, `dep`, `comment`, `edit`, `sync` and `watch`. `--no-sync` skips the
background sync that every mutating command fires.

**The read commands take `--json`, and the mutating ones mostly don't:** `close --json` was refused
("unexpected argument"), and `close` prints `lv-584c closed @ fca48ef8`. `add --json` works.

### JSON shapes (seen, each with a `schema` field)

- `levi.add/1`: `{id, short, schema}`
- `levi.ls/1`: `{schema, resolution_mode: "head", tasks: [Task]}`
- `levi.next/1`: `{schema, tasks: [Task + reason, unblocks]}`, for example `reason: "P0; unblocks 1 open task;
  ranked 1st of 1 eligible"`
- `levi.show/1`: Task fields plus `body`, `blocked_by[]`, `blocks[]` (each `{id, short, status}`),
  `comments[]` and `history[]` (each `{to, anchor, applies_here, at, by}`)
- `levi.check-claims/1`: `{schema, git_ref, ok, tested_commit, tasks: [{id, short, title, status,
  resolution}]}`. It exits 1 when `ok` is false.

A Task in `ls` and `next` looks like this:

```json
{ "id": "584c6bc5531f43f68e3ec11e677fc3a8", "short": "lv-584c", "title": "fix flux capacitor",
  "status": "open", "priority": "P0", "labels": ["engine"], "resolution": "exact",
  "created_at": "…Z", "created_by": "<git user.email>",
  "claim": { "dev": "<git user.email>", "machine": "Murph", "worktree": "/path/to/checkout",
             "git_ref": "refs/heads/fix", "at": "…Z", "ttl_secs": 86400 },
  "closed_elsewhere": [] }
```

## Claims

From `levi/src/commands/claim_ops.rs` and `levi-core/src/rank.rs`, and seen:

- Claims are **advisory**, keyed to (dev, machine, worktree), and expire after `ttl_secs` (24h by default,
  `[claim] ttl_secs`). A claim records the current full branch ref, so it needs a symbolic HEAD.
- `next --claim` and `start` check and append in one critical section under the store's flock, so parallel
  agents **on one machine** never get the same task. Across machines there's no lock. Two machines could
  claim the same task, and the newest event wins once they sync.
- `steal` takes a task over unconditionally. `drop` releases only your own claim. `close` released the claim
  in the run (`claim: null` afterwards).
- `next` ranks eligible tasks (open here, every blocker closed here, no live claim by someone else) by
  priority, then the number of tasks it transitively unblocks, then age, then id.
- **Branch responsibility outlives the claim.** `check-claims --git-ref B` fails unless every task *ever*
  claimed on `B` is closed at the tested commit. In the run, `lv-584c` was claimed on `main`, dropped, claimed again
  on `fix` and closed there. `check-claims --git-ref main` listed it as open before the merge (`--at HEAD~1`) and
  closed after it, and kept failing on `lv-4ef4`, which was claimed on `main` and is still open. Every branch a task was ever claimed on keeps
  the obligation, so claiming it again elsewhere doesn't transfer it. CI has to fetch `refs/levi/events` first. The repo ships a GitHub Action for this.

## levi-hub

`levi-hub serve --bind <addr>` (default `0.0.0.0:7377`) is a plain myko CellServer at `ws://<bind>/myko`. It
has no git and **no authentication**: its own startup warning says anyone who can reach it can read and write
all task data (seen). Without `MYKO_POSTGRES_URL` it keeps everything in memory (seen).

- The **hub leg** of `levi sync` pushes and pulls `LogEntry`s by comparing ids (bucketed hash reports
  `LogEntryBuckets` and `LogEntryBucketIds`), then publishes `CommitFact`s and `RefFact`s. In the run it
  printed `hub: pushed 12, pulled 0 event(s), published 4 fact(s)`.
- The hub saga `UnwrapLogEntries` and the command `ApplyLogEntry` unwrap each entry into its real entity,
  guarded by `Applied` for last-writer-wins, so queries and the dashboard see Tasks and other entities.
- The report `IssueGraphReport` serves the cross-project blocking graph to the dashboard.
- `levi watch` streams live events. In the run it printed `Task SET "from work via hub"` and `Claim SET` as
  they happened.
- `levi add --project <name>` files a task in another project through the hub. It needs that project to be
  known to the hub (`error: no project named 'otherproj' on the hub`, seen).
- The hub address lives in `.levi/config.toml`, which is meant to be committed. A clone without it can't
  `watch` (seen).

## levi-dash

This is a static wasm app that connects to `ws://<page-host>:7377/myko`, or another hub with `?hub=host:port`.
It has IDE-like splittable panes ([mullion]). Its views, from `levi-dash/src/pages/`:

- **overview**: open and closed counts, P0 alerts and a live activity feed
- **in_flight**: claims, grouped by dev, then machine, then worktree
- **browser**: any project's tasks, resolved against any branch from `RefFact`s and `CommitFact`s alone
- **issues**: the cross-project blocking graph from `IssueGraphReport`, one section per independent web

Pane layout is kept in the browser's `localStorage`.

**Seen:** `trunk build --release` (trunk 0.22.1) gave a static `dist/` with a 2.8 MB wasm. Served on loopback with
`?hub=127.0.0.1:17377`, it opened all four views live against the scratch hub with no setup. The overview showed
`work` with 3 open and 1 closed, at the main head, and an activity feed of CLAIM, TASK, STATUSCHANGE and COMMENT
events. The project browser listed the open tasks with the branch and status filters. In flight showed the git
email, then `Murph`, the worktree path and the claimed `lv-4ef4`. The blocking graph said nothing was blocked,
since `lv-4ef4`'s blocker is closed. Anyone who can load the page and reach the hub sees all of this.

[mullion]: https://github.com/ignition-is-go/mullion

## Where Rally's features could plug in

These are the points where the two meet, set beside what `design.md` says already. Choosing among them is for
the proposal.

| Rally | Levi's side of it | What it would read |
|---|---|---|
| Stories (`ready`) | `Task` | A story could name a levi task (`project_id/task_id`). A task's title and body are agent-written text. |
| Claims ("live in Levi", operator 2026-11-02) | `Claim`, `next --claim`, `start`, `steal`, `drop` | `levi show <id> --json` → `claim`. It's declared, advisory and expiring, and atomic on one machine only. |
| `merge` and observed commits (RAL-5) | `StatusChange` anchors and `closed_elsewhere` | "Closed at X, and X is an ancestor of HEAD" is an **observed** fact when `resolution` is `exact`. `squashed` and `partial` are weaker, and levi labels them as such. |
| `submit`, `verify`, `send-back` and requested reads | none | Levi has no review, evidence or verifier. Closed in levi means a commit exists, not that it was checked. |
| `block`, `unblock` and `blockers` | `Dependency` (a task waiting on a task, across projects with `via`) | Levi's blockers are between tasks. Rally's are a team's declaration with a reason. |
| Team state and the track record | none | Levi has no teams. `dev` and `machine` identify sessions only through the git email and hostname. |
| CI gate | `check-claims --git-ref` | Rally could record its `levi.check-claims/1` result as an observed fact on a submission. |
| The watchman's digest | `levi watch` and the hub | It's live, but over an unauthenticated hub. `design.md` already says not to trust one. |
| The ledger's chain and `check --expect` | `refs/levi/events` | It's content-addressed and union-only, with no single head to pin and no tamper check. A writer can add events but can't change one, and anyone with push access can add events as anyone. |

Two things worth knowing before choosing:

- **Identity is self-declared.** `dev` is whatever `git config user.email` says, and every session on one
  GitHub account shares a remote. Rally's credential-based `source` is stronger than anything levi records.
- **Reads need a checkout.** Status depends on HEAD, so Rally reading `levi --json` gets the view from the
  branch that checkout is on. To read a given branch without one means walking facts as the dashboard does,
  or using `check-claims --at REV`.
