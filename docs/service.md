# The Rally service

One service on one machine is the ledger's single writer (RAL-4, RAL-13). Machines reach it with the
`rally` command.

The ledger lives in Postgres on the same host, as myko entities (RAL-39, `docs/design.md`). The service runs myko
inside its own process without myko's listener, so its TLS port is the only way in: no client talks to myko.

## Setting it up

On the host, with a Postgres database for the ledger that only this account can reach:

```bash
export RALLY_STATE_DIR=~/.local/state/rally
export RALLY_DB=postgres://rally@127.0.0.1/rally
export RALLY_HEAD_FILE=~/.local/state/rally/ledger.head
# Only if the role has a password: one line, in a file only you can read (chmod 600).
export RALLY_DB_PASSWORD_FILE=~/.local/state/rally/db-password
rally serve --bind <LAN address>:6300
rally add-machine a1 --address <LAN address>:6300 --out a1.pair
```

`--db` (`RALLY_DB`) is the Postgres URL, and `--head-file` (`RALLY_HEAD_FILE`) is the ledger's head file, kept
outside Postgres (see "Detecting tampering"). Keep any password out of the URL, because the URL can show in
process listings, and Rally's Postgres library doesn't read `~/.pgpass`. Put it in the file named by
`--db-password-file` (`RALLY_DB_PASSWORD_FILE`) instead: Rally refuses one that other accounts can read or that
another account owns, and a password given in both the URL and the file. Messages show the address rebuilt
without any password, however the URL gave it, and `--help` doesn't show the values of these variables. Rally makes its table, `rally_events`, on first start. Its role needs to be able to create
it, and to read `data_directory` for the disk floor, which a superuser or a member of `pg_read_all_settings` can.
Postgres must be on the same host, with that directory visible to Rally.

The file holds a pair code, which holds a secret, so treat it like a password. It's private to your
account and is never printed, to keep it out of agents' transcripts. Move it to the machine being paired
without showing it, then:

```bash
rally pair < a1.pair && rm a1.pair
export RALLY_NAME=<who you are>
rally show
```

`rally add-machine viewer --read-only --address <LAN address>:6300 --out viewer.pair` makes a credential that can read but not write. `rally revoke <name>`
stops a token working straight away. Don't expose the port to the open internet.

## What's enforced

- **TLS with a pinned certificate.** A client only trusts the certificate named in its pair code.
- **A token on every route.** The host stores only hashes. A missing or wrong token gets a 401, including
  on routes that don't exist.
- **No web pages.** Any request carrying an `Origin` header is refused.
- **The service sets each record's credential and time.** A client only supplies a declared name, a kind and
  a change. Only Rally itself may record observed facts.
- **Read-only is enforced before any route.** A read-only credential can only use GET and HEAD.
- **Limits.** 16 KiB per request, and for each credential a request rate and a much lower write rate. The
  defaults allow about 17,000 writes a day, so up to about 280 MB. Writes stop when the ledger's table in
  Postgres, with its indexes, reaches 64 MiB, or while the disk under Postgres's data directory has under 5 GiB
  free, and reads carry on. If either can't be checked, writes stop too. Both are checked under the writer's
  lock, so concurrent writes can't pass them together, but Postgres grows its table in pages, so the size can
  end up to one write's growth past the limit. Set the floor with `--min-free-bytes`, and check it against the
  host's real free space before rollout.
- **Connections are capped and timed (RAL-35).** At most 64 connections are open at once, and 16 from any one
  address (`--max-connections`, `--max-connections-per-address`); past that, a new connection is closed before
  its TLS handshake. Each connection has 10 seconds to finish its handshake and 10 more to send its request's
  headers, and lasts at most 60 seconds in all (`--connection-seconds`), whether or not it has authenticated.
  60 seconds is longer than a write can wait on Postgres, so a slow write still gets its answer.
- **Failed tokens are limited by address (RAL-35).** An address may present 20 missing or wrong tokens at once,
  and regains one every 5 seconds (`--failed-token-burst`, `--failed-token-per-second`). Past that, its requests
  get a 429 before their token is looked at, so a guess can't tell a right token from a wrong one. That also
  refuses a real machine at that address until it recovers, so a machine with a mistyped token can lock itself
  out for a while. A request with no token counts as a failure too, so a health probe that sends none spends
  one each time. Each limit must be at least 1, and the service won't start otherwise.
- **Checks and past states have their own limits (RAL-35).** A check reads all of Postgres and replays it, and
  a read `as_of` a past record replays up to that record, so each costs about as much as the ledger. A
  credential may make 10 of them at once and regains one every 5 seconds (`--heavy-burst`,
  `--heavy-per-second`); past that, they get a 429 and reads of the current state carry on. Only one runs at a
  time across the service, and up to 8 reads of the current state; a request that can't start within 10
  seconds gets a 503 saying the service is busy. All work on the store runs in Tokio's blocking pool, so
  neither Postgres nor a replay holds up the service's own workers.
- **Responses are bounded (RAL-35).** A list of stories or teams gives at most 200, in order, with `next`
  when there are more: ask again with `?after=<next>`, or `rally show --after`. A story's or team's history
  gives its newest 100 entries, with `history_total` and, when there are more, `earlier`: ask again with
  `?before=<earlier>`, or `--before`. Pages are read separately, so writes in between can shift what a later
  page shows; each says its own `as_of`. The viewer reads up to 20 pages of each list and says when there was
  more.
- **A write is acknowledged only once Postgres has committed it.** If Postgres can't be reached before the
  record is sent, the write fails with a 503 saying nothing was recorded. If the connection fails after the
  commit is sent and before Postgres answers, the reply is a 503 saying the outcome is unknown: it may have
  been recorded, so check with `rally show` or `rally check` before retrying. The same holds if the client's
  connection ends before the reply arrives, including when the connection's 60-second deadline passes during
  a slow write: the client then gets no reply at all, and should check the same way. If it was recorded, the service
  refuses further writes until it's restarted and replays it. Any error from the commit counts as unknown, even
  one where Postgres clearly refused, as a precaution. If Postgres has a record that the service's own copy
  then fails to take, the reply is a 500 saying it was recorded and the service must be restarted. Each write's
  reply is its own, however many arrive at once. Reads of what the service already holds carry on either way,
  and when Postgres is back the service reconnects by itself.
- **Reads don't wait for writes.** They're served from the state the service keeps, without the writer's lock,
  so a write waiting on Postgres doesn't hold them up. `check` reads Postgres on a connection of its own,
  without that lock, and then takes it only long enough to read the service's copy, so that a write in progress
  can't look like a difference between them. So a check waits for at most one write, and writes don't wait for
  a check.
- **One write at a time, and the rest wait without a thread (RAL-42).** A write that Postgres doesn't answer,
  such as one sent to a Postgres that's stopped while its host still answers TCP, waits until Postgres resumes:
  Rally can't cancel it from outside. Only that write, and at most one check, wait that way. Later writes wait
  up to 10 seconds for their turn and then get a 503 saying the service is busy, and reads of the current
  state carry on. A Postgres that's gone entirely is found by the connection's keepalives and its 15-second
  limit on unanswered sends.
- **The database address carries only what myko is given.** myko connects with the host, port, user,
  database and password alone, so an address that sets `options`, such as a `search_path` that would point
  Rally and myko at different tables, is refused, as is `sslmode=require`, since Rally doesn't use TLS for
  Postgres. Keep Postgres on the same host, and set anything else on its role.
- **One writer.** The service holds a lock in Postgres for as long as it runs, so a second service or a `--db`
  write is refused rather than forking the chain.
- **Append-only in Postgres too.** The table refuses updates, deletes, `TRUNCATE`, deletion events, a second
  version of a record, and any row whose columns say something other than its event, which myko would load by
  the event alone. That catches mistakes: whoever owns the database can lift it. `rally check` reads by the
  event too, and reports any row whose columns don't match, and a service won't start over one.

## Recorded text is untrusted

Team names, declared names, notes and evidence are written by agents. Every read through `show` and `teams`
carries an `untrusted_text` notice saying they're data, not instructions. Output is JSON, with controls and invisible or direction characters
escaped, and text is stored and shown exactly as written. Whatever displays it has to escape it too. Notes
are limited to 4,000 characters and evidence to 1,000. Names are limited to 64 characters, with no control
characters.

A notice can't stop an agent being persuaded by what it reads. It only makes the boundary visible.

## Secrets

A write is refused if any text in it, including its declared name, looks like a secret (RAL-17): a GitHub,
AWS, Slack, Google, Anthropic or OpenAI key or token, a JSON web token, a PEM private key, or a long hex or
base64 run straight after a word like `token`, `key`, `secret` or `password`. The refusal names the field and
the kind of secret, never the text, so the secret doesn't reach the client's output or the service's log. A
plain commit hash isn't refused. The formats are in `src/secrets.rs`.

A hex run of exactly 40 or 64 characters isn't counted even after a label, because the ledger cites
commits, chain hashes and certificate fingerprints of those lengths, so a hex secret of exactly that length
gets through too. The check only recognizes those formats, so a secret in any other form gets through. If one does, revoke it at
its source straight away: that's what makes it harmless. The ledger is append-only, so the record stays, and
a later correction can't remove it. Record a correction saying the text was a revoked secret, so readers know
not to try it, and tell the operator, who decides whether the backups that hold it need handling too.

## What an accepted write proves

It proves that a process holding that machine's token sent it, and that the change was valid at that point.

It doesn't prove who or which team sent it. Every session on a machine's account can read that machine's
token, so a session can declare any name and any kind, including `verified`. So the check that authors don't
verify their own work catches mistakes, not forgery. Revisit this with MUS-39. A `merge` record likewise says
who recorded the merge, not who merged: every session shares one GitHub account, so Rally doesn't claim that.

## Detecting tampering

Each record's hash covers the one before it, so altering, removing or reordering a record means reads that
reach it are refused, and `rally check` says where. A read as of an earlier record still works, because that
history is intact.

Removing the newest records leaves a shorter chain that still holds, so ordinary reads don't notice: only
`rally check` does. The head is kept in the head file for this, outside Postgres, and `rally check` compares
against it and prints the current head. `rally check` reads Postgres directly, not the service's copy, so it also
says if the two differ, or if a record has more than one version in Postgres, which only a change past the append
path can leave. A service won't start over a record with two versions. Writes stop if that file is missing or isn't on the chain, because a write would
otherwise replace the evidence. A head file behind the ledger is allowed, because a crash leaves it that
way.

To recover, run `rally check`, keeping the damaged files to look at. Then either restore the ledger and its
head file together from a backup, or, if the chain is intact and a head saved elsewhere still matches, run
`rally --db <url> --head-file <file> adopt-head` on the host. It refuses a broken chain, but it does accept any removal
of the newest records that `check` was reporting, so don't use it to make a warning go away, and don't edit
the head file by hand. It takes the writer lock, so stop the service first: while it runs, `adopt-head` is
refused, because the service could move the head in between.

Save that head somewhere other than the host from time to time, then check it with
`rally check --expect SEQ:HASH`. That's the only protection against the case below.

This detects and doesn't prevent. **Someone with write access to the host, or the ledger's Postgres role, can
rewrite the whole ledger, its hashes, its history and the head file together, and `rally check` alone will
pass.** Only a head saved elsewhere
would show it, and it can't show anything changed after that head was saved, or show which version is
true. So a head saved elsewhere is the real protection: a head file reset to record 0 with records present
passes `check` and allows writes. Without a saved head, this also goes unseen if someone winds the head file back along with the
records, or removes every record and the head file, which looks like a new ledger.

## Backups

From another machine, with a read-only credential (RAL-15):

```bash
rally add-machine backup --read-only --address <LAN address>:6300 --out backup.pair   # on the host
rally --config ~/.config/rally/backup.json pair < backup.pair && rm backup.pair       # on the other machine
rally --config ~/.config/rally/backup.json backup --to ~/.local/state/rally-backup
```

The backup directory is private, and holds `records.jsonl`, every record exactly as stored, one per line,
and `head.json`, the head they reach and when it was saved. Each run rechecks the local copy against its
head, pulls only the records after it through `/v1/export`, and checks that each new one chains on from it.
If the service's ledger no longer extends the saved head (records removed, or any record changed), it refuses
with "rewrite" and writes nothing: that's the host's ledger being rewritten, as `rally check --expect` would
show. The output's `expect` is the head to pass to `rally check --expect`. When the service can't be reached,
it fails and says how long ago the last backup was, and that it's overdue past `--max-age-hours` (24).

To restore, make a new, empty Postgres database and pick a new head file, then:

```bash
rally --db <new database> --head-file <new head file> restore --from ~/.local/state/rally-backup
rally --db <new database> --head-file <new head file> check --expect <the backup's expect>
```

Restore rechecks the backup's chain and replays it before writing anything, refuses a database that already
has records or a head file that already exists, writes every record in one transaction with the table's usual
protections, saves the head file, and then reopens the ledger as the service would and checks it. Its output
gives `expect`, and the backup's own `head.json`, which is the same unless the last backup stopped before
saving it. A service started on the restored ledger carries on from its head.

### Scheduling on a1

a1 sleeps with its lid closed, so its backups are a launchd agent, `deploy/a1/com.rally.backup.plist`: every 15
minutes it runs `rally backup` with the read-only backup pairing into `~/.local/state/rally-backup`, logging to
`~/.local/state/rally-backup.log`. launchd runs a missed interval when the machine wakes, so the first run after
the lid opens is the catch-up. The command exits non-zero when the service can't be reached, saying how long ago
the last backup was and whether that's overdue, and the viewer shows the same from `head.json` when started with
`rally web --backup-dir ~/.local/state/rally-backup` (overdue past `--backup-max-age-hours`, 1 by default, which
is four missed runs). A Muster message to the watchman when it's overdue is a later step, limited to one a day.

Installing it changes the operator's machine, so it's the operator's to do, on their word:

```bash
install -m 755 target/release/rally ~/.local/bin/rally
sed "s|__HOME__|$HOME|g" deploy/a1/com.rally.backup.plist > ~/Library/LaunchAgents/com.rally.backup.plist
launchctl bootout gui/$(id -u)/com.rally.backup 2>/dev/null    # only if an older one is loaded
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.rally.backup.plist
launchctl kickstart -k gui/$(id -u)/com.rally.backup    # first run now; then tail the log
```

launchd doesn't expand `~` or `$HOME`, so the `sed` fills in the account's home. The agent runs `rally`
directly, not through a shell, because macOS asks for Local Network access per program: the first run needs it
for `rally` itself. If a run logs "No route to host (os error 65)" while the same backup from a terminal works,
that access is missing. Allow `rally` under System Settings, Privacy & Security, Local Network, or answer the
prompt on the first run, then kickstart it again. Seen on a1, macOS 15.7, on 2026-11-04, when the agent
still ran through `/bin/sh`.

The log isn't rotated. Each run adds one line of about 107 bytes (measured on a1, 2026-11-04), so
at four runs an hour it grows by under 4 MB a year even if a1 never slept, and can be left; to clear it,
`: > ~/.local/state/rally-backup.log` is safe at any time.

To stop it: `launchctl bootout gui/$(id -u)/com.rally.backup`. The agent holds no secret; the pairing stays in
`~/.config/rally/backup.json`, readable only by the account.

## Blockers

A team says what it's waiting on with `rally block --team <team> [--story <its story>] [--on-story <story>]
[--on-team <team>] --note <what>` (RAL-20). With `--story`, that story is blocked; without, the whole team is,
and the two are listed apart as `blocks: story` or `blocks: team`. `--on-story` may name another team's story,
even one the ledger doesn't track. The blocker is the record's number, and it stays open until someone runs
`rally unblock <number> --note <why>`, which records who closed it and why. A blocker is apart from a story's
state and a team's declared state: blocking changes neither.

`rally blockers` lists the open ones, each with `waits_on`, the open blockers it waits on directly, and `chain`,
everything it waits on through those, nearest first. Waiting on a story means its open blockers; waiting on a team
means every open blocker that team holds. A chain that comes back round says `cycle`, and one over 100 long says
`chain_truncated`. `--all` adds closed blockers with their closure, and `--as-of` shows them as they stood. Over
the service these are `/v1/blockers` and `/v1/blockers/all`.

## Backfilling an outage

While the service is down, teams use the fallback board in Muster's `docs/board.md`. Afterwards, each
entry made there is recorded with when it was made and by whom (RAL-30):

```
rally --as <you> --entered-at-ms <when, ms since the epoch> --entered-by <who entered it> <the command as it was>
```

for example `rally --as watch --entered-at-ms 1793800000000 --entered-by rally-team claim RAL-30 --team rally`.
The record's own time and source stay the service's time and whoever backfilled it, and the history
shows both as `at_ms` and `backfilled`, so the gap stays visible and a backfilled record can't pass for a
live one: an entry time at or after the record's own is refused. The entry is checked as the name that
entered it, so a backfilled verification is still refused for its own submitter and still names its
commit. Backfill entries in the order they were made.

## Known limits

- `--db` works on the ledger's database directly, with no service and no checks on who is writing. It's for
  tests and for the host's own administration. Its reads (`check`, `show`, `teams`, `web`) are read-only in
  Postgres and work while the service runs. Its writes need the writer lock, so they wait up to 5 seconds and
  are refused while the service runs.
- The disk floor is checked before each write, so it can't see what other programs write in between, and
  it only stops Rally, not Postgres: Postgres's own write-ahead log and other databases can still fill the disk.
  It doesn't count backups stored elsewhere, and it can't free space. The size limit counts Rally's table and
  indexes, not the write-ahead log.
- Postgres must be durable for an acknowledged write to be: keep `fsync` and `synchronous_commit` on, as they are
  by default.
- A Postgres that hangs rather than stops, for example a stopped process, holds a
  write until it recovers, with no deadline of Rally's own: the operating system keeps the connection open, so
  it can be indefinite. Later writes and checks queue behind it, each holding one of the service's blocking
  threads, so reads carry on only until those run out (RAL-42). If the connection does fail after the commit
  was sent, the outcome is reported as unknown.
- myko is given the database's address as a URL, with the password percent-encoded in the part myko hides in
  its own messages, and Rally also removes the password from any myko error it passes on. That URL keeps only
  the host, port, user, database and password, so other connection settings in `--db` don't reach myko's
  connections (RAL-42). Rally installs no logger for myko.
- The password file is checked, then read, so a file swapped in between isn't caught, and its directory isn't
  checked (RAL-42). Keep it in a directory only this account can write.
- Tests show a commit cut off before Postgres committed it (rolled back), and one that committed with its reply
  lost; a real reply lost in transit isn't reproduced.
- Backups (RAL-15) are scheduled only once the operator installs the launchd agent on a1 (above). A restore on
  a1 was run for real on 2026-11-04; the run is recorded in [deploy-murph.md](deploy-murph.md).
- Write limits are per machine, so all of a machine's sessions share them. Watch for 429s in real use.
- One compromised credential could reach the 64 MiB stop in about 6 hours, and several credentials
  proportionally faster, which blocks everyone's writes
  while reads carry on. If that happens, revoke the credential and find its records by credential with
  `rally show`. Those records stay in the ledger, because it's append-only. Raising
  `--max-ledger-bytes` restores writes but costs memory, as below, so check the host's memory first.
- Connection and failed-token limits go by address, so many addresses can still share out the 64 connections,
  and a machine sharing an address with an attacker, such as another account on the same host, shares its
  limits. A closed connection is refused at TCP level, after the operating system accepts it, so a flood still
  costs the host something. Tokens are 256 random bits, so guessing isn't practical either way. This is why the
  service stays off the open internet.
- The service holds every record in memory twice for as long as it runs: as text in myko's store, and parsed in
  the state it keeps between requests, so that a read of the current state or an append doesn't replay the whole
  ledger (RAL-35). An append keeps about 24.6 KB for a 12.2 KB record, measured by the allocator. A check still
  reads all of Postgres and replays it, on a connection of its own and without the writer's lock, one check at a
  time. A read `as_of` a past record still replays up to that record, copying only the records it needs, one at
  a time across the service.
  Measured with a release build on a1 at the 64 MiB limit, with 64,001 typical 300-character records: 1.3 s to
  start, under 0.01 s for a read, 0.36 s for a check and 0.05 s for a write through the service, and 201 to 214 MiB
  held. With 4,751 worst-case records of about 12 KB: 0.7 s, under 0.01 s, 0.34 s and 0.02 s, and 205 to 220 MiB.
  With four read-only credentials checking in a loop, writes still took 0.04 to 0.06 s and the service held
  292 MiB; before the kept state, they took 0.7 to 0.9 s and it held 606 MiB. With eight read-only loops of
  `show --as-of 60000`, it held 314 to 318 MiB; before they ran one at a time, 1,058 to 1,118 MiB. Both were
  measured with the per-credential limit on checks and past states lifted, so they show the one-at-a-time
  limit alone.
  The limit stays at 64 MiB until the rest of RAL-35 is done: the host shares its memory with Muster's daemon.
- A damaged `machines.json` logs a line for each request, before any rate limit, until it's fixed.
- If `cert.pem` or `key.pem` goes missing, the service refuses to start rather than making a new
  certificate, which would break every machine's pin. Restore it, or remove both and pair every machine
  again.
- File permissions keep secrets private on Unix only. Windows isn't a supported host.
- Any process on the host's account can read or change the ledger, the key and the token hashes, and so can
  anyone who holds the ledger's Postgres role.
- Replacing the certificate means pairing every machine again.
