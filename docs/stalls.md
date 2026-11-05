# Stalls (RAL-7)

`rally stalls` gives the watchman prompts from Muster's roster and Rally's records: each row of the design's stall
table that holds for a team (`docs/design.md`, "Stalls"), and whether every team is waiting at once. It's
read-only and sends nothing. Every row is a prompt for the watchman, never a conclusion.

```
rally stalls [--projects projects.json] [--quiet-after 20m]
```

Teams come from the projects list, as for `rally wake` (`docs/wake.md`): each project's `team`, and the Muster
project names or session nicknames in `sessions` that are that team's sessions on the roster.

## The rows

| Row | When it holds | Prompt |
|---|---|---|
| `cant_be_woken_with_claim` | The team holds a claimed or sent-back story, and none of its sessions shows a way to wake it (`wake: none`), or it has no session on the roster | Agent may be gone: ask, then reassign |
| `missed_check_in` | Its sessions are all idle and one can be woken, and every claim has gone without a record for `--quiet-after` | Possibly stuck: ask the team |
| `declared_blocked` | Its declared state is `blocked`, or it raised an open blocker | Real blocker: help or escalate |
| `out_of_work` | Idle and wakeable, no claim, a story ready by `rally next`'s rule, and not resting | Out of assigned work: offer more |
| `every_team_waiting` | No team is working, and the ledger has had no record for `--quiet-after`. Each team's row names what it waits on | Say so to the operator |

The last row is the stall of 2026-10-30, the watchman's finding 21: "The process can stall with everyone behaving
correctly". One team was holding on an operator decision, one was waiting on the watchman's signal, one had no
claimable story, and the watchman wasn't polling, by design. No single team was a stall, so this row looks at all of
them together.

A team is working when one of its sessions declares an activity other than `idle`, or when one of its claims has a
record inside `--quiet-after`. A team that's declared itself `resting`, with no claim, isn't flagged: resting after
a story is normal under the board process.

## Labels

Muster's facts keep Muster's own labels. `wake: waiting` is observed, and means "probably wakeable". Every other
wake value, and `activity`, are declared. `wake: none` means no sign that a message would wake the session, not that
it's gone. Idleness is judged by a session's declared activity, since a busy session can be waiting for messages
too. Only a session that declares no activity counts as idle on `wake: waiting` alone. Rally's facts (claims, states,
blockers) are declared. Without `muster-shim`, or if its roster can't be read, only `declared_blocked` is shown, since
the other rows need the roster.

## The check-in length

How long a check-in lasts hasn't been agreed with the operator. `--quiet-after` is the same setting as
`rally wake --quiet-after` (`RALLY_WAKE_QUIET_AFTER`), and has no default. Without it, `missed_check_in` and
`every_team_waiting` aren't looked for, and the report says why under `check_in`.
