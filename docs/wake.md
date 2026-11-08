# Waking idle teams (RAL-54)

`rally wake` gives an idle team its next story, so a team with ready work isn't left waiting for someone to
notice. It comes from the operator's question of 2026-11-05, whether a rally can run without a watchman: two
teams sat idle with ready work that night, and one slept for about 2 hours with no way to wake it.

It runs on a1, beside `rally web`, where `gh` can read each project's roadmap and `muster-shim` can read Muster's
roster and send messages. It reads the ledger through the paired service. Each run is one pass, or `--every
<time>` repeats it until stopped, for example under launchd.

## Each pass

For each project in the projects list (`--projects`, or the defaults in `web/projects.json`) that names a
`team`:

1. **Its sessions** are the roster's sessions listed for it in `sessions`, by Muster project name or session
   nickname. Rally never guesses them from the team's name, because a session's project doesn't always name its
   team: on 2026-11-05 both Rally's and Muster's teams had sessions in a project called `src`. A team with no
   sessions listed or found is reported as "no sessions known", never as idle.
2. **Its next story** is RAL-56's pick, the same as `rally next`, from the roadmap on GitHub and the ledger. With
   nothing ready, nothing is sent.
3. **A resting team** (`rally team-state --team <team> resting`) is left alone.
4. **It's woken if it's idle**: every session declares `idle` (or, declaring no activity, shows `wake: waiting`),
   with nothing claimed. A busy session can be `wake: waiting` too, so that alone doesn't make it idle (RAL-66). It's also woken
   if its claims have gone without a record for `--quiet-after`.
5. **At most once per `--interval`.** One exception: when the operator answers a question one of its shelved
   stories waits on, that story comes back first, so the team is woken straight away.
6. **A team that can't be woken** (every session `wake: none`) is reported as such, as a prompt to ask the team
   or the operator, never as a sign it's gone. Nothing is sent to it.

The digest reads "Rally suggests your next story, team rally: RAL-55: … (why)", points to `rally next`, says it's
coordination, not an order, and carries the untrusted-text notice, since story titles come from roadmaps that
agents write.

## Times

Both times are agreed with the operator and have no default:

| | Flag | Variable | Without it |
|---|---|---|---|
| A claim counts as quiet after | `--quiet-after` | `RALLY_WAKE_QUIET_AFTER` | only idle teams are woken |
| Least time between two digests to a team | `--interval` | `RALLY_WAKE_INTERVAL` | nothing is sent; teams are reported |

## Muster or Marshal is optional

`rally wake` reads the roster and sends through Marshal's shim or Muster's (RAL-120): the commands in
`RALLY_ROSTER_FROM`, comma-separated, by default `marshal-shim,muster-shim`, as the viewer reads the roster. The
roster is all of theirs merged (RAL-126), each session naming its `source`, and the report's `roster.read_with`
lists those read. A message goes through
the first that knows the team, named in each team's `sent_by`. It falls back to the next only when the shim says
nothing was sent (the team isn't on that daemon's roster, or the daemon couldn't be reached) or can't be run; any
other failure is reported and nothing more is sent, so a shim that delivered and then failed can't cause a second
copy through the other daemon. Without any of them, or with `--report-only`, it only reports: in its output and in `wake-status.json`, beside its state file, it lists who's
idle, who can't be woken, and each team's next story. The sender is a small trait, so another harness's
messaging, such as Prime Agent's (RAL-59), can be added as a sender later.

## Files

- `wake.json` (beside the pairing, or `--state-file`): when each team was last woken, and which of the
  operator's answers it has been woken for. Kept on a1, not in the ledger.
- `wake-status.json`, beside it: the last pass's report.

## When a pass fails (RAL-86)

With `--every`, a pass that fails doesn't end the loop when the failure can clear with time: the service refusing
with 429 ("too many requests", "too many checks and past-state reads"), 408 or 5xx; the service being unreachable or
restarting; Muster being down. The failed pass is logged on stdout as one JSON line, and the loop waits and tries
again:

```json
{"pass":"failed","error":"refused (429): too many checks and past-state reads","failures_in_a_row":1,"retry_in_ms":30000}
```

- The wait is `--every` after the first failure in a row, doubling with each one after, up to 16 times `--every`. If
  the service sends `Retry-After` (whole seconds), the loop waits at least that long, and the line shows it as
  `retry_after_ms`. Nothing waits longer than an hour. A pass that succeeds starts the count again.
- It **exits** (status 1, the reason on stderr) only on what waiting won't clear: a pairing or `--config` that
  can't be read, a `--projects` file that can't be read as a projects list, or any other refusal, such as 401 (a token that isn't valid) or 403 (read-only).
  These are checked before every pass. It also exits if its state file can't be written (below). Bad flags are refused before the loop starts.
- Without `--every`, one pass, and a failure exits as before.

A pass that fails on a read writes nothing: every read happens before any digest is sent, and a send that fails is
reported on the team's row, not as a failed pass. So a team isn't woken twice, or marked woken when it wasn't. If
`wake.json` can't be written after a send, the loop **exits** rather than retrying, since the next pass would reload
the old times and wake the same teams again. `wake-status.json`, the last report, is written after it, so failing
to write it is a failed pass, retried as usual.

## Limits

- Activity and wake on the roster are mostly declared, except `wake: waiting` (see design.md, "What Muster's
  roster can tell Rally"). So idle is a reading, not a fact. A digest to a busy team is a nudge it can ignore.
- `muster-shim tell` can be sent by any process on the account, so a team can't tell Rally's digest from
  another's, apart from its wording.
- The quiet time counts from the newest record on each claimed story, which is the team's own declaration.
