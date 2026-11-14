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

## The open rally's teams, and the watchman's batches (RAL-163)

On 2026-11-13 `rally stalls` was blind to the open rally. It took its teams from the projects list (`rally`,
`muster`, `marshal`, ...), while the rally's teams were its sessions' nicknames (`deft-pelican`, `sleek-moth`,
...). It flagged nothing about them, only past rallies' blockers. With a rally open, it now checks the rally's
teams: those named when the rally opened, and those with live work in its projects (a claim, a story sent back
or in review, one riding a batch, or a landing that owes a submit). Each team's sessions on the roster are
found by its name, which is its session's nickname, and by any session confirmed as a restart of it
(`rally rally-session --succeeds`). Claims and blockers are the rally's. Teams in the projects list outside the
rally are named under `rally.outside`, not checked. With no rally open, the projects list is used as before.

The watchman's batches (`rally batch`, RAL-157) are read as well:

| Row | When it holds | Prompt |
|---|---|---|
| `riding_batch` | The team's claim is a car in a batch still gating, and that batch is past its expected time, or is built on a batch that didn't land | For the watchman: look at the batch's gate. The team is waiting on it, not stuck |
| `owes_submit` | A story of the team's landed in a batch, and no submit has come since, past `--quiet-after` | Ask the team to submit, with the landed commit as evidence |

A claim riding a batch is a reason, not a stall: it's never a missed check-in, and in `every_team_waiting` the
team is named as waiting on the watchman's batch N and for how long. These two rows come from Rally's records
alone, so they're shown even when no roster can be read. A team busy on its own long job, such as a slow suite,
is working, and isn't flagged.

One approximation, noted by the watchman: "submitted since landing" is true after any later submit of the story,
so a story in parts looks settled once one part is submitted after it landed.

## A stopped lock holder (RAL-75)

From the DiscoSim rally's issues log, §4.4: on 2026-11-06 a runner shell sat stopped (state `T`) for about 57
minutes while it held the one-heavy-run lock, so no sweep ran and the queue behind it waited. Its status still said
"sweeps running".

`--lock <path>` (repeatable) names a lock file taken with `flock <path> <command>`. For each one, `rally stalls`
reads the holder and the waiters from `/proc/locks`, matching the file's device and inode, and walks the holder's
process tree. It adds a prompt if anything in that tree is stopped (`T`), traced (`t`, maybe a debugger), or the holder is
a zombie (`Z`):

| Row | When it holds | Prompt |
|---|---|---|
| `stopped_lock_holder` | A process holding the lock, or under its holder, is stopped, or the holder is a zombie | Ask its session, or the operator, to resume it (`kill -CONT`) or end it |

Each process is listed with its pid, state, command and working directory. Its session is guessed from a session
ID in that directory, such as a scratchpad path, and named from Muster's roster when the roster has it. The guess is
labelled as one. The process states are the kernel's account (observed). A lock that isn't held, or whose holder is
running or asleep, gives no prompt. Every lock read is listed under `locks`, with its holders and the number
waiting, or why it couldn't be read. A lock nobody holds says "no holder found in /proc/locks", rather than showing
an empty list. `/proc/locks` prints the superblock's device, which on btrfs subvolumes and overlayfs isn't the one
`stat` gives, so when no line has the file's device and inode, lines with its inode alone are taken, and
`matched_by` says so: a file on another device with the same inode would match too. Open file description locks
have no owning process (pid -1) and are skipped. Linux only, and only processes this user can see. The lock row needs no
roster, so it's shown even when the roster can't be read.

The issues log also suggests asking a run not to start the next one, rather than to "hold", since a hold may have
caused that stop. The prompt says so.

## A lock holder doing almost nothing (RAL-163 part 2)

On 2026-11-13 a team's test run, a godot under `dsm-heavy`, sat at 0.9% CPU for 11 minutes under a 40-minute
timeout, holding one of the heavy-run slots. It wasn't stopped, so the stopped-holder row above didn't see it.

`--lock` now takes a `*` or `?` in the file name, so `--lock '/tmp/discosim-heavy*.lock'` reads every slot of
`dsm-heavy`'s pool (`/tmp/discosim-heavy.lock` and `/tmp/discosim-heavy.<k>.lock`). With `--hung-after
<window>`, each holder's whole process tree is read twice, `--cpu-sample` apart (5 s by default), and a holder
that has run at least the window while its tree used less than 2% of a CPU over the sample is a prompt:

| Row | When it holds | Prompt |
|---|---|---|
| `possibly_hung_lock_holder` | The holder has run at least `--hung-after`, and its whole process tree used under 2% of a CPU over `--cpu-sample` | Ask the team whose run it is to look at it |

Each such process is named by the tree's busiest process, with its command line and working directory, as
"possibly hung: godot --headless ..., ~/.cache/sleek-moth/..., 0.9% CPU over 5 s, running 11 min". The team is
guessed from a roster nickname in the working directory, and labelled so. It's a prompt, never a finding: a run
waiting on something legitimate, such as the network, looks the same. Rally stops and kills nothing. No window has
been agreed with the operator, so `--hung-after` has no default, and without it nothing is sampled. CPU time and
start times are the kernel's (`/proc/<pid>/stat`, observed). Linux only.

## One command line, from the projects list (RAL-164)

`rally stalls` and `rally wake` read the projects list beside the pairing (`projects.json`) when not given
`--projects`, as `rally brief`, `rally digest` and `rally next` do, and the built-in list otherwise. A project
entry may say what stalls checks for it, as the file's keeper declares it:

```json
{"prefix": "DSM", "repo": "Castor-o7/DiscoSim", ...,
 "stalls": {"locks": ["/tmp/discosim-heavy*.lock"], "hung_after": "5m"}}
```

With a rally open and no `--lock` given, each of the rally's projects' `stalls.locks` is read, with its own
`hung_after` (sampled over `--cpu-sample`, 5 s by default). Flags still override: `--lock` and `--hung-after` given
on the command line are used instead, for every lock. The check-in, `--quiet-after`, stays a flag or
`RALLY_WAKE_QUIET_AFTER`, since it's the muster's window, not a project's. `--prompts-only` prints nothing at all
when there's nothing to ask (no prompt, no every-team-waiting, no error), and succeeds either way, so the watchman's
cron line can be:

```bash
RALLY_WAKE_QUIET_AFTER=30m rally stalls --prompts-only
```

For DiscoSim the watchman set 5m: a parse-error hang shows inside a minute and holds near 0% CPU, and no honest
heavy job sits under 2% of a CPU for five minutes while holding a slot (a job waiting for a slot holds none).

**The projects list tolerates fields it doesn't know** (RAL-164). The file is shared by the CLI, the digest loop
and the viewer, so a reader from this build on leaves out a field it doesn't know, saying so on standard error,
rather than refusing the whole file. Readers from before this build still refuse it: add a new field, such as
`stalls` or `gate`, only once every reader on the machine runs a build that knows it.

## A watchman's digest that stopped (RAL-87)

On 2026-11-06 the watchman's `rally digest --every 30s` exited on a 429 and nothing noticed (issues log §2.7).
With `--digest-quiet-after <window>`, `rally stalls` adds a `digest_quiet` prompt for each watchman whose digest
looks stopped, from the state `rally digest` keeps on this machine (`--digest-state-file`, by default
`digest.json` beside the pairing): no pass recorded at all, the last pass older than the window, or records first
seen longer ago than the window and still not sent. A quiet ledger sends nothing and isn't flagged. The watchmen are
the designated ones; with none designated, `--digest-watchman <name>` names one, declared. No window has been
agreed with the operator, so it has no default (`RALLY_DIGEST_QUIET_AFTER`), and without it nothing is checked.
`rally brief` takes the same flags and gives the same check as `digest`. The state is this machine's, so run the
check where the digest runs.

## Labels

Muster's facts keep Muster's own labels. `wake: waiting` is observed, and means "probably wakeable". Every other
wake value, and `activity`, are declared. `wake: none` means no sign that a message would wake the session, not that
it's gone. Idleness is judged by a session's declared activity, since a busy session can be waiting for messages
too. Only a session that declares no activity counts as idle on `wake: waiting` alone. Rally's facts (claims, states,
blockers) are declared. The roster is every one of `RALLY_ROSTER_FROM`'s (by default `marshal-shim` and
`muster-shim`) merged, each session row naming its `source` (RAL-126); the report's `roster.read_with` lists those
read, and `roster.passed_over` any that failed. Without one, only
`declared_blocked` is shown, since the other rows need the roster.

## The check-in length

How long a check-in lasts hasn't been agreed with the operator. `--quiet-after` is the same setting as
`rally wake --quiet-after` (`RALLY_WAKE_QUIET_AFTER`), and has no default. Without it, `missed_check_in` and
`every_team_waiting` aren't looked for, and the report says why under `check_in`.
