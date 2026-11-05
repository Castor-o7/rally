# Rally and Levi: additive features vs conflicts

[Levi](https://github.com/ignition-is-go/levi) is ignition-is-go's agent-first issue tracker. Rally is this
project's ledger of verification, evidence and team state. This page sorts Rally's features by what they
would do to a Levi user who changes nothing. It's the Rally counterpart of "Additive features vs conflicts" in
Muster's `docs/muster-vs-marshal.md`, and it feeds RAL-46 (what moves into Levi) and RAL-48 (the PR text).

The rule, from the operator: **no PR of ours to ignition-is-go may change their workflow** unless they choose
it. So:

- **Additive:** new things nobody existing runs, or opt-in only. Their workflow is unchanged.
- **Default change:** different behaviour for a Levi user who changes nothing.
- **Conflict:** the same concept, file or command as Levi, where the two would disagree.

What Levi does here is from [levi-field-guide.md](levi-field-guide.md): Levi `56aceb2` (v0.9.4), built and
run on Murph, and its source. Where a point was checked again for this page, it says so. What Rally does is
from Rally `f2cac4c`.

## Where they stand today

Rally doesn't read or write Levi yet. RAL-40 is the story for reading it: Rally would read Levi from a
checkout with `levi --json`, write nothing to it, and label each fact by how it's known. A task closed at a
commit that's an ancestor of the branch's history is observed. A task's text and claims are declared, and
untrusted. Rally wouldn't trust a Levi hub, which by its own startup warning has no authentication.

Nothing here has been sent to ignition-is-go. Phase 5's stories (RAL-46 to RAL-48) build on a local checkout
first, and nothing goes upstream until the operator says so. **RAL-47 has already built Rally's review half for
Levi**: branch `ral47/upstream` at `a4add05` on the Rally team's Levi remote (`murph:git/levi.git`), 14 commits
on Levi v0.9.5 (`aed6027`), verified by the watchman and written up for the PR in
[levi-writeup.md](levi-writeup.md). It's built and not sent.

## Conflicts

These are the same concepts in both, and each would need a decision before anything moves.

| Concept | Rally | Levi | The conflict |
|---|---|---|---|
| **Claims** | `rally claim` and `release` on a ready or sent-back story. The service checks the holder on `submit`. | `Claim` events, `levi next --claim`, `start`, `steal`, `drop`. Advisory, keyed to (dev, machine, worktree), with a 24 h TTL. Atomic on one machine only. `check-claims` in CI keeps a branch answerable for every task ever claimed on it. | Two sources of truth. The operator ruled that **claims live in Levi** (RAL-40), so Rally's claim commands would read Levi's or go, for projects on Levi. What happens to Rally's holder check, and to projects not on Levi, is still RAL-40's to settle. |
| **Stories and tasks** | Stories are IDs in each project's own `roadmap.md` (RAL-nn), whose team owns the text. Rally records their state and evidence, not their text. | `Task` events in `refs/levi/events`, with short ids `lv-xxxx`, a priority, labels and a body. | The same unit of work in two stores, with two id schemes. Mapping one to the other is for RAL-40; a Rally story could name a Levi task. |
| **`next`** | `rally next` (RAL-56): your team's own sent-back work, then shelved stories the operator has answered, then the first free story in roadmap order. Read-only. | `levi next`: open here, every blocker closed here, no live claim by another; ranked by priority, then how many tasks it unblocks, then age. `--claim` takes it. | Same name, different ranking. Changing `levi next`'s order would be a **default change** for every Levi user. |
| **`init`** | `rally init` (RAL-49) writes a `<!-- rally:begin -->` block into `CLAUDE.md` and `AGENTS.md`, whichever exist, or a new `CLAUDE.md`. | `levi init` writes a `<!-- levi:begin -->` block into `AGENTS.md` or `CLAUDE.md`. | The files are shared, but the blocks are separate and each tool replaces only its own, so the files don't clash. The **instructions** can: Levi's tells an agent to pick work with `levi next --claim --json` (checked at `56aceb2`), and Rally's tells it to `rally claim`. In one repo they'd need to agree, which is the claims question again. |
| **The hub on myko** | Rally's service is a myko server (myko 6.9.0) with Postgres, per-machine pairing tokens and a hash-chained ledger, written only through its own commands. | `levi-hub` is a plain myko CellServer (myko 6.5.1) with no git and no authentication. | Same framework, opposite trust. Rally wouldn't serve or trust a Levi hub, and a Levi hub couldn't hold Rally's records without the authentication it lacks. They'd stay separate servers. |
| **Blockers** | A team declares what it's waiting on, with a reason (`block`, `unblock`, `blockers`), including questions to the operator. | Upstream: `Dependency` events between tasks, across projects with `via`. RAL-47 adds blockers that aren't tasks: `levi block ID -m reason` and `levi unblock`, and a task with an open blocker leaves `next`. | With RAL-47, Levi has both kinds, so a project on Levi could keep its task-level blockers there. What stays Rally's is the team-level declaration (a team blocked, not one task) and questions to the operator with their shelving window. Which one a team uses for a task's blocker is for RAL-40 to settle, like claims. |
| **Identity** | A credential per machine, from pairing, plus a declared name. Authors show as "a1 credential, declaring gleaming-tench". | `dev` is the git `user.email`, and `machine` the hostname, both self-declared. | Rally's `source` is stronger than anything Levi records. A Levi fact about who did something is declared, and Rally would label it so. |

## What could be offered to Levi as additive PRs

### Built: RAL-47's review stack

The first candidate is built. `ral47/upstream` (`a4add05`, on v0.9.5) adds reviews, a review gate, requested
reads and non-task blockers, each as new event types in `refs/levi/events`. What each part does to a Levi user
who changes nothing:

| Part | Additive? | Effect on a user who changes nothing |
|---|---|---|
| `Review` events; `levi verify` and `send-back` | additive | none until someone runs them. A task's own fields don't change. |
| `check-claims --require-review`, the review gate | additive: a flag | without the flag, `check-claims` passes and fails exactly as before. It does report the new review fields on every run, so a CI log shows what the gate would say before anyone turns it on. |
| Requested reads: `request-read`, `withdraw-read` | additive | none until someone asks for a read. |
| Non-task blockers: `block`, `unblock` | additive | none until someone blocks a task. A blocked task then leaves `next`, which is the point. |
| New JSON fields on `ls`, `next`, `show` and `check-claims` (`review`, `blockers`, and so on) | additive fields, on the same schema versions | a reader that ignores unknown fields sees no change. A strict parser that refuses unknown fields would break, as with any new field. |
| `next`'s reason text for a sent-back task | only with the new events | unchanged unless a send-back exists. |
| An older `levi` | unaffected | its materializer skips unknown event types ("forward compatibility with newer levi", with a test, checked at `56aceb2`). |
| An upstream `levi-hub` (v0.9.5) | carries them | **checked for this page**: a scratch test at `aed6027` sent the hub a LogEntry wrapping a `Review`. The hub logged "Unknown entity type or parse error for ingest: Review", kept the LogEntry in its journal (so the sync's pull leg still hands it on unchanged), and went on to unwrap the next `Task`. It just can't answer queries about reviews. The branch's own hub does: its test `review_log_entry_unwraps_into_queryable_review`. |
| `levi-dash` | unaffected | RAL-47 doesn't change it, and it queries only the types it knows (tasks, claims, comments, statuses, dependencies, facts, projects). Its activity feed lists every LogEntry by type name, so a review shows there as a `Review` line. It doesn't show reviews otherwise; the write-up calls that a later change. |

So the stack meets the rule as it stands: nothing in it changes what an existing Levi command does for a user
who doesn't run the new ones or pass `--require-review`.

### Not built

Each of these is new to Levi and would run only for someone who uses it.

| Feature | How it would fit Levi | Why it's additive |
|---|---|---|
| **A tamper check on `refs/levi/events`** (from `rally check --expect`) | A command that takes a commit saved earlier and fails unless the events ref still contains it. Levi only adds events and merges by union, so a saved commit should stay in its history. | A new command nobody runs. It would catch a ref that was reset or rewritten, which Levi doesn't check today. It can't stop someone with push access adding events as anyone, because Levi has no authentication. |
| **Record `check-claims` results as evidence** | Rally-side only: a submission could carry a `levi.check-claims/1` result as an observed fact. | Nothing in Levi changes. |
| **Reading `levi --json`** (RAL-40) | Rally-side only, read-only, from a checkout. | Nothing in Levi changes. Levi's read commands already take `--json` with a `schema` field. |
| **The session-start summary** (`rally session-start`) | A `levi session-start` that prints what a session should know: its claims, and `next`. | Only someone who wires it into a hook runs it. |

A PR that touches `levi next`'s ranking, `levi init`'s block, or what `close` requires (a verification first,
say) would be a **default change**, and would have to be opt-in (a flag or a config key) to meet the rule.

## What stays Rally-only

These depend on things Levi doesn't have, and don't fit its model of events in a git ref:

- **The service and its trust:** pairing and per-machine tokens, the Postgres ledger with its hash chain and
  head file, `backup`, `restore` and `adopt-head`.
- **Team state and the track record:** `team-state`, `teams`, `record`, `annotate` and `track-record`. Levi
  has no teams.
- **The operator's role:** the signed designation of the watchman (`designate`, `designation`, RAL-51), the
  watchman's re-reads after a handover (`handover`, RAL-52), and questions to the operator with a shelving
  window (`shelve`, RAL-56).
- **Corrections** (`correct`): Rally keeps the original record and annotates it. Levi's last-writer-wins would
  replace it.
- **Rally's viewer** (`rally web`). `levi-dash` already covers Levi's own views.
