# Rally's design ideals

**RAL-114.** The anchor is the operator's own statement. Everything after it is labelled: **decided** means
the operator has already ruled on it (most rulings are in [design.md](design.md)), and **proposed** means
the team wrote it to expand on the statement, and it stands only once the operator confirms it.

## The statement

Josh, the operator, on 2026-11-08, relayed verbatim by the watchman (mellow-weevil):

> rally is a system for maintaining a relationship to verified work in a dynamic development environment,
> and providing a sleek, reactive dashboard to view the current state of the dynamic development
> environment, also known as a 'rally'.

That makes two pillars:

1. **The ledger.** It keeps a relationship to *verified* work while the environment keeps changing.
2. **The dashboard.** It shows the *current* state of that environment, the rally, sleekly and reactively.

## Already decided

These come from the operator's earlier rulings and fit under the two pillars. They are listed here so the
ideals can be read in one place, and they are not reopened.

- Rally records and informs. It doesn't command, and agents and the operator can correct it
  ([design.md](design.md), carried over from Muster).
- Rally is the bones and Muster is the musculature: a deterministic ledger of the muster's state, and
  the place where work is recorded and verified ([design.md](design.md), 2026-11-02).
- Rally maintains a relationship to security, both of the information and of the environment
  ([design.md](design.md); [threat-model.md](threat-model.md)).
- A visually appealing web app showing the current ledger and past ledgers is a goal ([design.md](design.md)).
- While a watchman is designated, merges wait for the watchman's read at the head being merged. Otherwise
  another team or the operator verifies ([design.md](design.md), RAL-51 and RAL-52).

## Proposed: what each pillar means

**Proposed, not decided.** These are the team's reading of the statement. Each one names the existing
behaviour it describes, so it adds no new rule until the operator confirms it.

### The ledger: a relationship to verified work

- **P1. Verified is a relationship, not a badge.** A verification is tied to the exact commit that was
  read (RAL-31). When the head moves, Rally says the read and the merge differ rather than letting an old
  read cover new code (`commit_differs`, RAL-33). "Maintaining" the relationship means noticing when it
  lapses.
- **P2. Every fact says how it's known.** Observed, declared or verified, with its source and time (RAL-11).
  Rally never upgrades a declaration into a fact because it was repeated.
- **P3. The environment is dynamic, so the ledger is history, not a snapshot.** Corrections are added
  beside what they correct, never written over it (RAL-3, RAL-21), and the hash chain shows any rewrite
  (RAL-14). The same records always rebuild the same state (RAL-12).
- **P4. Plain facts, no scores.** Track records are counts and dates, never points, ranks or comparisons
  between teams (RAL-21, RAL-23).
- **P5. Cheap to ask.** One query should answer "what's next", "what's waiting" and "who's blocked" so agents
  spend tokens on the work, not on rebuilding state (README; `rally next`, `rally brief`).

### The dashboard: sleek and reactive

- **P6. Reactive means it stays current without being asked.** The viewer updates as the ledger changes,
  says how fresh it is (RAL-90), and reloads itself onto new page code (RAL-112). A stale view says it's
  stale.
- **P7. Sleek serves legibility.** Styling may be expressive, the "modern sci-fi magitech" look of
  RAL-115, but it must never hide or blur a fact's provenance label, a state's colour meaning (RAL-91), or
  the line between ticks and Verified.
- **P8. Read-only, always.** The dashboard cannot change the ledger (RAL-25). It shows the rally. It never
  runs it.
- **P9. The rally is the unit.** The dashboard leads with the open rally: its teams, its stories, its events
  and its report (RAL-104). Past rallies stay readable as past.
- **P10. Everywhere the operator looks.** Light and dark, desktop and phone (RAL-26, RAL-111).

## Open questions for the operator

Recorded as a question to the operator in Rally (`rally block --story RAL-114 --on-team operator`).

1. Do P1 to P10 read right as the meaning of your statement? Strike or reword any of them.
2. Should the statement replace the README's opening description of Rally, or sit beside it?
3. Is "a rally" the open rally (one period with a name, as `rally open-rally` makes it), or the whole live
   environment at any moment? P9 assumes the first.
