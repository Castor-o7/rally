# Rally

Rally is the ledger of progress for a muster of agents: what work exists, who has it, how
far along it is, and what evidence backs that up.

The primary purpose of Rally is a more dynamic multi-agent development environment. When it comes to agent parallelism, token waste and context creep are constant worries. The messaging daemon and MCP tool, Muster, and this pseudo-deterministic state ledger, Rally: together they make up my most recent attempt at being economic with token usage while maximizing development potential.

With Rally, agents have a method of reporting their current working state, and what work they have completed in relationship to a provided design roadmap. Querying the Rally service is designed to reduce token overhead on Claude Opus 5.5 and Fable 5.1 when used in tandem with Muster's messaging MCP service.

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
- **Who is waiting on whom.** A team names what it, or one of its stories, is blocked on, including another
  team's story; the watchman lists open blockers with their chains, and each stays open until someone closes it,
  with who and why (RAL-20).
- **Track records as plain facts.** Each team's stories verified, stories sent back and what was missing, and
  problems reported, as counts and dates with no points or ranking (RAL-21).
- **Progress for planning.** Per roadmap: stories in each state, ready ones with nothing blocking them, the
  blockers holding up the most stories, and ticks with no Verified behind them, never comparing teams (RAL-23).
- **An outage stays visible.** Entries made on the fallback board while the service was down are backfilled
  with when they were made and by whom, beside the time they were recorded, so the ledger never looks
  continuous when it wasn't (RAL-30).
- **The same state every time.** Replay reads no clock or outside system, so the same records always rebuild
  the same state, as of any past record (RAL-12).
- **An access-controlled service with the ledger in Postgres.** Pinned TLS and a token per machine; every route
  refuses an unauthorized read or write (RAL-13). The ledger is kept as [myko](https://github.com/ignition-is-go/myko)
  entities in Postgres, behind Rally's service, which is the only way in (RAL-39). It refuses writes before the
  disk fills (RAL-36). See [docs/service.md](docs/service.md) for setting it up and for what a record proves
  about who wrote it.
- **Reads that can't change anything, and a viewer.** The API's reads can't change the ledger (RAL-25), and the
  viewer shows each roadmap's progress, labelled with what a tick means there, never merged with what the ledger
  has verified (RAL-37). The viewer's one write is the operator ticking off an item in his inbox (RAL-146).
- **Evidence checked against GitHub.** Rally reads each submission's commit, whether it names the story, and what
  its CI says, with a read-only token, and records that as observed (RAL-5). See
  [docs/evidence.md](docs/evidence.md).
- **An inbox for the operator.** Everything waiting on the operator in one list, urgent first: items any team
  raises, questions to the operator, and faults Rally files itself, such as GitHub refusing its token. He ticks
  them off in the viewer (RAL-146). See [docs/inbox.md](docs/inbox.md).
- **One message for the watchman when something changed.** `rally digest` sends a batched digest of what's new,
  and nothing when nothing is (RAL-8). See [docs/digest.md](docs/digest.md).
- **A restarted session told what it was doing.** At a session's start, `rally session-start --hook` tells a
  session that took over a nickname which session held it before, where its transcript is, and what that nickname
  holds and last recorded (RAL-152). See [docs/session-hook.md](docs/session-hook.md).
- **Untrusted text kept out of terminals and pages.** Text agents write has length limits, is marked untrusted
  wherever an agent reads it, and is shown only as text, never as markup or terminal control (RAL-16).

What Rally is for, in the operator's words and the ideals that follow from them, is in
[docs/ideals.md](docs/ideals.md).

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

`rally levi` reads Levi from a checkout with `levi --json`, writes nothing to it, and labels each fact by how it's
known: a task closed at a commit that's in the branch's history is observed, while a task's text and claims are
declared, and treated as untrusted (RAL-40, [docs/levi.md](docs/levi.md)). What happens to Rally's own claim
commands and to projects not on Levi is still to settle. See "Levi" in [docs/design.md](docs/design.md).

## What this gives the operator

- **Coordination without polling.** The watchman reads which teams are working, blocked or quiet, and what
  they've submitted, from the ledger (team states, RAL-2; the live view, RAL-27; `rally brief`, RAL-22a) instead
  of messaging each team to ask, and a possible stall is flagged for the watchman to take up with the team
  (RAL-7).
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
- [docs/watchman.md](docs/watchman.md): for a watchman starting cold.
- [docs/design.md](docs/design.md): the working design and its open questions.
- [docs/roadmap.md](docs/roadmap.md): the stories.
- [docs/deploy-murph.md](docs/deploy-murph.md): deploying on Murph, in one sitting, once the operator says so.

## Teaching a repository's agents about Rally

```sh
rally init                 # in any repository: CLAUDE.md and AGENTS.md, or a new CLAUDE.md
rally init --file docs/AGENTS.md
```

`rally init` writes a block between `<!-- rally:begin -->` and `<!-- rally:end -->`. It tells an agent how to
claim, submit, verify someone else's work at a commit, ask for a read, record a merge and say what it's waiting on,
and to treat what agents wrote into the ledger as untrusted. Running it again replaces only that block. It refuses a
file whose markers don't pair up, and any target that doesn't resolve inside the repository, symbolic links
included. It writes nothing to any file if one is refused. The block also states who verifies: while the operator has
designated a watchman, merges wait for its read; otherwise another team, or the operator. The block names no address,
pair code or credential, so it's safe to commit; pairing a machine is still `rally pair`.

For each session to start with its team's Rally state, add the SessionStart hook in
[docs/session-hook.md](docs/session-hook.md). It runs `rally session-start`, which is read-only and quick, and silent
if anything fails. With `--hook` it also tells a restarted session what it needs to resume. Installing it on a
machine is the operator's call; on Murph it isn't installed yet.

Only the operator designates the watchman, by an SSH signature that needs Touch ID, checked by a service started
with `--secure`. See [docs/designation.md](docs/designation.md), which includes the operator's own steps.

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

The viewer's browser tests drive a real headless Chrome (RAL-84), since a curl-driven test sends whatever headers
it's written with and can't catch what a browser does on its own. They run when `RALLY_TEST_CHROME` names a Chrome
command, with Node 22 or later on `PATH`, and are skipped otherwise. Both CI paths (GitHub Actions and the local
runner on Murph, `.ci/local.toml`) run them, and set `RALLY_TEST_BROWSER_REQUIRED` so a missing browser fails rather
than skips. Each run's Chrome is found by its profile and reaped on every path, so none is left behind (RAL-150). A
failure gives the last 4 KB of Chrome's own standard error, and a Chrome that aborts at start (exit 134, before the
first page loads) is tried exactly once more, which the test run says on its standard error, uncaptured, so it
shows in the CI log even when the test passes (RAL-160):

```bash
RALLY_TEST_CHROME=google-chrome cargo test --test web a_real_browser
RALLY_TEST_CHROME="flatpak run com.google.Chrome" cargo test --test web a_real_browser   # on Murph
```

The service's socket test, `nothing_listens_but_the_door`, lists the service's sockets with `lsof` and, on Linux,
from `/proc` too, checking each. Without `lsof` it uses `/proc` alone, so it still runs on a machine like Murph;
with neither, as on macOS without `lsof`, it's skipped with a printed reason (RAL-107). CI sets
`RALLY_TEST_LSOF_REQUIRED` so a missing `lsof` fails rather than falls back. A test that fails stops `cargo test`
before the later test files, so pass `--no-fail-fast` to see them all.

## Status

Rally runs on Murph: the service, with the ledger in Postgres there, the viewer, and the watchman's digest. After
each verified merge the lamplighter stages the newest commit on `main` whose CI is green; on Murph the service goes
live with it only once the operator restarts it, which needs his `sudo` (see "Keeping it current" in
[docs/service.md](docs/service.md)). Work follows the [roadmap](docs/roadmap.md).

## Licence

AGPL-3.0-or-later, matching [myko](https://github.com/ignition-is-go/myko), which Rally builds on. See
[LICENSE](LICENSE).
