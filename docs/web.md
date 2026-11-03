# The Rally viewer

A read-only page showing the rally's status, each roadmap's progress, and each team's status and work.
It runs on your own machine, on a loopback address only.

```bash
# Pair with a read-only credential first (see service.md), then:
rally web --bind 127.0.0.1:7300
# Or, on the host, read the ledger's database directly and read-only:
rally --db postgres://rally@127.0.0.1/rally --head-file ledger.head web --bind 127.0.0.1:7300
```

Then open <http://127.0.0.1:7300>. It needs `gh` signed in to read roadmap files, and reads them every 5
minutes. The ledger and roster are read every 30 seconds.

## How to read the figures

- **Roadmap ticks** are counted from each project's roadmap file, with the file, commit and read time
  shown. A tick means something different in each project, so each is labelled and they're never added
  into one number. `web/projects.json` holds the meanings, or use `--projects`. Every checkbox in the
  file counts, outside code examples. Percentages round down.
- **Verified** comes only from the ledger, and is never worked out from ticks. It reads "not available
  yet" for a project with no stories in the ledger, and it covers only the stories the ledger tracks.
- **Teams** show what's declared to the ledger, and Muster's roster with Muster's own labels: observed,
  attested or declared. "Can be woken" is observed only when the daemon sees a wait, and it's a best guess, and nothing here proves a session is alive.
- **Ledger check** repeats `rally check`. It can't show a rewrite of the whole host without a saved head.

## Project pages (RAL-43)

Each project's name in the roadmap list opens its own page (`#/project/<prefix>`), built only from
`/api/status` for a project in the list; any other prefix shows "Unknown project". A page shows:

- the roadmap's stories, open and ticked, from the same boxes the overview counts;
- the ledger's records for that prefix, or a "not deployed yet" line until the service is (RAL-27);
- the team's day summary: the newest `docs/muster/YYYY-MM-DD.md` in that project's repository at the
  roadmap's commit, with its file, commit and read time. Other names in that folder are ignored;
- the last 20 merged pull requests, number, title and date only. Dated ticks aren't shown yet.

The overview ends with the roadmap boxes across projects (ticks still mean different things in each)
and the watchman's writeup: the day summary on Rally's own entry. Summaries and PR titles are written by
agents, so they're shown as text with the untrusted-text notice, like everything else recorded. All of
it is read through `gh` with the roadmaps, on their five-minute cadence and with their time and size limits.

## What protects it

- **Loopback only.** It refuses any other address. Viewing from a phone is RAL-34, with its own review.
- **Other sites can't read it.** Requests must name this address in `Host`, which stops DNS rebinding, and
  any `Origin` must be its own. There are no CORS headers.
- **Recorded text can't run.** The page only uses `textContent`, and its content security policy allows no
  inline script and no other site.
- **No write path.** It has no way to change anything. With `--db` its Postgres connection is read-only, and it should use a
  read-only credential with the service. The page warns if it isn't.
- **A failed check shows.** A broken chain appears as a red failed check, with the contents withheld. If the
  viewer stops answering, the page replaces the check with a warning.
- **Stale data shows.** `gh` and `muster-shim` reads give up after 20 seconds. Service reads have a 15-second limit on each connection step, not overall (RAL-42). A green check turns into a warning if the ledger
  hasn't been read for 3 minutes, and roadmaps warn after 15.

## What it doesn't protect against

- Any program on your machine and account can read the page, because it has no login. So can other accounts
  on the machine, which is outside the current threat model.
- Agents' text can still mislead a reader: it can't run, but it can say anything.
- The roadmap and roster read through your `gh` and `muster-shim` credentials, in a process that only runs
  fixed read commands. Project names in `--projects` come from you and are trusted.
