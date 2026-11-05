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
4. **It's woken if it's idle**: every session `idle`, or `wake: waiting`, with nothing claimed. It's also woken
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

## Muster is optional

`rally wake` sends through `muster-shim tell` when `muster-shim` is on the machine. Without it, or with
`--report-only`, it only reports: in its output and in `wake-status.json`, beside its state file, it lists who's
idle, who can't be woken, and each team's next story. The sender is a small trait, so another harness's
messaging, such as Prime Agent's (RAL-59), can be added as a sender later.

## Files

- `wake.json` (beside the pairing, or `--state-file`): when each team was last woken, and which of the
  operator's answers it has been woken for. Kept on a1, not in the ledger.
- `wake-status.json`, beside it: the last pass's report.

## Limits

- Activity and wake on the roster are mostly declared, except `wake: waiting` (see design.md, "What Muster's
  roster can tell Rally"). So idle is a reading, not a fact. A digest to a busy team is a nudge it can ignore.
- `muster-shim tell` can be sent by any process on the account, so a team can't tell Rally's digest from
  another's, apart from its wording.
- The quiet time counts from the newest record on each claimed story, which is the team's own declaration.
