# Waiting for a pull request's checks (RAL-185)

```sh
rally wait-green <PR> --repo <owner/repo>            # a pull request: its head is pinned at the start
rally wait-green <full sha> --repo <owner/repo>      # a commit
    [--head <full sha>] [--base main] [--check <name>]... [--timeout 1h] [--every 30s]
```

It waits until every check on the head has finished, then prints what each ended as and exits:

| exit | verdict      | meaning                                                                          |
|------|--------------|----------------------------------------------------------------------------------|
| 0    | `green`      | every expected check is there and passed (success, neutral or skipped)           |
| 1    | `red`        | a check finished and didn't pass; named, with its address                        |
| 3    | `moved`      | the pull request's head is no longer the pinned commit (or `--head`): wait again |
| 4    | `timed_out`  | still not finished: what was pending and what hadn't appeared yet                |
| 2    |              | a usage error, such as a repository that isn't `owner/repo`                      |

**Which checks.** Unless `--check` names them, it waits for every check that ran on the base branch's tip
and its first parent: GitHub's check runs and the commit statuses, such as ci-local's `local/check`. So a
head whose GitHub checks have finished while ci-local's status hasn't been posted yet is still waiting,
not green. A check there that isn't expected still counts: a red is red.

**Observed, recording nothing.** It reads GitHub through `gh` and writes nothing to the ledger. A read that
fails is tried again until the timeout, and the last failure is said as `last_read_failed`.

**Why it exists.** On 2026-11-14 a team waited on its pull requests with a home-made `jq` loop that never
exited: `[...]|length,(.statuses|length)` applies `.statuses` to the array, so its test could never
pass. For a whole night green pull requests sat unsubmitted while the merge queue waited on them. Use this
instead, in the background, and submit when it says green:

```sh
rally wait-green 270 --repo Castor-o7/rally && rally submit RAL-183 --team <team> --evidence "Castor-o7/rally@<head>"
```
