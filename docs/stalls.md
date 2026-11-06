# Stalls (RAL-7)

`rally stalls` gives the watchman prompts from Muster's roster and Rally's records: each row of the design's stall
table that holds for a team (`docs/design.md`, "Stalls"), and whether every team is waiting at once. It's
read-only and sends nothing. Every row is a prompt for the watchman, never a conclusion.

```
rally stalls [--projects projects.json] [--quiet-after 20m] [--lock /tmp/discosim-heavy.lock]
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

## A stopped lock holder (RAL-75)

From the DiscoSim rally's issues log, §4.4: on 2026-11-06 a runner shell sat stopped (state `T`) for about 57
minutes while it held the one-heavy-run lock, so no sweep ran and the queue behind it waited. Its status still said
"sweeps running".

`--lock <path>` (repeatable) names a lock file taken with `flock <path> <command>`. For each one, `rally stalls`
reads the holder and the waiters from `/proc/locks`, matching the file's device and inode, and walks the holder's
process tree. It adds a prompt if anything in that tree is stopped (`T` or `t`) or the holder is a zombie (`Z`):

| Row | When it holds | Prompt |
|---|---|---|
| `stopped_lock_holder` | A process holding the lock, or under its holder, is stopped, or the holder is a zombie | Ask its session, or the operator, to resume it (`kill -CONT`) or end it |

Each process is listed with its pid, state, command and working directory. Its session is guessed from a session
ID in that directory, such as a scratchpad path, and named from Muster's roster when the roster has it. The guess is
labelled as one. The process states are the kernel's account (observed). A lock that isn't held, or whose holder is
running or asleep, gives no prompt. Every lock read is listed under `locks`, with its holders and the number
waiting, or why it couldn't be read. Linux only, and only processes this user can see. The lock row needs no
roster, so it's shown even when the roster can't be read.

The issues log also suggests asking a run not to start the next one, rather than to "hold", since a hold may have
caused that stop. The prompt says so.

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
