# ci-local: the projects' CI on Murph (RAL-144)

ci-local runs each project's CI on Murph and posts the results back to GitHub as commit statuses,
so a pull request shows `local/check` (and the rest) next to Actions' checks. It was built when
Josh's Actions minutes ran out (blocked until 2026-12-02, with a $0 budget). Only private
repositories spend minutes, so:

- **Public repositories (rally, marshal, muster) run both** (Josh, 2026-11-11). GitHub's standard
  hosted runners bill nothing there, so their workflows keep their `push` and `pull_request`
  triggers, and Actions' `check` stays the required check. `local/*` runs alongside, as a second
  check that doesn't depend on GitHub. Each says so with `hosted_runs` in its `.ci/local.toml`.
- **A private repository (RootyRoopsDAW)** has no `hosted_runs`. Its workflows run only by hand,
  and `local/*` is its required check.

It watches four repositories (`scripts/ci-local/repos.toml`): rally, marshal, muster and
RootyRoopsDAW. DiscoSim isn't one of them: it keeps `dsm-gate`.

## What it runs, and when

Every 60 seconds it asks GitHub for each repository's `main` head and every open pull request's head.
Each commit it hasn't run yet is run once:

- **A branch in the same repository**: its jobs run, and each posts `local/<job>` as pending, then
  success or failure. The description says how long it took and where its log is on Murph.
- **A branch from before local CI**: neither its head nor the commit where it left `main` has
  `.ci/local.toml`. Nothing is run or posted, since a red `local/ci-config` there would only be
  noise. It runs once the branch merges `main` in. A branch that *removes* the file from a base
  that had it is still checked, and fails.
- **A fork's branch**: never run here. It gets `local/skipped-fork` (failure), saying a maintainer
  has to push the branch to the repository to test it. rally, marshal and muster are public, so
  anyone can open a pull request from a fork, and running it would run a stranger's code on the
  machine that holds the daemons and their tokens. That's why this isn't a self-hosted Actions
  runner.

## Trust: who can run code on Murph

Jobs run as `joshd`. A job can read everything in `~`: the daemons' tokens, the rally and muster
credentials, SSH keys, Josh's saves. The scrubbed environment (below) keeps tokens from riding in
on environment variables, but it isn't a sandbox, and nothing stops a job from reading a file.

So **"same-repository branches only" is the whole security boundary**: anyone who can push a branch
to rally, marshal, muster or RootyRoopsDAW can run code on Murph with Josh's account. Forks can't,
because their pull requests are never run. Keep each repository's collaborators (Settings →
Collaborators) to people and agents Josh would let run code on Murph. **Josh: confirm the four
collaborator lists match that.**

A job's environment is an allow-list (`PATH`, `HOME`, `USER`, `LANG`, `LC_*`, `TZ`,
`XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS`, the Rust toolchain's `CARGO_HOME`/`RUSTUP_*`) plus
the job's own `env`. Nothing whose name looks like a secret (`TOKEN`, `SECRET`, `PASSWORD`,
`SSH_*`, `GH_*`, `GITHUB_*`) is passed, and a job's config can't ask for one.

A new push to a pull request is a new commit, so it runs again. A commit already run isn't run
again, even after a restart (`~/.cache/ci/state.json`). To run one again, use the one-shot below
with `--post`.

## The jobs: `.ci/local.toml`

Each repository says what its CI is in `.ci/local.toml`, read from the commit being tested, so a
change to the jobs is tested by its own pull request:

```toml
workflows = ["ci.yml"]          # workflows whose every job must appear below

[jobs.check]
mem = "10G"                     # MemoryMax for the job's scope (no swap)
cpu = "600%"                    # CPUQuota: 600% is six cores
timeout_min = 40
postgres = true                 # a scratch cluster for this job; its URL is $CI_POSTGRES_URL
env = { SOME = "value" }
run = """
cargo test
"""

[jobs.all]
needs = ["check", "conformance"]   # passes when all of these pass; runs nothing itself

[jobs.macos]
manual = "needs macOS: run the workflow by hand"
```

A job has exactly one of `run` (a bash script, run with `-eo pipefail`), `needs` (an aggregate, as
muster's `check` is in its workflow) or `manual` (not run here, and why). Murph is Linux, so macOS and Windows jobs are `manual`. They're still in their
workflow, which runs on GitHub as before on a public repository and by hand on RootyRoopsDAW (below).

**The drift check** runs before any job and posts `local/ci-config`. It fails when:

- a job in one of the named `workflows` isn't in `.ci/local.toml`, as `run` or `manual`, so the
  two can't drift apart, or
- `hosted_runs` is set but GitHub doesn't confirm the repository is public. The checkout declares
  it, so a private repository's branch could otherwise turn on billed runs just by saying so. The
  runner asks `GET /repos/{owner}/{repo}` (with its token) for each commit that declares it. A
  private answer, or no answer, is a problem, and the workflows are then held to the
  dispatch-only rule below, or
- with `hosted_runs` set (a public repository), a job's `runs-on` names a larger runner (such as
  `ubuntu-latest-8-cores` or `windows-2022-xlarge`), which bills even on a public repository, or
- without `hosted_runs`, any workflow in `.github/workflows` still has a trigger other than
  `workflow_dispatch` or `workflow_call` (a `push`, `pull_request` or `schedule` would run on GitHub's runners on its own),
  unless `.ci/local.toml` lists it under `[exempt]` with a reason **and** none of its jobs can run in
  this repository: each job's `if` names `github.repository == ...`, or every job it needs is gated
  that way and its `if` has no `always()`, `failure()` or `cancelled()`. marshal's upstream
  `release.yml` and `public-ci-red-team.yml` are exempt this way: they're upstream's, kept as
  upstream wrote them, and on the fork a push only records skipped jobs.

Run it on a checkout: `python3 scripts/ci-local/ci_local.py check-config <checkout> --repo
owner/repo`. Without `--repo`, a checkout that sets `hosted_runs` fails, because nothing has
confirmed the repository is public.

## Caps

Murph has 32 GB, no swap, and `/tmp` in RAM, and Josh plays DiscoSim on it.

- Each job runs in its own `systemd-run --user --scope` with `MemoryMax`, `MemorySwapMax=0` and
  `CPUQuota` from its config, under `timeout` with its `timeout_min`. It's started with
  `--expand-environment=no`: otherwise systemd-run expands `${VAR}` in the script from its own
  environment before bash runs it. A job over its memory is
  killed, and the job fails. The machine doesn't.
- `CARGO_TARGET_DIR` is `~/.cache/ci/target/<repo>/<job>`, kept between runs so builds are
  incremental. `TMPDIR` is a fresh `~/.cache/ci/tmp/...`, removed afterwards. Nothing builds in
  `/tmp`.
- At most N CI jobs run at once, through lock files in `~/.cache/ci/locks`, as `dsm-heavy` does.
  N is the number in `~/.config/ci-local.slots` (default 2), re-read before each job, so it can be
  turned down while something heavy runs. These are CI's own slots, alongside `dsm-heavy`'s, not
  shared with them: taking DiscoSim's slots would make the rally's teams wait on CI.
- **The shared guard is memory**: no new CI job starts while `MemAvailable` is under 6 GB, whatever
  else is running, and each job's `MemoryMax` bounds what it can take once started.
- **While Josh's DiscoSim window is open**, jobs don't stop. They run one at a time, at `nice 19`,
  with CPU capped at 200%. Pausing would bring back the bottleneck this replaced.

## Running one commit by hand

```bash
python3 scripts/ci-local/ci_local.py run <repo> <full sha>          # logs only
python3 scripts/ci-local/ci_local.py run <repo> <full sha> --post   # and post the statuses
python3 scripts/ci-local/ci_local.py run <repo> <full sha> --job check
```

Logs go to `~/.cache/ci/<repo>/<sha>/<job>.log`, the same place the watcher writes them. The
first lines say the slot, the caps and whether it ran throttled.

## GitHub's rate limit

The watcher makes two requests a repository a minute, about 480 an hour for four repositories,
against GitHub's 5,000. Both are conditional (`If-None-Match` with the last ETag), and GitHub
doesn't count a `304 Not Modified`, so a quiet hour costs almost nothing. It reads
`X-RateLimit-Remaining` from every answer. Below 300 it stops polling until GitHub's reset time,
and logs when it backs off and when it resumes.

## Releases, and the macOS and Windows jobs

On the public repositories, the workflows run on their own as before, macOS and Windows jobs
included. On RootyRoopsDAW, each one keeps only `workflow_dispatch`. To run one by hand, use the
repository's Actions tab, "Run workflow", or:

```bash
gh workflow run release.yml -R Castor-o7/muster --ref main
```

On a public repository (rally, marshal, muster), GitHub's hosted Linux, macOS and Windows runners
bill no minutes, so a hand-run release or macOS job costs nothing. On RootyRoopsDAW, which is
private, it does: run its `portable` job by hand only when it's worth the minutes. marshal's jobs
that name `self-hosted` runners stay dispatch-only and must not be pointed at Murph (see the fork
note above).

## Setting it up (Josh: steps an agent can't do)

1. **Packages the workflows installed with `apt`**, which Murph doesn't have yet:
   `sudo pacman -S lsof openbsd-netcat mingw-w64-gcc`. Without `lsof`, rally's socket test reads
   `/proc` only. Without `nc` and the mingw compiler, muster's and marshal's jobs that need them
   fail and say so.
2. **A token**: a fine-grained personal access token for Castor-o7, limited to the four
   repositories, with *Commit statuses: read and write*, *Contents: read* and *Pull requests: read*,
   in `~/.config/ci-local/token` (mode 600). It's what the runner posts with and fetches the private
   repository with. Without it, `serve` and `run --post` refuse to start: they never fall back to
   `gh`'s token, which can do far more. A `run` without `--post` needs no token.
3. **Install the runner and its unit**:
   ```bash
   install -D -m 644 -t ~/.local/share/ci-local scripts/ci-local/ci_local.py scripts/ci-local/repos.toml
   install -D -m 644 deploy/murph/ci-local.service ~/.config/systemd/user/ci-local.service
   systemctl --user daemon-reload && systemctl --user enable --now ci-local
   journalctl --user -u ci-local -f
   ```
4. **Point RootyRoopsDAW's required check at the local status.** Its ruleset requires `rust` from
   GitHub Actions (integration 15368), which a status from Murph can't satisfy. In its Settings →
   Rules → "Protect main" → Require status checks, replace it with `local/rust`, with no source app
   selected. The public repositories keep Actions' `check` as the required check. Adding
   `local/check` beside it is optional.
5. **Optional, RootyRoopsDAW only**: in its Settings → Actions → General, allow only the workflows
   that are run by hand, so nothing can spend minutes even by mistake.

## Updating the runner

The unit runs the copy in `~/.local/share/ci-local`, not a checkout, so an unmerged branch can't
change what runs. After a change to `scripts/ci-local` merges, install it again from `main` (step
3's first line) and run `systemctl --user restart ci-local`. A job already running keeps its own
scope and finishes. The runner skips nothing on restart: a commit whose result wasn't recorded
runs again. That rerun waits for the first run's scope to end, through a lock per repository,
commit and job (`~/.cache/ci/locks/run-*.lock`) that the job's scope holds. The two share a
checkout, a `TMPDIR` and the job's scratch Postgres, and on 2026-11-11, running them together
made rally#229's `local/check` a false red.

## What it doesn't do

- Run a fork's pull request, run Windows or macOS jobs, or run releases (all above).
- Cancel a superseded run: when a pull request is pushed again, the older commit's jobs finish.
- Link a status to its log on the web: the description gives the log's path on Murph.
