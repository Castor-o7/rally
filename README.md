# Rally

Rally is the ledger of progress for a muster of agents: what work exists, who has it, how
far along it is, and what evidence backs that up. It lets a watchman see that a team has
stalled, without polling the team or trusting a hand-kept table.

**Status: early, not deployed.** The record format, story states, team states, replay and a service with
access control exist. The ledger is kept as [myko](https://github.com/ignition-is-go/myko) entities in Postgres,
behind Rally's own service, which is the only way in. See [docs/service.md](docs/service.md) for setting it up
and for what a record proves about who wrote it.
Work follows the [roadmap](docs/roadmap.md).

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

## Licence

AGPL-3.0-or-later, matching [myko](https://github.com/ignition-is-go/myko), which Rally builds on. See
[LICENSE](LICENSE).
