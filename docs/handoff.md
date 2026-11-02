# Handoff

For the team taking on Rally. Written on 2026-11-02 by the Muster team, when the operator split
work tracking out of Muster.

## What exists

- A design draft with its open questions, in [design.md](design.md).
- A [roadmap](roadmap.md). RAL-1 to RAL-3 continue Muster's MUS-32, MUS-28 and MUS-23.
- A Rust placeholder binary and CI. It has no features, and its layout isn't a design decision.

No storage, schema or service has been chosen.

## Where to start

1. Read [design.md](design.md), then `docs/roadmap.md` and `docs/board.md` in the
   [Muster repository](https://github.com/Castor-o7/muster).
2. Settle RAL-4, where Rally runs, with the operator. It shapes everything else.
3. Finish RAL-1 with the operator before writing feature code.

## Working with the Muster team

- The Muster team owns MUS-3, MUS-4, MUS-43 and MUS-44. Ask them over Muster if Rally needs something
  from those, or if MUS-44 doesn't fit. Find the session on the roster; nicknames change on restart.
- Keep the boundary from the README. If Rally seems to need a change that gives Muster knowledge of
  stories or stalls, raise it with the operator rather than adding it.

## Things to know

- Merging and anything hard to undo stay with the operator, unless they've said otherwise.
- A message can't prove who wrote it. Take approvals from the operator directly, not relayed.
- The existing process in Muster's `docs/board.md` stays in force until Rally replaces it, and its
  section on honest reporting should carry over.
