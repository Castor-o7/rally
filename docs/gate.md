# Gating a branch move (RAL-70)

From the DiscoSim rally of 2026-11-06 (issues log §2.5): the watchman moved `main` to f979331 while DSM-42 had
already been sent back by a peer. `rally waiting` doesn't show a send-back, so nothing stopped it.

```sh
rally gate 19cc9d1..origin/main                 # in the checkout; exit 0 only if every story named is verified
rally gate 19cc9d1..f979331 --repo ~/src/discosim --allow DSM-60   # DSM-60 is only filed on the roadmap here
```

## What it reads

- The **commit messages** in the range, through `git log <range>` in `--repo` (default `.`). No files, no checkout,
  and the checkout's git config can't make it run a program (fsmonitor and hooks are off, no pager, no signatures).
- In each message, the parts that say which story it's for: the **subject**; for a merge commit, the body's
  **first line**, where GitHub writes the pull request's title; and the **trailers** (`Story: RAL-70`), if the
  closing paragraph is all `Key: value` lines. The rest of the body is left out, since bodies cite other stories in
  passing ("after MUS-95 was handed out twice").
- An ID is matched as evidence matches it (RAL-5): a whole word, `RAL-5` but not `RAL-50` or `XRAL-5`.
- The ledger's current state of every story, from the service or `--db`.

## When it refuses

It exits non-zero, printing the same JSON after "the gate refused", if any story named is:

| State | Why |
| --- | --- |
| `sent_back` | sent back |
| `in_review` | submitted but not verified |
| `claimed` | claimed, not submitted |
| `ready` | ready, not claimed |
| not in the ledger, with a prefix the ledger uses (`DSM-99` when there are DSM stories) | not in the ledger |

An ID-shaped word whose prefix the ledger has never used, such as `SHA-256` or `UTF-8`, is listed under
`not_story_ids` and doesn't refuse.

`--allow <STORY>` (repeatable) is the watchman's word that the range only mentions a story, as a commit that
files it on the roadmap does, or a part of a multi-part story (§2.1) that the watchman has read. It's listed under
`allowed`, and isn't refused. The flag is recorded nowhere: say why in the merge or the ledger.

## A stale head (RAL-103)

For a range `<base>..<head>` (or `<base>...<head>`; an empty side is `HEAD`), the gate also refuses if the head
doesn't contain the base's tip: `git merge-base --is-ancestor <base> <head>` fails. Such a head is a stale branch.
`git log <base>..<head>` lists only the head's own commits, so its stories can all be verified while moving a
branch to it, or merging it, drops what landed on the base since. The refusal names both commits, in its first
line and under `stale` (`base`, `head`, `why`), and `--allow` doesn't lift it: bring the branch up to date with
its base, by a merge or a rebase, and gate again. An end the checkout can't find is refused too. A range that
isn't two ends, such as a single commit, has no base and isn't checked.

## Verified parts (RAL-79)

A story that isn't Verified still passes for each commit that a verified part's read covers (`docs/parts.md`). A
part's read, the `--commit` of `rally verify <ID> --part k`, covers:

- that commit;
- **its ancestors.** A read of part k is taken to cover the commits before it on that branch, such as part k's own
  earlier commits and the parts before it. The check is `git merge-base --is-ancestor` in `--repo`.

Nothing after the read is covered, **merge commits included**. A merge's parents say nothing about its own tree, and
anyone can write a merge of any tree with any parents, the repository's root among them, under a message naming the
story. So a pull request's merge of a part read after the read needs `--allow`, or a read of the merge commit
itself, which then covers it.

The story is refused if any commit naming it isn't covered: work pushed after the last part's read still needs
that part read, or `--allow`. Covered commits are listed under `covered_by_parts`, with the story's `parts`.
Coverage is per story. An ancestor that names a *different* story doesn't pass because of this one's part: that
story is still judged on its own state and parts. A part merged by squash isn't an ancestor of the commit read, so it
still needs `--allow` too.

## What it doesn't check

- That the commit verified is the one in the range. `rally show <STORY>` gives `verified_commit` and
  `commit_differs`, and the digest flags a merge whose head wasn't read (RAL-31, RAL-67). Whether a
  re-based head carries the change that was verified is `rally rebased`, below.
- Work that names no story at all.
- A range over 5,000 commits, or a ledger over 50 pages of stories, is refused rather than read in part.

## Is the re-based head the change that was verified? (RAL-170)

When main moves under a verified pull request, its team merges main in, and the head that will merge
isn't the commit that was read. `rally rebased` answers whether it's the same change, observed and
recording nothing:

```sh
rally rebased RAL-170 --head <full sha>                   # in a checkout that has both commits
rally rebased RAL-170 --head <full sha> --checkout ~/src/rally-wt/x --base main
```

- **The comparison:** the verified commit's change against its merge base with main, and the head's
  change against its merge base with main's tip (read from GitHub), compared patch by patch: each
  file's added and removed lines, hunk by hunk, without line numbers or context, so a hunk that only
  moved is the same and a file list that matches isn't enough. Each file is read from its
  `diff --git` line, and its header (up to its first `@@`) gives the facts compared too: a mode
  changed, a rename, a file added or deleted, and a binary file's blob ids, so a binary file, a
  mode change or a pure rename is never invisible. Inside a hunk, a line starting `--- ` or `+++ `
  is a removed `-- ` or added `++ ` line, not a header. A `\ No newline at end of file` after a
  changed line is part of the change, so a head that differs only by a final newline isn't the
  same (RAL-172).
- **What it can't see, said in every answer (`caveat`):** hunks are compared in order without
  line numbers, so a hunk moved, unchanged, to another place in its file reads as the same change.
- **The verdict:** `same change` only when every file's changed lines are identical. Otherwise
  `changed`, with each file that differs and its first differing hunk as it is in each. `can't tell`
  when it can't be compared: a commit missing, GitHub's compare cut short (over 250 commits or 300
  files), a file whose patch GitHub won't show, or a verified commit with no change against main (one
  already merged). It never says `same change` then.
- **Conflict resolutions, said separately:** every merge into the head since the verified commit is
  remerged (`git show --remerge-diff`), and any that differs from the merge git would have made, a
  conflict resolved or a change made in the merge itself, is named with its files. Nobody read that
  change, whatever the verdict.
- **Local git first:** if the checkout has the verified commit, the head and main's tip (`git fetch`
  first). Otherwise GitHub's compares, saying why local git wasn't used; GitHub has no remerge diff,
  so the conflict resolutions are then `null` and said to be unknown, and GitHub's compare shows no
  file modes, so `modes_note` says a mode change can't be seen there (RAL-172).
