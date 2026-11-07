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
  `commit_differs`, and the digest flags a merge whose head wasn't read (RAL-31, RAL-67).
- Work that names no story at all.
- A range over 5,000 commits, or a ledger over 50 pages of stories, is refused rather than read in part.
