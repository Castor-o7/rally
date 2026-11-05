# Levi PR L2: the description

The description for the second pull request to ignition-is-go/levi, from `l2/check-events` at `6148002` on the
Rally team's Levi remote, on Levi main `aed6027` (v0.9.5). It's the first "Not built" row of
[rally-vs-levi.md](rally-vs-levi.md), a tamper check on the events ref, taken from `rally check --expect`. It
doesn't depend on L1. Everything below the line is pasted into the PR as it stands. It carries no attribution
line, by Levi's rule. Whether and when it's opened is for the operator to say.

---

## `levi check-events`: notice when the events ref loses history

Levi only ever adds events, and merges by union. So a head of `refs/levi/events`, once seen, should stay in the
ref's history, with every event it held. Nothing checks that today. A force-push, a reset or a hand-edited tree
can drop tasks, claims or closes, and every command simply materializes what's left.

This adds one command that checks it.

```
levi check-events                    # print refs/levi/events' head, to save elsewhere
levi check-events --expect SHA       # fail unless that saved head is still held
```

### What changes if you change nothing

**Nothing.** It's a new command. No existing command, event, field or default changes.

### What it does

- **With no flag**, it prints the ref's current head. You save it somewhere the ref's writers can't reach: a CI
  artifact, a release note, another repository.
- **With `--expect SHA`**, it exits non-zero unless all of these hold:
  - that commit is the head or one of its ancestors;
  - every event stored at that commit is still in the head's tree. Only the head's tree is read, so a commit on
    top of a saved head could otherwise leave events out and still pass.
- **Each failure gives its own reason:**
  - the ref is absent;
  - the commit isn't in this repository (fetch the ref first);
  - the object isn't a commit (a tree or a blob was passed);
  - the commit is no longer in the history (reset or rewritten);
  - N events from it are missing from the head (rewritten).
- **`--json`** gives `levi.check-events/1`, with `head`, `expected`, `ok` and `reason`.
- **It doesn't change the events ref.** It reads the local ref as it stands and never syncs first, so it checks
  what this checkout holds. Like every levi command it needs git `user.email` set, so set one in CI.

### What it doesn't do

- **Catch events added as someone else.** Anyone who can push the ref can write any event, as anyone. This
  catches removal and rewriting, not forgery.
- **Keep the saved head safe.** That's up to wherever you store it. A head saved where the ref's writers can
  also write proves nothing.

### How it was tested

- `cargo test --workspace` passes all 132 tests: Levi's 125 and 7 new, in `levi/tests/cli_check_events.rs`:
  - a saved head still passes after more events are added;
  - a ref wound back past the saved head fails, and so does a new history that doesn't descend from it;
  - a commit on top of the saved head whose tree leaves a shard out fails, and `ls` is shown to have lost a task;
  - an unknown commit, an absent ref, a short hash and a tree each fail with their own reason;
  - checking doesn't move the ref;
  - two clones of one bare remote append while apart and sync. Heads saved on either clone still pass on both,
    and both list the same tasks.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` and `cargo fmt --all --check` pass.
  `levi-dash` is untouched.

### Commits

Three conventional commits on `aed6027`, with no merge commits.

| Commit | Change |
|---|---|
| `e3eca57` | feat: check that a saved head of refs/levi/events is still in its history |
| `51365d9` | fix: say when check-events' expected object isn't a commit |
| `6148002` | fix: check-events fails when events are missing from a descendant head |
