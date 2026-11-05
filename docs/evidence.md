# Checking submitted evidence (RAL-5)

When a story is submitted, the service checks its evidence against GitHub and records what it found, as an
**observed** `evidence_check` written by Rally itself. It's shown beside the evidence in `rally waiting` and in
the story's history.

## What's checked

Evidence written as `owner/repo@<full commit>` (40 or 64 hex digits) is checked for:

- **`exists`:** the repository has that commit.
- **`names_story`:** the commit's message, or the title of a pull request containing it, names the story ID as a
  whole word (`RAL-5`, not `RAL-50`).
- **`ci`:** what the commit's check runs and combined status say together: `passed`, `failed`, `pending`, or
  `none` when it has neither.

Evidence in any other form is recorded as `checkable: false`, with the reason. What a team didn't test goes in
its own field, `rally submit --untested "..."`, not in the evidence.

The record is evidence, not a verdict. `ci: passed` is GitHub's word on that one commit. A reader still reads
the work, and RAL-31 still compares the commit that's merged with the one that was read.

## When

The check runs in the background after a submit, on a fixed pool of two workers, so a slow GitHub never adds
threads. CI is usually still running at submission, so a pending check is tried again every `--evidence-poll`
(1 minute) until it's final, for at most `--evidence-ci-wait` (30 minutes) from the submission. Then it's
recorded as still pending, saying the wait ran out. There's one record per submission. A read of GitHub that fails, from a network error, a 5xx or a rate limit, is
treated like CI still running: tried again until the wait runs out, then recorded with what failed. When a
commit's message doesn't name the story and its pull requests can't be read, `names_story` is recorded as unknown,
not false. After a restart, every submission still in review without a check is queued again.

**What a submission costs.** Any submitter can make the service ask GitHub about any public `owner/repo`: up to
about four calls a minute, for up to 30 minutes, per submission. A token's 5,000 calls an hour covers that, given
each credential's write limit on submissions, but it's a cost a submitter controls.

## Least privilege (RAL-18)

- The service only sends GETs to `api.github.com`, for a commit, the pull requests containing it, its check runs
  and its status. The host is fixed in the code. No redirect is followed, so the token can't be carried
  elsewhere, and a test shows it.
- It never checks out, builds or runs submitted code.
- The token is read from `--github-token-file` (or `RALLY_GITHUB_TOKEN_FILE`). Under `--secure`, the file must be
  root-owned like the operator's key. It's never printed.

Without a token, nothing is asked of GitHub, and each check is recorded as "not checked: the service has no
GitHub token". An anonymous allowance isn't enough: Murph's is shared and was used up when this was built.

### The operator's step

Create a fine-grained token, read-only, for the Castor-o7 repositories only, with **Contents**, **Commit
statuses**, **Checks**, **Pull requests** and **Metadata**, all read. Then, on Murph:

```sh
sudo install -o root -g root -m 0644 /dev/stdin /etc/rally/github-token <<< '<the token>'
```

and add `--github-token-file /etc/rally/github-token` to the service's `ExecStart`. With the system unit, it runs
as `joshd`, which must be able to read the file. The 0644 above allows that; tighten it with a group if the host
has other users.
