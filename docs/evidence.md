# Checking submitted evidence (RAL-5)

When a story is submitted, the service checks its evidence against GitHub and records what it found, as an
**observed** `evidence_check` written by Rally itself. It's shown beside the evidence in `rally waiting` and in
the story's history.

## What's checked

Evidence written as `owner/repo@<full commit>` (40 or 64 hex digits) is checked for:

- **`exists`:** the repository has that commit. GitHub answers 404 both for a missing commit and for any
  commit in a private repository the token can't read, so after a 404 the repository itself is read: only
  when the token sees it is the commit recorded as not existing. Otherwise `exists` is left out and the
  reason says the repository isn't visible to the token, so the commit's existence is unknown (RAL-128).
- **`names_story`:** the commit's message, or the title of a pull request containing it, names the story ID as a
  whole word (`RAL-5`, not `RAL-50`).
- **`ci`:** what the commit's check runs and combined status say together: `passed`, `failed`, `pending`, or
  `none` when it has neither. A submission often comes before its CI starts, so `none` is looked for again as
  `pending` is, until the wait runs out, and only then recorded, with the reason (RAL-132).
  When GitHub refuses the check runs with 403 "Resource not accessible by personal access token" (or "by
  integration"), the token can't read them at all: a fine-grained token has no Checks permission, so on a
  private repository this never passes. The check doesn't try again until the wait runs out; it reads the
  commit's statuses alone, records at once, and says so in the reason: check runs can't be read with the
  service's token, so CI was read from commit statuses alone. A commit with no statuses then reads as `none`,
  with "no CI reported", never as `passed` (RAL-147). Any other refusal of the check runs, such as a rate limit
  (also a 403) or a bad token (401), is a failed read as before, tried again until the wait runs out.

Evidence in any other form is recorded as `checkable: false`, with the reason. What a team didn't test goes in
its own field, `rally submit --untested "..."`, not in the evidence.

A check is never rewritten. When one is wrong, such as `exists: false` from before RAL-128, or `ci: none` from
before RAL-132, anyone can say so with `rally correct <STORY> --corrects <check record> --note "..."`. The story
list gives the corrections beside the check, declared, and the viewer shows them after it (RAL-134).

## At submit, and correcting it (RAL-71)

`rally submit` and `rally amend` check the evidence before anything is written: it must be
`owner/repo@<full commit>`, and when the projects list (`--projects`, or `RALLY_PROJECTS`, or the defaults)
has the story's prefix, the repository must be that project's, compared without case. A refusal says what to
write. Evidence that really can't take that form, such as work outside GitHub, is recorded with
`--not-checkable`, and the service's check then records it as not checkable.

A typo found while the story is in review is fixed by its submitter with
`rally amend <STORY> --team <team> --evidence "owner/repo@<full commit>"`, without a send-back. The amendment is a
submit record marked `"amend": true`: the state stays In review, the earlier submission stays in the history, and
the amendment is the latest submission, so it's the one this check reads and the one `rally waiting` shows. Only
the submitter can amend, and only in review, and an amendment that changes nothing is refused; a plain resubmit
in review is refused, saying to amend. Once a submission is amended, a verify or send-back that names a commit
must name the amended evidence's commit, so a read of the work as it was before the amendment can't verify it;
the refusal says which record amended it and which commit to read. A send-back naming no commit still stands.

The record is evidence, not a verdict. `ci: passed` is GitHub's word on that one commit. A reader still reads
the work, and RAL-31 still compares the commit that's merged with the one that was read.

## When the merged commit isn't the one read (RAL-85)

`rally show` flags `commit_differs` when a Verified story's latest recorded merge isn't a commit its verification
covers: the verified commit, the verifier's own also-reads (RAL-67), or a read of the merged commit. When the head
that landed took something after the read, such as a merge of main, a reader who isn't one of the story's authors
reads that head and records it:

```sh
rally read-merged <STORY> --commit <latest merged commit> --note "what changed since the verify" [--team <team>]
```

It's a verified-kind record of its own, under the reader's own name and credential, shown as `merged_reads` beside
the verification rather than added to it, so a watchman after the one who verified can write it, and so can the
verifier. It must name the story's latest recorded merge, which the verification mustn't already cover, and it's
dropped with the verification when a new verdict replaces it. A read_merged is labelled, not gated: like a
verify, any verifier who isn't an author can write one, and `rally verifications` labels it watchman, peer or
other by the designation in force when it was written.

## When

The check runs in the background after a submit, on a fixed pool of two workers, so a slow GitHub never adds
threads. CI is usually still running at submission, so a pending check is tried again every `--evidence-poll`
(1 minute) until it's final, for at most `--evidence-ci-wait` (30 minutes) from the submission. Then it's
recorded as still pending, saying the wait ran out. There's one record per submission. A read of GitHub that fails, from a network error, a 5xx or a rate limit, is
treated like CI still running: tried again until the wait runs out, then recorded with what failed. When a
commit's message doesn't name the story and its pull requests can't be read, `names_story` is recorded as unknown,
not false. After a restart, every submission without a check is queued again (RAL-149): each still in review, and each submitted since the ledger's first check whatever its story's state now (verified, sent back, merged), since a stop can come between a submit and its check while the story moves on before the start. A submission an amendment replaced isn't checked; its amendment is. Each such check's reason starts with the fixed phrase "checked late, after a service restart", then the story's or part's state by then, so a search finds every one. The ledger takes one check per submission, so nothing is checked twice.

**What a submission costs.** Any submitter can make the service ask GitHub about any public `owner/repo`: up to
about four calls a minute, for up to 30 minutes, per submission. A token's 5,000 calls an hour covers that, given
each credential's write limit on submissions, but it's a cost a submitter controls.

## Least privilege (RAL-18)

- The service only sends GETs to `api.github.com`, for a commit, the pull requests containing it, its check runs
  and its status. The host is fixed in the code. No redirect is followed, so the token can't be carried
  elsewhere, and a test shows it.
- It never checks out, builds or runs submitted code. Two tests show it: a whole check against a stand-in
  GitHub sends exactly four requests, each a GET with no body, of the commit, its pull requests, its check runs
  and its status, and nothing else (no clone, no tarball); and the evidence module can't start a program, since
  its source uses no process API.
- Its tests can point the whole service at a stand-in GitHub, to show what it does when GitHub refuses its token
  (RAL-153): `RALLY_TEST_GITHUB_BASE=http://127.0.0.1:<port>`. That's kept out of a real start three ways. Only a
  debug build reads it: a deployed binary is a release build, which ignores it and says so on standard error.
  Only a loopback `http://127.0.0.1:<port>` is taken, so the token can't be sent to another host. It's never
  taken under `--secure`. When it is taken, the service says so on standard error.
- The token is read from `--github-token-file` (or `RALLY_GITHUB_TOKEN_FILE`). Under `--secure`, the file must be
  root-owned like the operator's key. It's never printed.

Without a token, nothing is asked of GitHub, and each check is recorded as "not checked: the service has no
GitHub token". An anonymous allowance isn't enough: Murph's is shared and was used up when this was built.

### Open pull requests' roadmaps (RAL-68)

The token has one other use. `rally new-story` asks the service, at `/v1/open-ids`, for the highest number of its
prefix written in the roadmap of any open pull request, so a number someone wrote by hand in an open pull request
isn't handed out again, as MUS-95 was on 2026-11-05. The service lists the repository's open pull requests (at
most 100) and reads the roadmap file at each one's head, all GETs to `api.github.com`. The repository comes from
`--repo owner/repo`, or else the checkout's `origin` on github.com; the path is `--roadmap`, which must be a plain
path inside the repository, such as the default `docs/roadmap.md`. The service reads only repositories of the
owners it's given, `--github-owners` (or `RALLY_GITHUB_OWNERS`), comma-separated, `Castor-o7` by default; a
request can't widen that, and a repository outside it is refused before GitHub is asked. Without that limit any
paired client could spend the token on any repository it can see, private ones included, and learn their story
numbers.

`new-story` prints what it found as `open_pull_requests`: `checked: true`, labelled observed, with the highest
number and the pull requests that use the prefix. If anything can't be read (no token, GitHub down, one pull
request's file unreadable, more than 100 open, or no repository given), it prints `checked: false` with the
reason, and the number comes from the ledger and the local roadmap only. It never counts a partial read as a
check.

### Showing the token can't merge

`rally check-github-token` is for the operator, run by hand; the service never runs it. Point it at an
**already-merged** pull request. It reads the pull request with the token and refuses to go on unless it's closed
or merged; then it tries to merge it, naming head `0000000000000000000000000000000000000000`, which no commit has.
GitHub can't merge a merged pull request, so nothing can merge even if the token could write:

```sh
rally check-github-token --github-token-file /etc/rally/github-token --repo Castor-o7/rally --pull 124
```

It succeeds only when GitHub refuses the merge for want of access (403, or 404 on a pull request it just read).
If GitHub gets past the access check (on a merged pull request a writable token gets 405 "not mergeable"; 409 and
422 count the same) it fails and says the token can write; any other answer fails as unclear. Its tests show
each case, that an open pull request is refused with only a GET sent, and that the merge names the impossible
head.

### The operator's step

Create a fine-grained token, read-only, for the Castor-o7 repositories only, with **Contents**, **Commit
statuses**, **Checks**, **Pull requests** and **Metadata**, all read. (Fine-grained tokens offer no **Checks**
permission today, so private repositories' check runs stay unreadable and their CI is read from commit statuses
alone, RAL-147.) Then, on Murph:

```sh
sudo install -o root -g root -m 0644 /dev/stdin /etc/rally/github-token <<< '<the token>'
```

and add `--github-token-file /etc/rally/github-token` to the service's `ExecStart`. Then run
`rally check-github-token` as above, and keep what it prints with the story. With the system unit, it runs
as `joshd`, which must be able to read the file. The 0644 above allows that; tighten it with a group if the host
has other users.
