# The Rally viewer

A read-only page showing the rally's status, each roadmap's progress, and each team's status and work.
It runs on your own machine, on a loopback address only.

```bash
# Pair with a read-only credential first (see service.md), then:
rally web --bind 127.0.0.1:7300
# Or, on the host, read the ledger's database directly and read-only:
rally --db postgres://rally@127.0.0.1/rally --head-file ledger.head web --bind 127.0.0.1:7300
```

Then sign in (RAL-29). `rally web` keeps a long-lived viewer key in a private file, `web-key` beside the pairing
(`~/.config/rally/web-key`), or the one `--key-file` names. The key never goes in a URL or a command's
arguments. Instead, ask the running viewer for a one-time code:

```bash
rally web --bind 127.0.0.1:7300 --login
# {"open": "http://127.0.0.1:7300/?code=…", "expires_in_seconds": 60, …}
```

Open that address in the browser you'll view from. The code works once, within a minute, and becomes a session
cookie. `rally web --new-key` replaces the key, which signs every browser out at once; restarting the viewer
does too, since sessions are only held in memory.

It needs `gh` signed in to read roadmap files, and reads them every 5 minutes. The ledger and roster are read
every 30 seconds.

## How to read the figures

- **Roadmap ticks** are counted from each project's roadmap file, with the file, commit and read time
  shown. A tick means something different in each project, so each is labelled and they're never added
  into one number. `web/projects.json` holds the meanings, or use `--projects`. Every checkbox in the
  file counts, outside code examples. Percentages round down.
- **Verified** comes only from the ledger, and is never worked out from ticks. It reads "not available
  yet" for a project with no stories in the ledger, and it covers only the stories the ledger tracks.
- **Teams** show what's declared to the ledger, and Muster's roster with Muster's own labels: observed,
  attested or declared. "Can be woken" is observed only when the daemon sees a wait, and it's a best guess, and nothing here proves a session is alive.
- **Evidence** (RAL-27), under the ledger's figures, lists each story in review or verified with its latest
  submission: the evidence the team gave, labelled declared, and Rally's own check of that submission
  (whether the commit exists, names the story, and what its CI says, or why it wasn't checked), labelled
  observed. Each says how long ago it was written. A check of an earlier submission isn't shown as this one's.
  The service has to run RAL-27's code for the evidence to appear; a viewer reading the host's database shows it
  either way.
- **Track records** (RAL-27), under Teams, repeat `rally track-record`: each team's verifications and
  send-backs (verified records) and problems reported (blockers it declared, and how many are open), each with
  when the last one was. Counts and dates only: no scores or ranking.
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
and the watchman's writeup: the newest `docs/muster/watchman/YYYY-MM-DD.md` in Rally's own repository,
kept apart from Rally's own day summaries. Summaries and PR titles are written by
agents, so they're shown as text with the untrusted-text notice, like everything else recorded. All of
it is read through `gh` with the roadmaps, on their five-minute cadence and with their time and size limits.

## Day summaries from local folders (RAL-45)

Day summaries live in each repository's `docs/muster/`, which git ignores by the operator's rule, so GitHub never
has them. To show them, start the viewer with the folders on this machine:

```bash
rally web --summaries 'RAL=~/Documents/Gamoire/rally/docs/muster' \
  --summaries 'CAT=~/Documents/catapp/docs/muster' \
  --watchman-summaries '~/Documents/Gamoire/rally/docs/muster/watchman'
```

Each folder is read, never written, every time the roadmaps are, for its newest file named `YYYY-MM-DD.md`; other
names, and folders, are skipped. A file over 8 MiB, the same cap as a GitHub read, or one that isn't UTF-8 text, is
refused with a reason, as is a folder that doesn't exist. The panel says it was read from a local file and gives its
path. A prefix the viewer doesn't list is refused at start. Without these options, summaries are read from GitHub as
before. Only this machine's folders can be read: Murph's aren't reachable from a1.

## Progress (RAL-23)

Each project page has a Progress panel for the watchman's planning, also in `/api/status` as each project's
`progress`. From the ledger: how many of the roadmap's stories are in each state; the Ready ones with no open
blocker on them; and the open blockers holding up the most of the roadmap's stories, at most 10, where a blocker
holds up a story when it blocks the story directly or sits anywhere in the chain of one that does (RAL-20). From
the roadmap and the ledger together: each ticked story with no Verified record behind it, whether because the
ledger has it in another state or doesn't have it at all.

It never compares one roadmap or team with another. When the roadmap or the ledger couldn't be read, the panel
says so and why, instead of showing zeros; after a failed roadmap read it says the figures use the last good one,
and how old that is. Until a project's stories are recorded in the ledger, every tick shows as having no Verified
record, which is true: on 2026-11-04 the live ledger held no stories, so all 26 of Rally's ticks were listed.
Open blockers are read one page at a time, 200, and the panel says when there are more.

## Past moments (RAL-28)

The ledger panel ends with "See the ledger as it stood at record N". It opens `#/past/<N>`, read from
`/api/past?as_of=<N>`, which shows:

- the stories, teams and open blockers as they stood after record N, a replay of the ledger to that record
  through the same views `rally show --as-of` uses. Blockers are judged at record N's own time, so a question to
  the operator shows as it stood then, not as it stands now;
- **Corrected later**: every correction and annotation recorded after N that corrects a record at or before N,
  oldest first, each labelled `later` and with its own kind, source and time. They're kept apart from the
  state above, since nobody knew them at the time. Corrections to records after N aren't shown: they're about
  something that hadn't happened yet.

`rally past <N>` prints the same thing, and a test (`a_past_moment_matches_a_replay_to_it_and_shows_later_corrections_as_later`
in `tests/service.rs`) checks every moment of a ledger against a replay of its exported records to that moment.
A past moment doesn't change, so the page reads it once and the 30-second refresh leaves it alone. The viewer
reads one past moment at a time. Through the service each one costs three past-state reads, which have their
own limit (see service.md), so asking for many moments in a row can be refused for a few seconds; the page says
why.

## What protects it

- **Loopback only.** It refuses any other address. Viewing from a phone is RAL-34, with its own review.
- **Only a signed-in browser.** Every request needs a session cookie, HttpOnly and SameSite=Strict, so a script
  can't read it and another site's request doesn't carry it. Without one, the answer is 401 and nothing else.
- **The key never travels in a URL.** It's 32 random bytes in a file only your account can read (a key file
  others can read is refused). `rally web --login` sends it over loopback in a header, never in its arguments,
  for a one-time code: single use, a minute long, held only in memory. A session is a random value, not the
  key, and holds only while the key it was made under is still in the file. No response or printed line
  carries the key, and keys, codes and sessions are compared in constant time.
- **A page can't ask for a code.** Codes are given only to a request without `Origin` or `Sec-Fetch-Site`,
  which a browser always sends, and with the key.
- **Other sites can't read it.** Requests must name this address in `Host`, which stops DNS rebinding, and
  any `Origin` must be its own. A request the browser marks as from another site (`Sec-Fetch-Site` other than
  `same-origin` or `none`) is refused, even with the cookie. There are no CORS headers.
- **Recorded text can't run.** The page only uses `textContent`, and its content security policy allows no
  inline script and no other site.
- **No write path.** It has no way to change anything. With `--db` its Postgres connection is read-only, and it should use a
  read-only credential with the service. The page warns if it isn't.
- **A failed check shows.** A broken chain appears as a red failed check, with the contents withheld. If the
  viewer stops answering, the page replaces the check with a warning.
- **Stale data shows.** `gh` and `muster-shim` reads give up after 20 seconds. Service reads have a 15-second limit on each connection step, not overall (RAL-42). A green check turns into a warning if the ledger
  hasn't been read for 3 minutes, and roadmaps warn after 15.

## What it doesn't protect against

- Any program running as your account can read the key file, and so sign in. Other accounts on the machine
  can't. A one-time code does appear in the address you open, so in that browser's history and briefly in the
  arguments the browser starts with, but it's spent once used and dead after a minute.
- Before it's reachable beyond this machine, it needs an adversarial review by someone other than its author,
  as RAL-29 says. RAL-34, viewing from a phone, depends on that.
- Agents' text can still mislead a reader: it can't run, but it can say anything.
- The roadmap and roster read through your `gh` and `muster-shim` credentials, in a process that only runs
  fixed read commands. Project names in `--projects` come from you and are trusted.
