# The Rally viewer

A read-only page showing the rally's status, each roadmap's progress, and each team's status and work.
It runs on your own machine, on a loopback address only.

## One command: `rally view` (RAL-82)

```bash
rally view                 # start it if it isn't running, and open it in your browser, signed in
rally view --restart       # restart it, even a viewer started by hand with `rally web`
rally view --stop          # stop it
```

The first time, give it the settings `rally web` takes, for example
`rally view --summaries DSM=~/Documents/GitHub/Discoteque/docs/muster --watchman-summaries ~/src/rally/docs/muster/watchman`.
They're saved in `view-settings.json` beside the pairing (`~/.config/rally/`), so after that `rally view` alone is
enough, and a setting given again replaces the saved one. `rally view` restarts the viewer by itself when it runs
another build of Rally than the one you ran (by the commit it was built from, so a rebuild of the same commit
doesn't count; a build from uncommitted changes is told apart by its bytes, RAL-100), so an update shows up the next time you open it, or when it runs
other settings. The viewer keeps running after the command ends. Its output is in `view.log`, and what's
running is in `view-run.json`, both in the same folder. The viewer reads with the read-only pairing,
`viewer.json` there, when it exists, and none of these files ever takes that name. A viewer started by hand is opened as it is until
`--restart` replaces it. With `--no-open`, it prints the one-time address instead of opening a browser, and `--browser "flatpak run
com.google.Chrome"` opens it in a browser other than the desktop's default.

## By hand

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

Open that address in the browser you'll view from and press **Sign in**. The code works once, within a minute,
and becomes a session cookie. Opening the address spends nothing; only the button does (RAL-73). Browsers fetch
addresses ahead of a visit (Chrome preloads what's typed or pasted into the address bar), and before RAL-73 such
a fetch spent the code, so the visit itself found it used. `rally web --new-key` replaces the key, which signs every browser out at once. Restarting the viewer
signs no one out (RAL-80): each session is kept, as the SHA-256 of the session and of the key it was made under,
in a private file beside the key (`web-key.sessions`), and read back at start, so the file holds no cookie that
works. A session lasts 14 days from its sign-in, checked on every request and at start, and only the newest 32 are
kept. Sessions made under another key are dropped at start, and `--new-key` removes the file.

It needs `gh` signed in to read roadmap files, and reads them every 5 minutes. The ledger and roster are read
every 30 seconds.

The page polls every 30 seconds, and between polls every "12s ago" keeps counting on its own, so an age is always
current (RAL-90). A poll doesn't rebuild the page (RAL-89). Each section is drawn into a copy off the page, and
the page's own section is patched to match: a node that hasn't changed stays the same node, and only text and
attributes that differ are changed. So nothing flashes, and focus, a selection, a field being typed in and the
scroll position stay where they were. A section the reader opened or closed keeps that state, even on a draw
that isn't a poll, such as a change of page. The line under the title says "● Live · updated 3s ago": the dot turns amber once a poll is more
than 45 seconds late, and the line turns red after 3 minutes, or when the viewer can't be reached. Only a change
of that state is announced to a screen reader, not each second. Verified is green and In review amber, on every
bar too (RAL-91, below). A commit
in evidence written as `owner/repo@<40 hex>` shows as `owner/repo@<first 7>`, with the whole of it in its
tooltip, and IDs and ages stay on one line. Colours and that shortening can still change when the look is
agreed with the operator (RAL-26).

## The look (RAL-115)

The viewer's style is "modern sci-fi magitech", from the operator on 2026-11-08, and serves the dashboard pillar of
[ideals.md](ideals.md) (P6 to P10): smoked-glass panels with corner brackets over a faint grid and two slow auroras, section
names in spaced monospace capitals after a small rune, and a live indicator that breathes while polls arrive. The
decoration has colours of its own (`--arc-1`, `--arc-2`), which never stand for a meaning: every state colour and
`--ticked` keep the values RAL-91's test reads, and a bar's sheen is a shadow, so its background is exactly its
meaning's colour. Motion stops under `prefers-reduced-motion`. And it must stay light (RAL-118): no `backdrop-filter`, which
recomputed a blur under every panel on every scrolled frame; no filter on anything that moves; and every endless
animation only turns or fades a box of its own, which the compositor does without the page repainting. Everything is served by the viewer itself, as the
page's CSP requires, so there are no web fonts: the font stacks name ones the machine may have, and fall back to the
system's. A section fades up only when it's first drawn; a poll patches it in place (RAL-89), so it never replays.

## How to read the figures

- **Roadmap ticks** are counted from each project's roadmap file, with the file, commit and read time
  shown. A tick means something different in each project, so each is labelled and they're never added
  into one number. The projects and their meanings come from `--projects` if given, otherwise from
  `projects.json` beside the pairing (`~/.config/rally/projects.json`) if it exists, and otherwise from the
  built-in `web/projects.json` (RAL-74). The viewer's first line says which, as `projects`. A file that's
  there but can't be read or isn't a list stops the viewer, rather than falling back. Every checkbox in the
  file counts, outside code examples. Percentages round down. A roadmap that's read but has no checkboxes at
  all, such as DiscoSim's table, has no ticks to count, so its card shows **Progress from the ledger** instead
  (RAL-81): Verified of the stories the ledger tracks under its prefix, labelled as the ledger's. It isn't added
  to the boxes across projects, which name the projects left out.
- **Verified** comes only from the ledger, and is never worked out from ticks. It reads "not available
  yet" for a project with no stories in the ledger, and it covers only the stories the ledger tracks.
- **Colours follow meaning** (RAL-91). Each Verified bar, and each Progress from the ledger, is one bar over
  the stories tracked: Verified green, In review amber and Sent back pink, in the colours of the stories-by-state
  bar, each segment titled with its count, with a legend under it, and the rest as empty track. Ticks aren't a
  ledger state, so tick bars have a neutral colour of their own (`--ticked`), which the legend under each tick
  figure names. Green only ever means Verified, so a ticked box the ledger hasn't verified can't pass for one.
  A headless Chrome test reads every bar's computed colour in the light and dark schemes.
- **Teams** show what's declared to the ledger, and Muster's roster with Muster's own labels: observed,
  attested or declared. The roster lists each team's session (RAL-99) with the directory it was started in
  (home folder as `~`, the whole path on hover), its project and branch, its machine, its status (busy or idle,
  and whether a message would wake it, observed only when the daemon sees a wait: a best guess), what it says
  it's working on and when that was set, and its last action with the tool it called. Nothing here proves a
  session is alive. The operator's address isn't sent to the page.
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

## With no rally open (RAL-69)

The overview shows only the projects in the current rally: those where a team on Muster's roster now holds a story
(claimed, in review or sent back). A team is on the roster when a session's nickname is the team's name, or a session
matches the team's `sessions` in `projects.json`, as `rally wake` matches them. A team's name is never guessed from a
session's project. The roadmap cards, the ledger's figures and evidence, the declared teams, the track records and the
totals across projects all follow it. The ledger status panel says which prefixes are in, labelled observed.

Nothing is removed. Other projects are listed, collapsed, under "Not in this rally", and each keeps its page and
its records; `rally show` and past moments still show everything. If the roster or the ledger can't be read, the
viewer can't tell, so it shows every project with a warning instead of hiding any. `/api/status` carries the reading
as `rally` (`known`, `prefixes`, `held`, `active_teams`) and each project's `in_rally`.

## Rallies (RAL-76)

A rally is opened and closed in the ledger:

```bash
rally open-rally --name "Friday" --project MUS --project RAL --project MAR --team plush-tortoise --team wise-leopard
rally rallies                          # the open rally and every past one
rally close-rally --note "stood down"  # it becomes a past rally, kept whole
```

While a rally is open, the overview is its board: only its projects, only its teams, and only what was recorded
since it opened, plus each of its projects' stories still held (claimed, in review or sent back) whenever that
was, so work in flight when the rally opens stays on it. The rally's teams are shown whatever the age of their last
declared state. That covers the ledger's figures and evidence, the declared team states, the roster's sessions,
and the watchman's writeup if it was written before the rally opened (RAL-104). A listed session brings in the ledger team of its Muster
project, so `--team plush-tortoise` brings `rally` too. Track records stay all-time, and say so.

A rally lists sessions by their Muster nickname. By the operator's ruling of 2026-11-06, the rally takes in the
agents the operator started that are open and available when it opens, and nobody else unless specified (RAL-108).
The one exception is a listed session restarted under a new nickname, which continues as that session's successor.
While a rally is open, the ledger keeps, for each ledger team, the declared names that recorded for it, with their
first and last records and pairing credentials (`recorded_as` in `rally rallies`). A new nickname succeeds a listed
one when all of these hold:

- the listed nickname is no longer on Muster's roster;
- the new nickname recorded for a team the listed one recorded for;
- its first record for that team came after the listed one's last;
- it used the same pairing credential.

Each nickname has at most one successor, the earliest, and a successor can have its own when it restarts in turn.
So when the watchman restarts as mellow-weevil, it rejoins with its first record for `watchman`, and the page shows
"mellow-weevil after frank-crocodile". A session started later that records for `rally` while the rally's `rally`
session is still open doesn't join. With no roster to read, only the listed nicknames count. The names and the
roster are declared, so a succession is a reading, not proof. The rally's own list doesn't change.

The operator specifies the rest (RAL-110):

```bash
rally rally-session add cobalt-glen --note "helping on MAR"           # add a session to the open rally
rally rally-session add mellow-weevil --succeeds frank-crocodile --note "restarted"   # confirm a restart
rally rally-session remove late-lark --note "not a restart; started later"            # take a session out
```

An added session is in, and a removed one is out, whatever would otherwise take it in, whether it was listed, added
or detected as a restart. The latest record for a nickname decides. A detected restart counts straight away, and the
page shows it as an "unconfirmed restart" until it's confirmed with `--succeeds` or removed. A confirmed restart stops
another nickname being detected for the same session. The page lists what was added and removed since the rally
opened. Like opening and closing a rally, only the operator or the watchman may do it once `--rally-openers` is
set. It's one record kind, `rally_session`, and the first one ends the rollback window (see `docs/service.md`).

Nothing is removed. The ledger is append-only and hash-chained, and deleting records would fail its check, so a
closed rally is kept whole. The page lists past rallies, and each opens as its own board (`#/rally/<record that
opened it>`, RAL-77): the ledger as it stood when it closed (RAL-28), showing only its projects' stories recorded
while it was open or still held then, and its teams: those it named, and those holding its stories. The roster of
the time isn't kept, so a session that brought its project's team in then doesn't now. The board links to the
whole ledger at that record.
Projects outside the rally keep their pages. A project in the rally without an entry in `projects.json` is named
in a warning.

Opening and closing are declared, like every name in Rally, and the viewer shows who opened the rally. Only the
operator or the watchman may open or close one once the service's `--rally-openers` is set; see `docs/service.md`
(RAL-77). A second rally can't open while one is open. With no rally open, the viewer falls back to RAL-69's
reading below. A service older than RAL-76 has no `/v1/rallies`, and the page says so; any other failure to read
the rallies (a refused token, a network error) is a warning instead, since an open rally may then be missing.

## The report box belongs to the open rally (RAL-104)

While a rally is open, the report box at the end of the overview shows only what belongs to it:

- **Its events**, newest first, above the writeup. Anyone records one while a rally is open:

  ```bash
  rally event --headline "Leaked headless Chromes filled /run/user/1000" --severity incident \
    --backing "Castor-o7/rally#170" --backing "record 986"
  ```

  An event has a one-line headline, a severity (`notice` or `incident`) and, optionally, up to 8 things backing it
  (record numbers, pull requests, commits or files), each one line. Each is listed with who declared it, its record
  and its age, and an event with nothing backing it says so. Like every claim in Rally, it's declared: the viewer
  doesn't check the backing. With no rally open, `rally event` is refused, since there's no rally for it to belong
  to. A closed rally keeps its events, shown on its past-rally board, and the next rally starts with none.
- **The watchman's writeup, only if it was written after the rally opened.** Being dated the same day isn't
  enough: a rally opened in the afternoon doesn't take the morning rally's writeup. The viewer decides from when
  the file was written: its latest commit's time on GitHub (read with `gh api repos/<repo>/commits?path=<file>`),
  or its modified time with `--watchman-summaries`. Then it compares that with the record that opened the rally.
  An older writeup is named as written before the rally opened, and isn't shown. If the time can't be read, the
  page says so, and doesn't show the writeup as this rally's. A commit's time is the committer's clock, so a
  writeup committed from a machine whose clock is far off can land on the wrong side of the opening.

## A tab open across a restart reloads itself (RAL-112)

The page draws whatever `/api/status` sends, but its own code is loaded once, when the tab opens. So a tab left
open while the viewer restarted onto a new build kept drawing new data with old code. On 2026-11-06 that showed the
DiscoSim writeup in rally 0x5's report box, long after RAL-104 had hidden it, until a new window was opened.

The viewer now takes a fingerprint of the page code it serves: the first 16 hex digits of the SHA-256 of its index
page, script and stylesheet. It stamps that into the page (`<meta name="rally-page-code">`), and every status carries
it as `page_code`. When a status names other code than the tab runs, the tab reloads itself once to run it. It waits
while you're typing or have text selected, and reloads at most once for each code (kept in the tab's
`sessionStorage`), so a mismatch that persists can't make it reload forever. A tab opened before this change has no
fingerprint, so it needs one last reload by hand.

## Labels, once per column (RAL-93)

Every fact on the page is labelled with how it's known. A table column whose cells are all known the same way
says so once, in its header: Evidence's "declared", for instance, or Rally's check's "observed". A column that
mixes kinds keeps a label on each cell that has one. The State column is the usual case: a story in review carries
"declared", and a verified one carries none, since its state says so. A legend at the foot of the page says what observed, declared,
verified and attested mean.

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

A writeup's or day summary's markdown is drawn (RAL-94): headings, paragraphs, lists (nested by indent),
quotes, rules, fenced code and pipe tables, with inline code, bold, italics and links. The page builds each
part from text nodes and never uses `innerHTML`, so any HTML in the file, a `<script>` or an `<img onerror>`,
shows as text and runs nothing. A link is a link only to an `http` or `https` address, and opens in a new tab
with no referrer and no handle back to the viewer. Any other address, `javascript:` and `data:` included, is
shown as its label and the address in brackets, as text. A headless Chrome test checks each of these.

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
- **Only pressing Sign in spends a code.** `GET /?code=…` answers a page with one form, which posts the code
  back to `/`; the page has its own content security policy, which allows that form and nothing else (no
  script, no style, no other site). It shows the code only if it's a code's 64 hex digits. The POST is held to
  the same `Origin` and `Sec-Fetch-Site` rules as every request, so another site's form can't press it, and a
  POST without a code is still refused as a write. The sign-in page's referrer policy is `same-origin` (header
  and `<meta>`), where every other page's is `no-referrer`: under `no-referrer` a browser sends `Origin: null`
  with a form's POST, and `Origin: null` is refused, so the button never worked in Chrome. `same-origin` sends
  the real origin back to this page and nothing to any other site; the page links nowhere else.
- **A page can't ask for a code.** Codes are given only to a request without `Origin` or `Sec-Fetch-Site`,
  which a browser always sends, and with the key.
- **Other sites can't read it.** Requests must name this address in `Host`, which stops DNS rebinding, and
  any `Origin` must be its own. A request the browser marks as from another site (`Sec-Fetch-Site` other than
  `same-origin` or `none`) is refused, even with the cookie. There are no CORS headers. The refusal says how to
  get in instead (RAL-88): run `rally web --login` in a terminal and open the address from there or the address
  bar, since a link opened from another page or a browser extension (such as Claude in Chrome) is marked as from
  another site, and correctly refused.
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
  fixed read commands. Project names in `--projects`, or in `projects.json` beside the pairing, come from you
  and are trusted.
