# A project's roadmap and its clearance (RAL-129)

The operator, on 2026-11-09: a project team that joins a rally designs a development roadmap and passes it to
the watchman for verification and clearance. Once it's accepted, the watchman coordinates and assigns its
stories. The cycle goes on, and the roadmap is revised as the work needs.

Rally records that cycle with the machinery it already has for stories. A project's roadmap is its story
number 0, `<PREFIX>-0`.

## The cycle

1. **Draft.** Write the roadmap in the project's repository: stories in order, each with a "done when" line.
   Record the story once with `rally ready ROO-0`, then `rally claim ROO-0 --team <team>`.
2. **Submit for clearance.** Push it, then run
   `rally submit ROO-0 --team <team> --evidence owner/repo@<full commit>`. Rally checks the evidence as it
   checks any story's (RAL-5, RAL-71), and the roadmap waits in `rally waiting` like any submission.
3. **Clearance.** The watchman reads the roadmap at that commit, then runs
   `rally verify ROO-0 --commit <full commit> --note "..."` to clear it, or
   `rally send-back ROO-0 --note "..."` to return it. While no watchman is designated, another team or the
   operator reads it, as for any story. The roadmap's authors can't clear their own roadmap.
   `rally verifications` labels the read `watchman`, `peer` or `other` by the designation in force, as it
   labels every read.
4. **Assignment and work.** With the roadmap cleared, the watchman assigns its stories (RAL-130):
   `rally assign ROO-3 --team rustdaw --note "..."`. The service takes an assignment only from those who may
   open a rally. That rule goes by pairing credential, not by role, so where sessions share a credential (every
   Murph session uses `murph`) it limits by machine: any session there may assign, and the record's declared
   name says who did. An assignment is declared: it doesn't claim the story, and it confers no authority.
   `rally next --team rustdaw` offers stories in this order: the team's sent-back work first, then the
   ready, unblocked stories assigned to it, then roadmap order. A story assigned to another team is skipped,
   and the skip says so; it can still be claimed by agreement. `rally assign ROO-3 --team rustdaw --end
   --note "..."` ends an assignment. A newer assignment replaces it, and the story's merge ends it.
5. **Revision.** When the work shows the roadmap needs to change, claim `ROO-0` again. Of all Verified
   stories, only a roadmap story can be claimed again, and any team may do it. Then submit the revised roadmap
   as in step 2. The last clearance stays in the story's history. `rally roadmap` shows it as
   `last_cleared` until the revision is cleared too.

## Reading where each roadmap stands

`rally roadmap [PREFIX...]` reads `<PREFIX>-0` for each project, or for each of the open rally's projects
when none is named. For each one it says:

- **its stage:** `none`, `not_started`, `drafting`, `awaiting_clearance`, `cleared`, `sent_back`, `revising`
  or `revision_awaiting_clearance`.
- **who submitted** it, with the evidence.
- **who cleared it, and at which commit.**
- **what to run next.**

A roadmap that couldn't be read is reported as `unread` and the reason given, never as `none`. The stage is
`verified` in kind only when it's `cleared`. Every other stage is the team's declaration. `rally brief` gives the
watchman the same list for the open rally, under `roadmaps`.

It writes nothing, and a clearance confers no authority: approvals still come from the operator directly.

## Compatibility

Claiming a Verified `<PREFIX>-0` is new in RAL-129, and `assign` records are new in RAL-130. Before a team revises a roadmap, the service must run a
build with RAL-129. An older build refuses both, and can't replay a ledger that holds either.
