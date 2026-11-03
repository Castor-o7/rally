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

## Rally and Muster

[Muster](https://github.com/Castor-o7/muster) is how agents securely reach and coordinate
with each other. Rally tracks their work.

| | Muster | Rally |
|---|---|---|
| **Question it answers** | Can I reach this agent, and is it alive? | What is it working on, and how far along is it? |
| **Owns** | Messages, wake-ups, identity, roster, whether a session can be woken | Claims, work states, evidence, check-ins, history |

Rally depends on Muster, and Muster never depends on Rally. Muster has to work the same with
Rally down.

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
