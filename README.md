# Rally

Rally is the ledger of progress for a muster of agents: what work exists, who has it, how
far along it is, and what evidence backs that up. It lets a watchman see that a team has
stalled, without polling the team or trusting a hand-kept table.

What Rally gives a team, each part built and verified by the watchman:

- **A record that proves it hasn't been rewritten.** The ledger is hash-chained, and `rally check` reports any
  altered, removed or reordered record, including the newest ones being dropped (RAL-14).
- **Every record says how it's known:** observed, declared or verified, with its source and time, in a
  versioned format that refuses a record missing any of them (RAL-11).
- **Evidence attached to claims.** A team claims a story and submits it with evidence such as a commit;
  invalid state changes are refused, and every change keeps its history, corrections included (RAL-3).
- **Verification tied to a commit.** A verification names the full hash it read (RAL-31), a merge is recorded
  as a commit in any state without saying who merged (RAL-33), and when the two differ Rally says so, as an
  observation rather than anyone's claim. A requested read stays on the story until it's answered or
  withdrawn, and a merge made while one is open is marked on the request (RAL-32).
- **The same state every time.** Replay reads no clock or outside system, so the same records always rebuild
  the same state, as of any past record (RAL-12).
- **An access-controlled service with the ledger in Postgres.** Pinned TLS and a token per machine; every route
  refuses an unauthorized read or write (RAL-13). The ledger is kept as [myko](https://github.com/ignition-is-go/myko)
  entities in Postgres, behind Rally's service, which is the only way in (RAL-39). It refuses writes before the
  disk fills (RAL-36). See [docs/service.md](docs/service.md) for setting it up and for what a record proves
  about who wrote it.
- **A read-only API and viewer.** The API can't change the ledger (RAL-25), and the viewer shows each roadmap's
  progress, labelled with what a tick means there, never merged with what the ledger has verified (RAL-37).
- **Untrusted text kept out of terminals and pages.** Text agents write has length limits, is marked untrusted
  wherever an agent reads it, and is shown only as text, never as markup or terminal control (RAL-16).

## Where Rally came from

[Muster](https://github.com/Castor-o7/muster) lets Claude Code sessions find each other and talk, on one
machine or across the operator's network: a message wakes an idle session, so teams of agents can ask each
other for help, hand off work and report a story ready for review. It's built on myko, in Rust, with one daemon
serving every paired machine over pinned TLS.

Rally came out of using it. Once several teams were working through roadmaps over Muster, the work itself was
tracked on a hand-kept board (Muster's `docs/board.md`), and the stories for doing better were on Muster's own
roadmap: a native board (MUS-23), showing whether a team is available, working or blocked (MUS-28), and a
ledger design (MUS-32). The operator decided on 2026-11-02 that tracking work is a different job from carrying
messages, and split it out. Those stories continue here as RAL-3, RAL-2 and RAL-1, and Muster keeps only the
facts it can observe about a session, such as whether it can be woken (MUS-44). See
[docs/handoff.md](docs/handoff.md).

## Rally and Muster

[Muster](https://github.com/Castor-o7/muster) is how agents securely reach and coordinate
with each other. Rally tracks their work.

| | Muster | Rally |
|---|---|---|
| **Question it answers** | Can I reach this agent, and is it alive? | What is it working on, and how far along is it? |
| **Owns** | Messages, wake-ups, identity, roster, whether a session can be woken | Claims, work states, evidence, check-ins, history |

Rally depends on Muster, and Muster never depends on Rally. Muster has to work the same with
Rally down.

## Rally and Levi

[Levi](https://github.com/ignition-is-go/levi) is an agent-first issue tracker whose events travel with a
repository, in `refs/levi/events`, so a task's status follows from git history. The two are meant to split the
work like this:

- **Levi says what the work is**, and who has claimed it. Claims live in Levi, on the operator's word, so there's
  one source of truth for them.
- **Rally records what happened to it:** evidence, verification by someone other than the author, team state,
  and a history that can't be quietly rewritten.

Rally will read Levi from a checkout with `levi --json`, write nothing to it, and label each fact by how it's
known: a task closed at a commit that's in the branch's history is observed, while a task's text and claims are
declared, and treated as untrusted. This is next, as RAL-40, and what happens to Rally's own claim commands
and to projects not on Levi is still to settle. See "Levi" in [docs/design.md](docs/design.md).

## What this gives the operator

- **Coordination without polling.** The aim is that the watchman reads which teams are working, blocked or
  quiet, and what they've submitted, from the ledger, instead of messaging each team to ask, and that a possible
  stall is flagged for the watchman to take up with the team. Stories and submissions are recorded today; team
  states (RAL-2), the live view (RAL-27) and stall flags (RAL-7) aren't verified yet.
- **Progress you can recompute.** The analysis is as deterministic as the facts allow. Replaying the ledger
  always gives the same state, and `rally check` shows it hasn't been rewritten. Whether a state change was
  valid, or a commit exists, can be decided by a program. Whether work meets its "done when" line can't, so
  Rally records who judged it and on what evidence. Every figure says which kind it is, observed, declared or
  verified, and they're never blended into one score.
- **One view across projects.** The viewer shows each roadmap's progress beside what the ledger has verified,
  kept as separate figures, with where each came from and how fresh it is.

## Principles

- **Say how each thing is known.** Every record is observed, declared or verified.
- **Inform; don't command.** Rally flags a possible stall. The watchman decides what to do.
- **Authors don't verify their own work.**
- **No points, scores or weights.** The track record is plain facts, so that nothing rewards
  hiding a failure.

## Start here

- [docs/handoff.md](docs/handoff.md): for the team taking this on.
- [docs/design.md](docs/design.md): the working design and its open questions.
- [docs/roadmap.md](docs/roadmap.md): the stories.
- [docs/deploy-murph.md](docs/deploy-murph.md): deploying on Murph, in one sitting, once the operator says so.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The tests need Postgres, and fail without it rather than skip. By default each test process starts a scratch
cluster of its own with `initdb` and `pg_ctl` (found in `/opt/homebrew/bin`, `/usr/lib/postgresql/*/bin`, on
`PATH`, or in `RALLY_TEST_PG_BIN`), in a new temporary directory, on 127.0.0.1 only, and removes it afterwards. To
use a server instead, as CI does, set `RALLY_TEST_POSTGRES_URL` to an administration database on it, such as
`postgres://postgres@127.0.0.1:54329/postgres`; each test makes and drops a database of its own there. Tests
refuse port 5432, so they can't meet a Postgres someone is using.

## Status

Rally runs on the operator's machines and isn't deployed yet. Work follows the [roadmap](docs/roadmap.md). Next:

- load limits on the service (RAL-35), required before the first trial;
- the viewer with live ledger data (RAL-27);
- deployment on Murph, once Postgres is there.

## Licence

AGPL-3.0-or-later, matching [myko](https://github.com/ignition-is-go/myko), which Rally builds on. See
[LICENSE](LICENSE).
