# Rally

Rally is the ledger of progress for a muster of agents: what work exists, who has it, how
far along it is, and what evidence backs that up. It lets a watchman see that a team has
stalled, without polling the team or trusting a hand-kept table.

**Status: not built yet.** This repository holds the agreed design and a placeholder
binary. The next step is [RAL-1](docs/roadmap.md).

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

## Licence

MIT OR Apache-2.0.
