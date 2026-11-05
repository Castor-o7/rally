# Picking the next story (RAL-56)

`rally next --team <team>` gives a team its next story by a rule it can follow without a watchman. It runs in the
team's checkout, reading the roadmap there (`--roadmap`, `docs/roadmap.md` by default) and the ledger through the
paired service. It writes nothing: when a skip or a shelve should be recorded, it says which command to run.

The rule comes from the operator's question of 2026-11-05, whether a rally can run without a watchman, and the
operator's caveat: "if the operator doesn't respond within a certain reasonable window of time then shelve the
item. don't just inherently skip operator interaction items please". The operator then gave the times: "maybe
like 5 minutes, and if the operator skips interaction 3 times then the operator interaction items are all shelved
for an hour and then revisted after the hour has elapsed", and said the count starts again after the hour.

## The rule

The next story is, in order:

1. one of the team's own that was **sent back**, which comes ahead of new work;
2. a story the team **shelved whose question the operator has since answered**, oldest shelf first;
3. the **first story in roadmap order** that's unticked, has an ID, isn't claimed, in review or verified in the
   ledger, and isn't skipped.

The team's own claimed and in-review stories are listed as `current`, above the pick.

## Skips

A story is skipped, with the reason shown, when:

- another team holds it, it's in review, or it's verified and waiting for its tick;
- it has an open blocker on another team or story, including another team's question to the operator;
- its text says `Depends on X` and X is neither ticked on this roadmap nor verified or merged in the ledger.

A depends-on skip that isn't recorded yet comes with the `rally block` command that records it, so the skip
and its reason go into the ledger in one command.

## Stories that need the operator

Rally can't tell from a roadmap that a story needs the operator, so the team decides. A story that does **isn't
skipped**:

1. The team asks the operator, through Muster to the operator's address, and records the question:
   `rally block --team <team> --story <STORY> --on-team operator --note "<the question>"`.
2. Until the window agreed with the operator has passed, the story stays the pick. `rally next` shows when the
   question was asked and when a shelve is due, and `after_shelving` names the story that would come next.
3. Once the window has passed, `rally next` says to run `rally shelve <N> --team <team>`. The shelve is a record
   that keeps the question and when it was asked. The service checks it against its own clock and its own
   window, and refuses it before the window has passed. The team then takes its next story.
4. When the operator answers, the team records it with `rally unblock <N> --note "<the answer>"` and
   `--direct`, or `--relayed-by <who>` if someone passed it on. Closing a question to the operator must say
   which, and closing any other blocker mustn't. The shelved story then comes back first.

`operator` is reserved: no team can use it as its own name, in a claim, a submit, a block or a team state, so no
team can look like the operator answering.

## When the operator is away

There's one operator, so shelves are counted across the whole rally. **Three shelves in a row with no answer from
the operator between them** mean the operator is away:

- Rally itself records it, as an **observed** `operator_away` record with when it started, when it ends (an hour
  later) and the three shelves that caused it. No agent can write one, and the ledger refuses one whose shelves
  aren't the run of unanswered shelves.
- **During the hour,** no story waiting on the operator is offered. A question already open is held, and
  `rally next` says not to ask new ones. Shelved stories stay shelved.
- **An answer during the hour** ends it at once, judged by the service's time on the answer's record, never a
  time a writer declares, as a backfill does. Answered shelves come back first, as always. Other shelves stand
  until their own questions are answered.
- **After the hour,** every story it held, shelved or not, comes back into the pick in roadmap order, to be asked
  again. Its window counts from the hour's end, and it can be shelved again after that, keeping each shelf.
- **The count starts again** at any answer from the operator, and when the hour ends: it takes three new
  unanswered shelves to make the operator away again.

`rally blockers` shows the latest `operator_away` and the `unanswered_shelves` counting towards the next one.

## The times

The operator agreed them, and the service keeps them, with flags or environment variables to change them:

| | Default | Flag | Variable |
|---|---|---|---|
| Wait before a shelve | 5 minutes | `--operator-window` | `RALLY_OPERATOR_WINDOW` |
| Unanswered shelves that mean away | 3 | `--operator-away-after` | `RALLY_OPERATOR_AWAY_AFTER` |
| How long away lasts | 1 hour | `--operator-away` | `RALLY_OPERATOR_AWAY` |

Times are a number with `s`, `m`, `h` or `d`. `GET /v1/whoami` reports all three, and `rally next` uses the
service's, so what it says is due is what the service will accept.

## Limits

- The roadmap is read as the team's checkout has it, which may be behind `main`.
- "Depends on" is read from the item's text, up to the end of that sentence, and only IDs written in full (or
  as a range, `RAL-54 to 57`) count. A dependency written another way isn't seen.
- A story's claim and blockers are declared, as everything in the ledger is. The pick is only as good as what
  teams record.
